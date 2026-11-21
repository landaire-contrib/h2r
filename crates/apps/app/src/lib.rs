//! The host shell the recompiled game runs inside (see docs/runtime/app.md).
//!
//! [`run`] owns the macOS main thread: it opens the window, creates the wgpu
//! device on a Metal surface, opens the audio device, then starts the guest
//! on a worker thread with a [`GuestHost`]. The guest side uses the host
//! for everything platform-shaped:
//!
//! * [`GuestHost::gfx_context`] is the device to build `d3d::GfxBackend` on,
//!   and [`GuestHost::present_hook`] the closure for `GfxBackend::on_present`;
//! * [`GuestHost::take_input`] is the `InputManager` (keyboard, mouse and
//!   pads) to build `xapi::Xapi` on, and [`pump_input`] runs one input frame;
//! * [`GuestHost::mixer`] is the `audio::Mixer` the output device plays, to
//!   build `dsound::DirectSound` on;
//! * [`GuestHost::tick`] paces guest time to the wall clock, blocks while
//!   paused and says when to stop.
//!
//! [`GuestHost::headless`] gives the same interface with no window, so the
//! guest-side code is the same for `h2 run --headless` and tests.

pub mod audio_out;
#[cfg(any(target_vendor = "apple", windows))]
pub mod embedded;
pub mod fps;
pub mod frame;
pub mod input_pump;
#[cfg(not(any(target_os = "tvos", target_os = "ios")))]
pub mod keymap;
pub mod lifecycle;
pub mod mailbox;
pub mod menu_overlay;
#[cfg(target_os = "macos")]
mod metal_resize;
pub mod overlay;
pub mod pacing;
pub mod tile;
pub mod scale;
#[cfg(not(any(target_os = "tvos", target_os = "ios")))]
mod shell;
#[cfg(target_os = "linux")]
mod steam_keyboard;
mod surface;
pub mod watermark;
pub mod worker;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gfx::GfxContext;
use gfx::surface::{SurfaceId, SurfaceStore};
use input::InputManager;
use input::backend::keyboard::KeyboardMouse;
use input::backend::scripted::ScriptedInput;
use input::bindings::Bindings;

pub mod theater_overlay;

pub use frame::{Compositor, Filter, FrameError, FrameSink, FrameSource, FrameStats, ShownFrame};
pub use input_pump::{PumpReport, pump_input};
pub use lifecycle::{Control, Lifecycle};
pub use overlay::{Banner, Notice, NoticeKind};
pub use pacing::{PaceAction, PacePolicy, PaceStats, Pacer, VBlank, WindowedRate};
pub use scale::{DisplayAspect, NativeFit, Scaling, Viewport};
pub use watermark::{BuildInfo, LiveStats, Watermark};

