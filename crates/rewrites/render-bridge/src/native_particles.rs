//! Particle submission walk, on with the native renderer
//! (`impl.render.native`). The helper's parents still run: especially the
//! state updates in `0x00174420`.
//! A host may replace only the helper's single, fully captured draw, after
//! every target copy is prepared. It must preserve attribute carry/state.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use hooks::{BuildError, Builder, Feature, Flow, Manifest};
use engine::memory::GuestMemory;
use engine::points::{Point, Sig};
use timeline::native::{Metric, Stage};
use native_render::camera::GuestRead;
use native_render::effect_geometry::{self, DynamicCall};
use native_render::particles::{self, Emission};

pub const ID: &str = "native-particles";

/// Refusal lines printed per kind before they go quiet.
pub(crate) const LOG_LIMIT: u64 = 8;

pub use engine::points::scene_effects::{
    EFFECT_END as DYNAMIC_END, EFFECT_PUBLISH as DYNAMIC_PUBLISH, EFFECT_QUAD_READY as DYNAMIC_QUAD_READY,
    PARTICLE_EMIT_READY as EMIT_READY, PARTICLE_END as EMIT_END, PARTICLE_PUBLISH as EMIT_PUBLISH,
};
pub const DYNAMIC: Point = Point::new(
    "EFFECT_DYNAMIC",
    effect_geometry::DYNAMIC_DRAW,
    Sig::Site,
    "docs/graphics/native-particles.md",
);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Producer {
    Particle,
    Dynamic,
    Immediate(native_render::effect_immediate::Site),
}

pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches)
}

#[derive(Clone, Debug)]
pub struct Call {
    pub esp: u32,
    pub return_address: u32,
    pub emission: Emission,
}

pub trait Host: Send + Sync {
    fn try_emit(&self, _mem: &mut dyn GuestMemory, _call: &Call) -> bool { false }
    fn try_dynamic(&self, _mem: &mut dyn GuestMemory, _esp: u32, _data: Option<u32>) -> bool { false }

    /// Arm one call-scoped capture, not a global "next quad" match. False
    /// leaves the entire call on D3D. True is *armed*, never proof of a
    /// replaced draw. The adapter cancels on return with no matching draw,
    /// or on any state/order/geometry mismatch. No prefix may be dropped.
    fn arm(&self, mem: &mut dyn GuestMemory, call: &Call) -> bool;
    fn arm_dynamic(&self, mem: &mut dyn GuestMemory, call: &DynamicCall) -> bool;
    fn arm_effect(&self, mem: &mut dyn GuestMemory, site: native_render::effect_immediate::Site) -> bool;
    fn publish(&self, mem: &mut dyn GuestMemory, producer: Producer, data: u32);
    fn finish(&self, mem: &mut dyn GuestMemory, producer: Producer);
}

pub(crate) static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
static CALLS: AtomicU64 = AtomicU64::new(0);
static DYNAMIC_CALLS: AtomicU64 = AtomicU64::new(0);
static DECODED: AtomicU64 = AtomicU64::new(0);
static ARMED: AtomicU64 = AtomicU64::new(0);

pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}
pub fn counts() -> (u64, u64, u64) {
    (
        CALLS.load(Ordering::Relaxed),
        DECODED.load(Ordering::Relaxed),
        ARMED.load(Ordering::Relaxed),
    )
}

struct Read<'a>(&'a dyn GuestMemory);
impl GuestRead for Read<'_> {
    fn read_u32(&self, a: u32) -> Option<u32> {
        self.0.u32(a).ok()
    }
}

/// The walk's emitter: draws the call natively when the host takes it,
/// otherwise arms the call's capture and lets the guest execute. Snapshot
/// creation and successful command recording are the adapter's
/// responsibility, never inferred from arming.
fn emit(mem: &mut dyn GuestMemory, esp: u32, ebx: u32) -> bool {
    CALLS.fetch_add(1, Ordering::Relaxed);
    Stage::Particles.count(Metric::WalkExamined, 1);
    let Ok(emission) = particles::read_emission(&Read(mem), esp, ebx) else {
        Stage::Particles.count(Metric::WalkRefusedInputs, 1);
        return false;
    };
    let Ok(return_address) = mem.u32(esp) else {
        return false;
    };
    DECODED.fetch_add(1, Ordering::Relaxed);
    let call = Call {
        esp,
        return_address,
        emission,
    };
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if host.as_ref().is_some_and(|h| h.try_emit(mem, &call)) {
        Stage::Particles.count(Metric::WalkSuppressed, 1);
        return true;
    }
    Stage::Particles.count(Metric::WalkRefusedReservation, 1);
    if host.as_ref().is_some_and(|h| h.arm(mem, &call)) {
        ARMED.fetch_add(1, Ordering::Relaxed);
    }
    false
}

