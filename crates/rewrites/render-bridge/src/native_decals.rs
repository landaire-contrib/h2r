//! `native-decals`: scope dynamic decal callbacks for the ordered
//! `d3d::DrawAdmission` in boot. On with the native renderer
//! (`impl.render.native`).
//!
//! The observers do not suppress guest code. `0x00044040` still binds every
//! texture, writes every duplicate marker and emits every draw. The admission
//! replaces only a fully resolved D3D draw packet, after two-copy recording
//! succeeds. Sorted handlers `0x00041cc0` and `0x000423c0` are object/layer
//! replays, not decals, and are deliberately left alone.
//!
//! Refusals are tallied by reason.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use native_render::decal::Mode;

use hooks::{BuildError, Builder, Feature, Flow, Manifest};
use runtime::abi::TransferAbi;
use timeline::native::{Metric, Stage};

pub const ID: &str = "native-decals";
pub trait WalkHost: Send + Sync {
    fn draw(&self, mem: &mut dyn GuestMemory, start: u32, count: u32) -> bool;
}

static WALK: Mutex<Option<Arc<dyn WalkHost>>> = Mutex::new(None);

pub fn set_walk_host(host: Arc<dyn WalkHost>) {
    *WALK.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

const CURRENT_MODE: u32 = 0x004c_8790;

pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches)
}

thread_local! {
    /// The guest callback currently emitting on this render thread.
    static CURRENT: Cell<Option<(Mode, i16)>> = const { Cell::new(None) };
}

static EXAMINED: AtomicU64 = AtomicU64::new(0);
static ADMITTED: AtomicU64 = AtomicU64::new(0);
static REFUSALS: Mutex<BTreeMap<&'static str, u64>> = Mutex::new(BTreeMap::new());

/// `(examined draws, admitted)` and the running refusal tally.
pub fn counts() -> ((u64, u64), BTreeMap<&'static str, u64>) {
    (
        (
            EXAMINED.load(Ordering::Relaxed),
            ADMITTED.load(Ordering::Relaxed),
        ),
        REFUSALS.lock().unwrap_or_else(|e| e.into_inner()).clone(),
    )
}

pub fn note_examined() {
    EXAMINED.fetch_add(1, Ordering::Relaxed);
}

pub fn note_admitted() {
    let admitted = ADMITTED.fetch_add(1, Ordering::Relaxed) + 1;
    if admitted % 1800 == 1 {
        let ((examined, admitted), refusals) = counts();
        eprintln!(
            "native-decals: {admitted} of {examined} resolved draws native; refusals {refusals:?}"
        );
    }
}

/// Adds an admission or recording failure to the same tally.
pub fn note_refusal(why: &'static str) {
    let total = {
        let mut refusals = REFUSALS.lock().unwrap_or_else(|e| e.into_inner());
        *refusals.entry(why).or_default() += 1;
        refusals.values().sum::<u64>()
    };
    if total % 1800 == 1 {
        let ((examined, admitted), refusals) = counts();
        eprintln!(
            "native-decals: {admitted} of {examined} resolved draws native; refusals {refusals:?}"
        );
    }
}

/// The callback emitting on this thread, if any.
pub fn current() -> Option<(Mode, i16)> {
    CURRENT.with(Cell::get)
}

fn begin(mem: &dyn GuestMemory, list: i16) {
    let mode = mem
        .u16(CURRENT_MODE)
        .ok()
        .and_then(|v| Mode::from_guest(v as i16));
    CURRENT.with(|c| c.set(mode.map(|m| (m, list))));
}

fn end() {
    CURRENT.with(|c| c.set(None));
}

use points::scene_effects::{DECAL_ITEM_GEOMETRY, DECAL_RENDER_END};
const DECAL_ITEM_NEXT: u32 = 0x0004_4195;
const GEOMETRY_ALLOCATOR: u32 = 0x0050_9448;

fn item_range(mem: &dyn GuestMemory, record: u32) -> Option<(u32, u32)> {
    let index = mem.u32(record.checked_add(4)?).ok()? & 0xffff;
    let quads = i16::try_from(mem.u16(record.checked_add(0x2a)?).ok()?).ok()?;
    if quads <= 0 {
        return None;
    }
    let allocator = mem.u32(GEOMETRY_ALLOCATOR).ok()?;
    let array = mem.u32(allocator.checked_add(0x64)?).ok()?;
    let elements = mem.u32(array.checked_add(0x44)?).ok()?;
    if allocator == 0 || array == 0 || elements == 0 {
        return None;
    }
    let offset = mem.u32(elements.checked_add(index * 24 + 8)?).ok()?;
    let shift = mem.u32(allocator.checked_add(0x34)?).ok()? & 31;
    Some(((offset << shift) >> 4, u32::try_from(quads).ok()? * 4))
}

fn draw_item(mem: &mut dyn GuestMemory, record: u32) -> bool {
    if current().is_none() {
        return false;
    }
    Stage::Decals.count(Metric::WalkExamined, 1);
    let Some((start, count)) = item_range(mem, record) else {
        Stage::Decals.count(Metric::WalkRefusedInputs, 1);
        return false;
    };
    let host = WALK.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let drawn = host.is_some_and(|host| host.draw(mem, start, count));
    Stage::Decals.count(if drawn { Metric::WalkSuppressed } else { Metric::WalkRefusedReservation }, 1);
    drawn
}

pub struct NativeDecals;

impl Feature for NativeDecals {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Dynamic decals drawn by the native GPU renderer")
            .doc("docs/graphics/native-decals-research.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw()
            .observe_before(&points::DECAL_RENDER, "native_decals_begin", |o| {
                let Some(list) = o.arg_u32("list").map(|v| v as u16 as i16) else {
                    return;
                };
                let mem = o.guest();
                begin(&*mem, list);
            })
            .context_only();
        b.raw()
            .observe_before(&DECAL_RENDER_END, "native_decals_end", |_| end())
            .context_only();
        b.raw().before(&DECAL_ITEM_GEOMETRY, "native_decals_walk_item", |c| {
            let runtime = Arc::clone(c.ctx.runtime());
            if !runtime.read_dispatch(|dispatch| dispatch.has_resume(DECAL_ITEM_NEXT)) {
                return Ok(Flow::Continue);
            }
            if draw_item(&mut *runtime.memory(), c.cpu.esi) {
                Ok(Flow::Return(TransferAbi::resume(DECAL_ITEM_NEXT)))
            } else {
                Ok(Flow::Continue)
            }
        }).context_only().optional();
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_range_uses_the_live_allocator_and_signed_quad_count() {
        use engine::memory::FlatMemory;
        let mut mem = FlatMemory::new(0x0050_9000, 0x4000);
        let record = 0x0050_a000;
        let allocator = 0x0050_a100;
        let array = 0x0050_a200;
        let elements = 0x0050_a300;
        mem.set_u32(record + 4, 0x9999_0002).unwrap();
        mem.set_u16(record + 0x2a, 3).unwrap();
        mem.set_u32(GEOMETRY_ALLOCATOR, allocator).unwrap();
        mem.set_u32(allocator + 0x34, 5).unwrap();
        mem.set_u32(allocator + 0x64, array).unwrap();
        mem.set_u32(array + 0x44, elements).unwrap();
        mem.set_u32(elements + 2 * 24 + 8, 17).unwrap();
        assert_eq!(item_range(&mem, record), Some((34, 12)));
        mem.set_u16(record + 0x2a, u16::MAX).unwrap();
        assert_eq!(item_range(&mem, record), None);
    }
}