/// Shell settings.
#[derive(Clone, Debug)]
pub struct AppConfig {
    /// Window title (the shell appends status).
    pub title: String,
    /// Initial inner size in logical points.
    pub window_size: (u32, u32),
    /// Column `.0` of `.1` across the primary screen ([`tile::tile`]),
    /// in place of `window_size` and the system's placement.
    pub tile: Option<(u32, u32)>,
    /// Start in borderless fullscreen.
    pub fullscreen: bool,
    /// Displayed shape of the guest frame.
    pub aspect: DisplayAspect,
    pub scaling: Scaling,
    /// With Native resolution, Source aspect and Fit scaling, the game
    /// image follows the drawable's aspect while the raster catches up.
    pub native_fit: Option<NativeFit>,
    pub filter: Filter,
    /// Wait for the display's vblank when presenting (`Fifo`). Off uses
    /// `AutoNoVsync` (tearing allowed); guest pacing is unaffected either way.
    pub vsync: bool,
    /// How often the window presents (`H2_FPS`, [`fps::FpsTarget`]). The
    /// guest never sees it: 30 is today's shell exactly, and above 30 only
    /// the window's redraw rate changes (no interpolation yet).
    pub fps: fps::FpsTarget,
    /// Changes `fps` while the window runs (the Video screen's Frame Rate
    /// row); `None`: fixed for the run.
    pub fps_control: Option<fps::FpsControl>,
    /// Pause the guest (and audio) while the window is not focused.
    pub pause_on_focus_loss: bool,
    /// Click to capture the mouse for look; Cmd or focus loss releases it.
    pub mouse_capture: bool,
    /// Add physical controllers (GameController on macOS, gilrs elsewhere).
    pub gamepads: bool,
    /// Steam Controllers over raw HID (docs/input/steam-controller.md), added
    /// beside the platform's pads when `gamepads` is on. The caller starts
    /// the thread, so it can share it with the controller speaker.
    pub steam_pads: Option<input::steam_pad::SteamPads>,
    /// Keyboard/mouse shares player one with the first physical pad.
    pub keyboard: bool,
    /// Keyboard and mouse bindings (and stick/trigger deadzones).
    pub bindings: Bindings,
    /// Exclusive timeline source, replacing all live backends. Retain a clone
    /// and call advance_to(guest_ticks) before pump_input. Polls never advance it.
    pub scripted_input: Option<ScriptedInput>,
    /// Open the default audio device. Off (or no device) still gives the
    /// guest a mixer, at [`audio_out::FALLBACK_RATE`].
    pub audio: bool,
    pub pacing: PacePolicy,
    /// When the guest thread returns: keep the window up with the last frame
    /// and the outcome in the title (true), or close.
    pub keep_open_on_guest_exit: bool,
    /// How long a close waits for the guest thread to notice the stop request.
    pub shutdown_timeout: Duration,
    /// Print the window's status line to stderr once a second.
    pub log_status: bool,
    /// Lets another thread (a signal watcher) close the window.
    pub closer: Option<Closer>,
    /// A notice drawn over the game frame (`h2 play --follow`). F6 hides
    /// it; F5 closes the window with [`ExitReport::restart_requested`] set
    /// while it offers a restart. Neither key reaches the guest while a
    /// banner is configured.
    pub banner: Option<Banner>,
    /// The development-build watermark along the bottom of the game image
    /// (docs/runtime/app.md "Watermark"). F3 toggles it; F3 does not reach the
    /// guest while one is configured.
    pub watermark: Option<Watermark>,
    /// Our debug menu (docs/runtime/debug-menu.md), drawn over the game
    /// image while open. Backquote opens and closes it; while it is open
    /// the arrows, Enter, Space, Escape and Backspace drive it and no key
    /// press reaches the guest.
    pub debug_menu: Option<debug_menu::Shared>,
    pub theater: Option<theater_overlay::Overlay>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            title: "h2recomp".into(),
            window_size: (1280, 960),
            tile: None,
            fullscreen: false,
            aspect: DisplayAspect::Standard,
            scaling: Scaling::Fit,
            native_fit: None,
            filter: Filter::Linear,
            vsync: true,
            fps: fps::FpsTarget::Thirty,
            fps_control: None,
            pause_on_focus_loss: false,
            mouse_capture: true,
            gamepads: true,
            steam_pads: None,
            keyboard: true,
            bindings: Bindings::default(),
            scripted_input: None,
            audio: true,
            pacing: PacePolicy::default(),
            keep_open_on_guest_exit: true,
            shutdown_timeout: Duration::from_secs(3),
            log_status: false,
            closer: None,
            banner: None,
            watermark: None,
            debug_menu: None,
            theater: None,
        }
    }
}

/// Closes the window from another thread, the way Cmd+Q does: the guest is
/// asked to stop (which wakes it from a pause or a pacing sleep), then the
/// window closes, also when the guest has already exited and the window is
/// only showing its outcome. A close requested before the window is up
/// takes effect as soon as it is. Cloning shares it.
#[derive(Clone, Default)]
pub struct Closer(Arc<Mutex<CloserState>>);

type Wake = Box<dyn Fn() + Send>;

#[derive(Default)]
struct CloserState {
    requested: bool,
    life: Option<Lifecycle>,
    wake: Option<Wake>,
}

impl std::fmt::Debug for Closer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Closer").field(&self.requested()).finish()
    }
}

impl Closer {
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, CloserState> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Request the close. Repeats are harmless.
    pub fn close(&self) {
        let mut s = self.state();
        s.requested = true;
        if let Some(l) = &s.life {
            l.request_stop();
        }
        if let Some(w) = &s.wake {
            w();
        }
    }

    pub fn requested(&self) -> bool {
        self.state().requested
    }

