# Performance tooling

Status: Built. `h2 profile` and `h2 bench` need the CLI's `perf` cargo feature; `boot/menu-hold` has no registered boot driver (see "Boot hook").
Code: `crates/runtime/timeline/src/lib.rs`, `crates/tools/perf/src` (`cli.rs`, `sample.rs`, `profile.rs`, `budget.rs`, `bench.rs`, `hotloop.rs`). Tests: in those crates.
Switch: `H2_PROFILE*`, `H2_TIMELINE*`, `H2_PERF_FROM`, listed in [switches.md](../runtime/switches.md).

Three tools for finding out where time goes, in two crates:

- `crates/runtime/timeline`: dependency-free timeline events, written as Chrome
  trace-event JSON. The runtime, kernel and d3d call it directly.
- `crates/tools/perf`: the sampling profiler, `perf::Session` (the profiler plus
  a timeline capture around any run), and the benchmark harness. Behind
  `h2 profile` and `h2 bench`.

The commands are behind the CLI's `perf` cargo feature, off by default so
ordinary `h2` builds skip the profiler's dependencies:
`cargo run --release -p cli --features perf -- bench`. Without it,
`h2 profile` and `h2 bench` say how to rebuild. The runtime, kernel and
d3d always depend on `timeline`, which has no dependencies of its own.

Measure release builds. Debug builds
of the runtime are 10-100x slower and spend their time in different places.
Generated code is compiled with emit's own flags (`H2_OPT`, default `-O1`)
whatever the Cargo profile.

## Sampling profiler

```
h2 profile hot-loop/memory --seconds 5 --out $TMPDIR/prof
H2_PROFILE=$TMPDIR/prof <any binary that opens perf::Session::from_env()>
```

`h2 profile --help` lists every flag. Beyond the ones named below: `--top`
(rows per table), `--seconds` (how long the scenario keeps running,
default 3), `--hz`, `--cpu`, `--work` (build scratch), `--disc` (for
`boot/menu-hold`) and `--quick` (small workloads).

### How it samples

A sampler thread wakes at the chosen rate (default 1000 Hz). For each target
thread it suspends the thread with Mach `thread_suspend`, reads its registers
with `thread_get_state`, walks the frame-pointer chain, and resumes it with
`thread_resume`. `thread_info` records whether the thread was running or
blocked, so the profile is wall-clock with an on-CPU flag per sample
(`--cpu` keeps only on-CPU samples).

This was chosen over the alternatives for reliability:

- A `SIGPROF` handler runs on the target thread, which may be inside
  generated code holding the guest memory lock, or inside malloc. Everything
  the handler does would have to be async-signal-safe. `thread_get_state`
  needs no cooperation from the target.
- `sample(1)` and `xctrace` run as a separate process and need the
  generated dylib's symbols to be mapped back afterwards. In process, the
  dylib is still loaded and its line tables are at hand.
- Frame pointers are reliable here. The Apple arm64 and x86_64 ABIs keep
  them, Rust's Apple targets keep them, and emit's clang flags do not omit
  them. The walk only reads inside the thread's stack between its SP and
  base, requires each record to be aligned and to move toward the base, and
  strips pointer-authentication bits.

While a target is suspended the sampler only copies registers and stack
words into a fixed buffer: no allocation and no locks, because the target
might hold the allocator's or the runtime's lock. On arm64 the link register
is kept too. When the leaf has not pushed a frame yet, LR is its caller,
which the chain would otherwise skip; it is added only when it names a
function that is not already on the stack.

The sampler is macOS-only (`perf::sample::SUPPORTED`). On other hosts
`Sampler::start` returns `Unsupported`; the timeline and the benchmarks
still work.

Cost: each sample holds the target for the time `thread_suspend` takes to
stop it plus the walk. The mean is printed as "mean pause". Lower `--hz`
for long runs.

### Mapping generated code back to the guest

Every generated handler is the C function `h2fn_<entry>` (`emit::c::symbol`),
so a PC whose symbol has that name belongs to the guest function at
`<entry>`. The guest instruction comes from the line tables kept by emit's
default `H2_DEBUG=lines`. The PC gives a C source line, and the nearest
`/* 0x........ */` marker at or above that line in the same shard is the
instruction (emit's `c.rs` writes one before every instruction's block). So
no sidecar map is needed. Line tables name the shard relative to its source
directory (`./shard_NNNNN.c`, docs/lifter/BUILD-CACHE.md), which the symbolizer
reads from `src/` beside the dylib: the layout `h2 emit`, boot and the hot
loops all use. Objects shared through `H2_OBJ_CACHE` therefore resolve
against the output that loaded them, not the one that compiled them. With `H2_DEBUG=none`, or once the shard sources
or objects are deleted, samples still resolve to the guest function but not
to the instruction.

