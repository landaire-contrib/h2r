//! Marks the guest's sorted geometry entries for the resolved indexed-draw
//! path. The guest runs each handler, including its state and texture setup.
//! The two stack frames identify the entry while the indexed call is live.
//! Push-buffer work and other kinds of handlers keep their ordinary path.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::{memory::GuestMemory, points};
use native_render::camera::GuestRead;
use native_render::transparent::{self, Entry};

use hooks::{BuildError, Builder, Feature, Flow, Manifest};
use runtime::abi::TransferAbi;
use timeline::native::{Metric, Stage};

pub const ID: &str = "native-transparents";

use points::scene_effects::{GEOMETRY_RANGE_READY, SCENE_TRANSPARENTS_END};
const GEOMETRY_RANGE_TAIL: u32 = 0x0004_dae0;
const GEOMETRY_EMITTER_CALLER: u32 = 0x0004_d626;
const TRANSPARENT_DRAW_CALLER: u32 = 0x0004_1010;
const LAYER_DRAW_CALLER: u32 = 0x0003_bc90;
const FIRST_PERSON_LAYER_FRAME: u32 = 0xf0a4;
const SORTED_HANDLER_RETURN: u32 = 0x0001_e3f3;
const LIST_COUNT: u32 = 0x004b_6280;
const LIST_FIRST: u32 = 0x004b_6284;
const LIST_ENTRIES: u32 = 0x004b_6288;
const LIST_ORDER: u32 = 0x004b_628c;
const ENTRY_BYTES: u32 = 0x70;
const MAX_SORTED: u32 = 0x400;

pub struct IndexRange {
    pub indices: u32,
    pub count: u32,
    pub strip: bool,
}

pub trait WalkHost: Send + Sync {
    fn draw(&self, mem: &mut dyn GuestMemory, stage: Stage, range: IndexRange) -> bool;
}

static WALK: Mutex<Option<Arc<dyn WalkHost>>> = Mutex::new(None);

pub fn set_walk_host(host: Arc<dyn WalkHost>) {
    *WALK.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

fn sorted_data(mem: &dyn GuestMemory, ordinal: u32) -> Option<u32> {
    let first = mem.u32(LIST_FIRST).ok()?;
    let end = mem.u32(LIST_COUNT).ok()?;
    if end > MAX_SORTED || ordinal >= end.checked_sub(first)? {
        return None;
    }
    let order = mem.u32(LIST_ORDER).ok()?;
    let entries = mem.u32(LIST_ENTRIES).ok()?;
    if order == 0 || entries == 0 {
        return None;
    }
    let index = u32::from(mem.u16(order.checked_add(2 * ordinal)?).ok()?);
    if !(first..end).contains(&index) {
        return None;
    }
    entries.checked_add(index * ENTRY_BYTES + transparent::DATA_OFFSET)
}

thread_local! {
    static WALK_DATA: Cell<Option<u32>> = const { Cell::new(None) };
}

fn walk_stage(mem: &dyn GuestRead, esp: u32) -> Option<Stage> {
    let word = |offset| mem.read_u32(esp.checked_add(offset)?);
    if word(0x18)? != GEOMETRY_EMITTER_CALLER {
        return None;
    }
    let layer = word(0x78)?;
    match word(0x68)? {
        TRANSPARENT_DRAW_CALLER if layer == 15 => {
            indexed_entry(mem, esp)?;
            let frame_data = SCOPE.with(|slot| {
                let scope = slot.take();
                let data = scope.handler.map(|h| h.data);
                slot.set(scope);
                data
            })?;
            (WALK_DATA.get() == Some(frame_data)).then_some(Stage::Transparents)
        }
        LAYER_DRAW_CALLER => {
            use native_render::world_layers::{Site, layer_site};
            let caller = word(FIRST_PERSON_LAYER_FRAME)?;
            let first_person_layer = layer_site(layer, caller) == Site::FirstPerson
                || (layer == 20 && caller == 0x0004_020b);
            let list = mem.read_u32(native_render::skip::CURRENT_LIST)?;
            (first_person_layer && list != 0
                && mem.read_u32(list.checked_add(0x14)?)? == crate::native_first_person::FIRST_PERSON_ITEMS)
                .then_some(Stage::FirstPerson)
        }
        _ => None,
    }
}

fn walk_range(mem: &mut dyn GuestMemory, stage: Stage, range: IndexRange) -> bool {
    stage.count(Metric::WalkExamined, 1);
    if range.count < 3 || range.indices == 0 || (!range.strip && range.count % 3 != 0) {
        stage.count(Metric::WalkRefusedInputs, 1);
        return false;
    }
    let host = WALK.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let drawn = host.is_some_and(|host| host.draw(mem, stage, range));
    let metric = if drawn { Metric::WalkSuppressed } else { Metric::WalkRefusedReservation };
    stage.count(metric, 1);
    drawn
}

pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches)
}