    /// Connects the shell: `life` is the guest's lifecycle, `wake` asks the
    /// event loop to close.
    #[cfg_attr(any(target_os = "tvos", target_os = "ios"), allow(dead_code))]
    pub(crate) fn attach(&self, life: Lifecycle, wake: Wake) {
        let mut s = self.state();
        if s.requested {
            life.request_stop();
            wake();
        }
        s.life = Some(life);
        s.wake = Some(wake);
    }
}

/// How the guest thread ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuestExit {
    /// The guest function returned `Ok`.
    Returned,
    /// It returned an error (formatted with `{:#}`).
    Failed(String),
    /// It panicked.
    Panicked(String),
    /// It did not stop within `shutdown_timeout` of the close (the process
    /// exiting will end it).
    StillRunning,
    /// The window never came up, so it never started.
    NotStarted,
}

/// What [`run`] returns once the window closes.
#[derive(Clone, Debug)]
pub struct ExitReport {
    pub guest: GuestExit,
    pub frames: FrameStats,
    pub pacing: PaceStats,
    /// The window closed because F5 accepted the banner's restart offer.
    pub restart_requested: bool,
}

/// The guest thread's view of the host. `Send`; moved into the guest thread.
pub struct GuestHost {
    native_size: scale::NativeSize,
    gfx: GfxContext,
    sink: FrameSink,
    input: Option<InputManager>,
    keyboard: Option<KeyboardMouse>,
    mixer: Arc<Mutex<audio::Mixer>>,
    sample_rate: u32,
    life: Lifecycle,
    pacer: Pacer,
    epoch: Instant,
    pace_stats: Arc<Mutex<PaceStats>>,
    /// Keeps a headless channel's receiving end alive.
    _source: Option<FrameSource>,
    /// Consumes a headless host's mixer in real time (the window's host
    /// leaves that to the shell's `AudioOut`).
    _discard: Option<audio_out::DiscardRenderer>,
}

impl GuestHost {
    /// A host with no window: frames are accepted and counted, audio goes to
    /// a mixer consumed in real time by a `DiscardRenderer`, input has only the backends `config` enables
    /// (`keyboard` supplies the player-one fallback; `gamepads` reads real pads).
    /// `scripted_input` replaces both with the runner-controlled timeline.
    pub fn headless(config: &AppConfig) -> Result<Self, gfx::GfxError> {
        let ctx = GfxContext::headless()?;
        let (sink, source) = frame::channel(&ctx);
        let (input, keyboard) = build_input(config);
        let mixer = Arc::new(Mutex::new(audio::Mixer::new(audio_out::FALLBACK_RATE)));
        let life = Lifecycle::new();
        Ok(Self {
            native_size: scale::NativeSize::new(DisplayAspect::Source, Scaling::Fit, (0, 0)),
            gfx: ctx,
            sink,
            input: Some(input),
            keyboard,
            _discard: Some(audio_out::DiscardRenderer::start(
                Arc::clone(&mixer),
                audio_out::FALLBACK_RATE,
                Some(life.clone()),
            )),
            mixer,
            sample_rate: audio_out::FALLBACK_RATE,
            life,
            pacer: Pacer::new(config.pacing),
            epoch: Instant::now(),
            pace_stats: Arc::default(),
            _source: Some(source),
        })
    }

