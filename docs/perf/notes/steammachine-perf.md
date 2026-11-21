# Steam Machine profile and music hangs

Evidence for [../README.md](../README.md) and [../../runtime/steamos.md](../../runtime/steamos.md). Entries are dated; re-measure before acting on any row.

## 2026-11-20: where the frame goes, and why the music drops out

Measured on master 9214984b1 plus the working copy of that evening (the
Linux port). Device: Steam Machine (Zen 4, Navi 33, SteamOS 3.9, Game
Mode, Steam Linux Runtime 4). Module `H2_OPT=2` except where it says
`-O1`.

### How to reproduce

```sh
devenv shell -- flock /tmp/h2r-device.lock buck2 run -j 16 -c build_mode.mode=release \
  //platform/steamos:run -- --level 03a_oldmombasa
# on the device
./h2 agent wait '{"map":"03a_oldmombasa","map_ticks":150}' 600000
./h2 agent capture 600
./h2 agent hostmetrics
./h2 agent audio.loops      # output.sink.underruns, output.playout.starves
```

The `perf` commands are in [../profiling.md](../profiling.md), "Linux and
SteamOS: `perf`".

### Frame budget

| scene | fps | busy p50 / p95 / max ms | `cpu` mean ms | `d3d` mean ms | over budget | causes |
|---|---|---|---|---|---|---|
| main menu, ticks 900-1500 | 29.99 | 29.9 / 35.1 / 84.3 | 24.1 | 3.4 | 261 / 600 | `guest_cpu` 256 |
| 03a, map ticks 150-750 (cinematic) | 29.85 | 19.3 / 34.4 / 61.5 | 17.9 | 3.1 | 390 / 600 (pacing jitter) | `guest_cpu` 358 |
| 03a, first gameplay, 300 ticks | 27.95 | 34.9 / 36.4 / 48.3 | 27.9 | 6.2 | 300 / 300 | `guest_cpu` 300 |
| 01b gameplay, 600 ticks | 29.24 | 28.3 / 42.1 / 72.2 | 27.0 | 2.9 | 302 / 600 | `guest_cpu` 274 |
| 01a gameplay, 300 ticks, `-O1` | 28.24 | 31.5 / 42.1 / 55.6 | 27.4 | 5.4 | 194 / 300 | `guest_cpu` 180 |

The game thread is the limit. `gpu-wait` and `render-wait` stay under
0.2 ms. `hostmetrics` gives 22 to 35 ms of game-thread CPU per 33.3 ms
tick.

### Sampled profile

