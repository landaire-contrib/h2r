# Booting the recompiled XBE natively

Status: Built.
Code: `crates/runtime/recomp/src/lib.rs` (`build`, `load_module`, the build stamp), `crates/apps/boot/src/lib.rs` (`run`, `run_hosted`), `crates/apps/boot/src/registration.rs`, `crates/apps/cli/src/boot_cmd.rs` (`h2 build`, `h2 run`, `h2 serve`). Tests: `crates/runtime/recomp/` (unit tests, `tests/fast_tier.rs`), `crates/apps/boot/tests/`.
Switches: the boot runner's own are below; every other one is in [switches.md](switches.md).

`h2 build` lifts the whole `default.xbe` and compiles it to a host dylib.
`h2 run` loads that dylib into the runtime with the kernel, VFS, XAPI, D3D,
DSound and Bink HLE registered, then starts the guest at the XBE entry
point. It runs headless; `h2 play` ([app.md](app.md)) runs the same boot in
a window with live input and sound.

## Running it

```sh
# lift + emit + clang the whole XBE (clang takes minutes)
cargo h2 build /abs/path/to/disc/halo2.iso

# boot it (the tier flag below says what runs while a build is missing or stale)
cargo h2 run /abs/path/to/disc/halo2.iso
```

Pass the disc as an absolute path to the real ISO, with no symlinks or
copies. Nothing is ever written next to it.

| Flag | Default | Meaning |
| --- | --- | --- |
| `--out DIR` | `$TMPDIR/h2boot` | generated C (`src/`), objects, `libh2game.dylib` and its `.stamp` (image hash, generator source hash, IR passes, `H2_ACCESS_COUNT`, ABI, compiler flags: `run` rebuilds a dylib stamped for another image or generator), `kernel-trace.tsv` |
| `--tier auto\|fast\|clang` | `auto` | which module `run` boots: clang's optimised one when it is current, else the Cranelift fast tier with clang's build started in the background (`H2_TIER` is the same; `crates/runtime/recomp/src/tier.rs`, [cranelift-tier.md](../lifter/cranelift-tier.md)) |
| `--data DIR` | `~/Library/Application Support/h2recomp/data` (`%LOCALAPPDATA%\h2recomp\data` on Windows; `~/.local/share/h2recomp/data` on other Unix; `H2_DATA`) | VFS host storage (`disk/PartitionN`, `metadata/PartitionN`) |
| `--jobs N` | cores | clang parallelism |
| `--slices N` | 100000 (`h2 serve`: no limit) | cooperative scheduler slices before giving up |
| `--transfers N` | 50M | runner transfers per slice |
| `--no-stopgaps` | | run on the bare kernel: no boot stopgaps or wrappers, and no fallback interpreter (an unsupported instruction stops the run where generated code reported it) |
| `--fresh-data` | | empty `<data>/disk` and `<data>/metadata` first; refused, deleting nothing, if the disc, image or `--out` is inside either |
| `--no-d3d`, `--no-dsound`, `--no-bink`, `--no-xapi` | | leave that HLE crate unregistered |
| `--movies DIR` | app data (`~/Library/Application Support/h2recomp/movies`; `%LOCALAPPDATA%\h2recomp\movies` on Windows; `~/.local/share/h2recomp/movies` on other Unix) | where `h2 movies` installed the transcodes; the current ones play ([bink-hle.md](../audio/bink-hle.md)); the run logs `movies: playback (...)` or `movies: skip (...)` |
| `--no-movies` | | skip every movie (Bink HLE skip mode) |
| `--no-mappack` | | skip installing a `mappack.iso` beside the disc into `--data` (`H2_MAPPACK=0` does the same; [map-pack.md](../game/map-pack.md)) |
| `--no-halo1` | | skip the Halo 1 content setup |

The other flags are in `h2 --help` (`--arch`, `--settings`, `--film`, `--system-link`, `--host-light`, `--socket`).

Environment variables the boot runner reads. Every other variable and
`H2_FEATURES` switch, one line each, is in [switches.md](switches.md).