pub fn on_dynamic(mem: &mut dyn GuestMemory, esp: u32, stride: u32) {
    let call = match effect_geometry::read_call(&Read(mem), esp, stride) {
        Ok(call) => call,
        Err(detail) => {
            static REFUSED: AtomicU64 = AtomicU64::new(0);
            if REFUSED.fetch_add(1, Ordering::Relaxed) < LOG_LIMIT {
                let builder = esp.checked_add(16).and_then(|at| mem.u32(at).ok());
                let reason = if matches!(
                    builder,
                    Some(effect_geometry::CONTRAIL_BUILD | effect_geometry::EFFECT_QUAD_BUILD)
                ) {
                    "DynamicDeclarationOrMode"
                } else {
                    "UnownedDynamicBuilder"
                };
                eprintln!("native-particles-refused: dynamic builder={builder:?} reason={reason} detail={detail:?}");
            }
            return;
        }
    };
    if call.return_address == engine::points::ui_render::RECT_SUBMIT_RETURN {
        return;
    }
    DYNAMIC_CALLS.fetch_add(1, Ordering::Relaxed);
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(h) = host {
        h.arm_dynamic(mem, &call);
    }
}
fn boundary(mem: &mut dyn GuestMemory, producer: Producer, data: Option<u32>) {
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(h) = host {
        if let Some(data) = data {
            h.publish(mem, producer, data);
        } else {
            h.finish(mem, producer);
        }
    }
}

struct DynamicTail {
    edi: u32,
    esi: u32,
    ebx: u32,
    return_slot: u32,
}

fn walk_dynamic(mem: &mut dyn GuestMemory, esp: u32, data: Option<u32>) -> Option<DynamicTail> {
    let return_slot = esp.checked_add(12)?;
    let tail = DynamicTail {
        edi: mem.u32(esp).ok()?,
        esi: mem.u32(esp + 4).ok()?,
        ebx: mem.u32(esp + 8).ok()?,
        return_slot,
    };
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
    host.try_dynamic(mem, return_slot, data).then_some(tail)
}

fn return_dynamic(c: &mut hooks::Call<'_>, tail: DynamicTail) -> Result<Flow, runtime::GuestFault> {
    c.cpu.edi = tail.edi;
    c.cpu.esi = tail.esi;
    c.cpu.ebx = tail.ebx;
    c.cpu.esp = tail.return_slot;
    c.cpu.eax = u32::MAX;
    c.ret(20).map(Flow::Return)
}

pub struct NativeParticles;
impl Feature for NativeParticles {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            ID,
            "Native dynamic particle submissions; guest updates preserved",
        )
        .doc("docs/graphics/native-particles.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw().before(&EMIT_READY, "native_particles_walk_emit", |c| {
            let rt = Arc::clone(c.ctx.runtime());
            let Some(esp) = c.cpu.esp.checked_add(4) else { return Ok(Flow::Continue) };
            let saved_ebp = {
                let mut mem = rt.memory();
                let Ok(ebp) = mem.u32(c.cpu.esp) else { return Ok(Flow::Continue) };
                if !emit(&mut *mem, esp, c.cpu.ebx) {
                    return Ok(Flow::Continue);
                }
                ebp
            };
            c.cpu.ebp = saved_ebp;
            c.cpu.esp = esp;
            c.ret(u32::from(particles::STACK_BYTES)).map(Flow::Return)
        }).context_only().optional();
        b.raw()
            .observe_before(&DYNAMIC, "native_particles_dynamic", |o| {
                let mut mem = o.guest();
                on_dynamic(&mut *mem, o.cpu.esp, o.cpu.ecx);
            })
            .context_only();
        b.raw()
            .observe_before(&EMIT_PUBLISH, "native_particles_publish", |o| {
                let mut mem = o.guest();
                boundary(&mut *mem, Producer::Particle, Some(0));
            })
            .context_only()
            .optional();
        b.raw().before(&DYNAMIC_QUAD_READY, "native_effects_walk_quad", |c| {
            let rt = Arc::clone(c.ctx.runtime());
            let tail = walk_dynamic(&mut *rt.memory(), c.cpu.esp, None);
            match tail {
                Some(tail) => return_dynamic(c, tail),
                None => Ok(Flow::Continue),
            }
        }).context_only().optional();
        b.raw()
            .before(&DYNAMIC_PUBLISH, "native_particles_dynamic_publish", |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                let data = c.cpu.esi;
                if let Some(tail) = walk_dynamic(&mut *mem, c.cpu.esp, Some(data)) {
                    drop(mem);
                    return return_dynamic(c, tail);
                }
                boundary(&mut *mem, Producer::Dynamic, Some(data));
                Ok(Flow::Continue)
            })
            .context_only()
            .optional();
        for (point, producer) in [
            (&EMIT_END, Producer::Particle),
            (&DYNAMIC_END, Producer::Dynamic),
        ] {
            b.raw()
                .observe_before(point, "native_particles_end", move |o| {
                    let mut mem = o.guest();
                    boundary(&mut *mem, producer, None);
                })
                .context_only()
                .optional();
        }
        crate::native_effects::build(b);
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}
