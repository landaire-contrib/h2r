//! Native UI and HUD hooks, on with the native renderer
//! (`impl.render.native`, on by default).
//! Producers may skip emission after a complete native reservation. The UI
//! walk keeps the game's order, state setup and layout callbacks.

mod backdrop;
mod model;
mod outline;
mod ui;

use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use runtime::abi::TransferAbi;
use timeline::native::{Metric, Stage};

use hooks::{BuildError, Builder, Feature, Flow, Manifest};

pub const ID: &str = "native-hud";

pub use outline::{PUSH_BYTES as OUTLINE_PUSH_BYTES, Vertices as OutlineVertices};
pub use ui::UiScope;

/// Whether the native UI and HUD are on.
pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches)
}

use points::scene_effects::HUD_QUAD_READY;
const HUD_QUAD_TAIL: u32 = 0x0022_a854;
const HUD_QUAD_LOCAL_BYTES: u32 = 0x70;
const HUD_QUAD_VERTEX_BYTES: u32 = 0x1c;

/// The four vertices after the guest resolves the engine bitmap and
/// multiplies the texture coordinates by its live dimensions.
pub type HudVertices = [[(u8, [f32; 4]); 3]; 4];

fn vertices(mem: &dyn GuestMemory, frame: u32, colour: u32) -> Option<HudVertices> {
    // The helper writes white to register 9 before the vertex loop.
    if colour != u32::MAX {
        return None;
    }
    let base = frame.checked_sub(HUD_QUAD_LOCAL_BYTES)?;
    let mut vertices = [[(0, [0.0; 4]); 3]; 4];
    for (i, vertex) in vertices.iter_mut().enumerate() {
        let at = base.checked_add(i as u32 * HUD_QUAD_VERTEX_BYTES)?;
        let read = |o| {
            let v = f32::from_bits(mem.u32(at.checked_add(o)?).ok()?);
            (v == 0.0 || v.is_normal()).then_some(v)
        };
        *vertex = [
            (9, [1.0; 4]),
            (3, [read(16)?, read(20)?, 0.0, 1.0]),
            (0, [read(0)?, read(4)?, read(8)?, read(12)?]),
        ];
    }
    Some(vertices)
}

fn walk_quad(mem: &mut dyn GuestMemory, frame: u32, colour: u32) -> bool {
    Stage::Ui.count(Metric::WalkExamined, 1);
    let Some(vertices) = vertices(mem, frame, colour) else {
        Stage::Ui.count(Metric::WalkRefusedInputs, 1);
        return false;
    };
    let drawn = host().is_some_and(|host| host.draw_quad(mem, &vertices));
    ui::record(drawn);
    drawn
}

/// What the screen draws become: boot's glue over the GPU and the device.
pub trait Host: Send + Sync {
    fn draw_quad(&self, _mem: &mut dyn GuestMemory, _vertices: &HudVertices) -> bool {
        false
    }

    fn screen_fan(&self, _mem: &mut dyn GuestMemory, _vertices: &HudVertices) -> bool {
        false
    }

    fn fade_fan(&self, _mem: &mut dyn GuestMemory, _vertices: &[[(u8, [f32; 4]); 1]; 4]) -> bool {
        false
    }

    fn loading_bitmap_fan(&self, _mem: &mut dyn GuestMemory, _vertices: &[[(u8, [f32; 4]); 2]; 4]) -> bool {
        false
    }

    fn backdrop_wash(&self, _mem: &mut dyn GuestMemory, _vertices: &HudVertices) -> bool {
        false
    }

    fn backdrop_composite(
        &self,
        _mem: &mut dyn GuestMemory,
        _vertices: &engine::ui_backdrop::Fan<2>,
    ) -> bool {
        false
    }

    fn model_ranges(&self, _mem: &mut dyn GuestMemory, _ranges: &[engine::ui_model::Range]) -> bool {
        false
    }

    fn outline(&self, _mem: &mut dyn GuestMemory, _vertices: &OutlineVertices) -> bool {
        false
    }

    fn inline_ui_quad(&self, _mem: &mut dyn GuestMemory, _primitive: u32, _bytes: Vec<u8>) -> bool {
        false
    }

    fn enter_ui(&self, _mem: &mut dyn GuestMemory, _scope: UiScope, _stack: u32, _eligible: bool) {}

    fn leave_ui(&self, _mem: &mut dyn GuestMemory, _scope: UiScope, _stack: u32) {}
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);

/// Installs the host (boot). Without one nothing is planned or answered.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

fn host() -> Option<Arc<dyn Host>> {
    // HOST is only held to clone the host: never across its calls.
    HOST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub struct NativeHud;

impl Feature for NativeHud {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            ID,
            "The HUD's and the UI's screen draws met by the native GPU renderer",
        )
        .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) {
            return Ok(());
        }
        ui::place(b);
        b.raw().before(&HUD_QUAD_READY, "native_hud_walk_quad", |c| {
            let rt = Arc::clone(c.ctx.runtime());
            if !rt.read_dispatch(|dispatch| dispatch.has_resume(HUD_QUAD_TAIL)) {
                return Ok(Flow::Continue);
            }
            let Some(esp) = c.cpu.esp.checked_add(4) else { return Ok(Flow::Continue) };
            let mut mem = rt.memory();
            if mem.u32(c.cpu.esp).ok() != Some(7)
                || !walk_quad(&mut *mem, c.cpu.ebp, c.cpu.ebx)
            {
                return Ok(Flow::Continue);
            }
            // The fan primitive word is already on the stack.
            c.cpu.esp = esp;
            Ok(Flow::Return(TransferAbi::resume(HUD_QUAD_TAIL)))
        }).context_only().optional();
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod walk_tests {
    use super::*;
    use engine::memory::FlatMemory;

    #[test]
    fn the_hud_walk_uses_the_live_corner_order_and_separate_uv_register() {
        let mut mem = FlatMemory::new(0x1000, 0x1000);
        let frame = 0x1800;
        for i in 0..4 {
            let at = frame - HUD_QUAD_LOCAL_BYTES + i * HUD_QUAD_VERTEX_BYTES;
            for (j, word) in [i as f32, -0.0, 0.5, 2.0, 10.0 + i as f32, 20.0].into_iter().enumerate() {
                mem.set_u32(at + 4 * j as u32, word.to_bits()).unwrap();
            }
        }
        let quad = vertices(&mem, frame, u32::MAX).unwrap();
        assert_eq!(quad[2][0], (9, [1.0; 4]));
        assert_eq!(quad[2][1], (3, [12.0, 20.0, 0.0, 1.0]));
        assert_eq!(quad[2][2].0, 0);
        assert_eq!(quad[2][2].1.map(f32::to_bits), [2.0, -0.0, 0.5, 2.0].map(f32::to_bits));
        assert!(vertices(&mem, frame, 0).is_none());
        mem.set_u32(frame - HUD_QUAD_LOCAL_BYTES, f32::NAN.to_bits()).unwrap();
        assert!(vertices(&mem, frame, u32::MAX).is_none());
    }
}
