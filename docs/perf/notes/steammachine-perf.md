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