    /// Reads the native render extent in physical pixels. Explicit display
    /// aspects fit inside the drawable. The handle follows window resizes
    /// and can move to the render thread.
    pub fn native_size(&self) -> impl Fn() -> (u32, u32) + Clone + Send + Sync + 'static + use<> {
        let size = self.native_size.clone();
        move || size.get()
    }

    /// Reads the extent the compositor gives a frame of the supplied raster
    /// size. The camera uses its aspect even when the source is anamorphic.
    /// A headless host displays the source at its own size.
    pub fn display_size(
        &self,
    ) -> impl Fn((u32, u32)) -> (u32, u32) + Clone + Send + Sync + 'static + use<> {
        let size = self.native_size.clone();
        move |src| size.display_size(src)
    }

    /// The device every guest GPU object must live on: build
    /// `d3d::GfxBackend::new(host.gfx_context())` from it. Each call returns
    /// another handle to the same instance, adapter, device and queue.
    pub fn gfx_context(&self) -> GfxContext {
        share_context(&self.gfx)
    }

    /// The closure for `d3d::GfxBackend::on_present`. Each call makes a new
    /// sink on the same channel.
    pub fn present_hook(
        &self,
    ) -> impl FnMut(&GfxContext, &SurfaceStore, SurfaceId, u32, Option<&[u8; 768]>) + Send + 'static
    {
        self.sink.another().into_present_hook()
    }

    /// A sink for frames that are not gfx surfaces (CPU pixels, other views).
    pub fn frame_sink(&self) -> FrameSink {
        self.sink.another()
    }

    /// Prepares the host's return-to-menu picture on this device. The callback
    /// switches the window to it on `true` and lets the next published guest
    /// frame replace it after `false`.
    pub fn loading_screen(&self) -> Box<dyn Fn(bool) + Send + Sync> {
        self.sink.loading_screen()
    }

    /// The input manager, once. Build `xapi::Xapi::new(config, input, storage)`
    /// on it and run [`pump_input`] once per guest frame.
    pub fn take_input(&mut self) -> Option<InputManager> {
        self.input.take()
    }

    /// The keyboard pad's handle, for code that wants to inject keys
    /// (tests, scripted input). The window feeds it already.
    pub fn keyboard(&self) -> Option<&KeyboardMouse> {
        self.keyboard.as_ref()
    }

    /// The mixer the audio device renders: `dsound::DirectSound::new(mixer,
    /// layout)`. Created at the device rate, so no host-side resampling.
    pub fn mixer(&self) -> Arc<Mutex<audio::Mixer>> {
        Arc::clone(&self.mixer)
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Stop and pause flags, for checks from inside overrides.
    pub fn lifecycle(&self) -> Lifecycle {
        self.life.clone()
    }

    pub fn should_stop(&self) -> bool {
        self.life.should_stop()
    }

    /// Call between scheduler slices with the kernel's logical clock
    /// (`kernel.clock().counter()`, 100 ns ticks). Blocks while paused,
    /// sleeps while guest time is ahead of wall time, and returns
    /// [`Control::Stop`] once the window has asked the guest to stop.
    pub fn tick(&mut self, guest_ticks: u64) -> Control {
        loop {
            let (waited, c) = self.life.wait_while_paused();
            if c == Control::Stop {
                return c;
            }
            if waited {
                // Wall time ran on while paused; guest time did not.
                self.pacer.reset();
            }
            let action = self.pacer.decide(guest_ticks, self.epoch.elapsed());
            *self.pace_stats.lock().unwrap_or_else(|e| e.into_inner()) = self.pacer.stats();
            if let PaceAction::Sleep(d) = action
                && pace_sleep(&self.life, d) == Control::Stop
            {
                return Control::Stop;
            }
            // A pause that arrived during the pacing sleep (which ends early
            // on pause) is honoured before the guest runs again.
            if self.life.is_paused() {
                continue;
            }
            return if self.life.should_stop() {
                Control::Stop
            } else {
                Control::Continue
            };
        }
    }

    /// Pacing counters so far.
    pub fn pace_stats(&self) -> PaceStats {
        self.pacer.stats()
    }

    /// Handoff counters so far.
    pub fn frame_stats(&self) -> FrameStats {
        self.sink.stats()
    }
}

/// The pacer's sleep: frame stats count it as `pace`, not as guest CPU
/// (`h2 play` wraps its whole tick in `pace`; the Apple and Xbox apps'
/// guest did not), and the host perf line records how long it took against what it asked
/// for (`timeline::host::OVERSLEEP`: the host timer's granularity).
fn pace_sleep(life: &Lifecycle, d: Duration) -> Control {
    let _pace = timeline::phase::enter(timeline::phase::Phase::Pace);
    if !timeline::host::enabled() {
        return life.sleep(d);
    }
    let t = Instant::now();
    let c = life.sleep(d);
    let took = t.elapsed();
    timeline::host::SLEEP.record(took.as_nanos() as u64);
    // A stop or a pause ends the sleep early; that is no oversleep.
    timeline::host::OVERSLEEP.record(took.saturating_sub(d).as_nanos() as u64);
    c
}

/// Another handle on the same wgpu objects (all wgpu handles are shared
/// references).
pub fn share_context(ctx: &GfxContext) -> GfxContext {
    GfxContext {
        instance: ctx.instance.clone(),
        adapter: ctx.adapter.clone(),
        device: ctx.device.clone(),
        queue: ctx.queue.clone(),
        caps: ctx.caps,
    }
}