| Environment | Effect |
| --- | --- |
| `H2_FASTMEM=0` | **On by default.** Generated code does inline RAM checks/copies; unsupported accesses use Rust. `0` compiles without them; set it for both `h2 build` and `h2 run`/`play`, since changing compiler flags invalidates the dylib stamp. This is the gated inline path, not fault-driven fastmem. |
| `H2_X87_NATIVE=0` | **On by default.** Generated code runs range-guarded x87 arithmetic in native binary64 (see [x87-native.md](../lifter/x87-native.md)). `0` keeps every operation on the `h2x87_v1_*` helpers. A compiler flag: part of the dylib stamp and object cache keys, so set it for both build and run. |
| `H2_SSE_NATIVE=0` | **On by default.** Generated code runs guarded SSE add, subtract, multiply, divide and square root in native SIMD ([sse-native.md](../lifter/sse-native.md)). `0` uses the integer `h2s_*` helpers for every operation. A compiler flag, like `H2_X87_NATIVE`: set it for both build and run. |
| `H2_ACCESS_COUNT=0` | **On by default.** Generated handlers count their own guest memory accesses and charge them in bulk (see crates/recompiler/emit/RUNTIME-CONTRACT.md). `0` lets the runtime charge each accessor call; `block` selects the block clock, which counts planned block costs instead of accesses ([block-clock-imprecise-faults.md](block-clock-imprecise-faults.md)). It changes the C: part of the dylib stamp and the emit cache key, as `H2_PASSES` is. |
| `H2_BOOT_VERBOSE=1` | log boot notes, fallback analysis and cache diagnostics |
| `H2_BOOT_GFX=<dir>` | run D3D on a headless `GfxBackend` with the nv2a shaders and write presented frames to `<dir>/frame-NNNNN.png` |
| `H2_BOOT_UPLOAD_ALWAYS` | Any value, even `0`, counts as set. D3D re-reads every resource on every use, instead of trusting the write barrier (`UploadPolicy::Tracked`) |
| `H2_BOOT_GFX_EVERY=N` | with `H2_BOOT_GFX`, keep the first frame and every Nth (default 30); `never` renders on the GPU but writes no PNGs |
| `H2_BOOT_STOP_AT_PRESENT=N` | end the run once N frames are presented |
| `H2_BOOT_FPS_FROM=N` | measure the report's fps window from present N (default: the last 100) |
| `H2_BOOT_REALTIME=1` | pace the scheduler to wall-clock time (`Pacer::real_time`); off by default, so runs go as fast as the host allows |
| `H2_BOOT_CACHE=<dir>` or `off` | the boot cache (default `$TMPDIR/h2boot-cache`), shared by every `--out`: the fallback's unread jump-table targets, and the image's analysis (`analysis-<hash>-<size>-<analyzer>.h2a`), keyed on the XBE hash and size and on `ANALYSIS_SOURCE_HASH` (xbe and analyze sources, `Cargo.lock`, rustc version). `h2 build` writes the analysis and the fallback reads it at run time; `H2_TRACE` skips it. `off` caches nothing |
| `H2_BOOT_INPUT=BUTTONS@WHEN,...` | scripted pad timeline through the keyboard pad and XInputGetState. BUTTONS: `A B X Y BLACK WHITE LT RT UP DOWN LEFT RIGHT START BACK LS RS LUP LDOWN LLEFT LRIGHT` or a key name, joined with `\|`. WHEN: `T`, `T+D` or `T-U`, a slice or (`f` prefix) a present count; held 4 presents or 100 slices by default. Each press is printed. |
| `H2_BOOT_INPUT_FILE=<file>` | the same entries, one per line, `#` comments; merged with `H2_BOOT_INPUT` |
| `H2_BOOT_CAPTURE=<dir>`, `H2_BOOT_CAPTURE_FRAME=N` | record frame N for offline replay (see `crates/extensions/game/src/d3d_diag.rs`): every texture upload, surface, guest write, and the frame's `ResolvedDraw`s with their streams |
| `H2_BOOT_CAPTURE_SNAP=K[-L],...` | with a capture, read the colour target back after those draws of frame N (`snap-K.bin`, guest BGRA bytes) |
| `H2_BOOT_CAPTURE_SKIP=K[-L],...` | with a capture, drop those draws of frame N |
| `H2_BOOT_CAPTURE_LITE=1` | with a capture, record only the frame's draws, surfaces and snapshots (no uploads or guest writes); for snapshots |
| `H2_BOOT_CAPTURE_TEX=K[-L],...` | with a lite capture, still keep these texture uploads' bytes (`tex-K-L.bin`; ids from `textures.txt` and `frame.txt`, stable across `--fresh-data` runs) |
| `H2_BOOT_DRAWLOG=<file>` | log every backend call of the frames in `H2_BOOT_DRAWLOG_FRAMES` (present counts, default `240`; frame N matches `frame-N.png`) |
| `H2_BOOT_DRAWLOG_VERTS=1` | with `H2_BOOT_DRAWLOG`, add the vertex layout and the first 8 vertices of each stream |