The `h2gen.h` helpers are `static inline` and get folded into the handler,
so time in them counts as guest code. Calls that leave the handler count
against whatever they call (`h2abi_v1_read` is runtime, a kernel export is
kernel, and so on).

The search for a marker is clamped to the PC's own function. Markers
belong to the `h2fn_<entry>(` definition above them, so a line in a
handler's prologue or selector switch, which comes before its first
marker, has no instruction. It does not inherit the last instruction of
the previous function in the shard.

Symbolisation runs in process. `dladdr` finds each PC's image, asking the
dynamic loader at every lookup, so a dylib loaded after an earlier
profile or lookup is seen. addr2line then reads that image's symbol table
and DWARF line tables, including macOS debug maps that point at the
objects; this is cached per image path and load address. Lookups use
the image's preferred (link-time) address, `pc - slide`. On Mach-O the
slide is the image's load address minus its preferred `__TEXT` vmaddr,
read from the mapped header: 0 for the generated dylibs, 0x100000000 for
an executable such as `h2`. It must run
**before the generated dylib is unloaded**: finish or drop the `Session`
first.

### Attribution

Each sample is charged to the innermost frame that has a category. Frames
in libc, the allocator or std are skipped, so a `memmove` inside a memory
read counts as runtime.

| category | frames |
|---|---|
| `guest` | `h2fn_*` (generated code) |
| `runtime` | `runtime::*`, `h2abi_*`, `h2shim_*`, `h2x87*`, `x87::*`, `simd::*`, the module bridge (`h2bridge*`, `h2boot_*`) |
| `kernel` | `kernel::*`: HLE exports and the scheduler (`Machine::run`) |
| `d3d` | `d3d::*`: D3D HLE and push-buffer execution |
| `gfx` | `gfx::*`, `wgpu*`, `naga::*`, Metal (`MTL`, `AGX`) |
| `fallback` | `*fallback::*` (the boot driver's interpreter fallback), `ir::interp*` |
| `hle` | `xapi::*`, `dsound::*`, `bink_hle::*`, `audio::*`, `wma::*`, `movies::*` |
| `other` | nothing recognised |

### Outputs

Written into `--out` (default `$TMPDIR/h2profile`) or `H2_PROFILE`:

- `profile.folded`: folded stacks, root first (`thread;frame;...;leaf N`).
  Guest frames are `guest 0x<entry>` with an `@0x<insn>` leaf when the line
  tables allow (`--no-insns` to drop it). Samples taken while the thread was
  blocked end in `[off-cpu]`. `--by-category` puts the category at the
  root. Start-up frames (dyld, `main`, `lang_start`) are trimmed unless you
  pass `--full-stacks`. Open the file with `inferno-flamegraph`,
  `flamegraph.pl`, or <https://speedscope.app>.
- `profile.txt`: the top-N tables. They cover samples by category; self
  time (by charged frame); guest functions counting the services they
  called; guest functions counting only their own code; and the hottest
  guest instructions.
- `profile.json`: the same tables as JSON (`perf::profile::Summary`).

### From code

```rust
// Keep `_perf` declared after the loaded module, so it drops (and
// resolves) first.
let _perf = perf::Session::from_env();
```

`Session::from_env` reads the following:

| variable | meaning |
|---|---|
| `H2_PROFILE=<dir>` | sample the calling thread, write the profile there |
| `H2_PROFILE_HZ` | rate (default 1000) |
| `H2_PROFILE_TOP` | rows per table (default 25) |
| `H2_PROFILE_CPU=1` | on-CPU samples only |
| `H2_TIMELINE=<file.json>` | record a timeline and write it there |
| `H2_TIMELINE_CATS` | categories, e.g. `all`, `default`, `frame,d3d,gfx`, `all,-hle` |

A session starts its sampler before its timeline. If the sampler cannot
start (on a host other than macOS, with no target threads, or when the
thread cannot spawn), `start` fails and leaves nothing recording.
`Session::start_with(config, targets)` samples other threads too. Make each
`perf::sample::ThreadTarget::current(name)` on the thread itself.

### Linux and SteamOS: `perf`

The in-process sampler has no Linux implementation, so `capture N sample`
answers `sampling.unavailable` there. Linux `perf` samples the running game
instead. SteamOS ships it, and at the default `perf_event_paranoid` of 2 it
records user-space samples of your own processes without root:

```sh
P=$(pgrep -x h2)
G=$(grep -l '^guest$' /proc/$P/task/*/comm | cut -d/ -f5)   # the game thread
perf record -F 999 -t $G -o /tmp/guest.perf -- sleep 15
perf report -i /tmp/guest.perf --no-children --sort dso,sym
```

Generated handlers keep their `h2fn_<entry>` names and Rust symbols resolve
from `h2`. Neither `h2` nor its Rust code keeps frame pointers on Linux, and
DWARF unwinding stops at generated code, so `-g` call graphs are not
usable. To find who calls a hot libc routine (`memmove`, `malloc`), record
the last user calls with each sample (`-e cycles:u -j any_call,u`, which
Zen 4 supports) and charge each sample in libc to the first call in its
branch stack that came from outside libc (`perf script -F ip,brstack`).
Inside the Steam Linux Runtime container libc is the host's
`/run/host/usr/lib/libc.so.6`, which `perf` on the host reads as
`/usr/lib/libc.so.6`; it has no symbol table, so its samples show as bare
offsets.

## Timeline

```
h2 profile d3d/pushbuf-frame --timeline $TMPDIR/trace.json --cats all
H2_TIMELINE=$TMPDIR/trace.json <binary with a Session>
```

Open the JSON in <https://ui.perfetto.dev> or `chrome://tracing`. Guest
threads get their own rows (process "guest threads"); host-side events go
on the row of the host thread that recorded them (process "host").

| category | event | where |
|---|---|---|
| `frame` | `frame` instant, numbered | `d3d::Device::swap` |
| `guest` | `slice`: a guest thread on the CPU | `kernel::Machine::run` |
| `kernel` | `wait objects`, `wait critical section`, `sleep` spans on the guest thread's row, from block to next schedule | `kernel/src/sync.rs`, `threads.rs` |
| `kernel` | `idle: clock to deadline` instant | `Machine::run` |
| `interrupt` | span per DISPATCH_LEVEL call on the DPC row: `DPC`, or the periodic source's name (arg `routine`) | `Machine::run_on_dpc_stack` |
| `interrupt` | async span named after the `GuestInterrupt` (vblank, ...) from delivery to return | `runtime/src/interrupt.rs` |
| `d3d` | `Swap`, `KickOff`, `pushbuf drain` (arg `bytes`) | `d3d/src/device.rs` |
| `gfx` | `queue submit`, `present` | `d3d/src/gfx_backend.rs` |
| `hle` | `hle call` (arg `pc`) around every Rust override | `runtime/src/handler.rs` |
| `gfx` | `draw` (one D3D draw through the backend), `shader prepare`, `flush pass` (arg `draws`), `render encode`, `pipeline wait` (a draw's pipeline still compiling), `texture upload`, `surface write`, `surface snapshot` | `d3d/src/gfx_backend.rs`, `gfx/src/renderer.rs` |
| `gfx` | GPU round trips: `surface readback`, `pixel count readback`, `GPU wait` (`D3DSWAP_FINISH`) | `d3d/src/gfx_backend.rs` |
| `guest` | `slice` carries arg `accesses`: the guest accesses it charged to logical time | `kernel::Machine::run` |
| `kernel` | `idle: clock to deadline` carries arg `ticks`: the logical time the advance skipped | `Machine::run` |
| `sched` | `schedule` span per `Machine::run` iteration (the slice or DPC nests inside, so its self time is scheduler work); `pacer wait`; counter `logical us` (the logical clock, after every charge and idle advance) | `kernel/src/threads.rs` |
| `lock` | `memory lock wait`: a wait for the guest memory lock while another thread held it, on the waiting thread's row. Uncontended locks record nothing; `Runtime::memory_contention()` counts waits and their total length whatever is recording | `runtime/src/services.rs` |

`hle` is off by default because it fires on every kernel, XAPI and D3D call.

Cost when off: every hook is one relaxed atomic load and a branch. Nothing
allocates, locks or reads the clock unless a category is recording. A
`Span` is one pointer, null when inert, and its drop is an inlined null
check; the recording path is out of line.

Each `start` and `stop` begins a new capture generation. It is stored in
the same atomic word as the category mask, so a hook's one load gives
both whether to record and which capture the event belongs to, before
the clock read or anything else. The event is pushed, and a wait is
registered, only if that capture is still recording, which is checked
under the lock. So a
span (or wait) opened in one capture and closed after `stop`, or after the
next `start`, never lands in a trace.
`timeline::frame()` always bumps a frame counter (`timeline::frames()`),
one relaxed `fetch_add` per Swap, so fps is available without recording.
When on, an event is a clock read and a push under one mutex. The buffer
holds four million events by default; any beyond that are counted as
dropped and reported.

Adding an event:

```rust
let _t = timeline::span(timeline::Cat::D3d, timeline::Track::Host, "SetTexture");
timeline::instant_arg(timeline::Cat::Kernel, timeline::Track::Guest(tid), "signal", "handle", h as u64);
```

Names are `&'static str`; put variable data in the one numeric argument.

## Frame budget

```
h2 profile budget $TMPDIR/trace.json          # table
h2 profile budget $TMPDIR/trace.json --json   # every frame
```

`perf::budget` reads a trace back and splits the time between
consecutive `frame` instants into the *self* time of the game thread's
spans: each instant goes to the innermost span open then, or to
`(no span)`, so a frame's rows add up to its wall time. The game thread
is the host thread that recorded the frames; its spans are its own host
events plus the guest-track `slice` and DPC spans (the scheduler records
them on that thread). Guest-track kernel waits are left out, since they
cover a blocked guest thread while others run. With `--cats all`, `hle
call` spans take HLE time out of `slice`; with the default categories
`slice` self time is guest code, the runtime's accessors and HLE
together, and the sampler splits it.

The header gives, per frame, the logical time (`logical us` counter),
the logical/wall ratio, vblank DPCs, slices, charged accesses, and idle
advances with the logical time they skipped. Contended memory-lock waits
are listed per host thread. [notes/menu-budget.md](notes/menu-budget.md) is a main-menu budget
made with it.

## Benchmarks

```
h2 bench --out $TMPDIR/h2bench --label before
# ...change something...
h2 bench --out $TMPDIR/h2bench --label after
h2 bench compare $TMPDIR/h2bench          # the two newest runs
h2 bench compare a.json b.json --threshold 3
h2 bench --only hot-loop --samples 20
h2 bench list
```

| scenario | unit | what it runs |
|---|---|---|
| `hot-loop/alu` | ns/trip | register arithmetic loop in generated code (locals and the back-edge preemption poll) |
| `hot-loop/memory` | ns/trip | inc/load/add-to-memory loop through `h2abi_v1_read`/`write` |
| `hot-loop/call` | ns/trip | call/return per trip: two runner transfers and two dispatches |
| `hot-loop/x87` | ns/trip | x87 load/mul/add/sqrt/div/store in generated code ([x87 comparison](#which-tool-fits-which-job)) |
| `d3d/pushbuf-frame` | us/frame | `Device::kick_off` draining 1000 inline QUADLIST draws (26 words each) through push-buffer decode, immediate-mode assembly and a recording backend |
| `boot/menu-hold` | fps | boot to the main menu, then hold it for `--frames` frames (wall clock); needs the boot hook below, which is not built |

The hot loops are synthetic x86: assembled with iced-x86, lifted, emitted
and compiled with clang into `--work` (default `$TMPDIR/h2perf-work`,
reusing emit's object cache), then run by the real runtime. There are no
retail bytes. The push-buffer frame uses synthetic guest memory laid out
like crates/platform/d3d's tests.

Each scenario gets `--warmup` untimed runs (default 2), then `--samples`
timed ones (default 10). A scenario that cannot run (no clang, no boot
driver, no disc) or fails is recorded as skipped, with the reason.
`--quick` shrinks every workload; the tests use it.

A run is written to `<out>/<unix seconds>-<label>.json` and never
overwrites an existing file:

```json
{ "schema": 1, "label": "after", "git": "<commit>", "git_dirty": false,
  "host": { "os": "macos", "arch": "aarch64", "cpus": 12 },
  "started_unix": 1794721821, "samples": 10, "warmup": 2,
  "results": [ { "scenario": "hot-loop/call", "description": "...",
    "unit": "ns/trip", "better": "lower", "samples": [ ... ],
    "stats": { "median": 0, "mean": 0, "min": 0, "max": 0, "stddev": 0, "mad": 0 },
    "extra": { "trips": 200000 }, "skipped": null } ] }
```

`compare` matches scenarios by name and compares medians. A change counts
when it is bigger than both `--threshold` (default 5%) and twice the larger
relative MAD of the two sides, so a noisy scenario needs a bigger move.
Results are `REGRESSED`, `improved`, `same`, or `-` when one side is
skipped or missing. The exit status is 1 if anything regressed, so a script
can gate on it. `--json` prints the rows as JSON.

Adding a scenario: implement `perf::bench::Scenario` (`prepare` untimed,
`measure` one sample) and add it to `perf::scenarios`.

## Boot hook

The boot driver (crates/apps/boot) depends on `perf` only behind its `perf`
feature, which the CLI's `perf` feature turns on.

1. **Profiling `h2 run` and `h2 play`** (on master). With an `h2` built
   with `--features perf`, `H2_PERF_FROM=N` opens
   `perf::Session::from_env()` at present N, on the game thread, and
   finishes it when the slice loop ends, before the dylib is unloaded:

   ```
   H2_PERF_FROM=4220 H2_PROFILE=$R/prof H2_PROFILE_HZ=250 \
   H2_TIMELINE=$R/trace.json H2_TIMELINE_CATS=all h2 run disc/halo2.iso ...
   h2 profile budget $R/trace.json
   ```

   `H2_PERF_FROM=level` opens it at the first present after a level
   loaded instead, so a run straight into a level profiles from its first
   frame ([level-boot.md](../runtime/level-boot.md)):

   ```
   H2_PERF_FROM=level H2_PROFILE=$R/prof H2_TIMELINE=$R/trace.json \
   h2 play disc/halo2.iso --level 03a_oldmombasa --difficulty normal
   ```

   It prints `level boot: 03a_oldmombasa loaded at present N`, then
   `perf: session from present N+1, the first after 03a_oldmombasa loaded`.
   With no level requested it waits for the first game that is not the
   main menu.

   The fallback interpreter is already recognised (`boot::fallback::*` is
   the `fallback` category). The kernel, runtime and d3d events come from
   the hooks above. Swap counts frames. For frame times without a trace,
   `H2_FRAME_STATS=1` (docs/runtime/app.md "Frame stats") splits every frame into
   phases at a clock read per phase switch.
2. **`boot/menu-hold`** (no driver registered). `perf::boot::BootDriver`
   (`crates/tools/perf/src/boot.rs`) is the hook: `boot_to_menu()` runs until the
   main menu is up and `step()` runs one iteration of the slice loop.
   `perf::boot::set_boot_driver` registers a factory before
   `perf::cli::bench` or `perf::cli::profile` runs. Nothing in `crates/apps/boot`
   or `crates/apps/cli` calls it, so the scenario reports itself skipped.
   `perf::boot::hold` counts frames with `timeline::frames()` and reports
   frames per wall-clock second.

## Which tool fits which job

- **Runtime and emit fast paths:** `h2 bench --only hot-loop`
  before and after each change, then `h2 bench compare`. Use
  `h2 profile hot-loop/memory` or `hot-loop/call` to see which accessor,
  lock or dispatch step dominates, and `--by-category` to watch the
  runtime-to-guest ratio. `H2_PROFILE` with `H2_PERF_FROM` on an `h2 run` or `h2 play`
  (the first boot hook) gives the same tables for the real game, and the guest-instruction table
  shows which loops are worth a fast path.
- **wgpu submission (crates/platform/gfx):** `--timeline` with
  `--cats frame,d3d,gfx` shows `queue submit` and `present` spans against
  frames and `Swap`. The profile's `gfx` category, and the `d3d` versus
  `gfx` split, separate CPU-side translation from submission.
  `d3d/pushbuf-frame` benchmarks the CPU side without a GPU.
- **IR optimisation passes (crates/recompiler/opt):** `hot-loop/alu` and
  `hot-loop/memory` are mostly generated code, so a pass that helps should
  move them. Compare runs built with and without the pass (the scenarios
  rebuild through emit, so the pass applies). The "guest instructions"
  table and the `@0x...` folded leaves show which instructions the time
  goes to.
- **Native x87 (crates/recompiler/emit):** `hot-loop/x87` runs generated x87
  load/mul/add/sqrt/div/store code. Run it with the default and with
  `H2_X87_NATIVE=0` in separate `--work` directories;
  [x87-native.md](../lifter/x87-native.md) describes the guards and oracle
  checks. Hot-loop JSON results also record `cpu_ns_per_trip` on Unix, the
  mean thread CPU time per trip, excluding descheduling and background
  worker threads (`crates/tools/perf/src/hotloop.rs`). The printed ns/trip
  statistic is wall time.
- **Build dependencies:** `perf` brings in `addr2line` (with its loader:
  `gimli`, `object`, `memmap2`, `typed-arena`), `rustc-demangle`,
  `serde_json` and `iced-x86` (code_asm), but only into `cli` builds with
  `--features perf`. `timeline` has no dependencies. The hot-loop scenarios use emit's object
  cache under `--work`. Nothing here measures build time yet; a
  `build/emit-shard` scenario would fit the same harness if wanted.

## Device CPU cost (crates/platform/d3d)

`cargo bench -p d3d --bench pushbuf` replays a synthetic frame (1,200 draws,
mostly indexed, texture rebinding on two stages, constant uploads, inline
state through the push buffer, vertex-input switches) against a null backend,
so only the device's own work is timed. Nothing in it is retail data.
It runs over `SparseMemory` and calls the device directly (no runtime lock,
ABI or write-watch cost), uses 48 unchanging textures with
`UploadPolicy::Tracked`, a pixel shader that samples nothing, and changes
simple state on almost every draw (so translation-memo hits are rare). It
measures the device's own per-draw work, not whole-game CPU per frame or
other upload policies. It prints mean, median and p10 ms per frame; on a
shared machine the mean is dominated by scheduling outliers, so compare p10
or median. `H2_BENCH_FRAMES`, `H2_BENCH_DRAWS` and `H2_BENCH_TRACE=1`
(per-frame times) tune it (`crates/platform/d3d/benches/pushbuf.rs`). To profile, run
the bench binary under `sample <pid>` or `samply record`.

`cargo bench -p d3d --bench streams` times copying draws' vertex streams:
1,000 draws a frame over three streams of large vertex buffers, index windows
of 500 to 4,000 vertices, and one draw in `H2_BENCH_IMMEDIATE` (default 4) an
immediate `Begin`/`End` quad, directly and through the render thread. It also
prints allocations of 4 KiB or more per draw (the index list accounts for 0.75
of them at the default ratio; anything above is stream buffers the pool did not
supply). A stream is appended to an empty pooled buffer straight from guest
memory (`GuestMemory::read_append`, no zero fill); the pool hands out the
smallest buffer that fits, so a small buffer is not regrown when a large one
is waiting; immediate draws fill pooled buffers too.

Results are in [notes/device-cost.md](notes/device-cost.md).

## Test and bench knobs

Environment variables read by individual tests and benches, not by `h2`.
Unset, each takes its default.

| variable | read by | meaning |
|---|---|---|
| `H2_BENCH_FRAMES` | `cargo bench -p d3d` (`pushbuf`, `streams`, `vertex_fetch`) and the d3d benchmark tests (`--ignored`) | frames per run (the default differs per bench) |
| `H2_BENCH_DRAWS` | the `pushbuf`, `streams` and `vertex_fetch` benches and some d3d benchmark tests | draws per frame (defaults 1200, 1000 and 1000 in the benches) |
| `H2_BENCH_GUEST_ONLY` | `pushbuf` | set: run only the guest-memory dirty-tracking benchmark, then exit |
| `H2_BENCH_MODES` | `vertex_fetch` | `cpu` or `gpu`: time one fetch mode only, e.g. to profile it |
| `H2_BENCH_FILLS`, `H2_BENCH_SMALL`, `H2_BENCH_ROUNDS` | the ignored `render_churn` test in `crates/platform/d3d/tests/render_churn.rs`, its game-frame section | fills per visit (4), small draws per visit (0), rounds, the median reported (3) |
| `H2_DIFF_SEEDS` | `crates/rewrites/engine/tests/{differential,layer_differential}.rs` (`--features differential`, need `H2_XBE`) | comma-separated seeds; the layer test defaults to `1,7,99`, `differential` to its first default seed |
| `H2_DIFF_SCALE` | `crates/rewrites/engine/tests/differential.rs` | a multiplier on the cases per seed (default 1) |
| `H2_EMIT_FULL` | `crates/recompiler/emit/tests/entry.rs` (needs `H2_XBE`) | `1`: build the whole image through generated C instead of only the functions the entry run lifted (minutes, cached afterwards) |