/// The input manager the guest gets, and the keyboard handle the window
/// keeps. The keyboard and first physical pad share port 0.
fn build_input(config: &AppConfig) -> (InputManager, Option<KeyboardMouse>) {
    let convert = match config.bindings.convert.validate() {
        Ok(()) => config.bindings.convert,
        Err(e) => {
            eprintln!("app: invalid deadzones in bindings ({e}); using defaults");
            Default::default()
        }
    };
    let mut manager = InputManager::new(convert).expect("validated deadzones");
    if let Some(scripted) = &config.scripted_input {
        manager.add_backend(Box::new(scripted.clone()));
        return (manager, None);
    }
    let physical = config
        .gamepads
        .then(|| physical_pads(gamepad_backend(), config.steam_pads.as_ref()))
        .flatten();
    let keyboard = if config.keyboard {
        let kb = KeyboardMouse::new(config.bindings.clone());
        // The agent API's `input.pad` doesn't come through here: it merges
        // over the guest's controller reads at XAPI (crates/extensions/agent's tap).
        manager.add_backend(Box::new(input::backend::fallback::KeyboardFallback::new(
            kb.clone(),
            physical,
            convert,
        )));
        Some(kb)
    } else {
        if let Some(physical) = physical {
            manager.add_backend(physical);
        }
        None
    };
    (manager, keyboard)
}

/// The platform's pads and the Steam Controllers, as one backend.
fn physical_pads(
    platform: Option<Box<dyn input::HostBackend>>,
    steam: Option<&input::steam_pad::SteamPads>,
) -> Option<Box<dyn input::HostBackend>> {
    let Some(steam) = steam else {
        return platform;
    };
    let steam: Box<dyn input::HostBackend> = Box::new(steam.backend());
    Some(match platform {
        Some(p) => Box::new(input::backend::multi::Multi::new(vec![p, steam])),
        None => steam,
    })
}

#[cfg(all(feature = "game-controller", target_vendor = "apple"))]
fn gamepad_backend() -> Option<Box<dyn input::HostBackend>> {
    Some(Box::new(
        input::backend::game_controller::GameControllerPads::new(),
    ))
}

#[cfg(all(feature = "gamepads", not(target_vendor = "apple")))]
fn gamepad_backend() -> Option<Box<dyn input::HostBackend>> {
    match input::backend::gilrs_pads::GilrsPads::new() {
        Ok(p) => Some(Box::new(p)),
        Err(e) => {
            eprintln!("app: gamepads unavailable: {e}");
            None
        }
    }
}

#[cfg(not(any(
    all(feature = "game-controller", target_vendor = "apple"),
    all(feature = "gamepads", not(target_vendor = "apple"))
)))]
fn gamepad_backend() -> Option<Box<dyn input::HostBackend>> {
    eprintln!("app: built without the `gamepads` feature; keyboard only");
    None
}

/// Opens the window and runs `guest` on a worker thread until the window
/// closes. Must be called on the main thread (macOS UI rule; winit returns
/// an error otherwise). Returns after the window has closed and the guest
/// thread has stopped or timed out. Not on tvOS, where the app owns the
/// window: see [`embedded::Embedded`].
#[cfg(not(any(target_os = "tvos", target_os = "ios")))]
pub fn run<F>(config: AppConfig, guest: F) -> anyhow::Result<ExitReport>
where
    F: FnOnce(GuestHost) -> anyhow::Result<()> + Send + 'static,
{
    shell::run(config, Box::new(guest))
}

#[cfg(test)]
mod input_tests {
    use super::*;
    use input::PortIndex;
    use input::backend::scripted::Timeline;

    #[test]
    fn timeline_replaces_live_input_and_advances_only_on_runner_clock() {
        let script =
            Timeline::from_toml(include_str!("../../../../docs/input/input-menu-example.toml"))
                .unwrap()
                .playback();
        let config = AppConfig {
            scripted_input: Some(script.clone()),
            ..Default::default()
        };
        // No GPU, audio device, window or physical controller is needed.
        let (mut input, keyboard) = build_input(&config);
        assert!(keyboard.is_none(), "scripts replace live sources");
        input.sync();
        let h = input.open(PortIndex::new(0).unwrap()).unwrap();
        assert_eq!(input.connected_mask(), 1);
        assert_eq!(input.get_state(h).unwrap().gamepad.analog[0], 0);
        script.advance_to(10_000_000).unwrap();
        input.sync();
        assert_eq!(input.get_state(h).unwrap().gamepad.analog[0], 255);
        input.sync();
        assert_eq!(input.get_state(h).unwrap().gamepad.analog[0], 255);
        script.advance_to(11_000_000).unwrap();
        input.sync();
        assert_eq!(input.get_state(h).unwrap().gamepad.analog[0], 0);
        script.advance_to(12_000_000).unwrap();
        input.sync();
        assert_eq!(input.get_state(h).unwrap().gamepad.buttons.0, 2);
    }