The run prints a report when it ends. It covers:

- the outcome and the stop, with the last EIP;
- every thread's registers, EBP chain and raw stack;
- the stopgaps that fired, with counts;
- D3D, DSound and XAPI registration and their stats;
- fallback interpreter stats, and the first 40 addresses it began interpreting at;
- the last 40 kernel calls, decoded.

`kernel-trace.tsv` has one row per kernel import call: seq, thread, ordinal,
name, return address, args, EAX and decoded detail (object names for the
file APIs). Diff two of them to compare runs.
It is written through a 64 KiB buffer, flushed at the end of the run
(`trace::flush`), so a crash can lose the last rows; the report's last 40
calls come from memory and are complete.

## Hosted runs

`run` is `run_hosted(xbe, dylib, opts, Host::default())`. A host (`h2 play`,
[app.md](app.md) "Playing"; the agent and dedicated-host paths) fills in
`boot::Host` (`crates/extensions/game/src/run_opts.rs`). The fields a plain host sets:

| Field | Instead of | Goes to |
| --- | --- | --- |
| `gfx: Option<d3d::GfxBackend>` | `RecordingBackend` or `H2_BOOT_GFX` | `d3d_glue::install_with` (present hook already attached) |
| `audio_sink: Option<dsound_glue::SinkOpener>` | a `NullSink` | `dsound_glue::install_with_sink`: the same logical-time output, advanced after every slice, playing into the sink the opener returns (called on the output thread; `DeviceSink` is not `Send`) |
| `input: Option<input::InputManager>` | a headless keyboard pad | `xapi_glue::register_with` |
| `keyboard: Option<KeyboardMouse>` | that pad | what `H2_BOOT_INPUT` presses |
| `trace_limit: Option<usize>` | every call in memory | `trace::install_bounded`: the last N calls, and at most N older ones still open (calls that never return, such as thread exits, age out), stay in memory; the TSV still gets every completed call |
| `between_slices: Option<BetweenSlices>` | nothing | called after every slice, input pump and audio advance with the kernel; `false` ends the run with outcome `HostStop` (the output is halted as on any end) |

`Host` has more fields (`film_out`, `host_light`, `host_bots`, `settings`,
`level`, `debug_menu`, `theater`, `on_vfs` and others); each is documented on
the struct. Everything else (stopgaps, fallback, vblanks, report) is the
same in both modes. `tests/hosted.rs` (ignored; needs `$H2_XBE`, `$H2_DISC`,
`$H2_BOOT_DYLIB`) checks the sink is opened once off the runner thread and
fed on the guest's clock, the callback count, a monotonic clock and
`HostStop`; `trace.rs` tests the bounded trace.

## How it fits together

