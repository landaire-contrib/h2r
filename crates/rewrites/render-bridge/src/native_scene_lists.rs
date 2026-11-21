//! Retail scene list production, on with the native renderer
//! (`impl.render.native`, on by default). A builder reads under the
//! guest-memory guard and proposes all its writes before any output
//! becomes visible.
mod geometry;
mod lights;
mod objects;
mod object_lod;
mod records;
mod setup;
mod transparents;

use std::collections::HashMap;
use std::sync::Arc;

use engine::memory::{GuestMemory, MemFault};
use engine::points::{self, Point};
use runtime::guest::Access;
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};
use timeline::native::{SceneList, SceneListResult};

use hooks::{BuildError, Builder, Feature, Flow, Manifest, Switches};

const ID: &str = "native-scene-lists";
const NONE: u32 = u32::MAX;
const TAGS: u32 = engine::tags::TAG_ENTRIES_GLOBAL;
const FRAME: u32 = 0x004b_a034;
const OBJECTS: u32 = engine::render_loop::occlusion::guest::OBJECTS;

pub struct NativeSceneLists;

fn enabled(switches: &Switches) -> bool {
    hooks::switches::native_render_on(switches)
}

/// Guest word holding the multiplier sampled when this view starts building records.
#[derive(Clone, Copy)]
pub struct ViewLodScale(u32);

impl ViewLodScale {
    pub const fn new(guest_word: u32) -> Self {
        Self(guest_word)
    }

    /// Older snapshots and views before their first record reset use retail pixels.
    pub fn read(self, m: &dyn GuestMemory) -> f32 {
        match m.f32(self.0) {
            Ok(value) if value.is_finite() && value > 0.0 => value,
            _ => 1.0,
        }
    }
}

impl NativeSceneLists {
    /// Install LOD adjustment at the guest functions invoked by the fallback
    /// paths. These hooks also apply when native scene lists are disabled.
    pub fn install_guest_lod(b: &mut Builder, scale: ViewLodScale) {
        object_lod::install(b, scale);
    }

    /// Configure the replacement object setup and record writer for this view.
    pub fn with_lod_scale(scale: ViewLodScale) -> impl Feature {
        ScaledSceneLists(Some(scale))
    }
}

struct ScaledSceneLists(Option<ViewLodScale>);

impl Feature for NativeSceneLists {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Rust scene list builders").doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        ScaledSceneLists(None).build(b)
    }
}

impl Feature for ScaledSceneLists {
    fn manifest(&self) -> Manifest {
        NativeSceneLists.manifest()
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        if !enabled(b.switches()) {
            return Ok(());
        }
        install(
            b,
            Seam::Function(&points::SCENE_CLUSTER_GATHER, 4),
            objects::gather,
        );
        install(
            b,
            Seam::Function(&points::SCENE_OBJECT_ORDER, 4),
            objects::order,
        );
        install(
            b,
            Seam::Function(&points::SCENE_GEOMETRY_GATHER, 4),
            geometry::gather,
        );
        let scale = self.0;
        install(
            b,
            Seam::Function(&points::OBJECT_RENDER_SETUP, 20),
            move |m, cpu| setup::build(m, cpu, scale.map_or(1.0, |scale| scale.read(m))),
        );
        install(
            b,
            Seam::CallSite(
                &points::OBJECT_RECORD_APPEND,
                points::OBJECT_RECORD_APPENDED,
                SceneList::ObjectRecord,
            ),
            move |m, cpu| records::build(m, cpu, scale.map_or(1.0, |scale| scale.read(m))),
        );
        install(
            b,
            Seam::Function(&points::VISIBLE_LIGHT_LIST, 4),
            lights::visible,
        );
        install(
            b,
            Seam::Function(&points::LIGHT_GEOMETRY_FLAGS, 16),
            lights::geometry,
        );
        install(
            b,
            Seam::Function(&points::LIGHT_EXTRA_CASTER, 4),
            lights::extra_caster,
        );
        install(
            b,
            Seam::Function(&points::TRANSPARENT_EFFECT_ENTRY, 8),
            transparents::effect,
        );
        install(
            b,
            Seam::Function(&points::TRANSPARENT_POLYGON_ENTRY, 32),
            transparents::polygon,
        );
        install(
            b,
            Seam::Sites(
                &points::TRANSPARENT_GEOMETRY_ENTRY,
                &points::TRANSPARENT_GEOMETRY_QUEUED,
                SceneList::TransparentGeometry,
            ),
            transparents::geometry,
        );
        install(
            b,
            Seam::Sites(
                &points::TRANSPARENT_ORDER_BEGIN,
                &points::TRANSPARENT_ORDER_READY,
                SceneList::Transparents,
            ),
            transparents::build,
        );
        Ok(())
    }
}