static ACTIVATION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MapGeneration {
    activation: u64,
    globals: u32,
    bsp: u32,
    bsp_index: u16,
}

impl MapGeneration {
    fn read(mem: &dyn GuestRead) -> Option<Self> {
        let globals = mem.read_u32(points::GAME_GLOBALS.base)?;
        let bsp = mem.read_u32(crate::native_world::ACTIVE_BSP)?;
        if globals == 0 || bsp == 0 {
            return None;
        }
        let index = (mem.read_u32(globals)? >> 16) as i16;
        Some(Self {
            activation: ACTIVATION.load(Ordering::Acquire),
            globals,
            bsp,
            bsp_index: u16::try_from(index).ok()?,
        })
    }
}

#[derive(Clone, Copy)]
struct Frame {
    esp: u32,
    return_to: u32,
}

impl Frame {
    fn read(mem: &dyn GuestRead, esp: u32) -> Option<Self> {
        let return_to = mem.read_u32(esp)?;
        (return_to != 0).then_some(Self { esp, return_to })
    }

    fn contains(self, mem: &dyn GuestRead, child_esp: u32) -> bool {
        child_esp < self.esp && mem.read_u32(self.esp) == Some(self.return_to)
    }
}

#[derive(Clone, Copy)]
struct Scene {
    esp: u32,
    frame: Option<Frame>,
    map: MapGeneration,
}

#[derive(Clone, Copy)]
struct Handler {
    frame: Frame,
    data: u32,
    entry: Entry,
}

#[derive(Default)]
pub struct Scope {
    scene: Option<Scene>,
    handler: Option<Handler>,
}

impl Scope {
    pub fn begin_view(&mut self, mem: &dyn GuestRead, esp: u32) {
        self.scene = MapGeneration::read(mem).map(|map| Scene { esp, frame: None, map });
        self.handler = None;
    }

    pub fn begin_entry(&mut self, mem: &dyn GuestRead, esp: u32) {
        if let Some(scene) = &mut self.scene
            && scene.frame.is_none()
            && esp < scene.esp
        {
            // Capture the live scene frame only once the dispatcher enters
            // its first geometry handler.
            scene.frame = Frame::read(mem, scene.esp);
        }
        self.handler = self.read_handler(mem, esp);
    }

    fn read_handler(&self, mem: &dyn GuestRead, esp: u32) -> Option<Handler> {
        let scene = self.scene?;
        let frame = Frame::read(mem, esp)?;
        // The next function after the sorted-list dispatch is the effects
        // allocator. An invocation from another caller has no sorted slot.
        const NEXT_AFTER_SORTED_DISPATCH: u32 = 0x0001_e4e0;
        if !scene.frame?.contains(mem, esp)
            || !(points::SCENE_TRANSPARENTS.base..NEXT_AFTER_SORTED_DISPATCH).contains(&frame.return_to)
        {
            return None;
        }
        let data = mem.read_u32(esp.checked_add(4)?)?;
        let entry = transparent::read_entry(mem, data)?;
        Some(Handler { frame, data, entry })
    }

    /// Returns the exact sorted part only while its call frame survives.
    /// The dispatch reuses the frame for the next handler and always writes
    /// that handler's own entry pointer to its argument slot.
    pub fn indexed_entry(&self, mem: &dyn GuestRead, esp: u32) -> Option<(MapGeneration, Entry)> {
        let scene = self.scene?;
        let handler = self.handler?;
        if !scene.frame?.contains(mem, handler.frame.esp)
            || !handler.frame.contains(mem, esp)
            || mem.read_u32(handler.frame.esp.checked_add(4)?) != Some(handler.data)
            || MapGeneration::read(mem) != Some(scene.map)
        {
            return None;
        }
        let live = transparent::read_entry(mem, handler.data)?;
        (live == handler.entry).then_some((scene.map, live))
    }
}

thread_local! {
    // D3D resolves the indexed API call on the same guest execution thread
    // as the hook. The GPU worker receives only immutable resolved data.
    static SCOPE: Cell<Scope> = Cell::new(Scope::default());
}

/// Without a sorted handler the indexed draw cannot use the transparent adapter.
pub fn in_indexed_handler() -> bool {
    SCOPE.with(|slot| {
        let scope = slot.take();
        let active = scope.handler.is_some();
        slot.set(scope);
        active
    })
}

pub fn indexed_entry(mem: &dyn GuestRead, esp: u32) -> Option<(MapGeneration, Entry)> {
    SCOPE.with(|slot| {
        let scope = slot.take();
        let entry = scope.indexed_entry(mem, esp);
        slot.set(scope);
        entry
    })
}

struct ReadMemory<'a>(&'a dyn GuestMemory);

impl GuestRead for ReadMemory<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

pub struct NativeTransparents;