`perf record -F 999` on the whole process, 15 s, 01a and 00a with the
`-O1` module: the `guest` thread was on the CPU for 93% of the wall time.
Of its samples, generated code (`libh2game.so`) is about 50%, `h2` (the
runtime's accessors, the scheduler, D3D HLE) about 40%, and libc about 10%
(`memmove` and `malloc`). The `d3d-render` thread is about a third as busy.
The Rust side is a long tail: the largest entries on the game thread are
`Memory::lock` 1.8%, `runner::step` 1.5%, `Device::submit_inner` 1.3%,
`h2rt_thunk`, `Guest::take_dirty`, `Guest::read` and `WritebackView::read`
about 1% each. Mixing (`audio::dsound::Stream::play_frames` 0.3%,
`FxState::process` 0.2%) and vertex conversion (`StreamPlan::convert`
0.4%, on the render thread) are below 0.5%, so no scalar Rust loop is
hot enough to be worth SIMD. `Device::palette_entries` (0.7% of the game
thread, plus its two allocations per stage) read four stage palettes into
`DrawSnapshot::palette_entries` on every admitted draw, a field nothing
read; it is removed.

### Music hangs

Polling `audio.loops` every 1.5 s alongside `session.status` (03a, and
00a at `-O1`): the sink's `underruns` and the playout's `starves` rise
only while the game runs below 30 ticks a second, and stay flat while it
holds 30. In 03a the cinematic held 46 ticks per 1.53 s with no starves;
from map tick 2036 it fell to about 30 ticks per 1.5 s and the starves
went from 8 to 23 in 15 s. In 00a's cinematic at `-O1`, 20 ticks a second
for 12 s gave 22 starves. DirectSound output is paced on the logical
clock (`Pacing::Logical`, `crates/platform/dsound/src/output.rs`), so a
game at 65% speed renders 65% of the audio. The 150 ms jitter buffer then
runs dry, and each starve waits for it to refill to its target before
playing again, which is the audible hang. The ALSA path (cpal through
the PulseAudio plugin to PipeWire) is not involved: the output thread
and `cpal_alsa_out` together take under 0.1% of the process's samples,
and starves never rise while the game holds 30 ticks a second. The fix
is game-thread time.

### `H2_CPU=x86-64-v3` on the Linux module

A Linux branch of `H2_CPU` (`-march=<name>`, plumbed through `[h2] cpu`)
was built and measured, then removed:

| run | base | x86-64-v3 |
|---|---|---|
| device, main menu: `cpu` mean / busy mean ms | 24.12 / 29.76 | 24.67 / 30.63 |
| device, 03a ticks 150-750: `cpu` mean / busy mean ms | 17.88 / 21.19 | 17.79 / 21.16 |
| host 7950X3D, headless `H2_LEVEL=03a_oldmombasa`, presents 900-1500, fps (7 pairs) | 56.96-61.83 | 57.71-62.51 |

On the host x86-64-v3 was 0.8% to 2.0% faster in every pair; on the
device it was within run-to-run noise in both scenes. `znver4` on the host
was not distinguishable from either.

## 2026-11-21: LTO, allocations and memory access

Measured on master 64bf35e06's parent a7386fd98 plus this branch, except
where it says 64bf35e06. Same device and captures as above; "03a
gameplay" is map ticks 2450-2750 with the player idle, "01b" map ticks
600-1200.

### Thin LTO for `h2` (adopted, default in release)

`-c build_mode.lto=thin` passes `-Clto=thin` to binaries
(`build-support/buck/workspace_rules.bzl`, `_lto_flags`). The prelude
passes neither `-Cembed-bitcode=no` nor `-Ccodegen-units`, so rlibs carry
bitcode and rustc's default of 16 codegen units applies. Same module for
both columns:

| scene | busy mean ms, off | busy mean ms, thin | `cpu` mean ms, off / thin |
|---|---|---|---|
| main menu, 600 ticks | 29.71 | 29.48 | 24.10 / 24.00 |
| 03a cinematic, 600 ticks | 18.48 | 18.33 | 15.95 / 15.84 |
| 03a gameplay, 300 ticks | 37.70 (25.43 fps) | 36.53 (26.55 fps) | 30.34 / 29.71 |
| 01b, 600 ticks | 26.39 | 25.88 | 21.25 / 20.96 |
| host headless 03a, presents 900-1500, 3 pairs | 54.65-56.46 fps | 56.64-56.86 fps | |

The `h2` link (`rustc link` in `buck2 log critical-path`) takes 29.4 s
without LTO and 58.8-63.3 s with thin. `fat` was killed for memory on the
64 GB build host (9 GB free) after 172 s.

### Allocations (dhat)

`-c build_mode.dhat=true`, `H2_LEVEL=03a_oldmombasa
H2_BOOT_STOP_AT_PRESENT=900 h2 run`, headless on the host (so
`RecordingBackend` sites are headless-only). 10.0 M blocks over 900
presents, 11,100 a present including the load. By count:

| site | blocks | per present |
|---|---|---|
| `Guest::take_dirty` from the D3D write barrier (`d3d_glue::barrier_handler`) | 2,221,456 | 2,468 |
| `RecordingBackend::draw` (headless only) | 1,282,602 | 1,425 |
| `GuestMemory::read_words` (a byte `Vec`, then a `Vec<u32>`) | 927,942 | 1,031 |
| `level_boot::disc_levels` (once, at boot) | 671,600 | |
| `IndexedPass::candidate` (`Arc::new(draw.clone())`, 549 MB) | 575,546 | 640 |
| kernel trace (`trace::install_bounded`) | 482,299 | 536 |
| `Mixer::render_split` | 291,388 | 324 |

By bytes the largest are texture decode (2.6 GB), guest stream copies
(`read_append`, 3.1 GB) and the headless surface readbacks. The barrier
now drains into a per-thread buffer (`Guest::take_dirty_into`) and
`read_words` reads into its one `Vec<u32>`: 7.7 M blocks for the same run
(2,550 fewer a present), `take_dirty` 636 blocks, `read_words` 463,971.
That is too small to separate from run-to-run noise in a frame capture.

### Memory access

`perf stat -t <guest tid>` for 10 s in 03a (v3 build, 2026-11-20):
26.8 G cycles, 38.4 G instructions (IPC 1.43), 1.9 M dTLB load misses,
23.9 M iTLB load misses, 205 M L1D load misses of 12.6 G loads. Demand
fills from DRAM: 10.3 M in 5 s.

- **dTLB.** SteamOS runs THP `always`; 58 of guest RAM's 64 MiB
  (`0x40001000`) is on huge pages, and page walks from dTLB misses are
  about 0.2% of cycles. `MADV_HUGEPAGE` would change nothing here.
- **iTLB.** About 2.4 M misses a second against 690 MB of module text and
  90 MB of `h2` text on 4 KiB pages (`FilePmdMapped: 0`; the kernel lacks
  `CONFIG_READ_ONLY_THP_FOR_FS`, so `MADV_COLLAPSE` cannot fold file
  text). At about 25 cycles a walk that is 2-3% of the game thread. Copying
  the hot text into anonymous huge pages would remove it, but breaks
  `perf` symbols for that range; not built.
- **DRAM misses.** Sampling `ls_dmnd_fills_from_sys.dram_io_near:u` with
  call branches (`-j any_call,u`): 44% land in libc `memcpy`/`malloc`. Of
  those, 37% are `Guest::read_append` (copying a draw's vertex streams out
  of guest RAM) and 19% `Device::write_back_overlapping` (render-target
  write-back into guest RAM). Both copies are how the D3D path works, not
  waste a small change removes.

### Not changed: `VertexLayout` per draw

`Device::vertex_setup` clones the memoized `Vec<StreamLayout>` for every
programmable draw (0.3% of the game thread, one allocation per stream).
`ResolvedDraw::layout`, `IndexedStreams::layout`, `IndexedGeometry::layout`
and `DrawParts` all own a `VertexLayout`, and about 140 sites in d3d,
game, native-render and their tests build or move one, so an `Arc` is not
a contained change for that gain.

### The scene-effect passes now on in `h2 build` modules

From module generator `778b84f5`, which exposes the scene-effect site
entries, the native particle, HUD, transparent, decal, post and
scene-list passes run in a Buck-built module. The same package also has
64bf35e06 (model detail by camera distance). 03a gameplay with thin LTO
went from 26.55 fps (busy 36.53 ms, `render-wait` 0.0) to 19.74 fps. One
package, each pass turned off at run time (`run.py --env H2_FEATURES=...`):

| `H2_FEATURES` | fps | busy mean ms | `render-wait` ms | `cpu` ms | render-thread round trips a frame |
|---|---|---|---|---|---|
| (all on) | 19.74 | 46.63 | 7.74 | 32.79 | particles 176, HUD 57 |
| `-native-particles` | 21.94 | 42.51 | 4.46 | 31.35 | HUD 57 |
| `-native-hud` | 20.45 | 45.12 | 6.44 | 32.21 | particles 176 |
| `-native-particles,-native-hud` | 23.82 | 39.74 | 1.27 | 31.28 | 0 |
| all six off | 24.38 | 38.99 | 0.00 | 31.45 | 0 |
| `-native-transparents` / `-native-decals` / `-native-post` / `-native-scene-lists` / `-lod-distance` | 19.3-20.0 | 46.0-47.7 | 7.5-8.1 | 32.4-33.4 | |

The particle and HUD passes cost most of it. They block the game thread
on a render-thread round trip per admitted draw
(`ThreadedBackend::pin_external_run`, counted as
`native_<stage>_worker_round_trips`). The render thread is not GPU-bound:
`gpu-wait` stays at 0.01 ms, with about 5.8 ms of encode and 6.3 ms of
submit a frame. With all six off, about 2.5 ms of busy time (1.7 ms of
`cpu`) over the earlier build remains, which no run-time switch removes.

Per round trip, from the rows above: turning particles off removes 176
trips and 4.1 ms of busy time a frame (about 23 us each, 3.3 ms of it
`render-wait`); the HUD's 57 trips cost 1.5 ms (about 26 us each). Both
passes reach the device through `try_native_inline_draw` with a `Direct`
admission (`native_particles_admission.rs`, `native_hud_glue.rs`), so
`Device::submit_inner` takes the synchronous `prepare_external` route: the
asynchronous `queue_external_draw` route needs `native.is_none()` and a
`resolved_fallback`, and neither pass opts into `batch_key` pinning.

### Native producers queued instead of waiting (adopted)

Measured on 6078b140 plus this branch. A native producer's draw
(`Device::try_native_*_draw`) whose pass keeps its resolved D3D packet and
writes the same targets goes through `queue_external_draw`: the device
commits the suppression when it queues the draw, and the render thread
records the pass or draws the packet
([render-thread.md](../../graphics/render-thread.md), "Keeping
guest-visible semantics exact"). The particle, effect and single-draw UI
passes keep their packet. The transparents' range pass already kept one, so
its round trips go too. A UI model draw with sibling ranges still waits.

Device, 03a gameplay (map ticks 2450-2750), one capture a row. The
6078b140 row is the same package with `h2` built at that commit; the module
is the same generator's.

| `h2` | `H2_FEATURES` | fps | busy mean ms | `render-wait` ms | `cpu` ms | render-thread round trips a frame |
|---|---|---|---|---|---|---|
| 6078b140 | (all on) | 19.15 | 47.89 | 8.25 | 33.29 | particles 176, UI 57, transparents 4 |
| queued | (all on) | 23.96 | 39.50 | 0.00 | 32.93 | 0 |
| queued | (all on), second run | 24.60 | 38.67 | 0.00 | 32.29 | 0 |
| queued | `-native-particles,-native-hud` | 25.42 | 37.70 | 0.00 | 30.54 | 0 |

The 1.0 to 1.8 ms of busy time left over the last row is the two passes'
work on the game thread. Their target binding (`native_particles_prepare_ns`,
`native_ui_prepare_ns`) is 0.44 ms a frame; the rest is not broken down.

Parity on the host (7950X3D, RTX 5080; headless `h2 serve` with
`H2_BOOT_GFX`, the render thread on, a fresh data directory a run): the
frame of each map tick from `render.capture`, 6078b140 against queued,
compared with `h2 agent frame compare`, has 0 differing pixels and
`max_channel` 0 at 03a map ticks 400, 700, 1200, 1800 (cinematic), 2450,
2600 and 2750 (gameplay), and 01b map ticks 600, 1600, 3200, 4800, 6000
(cinematic), 6600, 6900 and 7200 (gameplay, which starts at map tick 6415).
The host profile, 300 ticks after the last capture:

| level | `h2` | fps | busy mean ms | `render-wait` ms | `cpu` ms | render-thread round trips a frame |
|---|---|---|---|---|---|---|
| 03a | 6078b140 | 18.70 | 53.47 | 9.32 | 36.22 | particles 186, UI 28, transparents 4 |
| 03a | queued | 29.10 | 34.36 | 0.00 | 28.07 | 0 |
| 01b | 6078b140 | 35.44 | 28.21 | 3.51 | 20.44 | particles 42, UI 46, transparents 11 |
| 01b | queued | 52.94 | 18.89 | 0.00 | 15.79 | 0 |

To reproduce on the device, run "How to reproduce" above, then
`./h2 agent wait '{"map":"03a_oldmombasa","map_ticks":2450}' 900000` and
`./h2 agent profile.capture '{"ticks":300}'`. On the host:

```sh
H2_OPT=2 H2_LEVEL=03a_oldmombasa H2_BOOT_GFX=$DIR/gfx H2_BOOT_GFX_EVERY=never \
  h2 serve disc/halo2.iso --out <module> --data $DIR/data --fresh-data
h2 agent session.wait '{"until":{"map_ticks":2370}}'
h2 agent render.capture '{"tick": <at.tick + 2450 - at.map_ticks>}'
h2 agent frame compare a/map-2450.png b/map-2450.png
```

`at.tick` starts again from 0 when a level's opening cinematic ends, so
the capture's tick is worked out from the map tick.
