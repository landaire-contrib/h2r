//! `native-post`: the view's post passes drawn by the native renderer
//! (docs/graphics/native-post-hud.md, "Stage 5: post and HUD"). On with
//! the native renderer (`impl.render.native`, on by default).
//!
//! The colour pass and each screen effect run their guest code. Entry
//! hooks supply the call's arguments and stack identity, and put them in
//! the frame step's scene under the view's post passes
//! (`scene_step::post_colour`, `post_effect`). Return hooks close that
//! exact call before its caller can issue another draw.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use runtime::abi::TransferAbi;
use timeline::native::{Metric, Stage};

use hooks::{BuildError, Builder, Feature, Flow, Manifest};

pub const ID: &str = "native-post";

use points::scene_effects::SCREEN_STRIP_READY;
pub const SCREEN_STRIP_TAIL: u32 = 0x0002_d8b2;
const SCREEN_FRAME_BYTES: u32 = 0xa8;

/// Values left live at the end of the screen strip's guest vertex loop.
pub struct StripTail {
    pub edi: u32,
    pub ebx: u32,
}

fn walk_strip(
    mem: &mut dyn GuestMemory, esp: u32, mxcsr: u32,
) -> Result<Option<StripTail>, runtime::GuestFault> {
    let tail = (|| {
        Some(StripTail { edi: mem.u32(esp.checked_add(0x34)?).ok()?, ebx: mem.u32(esp.checked_add(0x50)?).ok()? })
    })();
    let Some(tail) = tail else { return Ok(None) };
    let Some(call_esp) = esp.checked_add(SCREEN_FRAME_BYTES) else { return Ok(None) };
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return Ok(None) };
    if host.draw_strip(mem, esp, call_esp, mxcsr)? {
        Stage::Post.count(Metric::WalkSuppressed, 1);
        Ok(Some(tail))
    } else {
        Ok(None)
    }
}

/// Whether the native post is on.
pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches)
}

/// The caller's floating-point state at the hook: the numbers are the
/// game's bit for bit only under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fp {
    pub mxcsr: u32,
    pub x87_control: u16,
}

/// Which guest call owns a post transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostCall {
    Colour,
    Screen,
}

impl PostCall {
    fn cleanup(self) -> u32 {
        match self {
            Self::Colour => 28,
            Self::Screen => 16,
        }
    }
}

/// The guest stack at entry, before the feature chain installs its return
/// continuation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallSite {
    pub kind: PostCall,
    pub esp: u32,
    pub ret: u32,
}

/// What the colour pass becomes: boot's glue over the GPU and the device.
pub trait Host: Send + Sync {
    fn draw_strip(
        &self, _mem: &mut dyn GuestMemory, _esp: u32, _call_esp: u32, _mxcsr: u32,
    ) -> Result<bool, runtime::GuestFault> {
        Ok(false)
    }

    /// The call of `0x000214f0` with `args` (its seven stack words) is
    /// about to run: drop its quads and draw the glow natively. `true`:
    /// the device is armed.
    fn colour_pass(&self, mem: &mut dyn GuestMemory, site: CallSite, args: [u32; 7], fp: Fp) -> bool;

    /// The call of `0x0002d140` described by `call` is about to run.
    /// `true`: the device is armed. By default nothing is.
    fn screen_effect(&self, mem: &mut dyn GuestMemory, site: CallSite, call: ScreenEffect, fp: Fp) -> bool {
        let _ = (mem, site, call, fp);
        false
    }

    /// Finishes the call whose entry frame was at `esp`.
    fn finish(&self, _mem: &mut dyn GuestMemory, _kind: PostCall, _esp: u32) {}
}


/// One call of `0x0002d140`: EAX and its four stack words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenEffect {
    /// The `egor` tag datum.
    pub datum: u32,
    /// The address of its four values.
    pub values: u32,
    pub columns: u32,
    pub rows: u32,
    /// The address of the corners (x, y, u, v each).
    pub corners: u32,
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
static CALLS: AtomicU64 = AtomicU64::new(0);
static ARMED: AtomicU64 = AtomicU64::new(0);
static EFFECTS: AtomicU64 = AtomicU64::new(0);
static EFFECTS_ARMED: AtomicU64 = AtomicU64::new(0);

/// Installs the host (boot). Without one nothing is armed.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

/// The calls so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Calls of the colour pass.
    pub calls: u64,
    /// Calls the device was armed for.
    pub armed: u64,
    /// Calls of `0x0002d140`.
    pub effects: u64,
    /// Those the device was armed for.
    pub effects_armed: u64,
}

pub fn counts() -> Counts {
    Counts {
        calls: CALLS.load(Ordering::Relaxed),
        armed: ARMED.load(Ordering::Relaxed),
        effects: EFFECTS.load(Ordering::Relaxed),
        effects_armed: EFFECTS_ARMED.load(Ordering::Relaxed),
    }
}