impl Feature for NativeTransparents {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Ordered native vertex bindings for sorted transparents")
            .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // Both dispatch scopes last one view. A save occurs after the
        // render has drained, and the next view starts with fresh scopes.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw().before(&GEOMETRY_RANGE_READY, "native_scene_walk_indexed", |c| {
            let rt = Arc::clone(c.ctx.runtime());
            if !rt.read_dispatch(|dispatch| dispatch.has_resume(GEOMETRY_RANGE_TAIL)) {
                return Ok(Flow::Continue);
            }
            let mut mem = rt.memory();
            let Some(stage) = walk_stage(&ReadMemory(&*mem), c.cpu.esp) else {
                return Ok(Flow::Continue);
            };
            let strip = c.cpu.ebx as u8 != 0;
            let range = IndexRange { indices: c.cpu.edi, count: c.cpu.esi, strip };
            if !walk_range(&mut *mem, stage, range) {
                return Ok(Flow::Continue);
            }
            c.cpu.edx = 5 + u32::from(strip);
            Ok(Flow::Return(TransferAbi::resume(GEOMETRY_RANGE_TAIL)))
        }).context_only().optional();
        b.raw()
            .observe_before(&points::GAME_ACTIVATE, "native_transparents_generation", |_| {
                ACTIVATION.fetch_add(1, Ordering::AcqRel);
            })
            .context_only();
        b.raw()
            .observe_before(&points::SCENE_TRANSPARENTS, "native_transparents_view", |c| {
                WALK_DATA.set(None);
                let mem = c.guest();
                let mut scope = Scope::default();
                scope.begin_view(&ReadMemory(&*mem), c.cpu.esp);
                SCOPE.with(|slot| slot.set(scope));
            })
            .context_only();
        b.raw()
            .observe_before(&SCENE_TRANSPARENTS_END, "native_transparents_end", |_| {
                WALK_DATA.set(None);
                SCOPE.with(|slot| slot.set(Scope::default()));
            })
            .context_only();
        b.raw()
            .observe_before(&points::TRANSPARENT_GEOMETRY, "native_transparents_entry", |c| {
                let mem = c.guest();
                let data = (mem.u32(c.cpu.esp).ok() == Some(SORTED_HANDLER_RETURN))
                    .then(|| sorted_data(&*mem, c.cpu.esi)).flatten();
                WALK_DATA.set(data);
                SCOPE.with(|slot| {
                    let mut scope = slot.take();
                    scope.begin_entry(&ReadMemory(&*mem), c.cpu.esp);
                    slot.set(scope);
                });
            })
            .context_only();
        Ok(())
    }
}

#[cfg(test)]
mod walk_tests {
    use super::*;
    use engine::memory::FlatMemory;
    use native_render::skip::CURRENT_LIST;

    #[test]
    fn sorted_data_uses_live_indices_and_refuses_stale_entries() {
        let mut mem = FlatMemory::new(0x0040_0000, 0x001e_0000);
        let (entries, order) = (0x005a_1000, 0x005a_0000);
        for (at, value) in [(LIST_FIRST, 2), (LIST_COUNT, 4), (LIST_ENTRIES, entries), (LIST_ORDER, order)] {
            mem.set_u32(at, value).unwrap();
        }
        mem.set_u16(order, 3).unwrap();
        mem.set_u16(order + 2, 2).unwrap();
        assert_eq!(sorted_data(&mem, 0), Some(entries + 3 * ENTRY_BYTES + transparent::DATA_OFFSET));
        assert_eq!(sorted_data(&mem, 1), Some(entries + 2 * ENTRY_BYTES + transparent::DATA_OFFSET));
        assert_eq!(sorted_data(&mem, 2), None);
        mem.set_u16(order + 2, 1).unwrap();
        assert_eq!(sorted_data(&mem, 1), None);
    }

    #[test]
    fn an_index_range_requires_the_first_person_stack_and_owned_list() {
        let mut mem = FlatMemory::new(0x0040_0000, 0x001e_0000);
        let (stack, list) = (0x005a_0000, 0x004a_0000);
        mem.set_u32(stack + 0x18, GEOMETRY_EMITTER_CALLER).unwrap();
        mem.set_u32(stack + 0x68, LAYER_DRAW_CALLER).unwrap();
        mem.set_u32(stack + 0x78, 7).unwrap();
        mem.set_u32(stack + FIRST_PERSON_LAYER_FRAME, 0x0004_0271).unwrap();
        mem.set_u32(CURRENT_LIST, list).unwrap();
        mem.set_u32(list + 0x14, crate::native_first_person::FIRST_PERSON_ITEMS).unwrap();
        let classify = |mem: &FlatMemory| walk_stage(&ReadMemory(mem), stack);
        assert_eq!(classify(&mem), Some(Stage::FirstPerson));
        mem.set_u32(list + 0x14, 0x0050_0000).unwrap();
        assert_eq!(classify(&mem), None);
        mem.set_u32(list + 0x14, crate::native_first_person::FIRST_PERSON_ITEMS).unwrap();
        mem.set_u32(stack + FIRST_PERSON_LAYER_FRAME, 0x0002_c039).unwrap();
        assert_eq!(classify(&mem), None);
    }
}