- **`crates/runtime/recomp/src/lib.rs`** builds and loads the module;
  **`crates/apps/boot/src/lib.rs`**, the composition root, runs it and
  re-exports `game` (`crates/extensions/game`: every module below that is
  not `recomp`'s, and `RunOpts` and `Host`), which re-exports `recomp`
  (`boot::build`, `boot::tier`).
  - `build` runs analyze, lifts each function, then `emit::plan` writes
    C11 shards and `h2gen.h`.
  - clang builds the shards for the host architecture, and they are linked
    with `c/boot_bridge.c` into `libh2game.dylib`.
  - The bridge forwards the `h2abi_v1_*` imports to `runtime::ffi`. Rust
    implements the `h2shim_*` imports in `shim.rs`.
  - `load_module` reads the module's `h2gen_module` tables and registers every
    generated entry and resume as itself, so the module's direct calls
    (`H2_DIRECT_CALLS`) match. A slim host module's left-out owners are
    registered to the interpreter (`fallback::register_cold`).
- **`fallback.rs`, `x87.rs`, `sse.rs`**: mixed-mode fallback. `x87.rs`
  does all of its arithmetic, conversions and transcendentals in
  crates/recompiler/x87 (exact ext80).
  - When generated code stops on an instruction the lifter doesn't lower,
    the runtime hands the STOP to `fallback::recover`
    (`Dispatch::set_stop_recovery`, also for a STOP handed up through
    direct calls), which interprets from there with `ir::interp`. This is
    a stopgap like the others: `--no-stopgaps` turns it off.
  - x87 and leftover SSE instructions go to the emulators in `x87.rs`
    (a decoder over crates/recompiler/x87's `abi` state machine, so stack faults,
    source-format exceptions and condition codes match what generated
    code gets) and `sse.rs`.
  - Control goes back to generated code at the next registered entry or
    resume, call or return.
- **`stopgaps.rs`**: boot-only kernel behaviour, each a named blocker
  below. A stopgap replaces a kernel export the kernel stops on. It is
  only installed if the ordinal is in `Kernel::stopped_ordinals()`.
- **`trace.rs`**: wraps every kernel import token to record the TSV.
- **`d3d_glue.rs`**: `d3d::register` on a headless `RecordingBackend`
  (or `GfxBackend` with `H2_BOOT_GFX`), with `UploadPolicy::Tracked`.
  The write barrier subscribes to the runtime's write watch over all of
  RAM and wraps every d3d entry: on entry, `take_dirty` goes to
  `Device::note_written_pages`. Nothing is drained on return: other contexts can
  store while the call is in flight, so the device's own stores come back
  to it at the next entry instead.
- **`dsound_glue.rs`**: `dsound::register`. Completion events go to
  `Kernel::signal_event`.
- **`xapi_glue.rs`**: `xapi::register` with a kernel-VFS `Storage` and a
  headless `InputManager`.
  - `xapi::service_input` runs between scheduler slices.
  - With `--no-xapi`, the boot keeps its own XMountUtilityDrive override
    (`0x2d5b77`, which writes `xapi::register::addr::CACHE_ROW`).
- **`d3d_diag.rs`**: a backend wrapper; the report gains draw outcomes by
  target, the texture formats drawn and device events by reason, and
  `H2_BOOT_DRAWLOG` logs whole frames. `H2_SWEEP_DIAG=<file>`
  (`sweep_diag.rs`) logs one level loading-screen frame: every draw's
  combiner words and blend state, and the target read back after each.

## Measuring the idle menu

The main menu, idle, for performance work. Fresh data makes every run take
the same path; the script picks the Default profile and leaves the main menu
up, then the run stops on its own.

```sh
T=$TMPDIR/h2bench; OUT=$TMPDIR/h2boot
cargo build --release -p cli            # target/release/h2
# Once per generator (h2 run rebuilds a dylib whose .stamp doesn't match):
target/release/h2 build disc/halo2.iso --out $OUT
H2_BOOT_GFX=$T/gfx H2_BOOT_GFX_EVERY=never \
H2_BOOT_INPUT="START@f310,DOWN@f408,A@f419" \
H2_BOOT_FPS_FROM=440 H2_BOOT_STOP_AT_PRESENT=560 \
target/release/h2 run disc/halo2.iso --out $OUT --data $T/data --fresh-data
```

- Window: presents 440-560, the main menu after it has faded in. The run
  report's `frames:` line gives fps, mean, median and worst frame time over
  that window, and every 100 presents the run prints `present N: last 100 at
  X fps`.
- `H2_BOOT_GFX_EVERY=never` keeps the GPU path but writes no PNGs.
- The fallback loads or computes the image analysis on a background
  thread, reusing the cache from `h2 build` when there is one. Boot
  continues on a cache miss; an unsupported instruction or a transfer that
  needs the fallback can still wait for the analysis. The run reports waits
  over 100 ms, and `H2_BOOT_VERBOSE=1` shows when analysis finishes.
  A light host loads the full analysis on its first interpreter or
  unresolved-target lookup that needs it. Its slim-module interpreter
  retains one lifted cold owner across calls and resumes. Compare warm runs.
- Numbers mean something only on a quiet machine, and only from the same
  data directory state: use `--fresh-data` (or a new `--data`) and never
  share one between two runs at once.

`cargo build --release -p boot --example menu_cpu` builds a bounded driver
(`crates/apps/boot/examples/menu_cpu.rs`): `menu_cpu DISC OUT FRESH_DATA_DIR`. It
runs boot's normal headless backend, keeps 4096 trace rows and stops after
present 560. Its `MENU_CPU` line reports wall FPS, process user+system CPU
milliseconds per frame for presents 480 to 560, and
`guest_thread_cpu_ms_per_frame`, the runner thread's own CPU time. Process
CPU includes every process thread; GPU execution and time descheduled are
not CPU time. For pixel fidelity, `H2_GFX_MERGE_CHECK=N` renders frame N
both merged and one pass per draw and reports the differences
([graphics.md](../graphics/graphics.md) "Merged passes"). Runs with the
numbers are in [menu-budget.md](../perf/notes/menu-budget.md).

## Boot stopgaps in force

A stopgap replaces a kernel export the kernel stops on; the table in
`crates/extensions/game/src/stopgaps.rs` installs an entry only if its ordinal is in
`Kernel::stopped_ordinals()`, and only with stopgaps on (`--no-stopgaps`
turns them off). The other stopgaps:

| Stopgap | Where | Retire when |
| --- | --- | --- |
| Back/depth surface backing at CreateDevice, `0xf0000000` write-combined alias | `d3d_glue::install_back_surfaces`, called from `lib.rs` | d3d or the runtime provides it |
| Fallback from unread jump-table targets (every unguarded switch's, read from the image), and the unknown-switch stops they cause, allowed only for the site's own table. A net for a table the analyzer read short; the test `xbe_pins::unguarded_tables_hold_every_case_the_boot_fallback_would_find` pins that the boot's run-time read finds no target the analyzer lacks | `fallback::unread_targets`, `fallback::start_analysis` | the analyzer reads every table |
| Interpreting past instructions the lifter does not lower | `fallback::recover` (the runtime's stop-recovery hook), `fallback::set_recovery` | the lifter lowers them |
| `--no-xapi` only: XMountUtilityDrive `0x2d5b77` (research point) | `xapi_glue::install_no_xapi_mount` | not retired |

## Known gaps

- **RTT write-back replayed as a CPU write.** The barrier reports the
  device's own write-back pages at the next entry, so an unchanged render
  texture uploads again. Boot cannot tell them apart from other contexts'
  stores during the call: d3d's handlers would need to run under
  `Guest::writes_unreported(id)` for the boot's watch, or the device would
  need to report its own write ranges. Performance only.

## Retired

The kernel, XAPI and DSound rounds retired the boot's wrappers and game
overrides; only the `--no-xapi` XMountUtilityDrive override above stays.
The list of what was retired, with the commits, is in
[boot-log.md](notes/boot-log.md).

## Not verified

- Which entries of the kernel stopgap table install on the retail image:
  `Kernel::stopped_ordinals()` depends on the kernel's exports and on
  whether it has storage.

## Related

[switches.md](switches.md), [app.md](app.md), [kernel-imports.md](kernel-imports.md),
[notes/boot-log.md](notes/boot-log.md), [../lifter/cranelift-tier.md](../lifter/cranelift-tier.md).