/// The hook's body on any guest memory: `esp` at the entry (the return
/// address, then the seven words). `true` when the host armed the device.
pub fn on_colour_pass(mem: &mut dyn GuestMemory, esp: u32, fp: Fp) -> bool {
    CALLS.fetch_add(1, Ordering::Relaxed);
    let mut args = [0u32; 7];
    for (i, a) in args.iter_mut().enumerate() {
        let Ok(v) = mem.u32(esp.wrapping_add(4 + 4 * i as u32)) else {
            return false;
        };
        *a = v;
    }
    // The frame step's scene takes the call's arguments by value
    // (docs/graphics/framescene.md, slice 5), whatever the host does.
    crate::scene_step::post_colour(&*mem, args);
    // HOST is only held to clone the host: never across its calls.
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return false;
    };
    let Ok(ret) = mem.u32(esp) else { return false };
    let site = CallSite { kind: PostCall::Colour, esp, ret };
    let armed = host.colour_pass(mem, site, args, fp);
    if armed {
        ARMED.fetch_add(1, Ordering::Relaxed);
    }
    armed
}

/// The screen-effect hook's body: `eax` the datum, `esp` at the entry
/// (the return address, then the four words). `true` when the host armed
/// the device.
pub fn on_screen_effect(mem: &mut dyn GuestMemory, eax: u32, esp: u32, fp: Fp) -> bool {
    EFFECTS.fetch_add(1, Ordering::Relaxed);
    let mut w = [0u32; 4];
    for (i, a) in w.iter_mut().enumerate() {
        let Ok(v) = mem.u32(esp.wrapping_add(4 + 4 * i as u32)) else {
            return false;
        };
        *a = v;
    }
    let call = ScreenEffect {
        datum: eax,
        values: w[0],
        columns: w[1],
        rows: w[2],
        corners: w[3],
    };
    crate::scene_step::post_effect(&*mem, call.datum, call.values, call.columns, call.rows);
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return false;
    };
    let Ok(ret) = mem.u32(esp) else { return false };
    let site = CallSite { kind: PostCall::Screen, esp, ret };
    let armed = host.screen_effect(mem, site, call, fp);
    if armed {
        EFFECTS_ARMED.fetch_add(1, Ordering::Relaxed);
    }
    armed
}

/// The return continuation runs before the guest caller resumes.
pub fn finish(mem: &mut dyn GuestMemory, kind: PostCall, esp: u32) {
    if let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        host.finish(mem, kind, esp.wrapping_sub(4 + kind.cleanup()));
    }
}

pub struct NativePost;

impl Feature for NativePost {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The view's post passes drawn by the native GPU renderer").doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // Stateless, on or off: no history carried between frames (no
        // temporal AA, no exposure adaptation); every call is drawn from
        // what the guest asks for at its entry. The host's screen effects
        // are keyed by the map name it reads from guest RAM, and its proof
        // and fallback sets (by effect tag) only ever send an effect back
        // to the D3D path, which draws the same picture.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw()
            .before(&points::COLOUR_PASS, "native_post_colour", |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let fp = Fp {
                    mxcsr: c.cpu.mxcsr,
                    x87_control: c.cpu.x87.control,
                };
                let mut mem = rt.memory();
                on_colour_pass(&mut *mem, c.cpu.esp, fp);
                Ok(Flow::Continue)
            })
            .context_only();
        b.raw().after(&points::COLOUR_PASS, "native_post_colour_exit", |c| {
            let rt = Arc::clone(c.ctx.runtime());
            finish(&mut *rt.memory(), PostCall::Colour, c.cpu.esp);
            Ok(())
        });
        b.raw().before(&SCREEN_STRIP_READY, "native_post_walk_strip", |c| {
            let rt = Arc::clone(c.ctx.runtime());
            if !rt.read_dispatch(|dispatch| dispatch.has_resume(SCREEN_STRIP_TAIL)) {
                return Ok(Flow::Continue);
            }
            let Some(tail) = walk_strip(&mut *rt.memory(), c.cpu.esp, c.cpu.mxcsr)? else {
                return Ok(Flow::Continue);
            };
            c.cpu.edi = tail.edi;
            c.cpu.ebx = tail.ebx;
            c.cpu.ebp = 0;
            Ok(Flow::Return(TransferAbi::resume(SCREEN_STRIP_TAIL)))
        }).context_only().optional();
        b.raw()
            .before(&points::SCREEN_EFFECT, "native_post_screen_effect", |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let fp = Fp {
                    mxcsr: c.cpu.mxcsr,
                    x87_control: c.cpu.x87.control,
                };
                let mut mem = rt.memory();
                if crate::scope_zoom_hud::hud_profile(&*mem, c.cpu.eax, c.cpu.esp).is_none() {
                    on_screen_effect(&mut *mem, c.cpu.eax, c.cpu.esp, fp);
                }
                Ok(Flow::Continue)
            })
            .context_only();
        b.raw().after(&points::SCREEN_EFFECT, "native_post_screen_exit", |c| {
            let rt = Arc::clone(c.ctx.runtime());
            finish(&mut *rt.memory(), PostCall::Screen, c.cpu.esp);
            Ok(())
        });
        Ok(())
    }
}