    #[test]
    fn documented_bindings_drive_all_fallback_inputs_and_release_on_focus_loss() {
        let config = AppConfig {
            gamepads: false,
            bindings: Bindings::from_toml(include_str!("../../../../docs/input/input-default.toml"))
                .unwrap(),
            ..Default::default()
        };
        let (mut input, keyboard) = build_input(&config);
        let keyboard = keyboard.unwrap();
        keyboard.key_down("Space");
        keyboard.key_down("W");
        keyboard.mouse_button(input::bindings::MouseButton::Left, true);
        keyboard.mouse_motion(10.0, -10.0);
        input.sync();
        let h = input.open(PortIndex::new(0).unwrap()).unwrap();
        let pad = input.get_state(h).unwrap().gamepad;
        assert_eq!(pad.analog[0], 255);
        assert_eq!(pad.analog[7], 255);
        assert_eq!(pad.thumb_ly, i16::MAX);
        assert!(pad.thumb_rx > 0 && pad.thumb_ry > 0);
        keyboard.release_all();
        input.sync();
        assert_eq!(input.get_state(h).unwrap().gamepad, Default::default());
        assert_eq!(input.capabilities(h).unwrap().output.left_motor, u16::MAX);
    }
}

#[cfg(test)]
mod closer_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn counting() -> (Arc<AtomicUsize>, Wake) {
        let n = Arc::new(AtomicUsize::new(0));
        let m = Arc::clone(&n);
        (
            n,
            Box::new(move || {
                m.fetch_add(1, Ordering::SeqCst);
            }),
        )
    }

    #[test]
    fn a_close_wakes_a_paused_guest_and_the_window() {
        let c = Closer::new();
        let life = Lifecycle::new();
        let (wakes, wake) = counting();
        c.attach(life.clone(), wake);
        life.set_paused(true);
        let l = life.clone();
        let guest = std::thread::spawn(move || l.wait_while_paused());
        std::thread::sleep(Duration::from_millis(50));
        assert!(!guest.is_finished(), "paused");
        c.close();
        assert_eq!(guest.join().unwrap(), (true, Control::Stop));
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        // Again (a guest that already exited leaves only the window): the
        // window is still asked to close.
        c.close();
        assert_eq!(wakes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_close_before_the_window_is_up_applies_when_it_attaches() {
        let c = Closer::new();
        c.clone().close();
        assert!(c.requested());
        let life = Lifecycle::new();
        let (wakes, wake) = counting();
        c.attach(life.clone(), wake);
        assert!(life.should_stop());
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
    }
}

#[cfg(test)]
mod pace_tests {
    use super::*;

    /// The pacer's sleep is `pace` in frame stats (in the apps it used to
    /// count as guest CPU) and the host perf line sees it, with what it
    /// overslept.
    #[test]
    fn a_pacing_sleep_is_pace_time_and_reaches_the_host_perf_gauges() {
        timeline::phase::start(timeline::phase::Config::live(10_000_000_000)).unwrap();
        timeline::host::set_enabled(true);
        let before = timeline::phase::thread_ns(timeline::phase::Phase::Pace).unwrap();
        let life = Lifecycle::new();
        assert_eq!(
            pace_sleep(&life, Duration::from_millis(5)),
            Control::Continue
        );
        let paced = timeline::phase::thread_ns(timeline::phase::Phase::Pace).unwrap() - before;
        let slept = timeline::host::SLEEP.take();
        let over = timeline::host::OVERSLEEP.take();
        timeline::host::set_enabled(false);
        timeline::phase::stop();
        assert!(paced >= 5_000_000, "{paced}");
        assert!(slept.n >= 1 && slept.max_ns >= 5_000_000, "{slept:?}");
        assert!(over.n >= 1, "{over:?}");
    }
}