/// The existing pre-trim checks, for FrameScene's read-only extraction.
pub(crate) fn light_candidate_eligible(
    m: &dyn GuestMemory,
    record: u32,
    definition: u32,
    flags: u32,
) -> Option<bool> {
    lights::candidate_eligible(m, record, definition, flags).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    Memory,
    Layout,
    Unsupported,
    Residency,
    Lighting,
    Palette,
    ObjectModel,
}

impl From<MemFault> for Refusal {
    fn from(_: MemFault) -> Self {
        Self::Memory
    }
}

type Result<T, E = Refusal> = std::result::Result<T, E>;

fn at(base: u32, offset: u32) -> Result<u32> {
    base.checked_add(offset).ok_or(Refusal::Layout)
}

fn ptr(m: &dyn GuestMemory, addr: u32) -> Result<u32> {
    match m.u32(addr)? {
        0 | NONE => Err(Refusal::Layout),
        value => Ok(value),
    }
}

fn stack(m: &dyn GuestMemory, cpu: &CpuAbi, offset: u32) -> Result<u32> {
    Ok(m.u32(at(cpu.esp, offset)?)?)
}

fn fp_supported(cpu: &CpuAbi) -> Result<()> {
    // All SSE exceptions masked, round to nearest, gradual underflow.
    if cpu.mxcsr & !0x3f != 0x1f80 {
        return Err(Refusal::Unsupported);
    }
    Ok(())
}

fn tag(m: &dyn GuestMemory, table: u32, datum: u32) -> Result<u32> {
    if datum == NONE {
        return Err(Refusal::Layout);
    }
    let entry = at(table, (datum & 0xffff) * 16)?;
    if m.u32(at(entry, 4)?)? != datum {
        return Err(Refusal::Layout);
    }
    ptr(m, at(entry, 8)?)
}

#[derive(Clone, Copy)]
struct Pool {
    data: u32,
    end: u32,
    stride: u32,
}

impl Pool {
    fn read(m: &dyn GuestMemory, header: u32, stride: u32) -> Result<Self> {
        let end = m.u32(at(header, 0x38)?)?;
        if !(1..=0x10000).contains(&stride) || end > 0x10000 || m.u32(at(header, 0x24)?)? != stride
        {
            return Err(Refusal::Layout);
        }
        Ok(Self {
            data: ptr(m, at(header, 0x44)?)?,
            end,
            stride,
        })
    }

    fn slot(self, m: &dyn GuestMemory, datum: u32) -> Result<u32> {
        let i = datum & 0xffff;
        if i >= self.end || datum == NONE || datum >> 16 == 0 {
            return Err(Refusal::Layout);
        }
        let slot = at(self.data, i * self.stride)?;
        if m.u16(slot)? != (datum >> 16) as u16 {
            return Err(Refusal::Layout);
        }
        Ok(slot)
    }

    fn body(self, m: &dyn GuestMemory, datum: u32) -> Result<u32> {
        ptr(m, at(self.slot(m, datum)?, 8)?)
    }
}

struct Write {
    addr: u32,
    bytes: Bytes,
}

enum Bytes {
    Byte([u8; 1]),
    Half([u8; 2]),
    Word([u8; 4]),
    Array(Vec<u8>),
}

impl Bytes {
    fn slice(&self) -> &[u8] {
        match self {
            Self::Byte(bytes) => bytes,
            Self::Half(bytes) => bytes,
            Self::Word(bytes) => bytes,
            Self::Array(bytes) => bytes,
        }
    }
}

#[derive(Default)]
struct Output {
    writes: Vec<Write>,
    eax: Option<u32>,
    ecx: Option<u32>,
    edx: Option<u32>,
    al: Option<u8>,
    pop: u32,
}

impl Output {
    fn add(&mut self, addr: u32, bytes: Bytes) {
        self.writes.push(Write { addr, bytes });
    }

    fn byte(&mut self, addr: u32, value: u8) {
        self.add(addr, Bytes::Byte([value]));
    }

    fn half(&mut self, addr: u32, value: u16) {
        self.add(addr, Bytes::Half(value.to_le_bytes()));
    }

    fn word(&mut self, addr: u32, value: u32) {
        self.add(addr, Bytes::Word(value.to_le_bytes()));
    }

    fn array(&mut self, addr: u32, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            self.add(addr, Bytes::Array(bytes));
        }
    }

    fn check_write(&self, mem: &Guest) -> Result<()> {
        for Write { addr, bytes } in &self.writes {
            mem.check(*addr, bytes.slice().len(), Access::Write, 1)
                .map_err(|_| Refusal::Memory)?;
        }
        Ok(())
    }

    fn publish(&self, mem: &mut Guest, pc: u32) -> Result<(), GuestFault> {
        for Write { addr, bytes } in &self.writes {
            mem.write_bytes_from(pc, *addr, bytes.slice(), 1)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Seam {
    Function(&'static Point, u32),
    Sites(&'static Point, &'static Point, SceneList),
    CallSite(&'static Point, u32, SceneList),
}

/// The stage can depend on the view kind, which is read by the builder.
fn install(
    b: &mut Builder,
    seam: Seam,
    build: impl Fn(&dyn GuestMemory, &CpuAbi) -> (SceneList, Result<Output>) + Send + Sync + 'static,
) {
    let point = match seam {
        Seam::Function(point, _) | Seam::Sites(point, _, _) | Seam::CallSite(point, _, _) => {
            point
        }
    };
    b.raw()
        .before(point, "build scene list", move |c| {
            if c.addr != point.base {
                return Ok(Flow::Continue);
            }
            let runtime = Arc::clone(c.ctx.runtime());
            let missing = match seam {
                Seam::Function(_, _) => None,
                Seam::Sites(_, ready, stage) => {
                    (!runtime.read_dispatch(|d| d.has_generated(ready.base))).then_some(stage)
                }
                Seam::CallSite(_, ready, stage) => {
                    (!runtime.read_dispatch(|d| d.has_resume(ready))).then_some(stage)
                }
            };
            if let Some(stage) = missing {
                stage.count(SceneListResult::Fallback);
                stage.count(SceneListResult::Layout);
                return Ok(Flow::Continue);
            }
            let mut mem = runtime.memory();
            let (stage, output) = build(&*mem, c.cpu);
            let output = match output.and_then(|output| output.check_write(&mem).map(|()| output)) {
                Ok(output) => output,
                Err(reason) => {
                    stage.count(SceneListResult::Fallback);
                    stage.count(match reason {
                        Refusal::Memory => SceneListResult::Memory,
                        Refusal::Layout => SceneListResult::Layout,
                        Refusal::Unsupported => SceneListResult::Unsupported,
                        Refusal::Residency => SceneListResult::Residency,
                        Refusal::Lighting => SceneListResult::Lighting,
                        Refusal::Palette => SceneListResult::Palette,
                        Refusal::ObjectModel => SceneListResult::ObjectModel,
                    });
                    return Ok(Flow::Continue);
                }
            };
            // Write preflight and publication share the same memory guard.
            // Read the return address before making any guest writes.
            let transfer = match seam {
                Seam::Function(_, pop) => match mem.ret(c.cpu, pop) {
                    Ok(transfer) => transfer,
                    Err(_) => {
                        stage.count(SceneListResult::Fallback);
                        stage.count(SceneListResult::Memory);
                        return Ok(Flow::Continue);
                    }
                },
                Seam::Sites(_, ready, _) => TransferAbi::jump(ready.base),
                Seam::CallSite(_, ready, _) => TransferAbi::resume(ready),
            };
            output.publish(&mut mem, c.addr)?;
            if let Some(eax) = output.eax {
                c.cpu.eax = eax;
            }
            if let Some(ecx) = output.ecx {
                c.cpu.ecx = ecx;
            }
            if let Some(edx) = output.edx {
                c.cpu.edx = edx;
            }
            c.cpu.esp = c.cpu.esp.wrapping_add(output.pop);
            if let Some(al) = output.al {
                c.cpu.eax = (c.cpu.eax & !0xff) | u32::from(al);
            }
            stage.count(SceneListResult::Native);
            Ok(Flow::Return(transfer))
        })
        .optional()
        .context_only()
        .last();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lod_scale_comes_from_the_supplied_view() {
        use engine::memory::FlatMemory;
        let address = 0x10000;
        let scale = ViewLodScale::new(address);
        let mut live = FlatMemory::new(address, 4);
        assert_eq!(scale.read(&live), 1.0);
        live.set_f32(address, 6.0).unwrap();
        let render_snapshot = live.clone();
        live.set_f32(address, 0.5).unwrap();
        assert_eq!(scale.read(&live), 0.5);
        assert_eq!(scale.read(&render_snapshot), 6.0);

        for invalid in [-1.0, f32::NAN, f32::INFINITY] {
            live.set_f32(address, invalid).unwrap();
            assert_eq!(scale.read(&live), 1.0);
        }
        assert_eq!(scale.read(&FlatMemory::new(address + 4, 4)), 1.0);
    }

    #[test]
    fn native_scene_lists_follow_the_master_switch() {
        for (spec, expected) in [("", true), ("impl.render.native=off", false), ("impl.render=off", true)] {
            assert_eq!(enabled(&Switches::parse(spec).unwrap()), expected);
        }
    }
}
