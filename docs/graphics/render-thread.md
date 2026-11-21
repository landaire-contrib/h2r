# Render thread

Status: Built.
Code: `crates/platform/d3d/src/threaded.rs` (`d3d::ThreadedBackend`), `crates/extensions/game/src/d3d_glue.rs` (`render_thread_requested`, `render_thread_from`). Tests: `crates/platform/d3d/tests/threaded.rs`, `threaded_frame_stats.rs`, `depth_textures.rs`.
Switch: `H2_RENDER_THREAD=0` turns it off (on by default), listed in [switches.md](../runtime/switches.md).

On the Xbox the NV2A consumes the push buffer while the CPU goes on with the
next frame. `d3d::ThreadedBackend` gives the host the same overlap: it
wraps the `GfxBackend` and runs every `d3d::Backend` call on a thread named
`d3d-render`, in the order issued. The rule that must hold: the guest sees
the same values, in the same order, whatever the render thread's timing.
With `H2_RENDER_THREAD=0` the backend runs on the guest's thread and the
guest's CPU work and the host's rendering add up instead of overlapping;
that synchronous path is the reference the tests compare against.

## How it works

`h2 run` and `h2 play` run every guest thread cooperatively on one host
thread, the runner (`kernel::Machine::run`; in `h2 play` the thread named
`guest`, [app.md](../runtime/app.md)). D3D is a set of HLE overrides
(`d3d::runtime_glue`), so the device's work runs on that thread inside an
override, holding the guest memory lock and then the device lock.

| work | where | thread |
|---|---|---|
| push-buffer decode and inline state | `Device::sync_push_buffer`, `drain_until_callback` | runner |
| state translation, draw resolve (streams and indices copied out of guest memory) | `Device` draws (`device.rs`, `device/raster.rs`) | runner |
| texture decode | `Device` texture cache | runner |
| the device model: RTT dirty bits, lazy write-back, depth generations, fences, register words | `Device` | runner |
| shader preparation (NV2A translation cached by key), uniforms, bind groups | `Nv2aShaders::prepare` (`crates/platform/d3d/src/nv2a.rs`) | render thread |
| recording into the pending pass | `GfxBackend::record`, `gfx::Renderer::record` | render thread |
| encoding and submission | `GfxBackend` (`MAX_ENCODED_PASSES` = 1024 per submission, target switches, surface operations, `Swap`) | render thread |
| uploads, surface writes, snapshots, copies | `GfxBackend` | render thread |
| the present hook (PNG or the app's frame copy) | `GfxBackend::present_with_gamma` | render thread |
| the `D3DSWAP_FINISH` GPU wait | `GfxBackend` (`SwapFinish`) | render thread |

The hook publishes into a shared frame slot (`crates/apps/app/src/frame.rs`).
The `h2 play` window thread redraws the latest slot whenever the window
size changes, independently of guest frame publication
(`crates/apps/app/src/shell.rs`). On macOS, the window thread reconfigures
and presents the Metal drawable during the resize callback
(`crates/apps/app/src/surface.rs`, `crates/apps/app/src/metal_resize.rs`).
The persistent pipeline cache and its compile workers belong to the backend
and move with it.

The boot glue wraps the backend when `d3d_glue::render_thread_requested()`
holds (only the value `0` turns it off, `render_thread_from`): the
headless one with `H2_BOOT_GFX`, the window's in `h2 play`
(`boot::Host::render_thread`), and the one behind the in-between frames.
`H2_RENDER_THREAD_STATS` prints the waits when the backend is dropped.
Frame stats forward the worker's counters and timers through a queued
marker at the device's `Swap` boundary, so frame numbers hold without
`Swap` waiting; `render-wait_us` measures the guest thread's reply and
throttle waits apart from `cpu`
(`crates/platform/d3d/tests/threaded_frame_stats.rs`). `menu_cpu`
(`crates/apps/boot/examples/menu_cpu.rs`) prints
`guest_thread_cpu_ms_per_frame`, the runner thread's own CPU time, which is
the guest's critical path.

**Batches.** Calls collect in one pending list and cross the channel
together: one send and one wake per batch. A batch goes at every call that
waits (before the wait), at every present (before the frame-in-flight
wait), every `threaded::BATCH` (64) calls and on drop. Texture-proxy drops
go into the same list, so they keep their place. The render thread runs a
batch in order, so the calls and their order are the synchronous ones; they
only start later. The device hands each draw over with
`Backend::draw_owned`, so the render thread takes the `ResolvedDraw`
(streams, indices, state, shader inputs) without a copy and frees it there.
The stream buffers come back through a bounded pool (`SPARE_DRAWS`) for the
device to refill; vertex-cache snapshots ([d3d.md](d3d.md), "Vertex cache")
are shared, not copied. Wrappers forward `draw_owned` (`DiagBackend`,
`Headless`, `SceneSink`); one that does not falls back to `draw`, which copies.

## What can move

The guest observes the GPU only through values: guest memory the device
writes (write-backs, register words, fence words, `Lock`), the answers of
D3D entries, and pixel counts. Everything the device computes from guest
memory stays where guest memory is current, on the runner, in program
order. Everything after that is host work whose only output is pixels on
host surfaces.

The seam is the `d3d::Backend` trait. A `ResolvedDraw` holds no guest
pointers ([d3d.md](d3d.md), "Draws"): the device has copied the streams and
indices and translated the state.

- **Stays on the runner:** push-buffer decode, the pushed-state snapshot,
  state translation, draw resolve, texture decode, the device's whole
  model. Its order is the guest's program order.
- **Moves to the render thread:** every `Backend` call, run in the order
  issued on one queue.

The push buffer is decoded on the runner. The drain reads the state arrays
and resource memory at the moment of the draw (game code changes them right
after), and decoding it elsewhere would need real asynchronous fences, which
Halo would then wait on. Likewise the parts of a draw that read guest
memory (state arrays, vertex attribute format, stream and texture headers,
index lists, vertex bytes not in the vertex cache, the PSDEF and device
fields) stay at the call: the game rewrites all of them right after it
returns, and the device answers every fence as complete when it hands it
out, so a deferred copy would need every fence, `Lock`, `BlockOnResource`
and busy check to wait for it.

Where the guest waits on the GPU, all on the runner:

- **Fences** (`SetFence`, `BlockOnTime`, the reference counters at
  `+0x2C`/`[+0x30]`): the device drains first, so every fence has completed
  when it is handed out ([d3d.md](d3d.md), "GPU model").
- **`BlockOnResource` and the busy check `0x3fb710`**: drain, then lazily
  write back the render-to-texture levels the resource overlaps:
  `read_surface`/`read_depth`, a GPU round trip through the queue.
- **`KickOff`**: drains and rewrites the NV2A register words
  (`NV_PFIFO_CACHE1_DMA_PUT/GET` equal, `NV_PGRAPH_STATUS` idle).
- **Push-buffer callbacks** (`0x24970`): drain, write back, call.
- **Pixel counters**: `read_pixel_count` waits for its report;
  `GetVisibilityTestResult` polls (`try_pixel_count`) and answers
  `D3DERR_TESTINCOMPLETE` for a recent test whose count is not back
  ([d3d.md](d3d.md), "Visibility tests").
- **Depth aliases** (`0x509368`, the menu's fog pass): decoded from the
  depth buffer on the GPU (`GfxBackend`, `Backend::depth_alias_texture`).
- **`Swap`**: present, then the `D3DSWAP_FINISH` wait on the render thread.
- **Vblank and the frame wait** (`0x12b2e0`) wait on the logical clock,
  never on the GPU or wall time ([d3d.md](d3d.md), "Vertical blank").

## Keeping guest-visible semantics exact

Each `Backend` method falls in one of these classes
(`crates/platform/d3d/src/threaded.rs`):

1. **Reads** (`read_surface`, `read_depth`, `read_pixel_count`): queued,
   and the runner waits for the answer. Everything queued before has run,
   so the bytes are what a synchronous backend returns. These were full
   GPU round trips already, so the wait adds the queue's backlog, no more.
   `try_pixel_count` is the exception: it answers from the counts already
   landed and never waits. A count is the same either way; only whether it
   is back yet depends on timing, and the device turns that into
   `D3DERR_TESTINCOMPLETE` only where the hardware could ([d3d.md](d3d.md),
   "Visibility tests"; `Device::set_visibility_poll(false)` waits instead).
2. **Handles** (`create_surface`, `create_texture_target`,
   `upload_texture`, `snapshot_surface`, `surface_texture`,
   `report_pixel_count`): the guest side hands out a proxy id at once; the
   render thread maps it to the inner backend's handle when it reaches the
   command. A proxy texture freed by the device is freed on the render
   thread after the work queued before the drop.
3. **Draw outcomes** are decided on the guest's side. The device's state
   depends on them: a drawn draw marks an RTT level dirty and bumps the
   depth generation, a skipped one records `Event::DrawSkipped`.
   `Backend::judge` gives a `DrawJudge`, made before the backend moves to
   the render thread, that answers what `draw` will return from the draw
   and whether each bound texture has a sampleable view (known from the
   texture's host format when its proxy is made). For `GfxBackend` it runs
   the same routing function `draw` does (`route`: built-in shader,
   provider, or skipped), and for the provider `Nv2aJudge`, which builds
   the same program key (`program_key`) and runs the same WGSL translation
   and validation (`translate`), cached by key the way `prepare` caches
   compiles. So an untranslatable draw is skipped with the same reason as
   with the synchronous backend, and the run goes on. The draw still goes
   to the render thread, because `GfxBackend` submits a pending pass for
   another target before it skips.
4. **Other outcomes** (`clear_rects`: `Drawn`; `write_surface`,
   `copy_surface`: `true`; `snapshot_surface`, `surface_texture`,
   `report_pixel_count`, `depth_alias_texture`: `Some`) are answered at once
   with the answer `GfxBackend` gives for every valid call.
5. **Checks.** If the render thread later gets an answer other than the one
   returned (a judge that disagrees, or one of the above), it is latched
   and the **next backend call fails** with a named error. The run then
   stops instead of continuing with a device that differs from a
   synchronous run. `ThreadedBackend::misses` counts them.
6. **Device-internal reads.** A texture over the depth buffer (`0x509368`,
   `0x50936C`: the menu's fog/distortion pass) is made without a guest
   wait. `Backend::depth_alias_texture` lets the backend read back, decode
   and upload at that point in the queue, once per depth generation.
   Whether a description decodes depends only on the description, so the
   device checks it once per description on zero bytes and skips exactly
   where the synchronous path skips. `GfxBackend` makes the texture itself,
   on the GPU (`gfx::guest_words::GuestWords::depth_alias`, [d3d.md](d3d.md)
   "Depth aliases on the GPU"), so neither thread reads the depth buffer
   back for the formats that pass covers.
7. **Native draws.** A draw a native pass admits
   ([native-renderer.md](native-renderer.md), "Admission") is queued
   (`Backend::queue_external_draw`) when its pass retains the resolved D3D
   packet (`ExternalPass::resolved_fallback`), the packet writes the same
   colour and depth as the pass (`ExternalBinding::same_draw_writes`) and
   the judge answers `Drawn` for it. The device marks the targets written
   at once, and for a native producer (`Device::try_native_*_draw`)
   reports the draw committed, so the guest skips its own emission. The
   render thread records the pass or, if it refuses, draws the packet,
   which is the draw the guest would have issued. The guest's memory and
   the device's model are those of a committed draw either way, so the
   render thread's choice is not visible to the guest. Any other admission
   waits for the render thread's recording (`Backend::prepare_external`),
   one round trip each, unless its pass pins the attachments once per run
   (`ExternalPass::batch_key`). The particle, effect, single-draw UI and
   transparent passes are queued; a UI model draw with sibling ranges
   waits, because its packet holds only the first range.

Against the requirements:

- **Fences and reference counters.** The device model says every fence has
  completed when handed out, the register window says the FIFO has fetched
  everything, and both hold as far as the guest can tell: the only ways to
  look at GPU results are the reads in class 1, which wait.
- **RTT write-backs** (lazy, `rtt-direct`). The device decides when a level
  is written back; the write-back's `read_surface` waits for the draws
  queued before it. In-place sampling of a level binds the surface's own
  view on the render thread, after the draws into it.
- **Guest reads of GPU-written memory, pixel counters.** Class 1.
- **Vblank and present pacing.** Vblanks, the frame wait and the swap count
  are the device's and the kernel's, on logical time. The render thread
  finishes presents in order. `Swap` waits on wall time when the guest is
  more than two presents ahead (`MAX_FRAMES_IN_FLIGHT`), which bounds
  memory and latency. That wait, like every class-1 wait, is on host wall
  time and never charges logical time, so it changes no guest-visible
  value. `H2_BOOT_REALTIME` makes logical time follow wall time; the render
  thread only lets the runner reach a vblank sooner.
- **Determinism.** The render thread never reads or writes guest memory,
  the logical clock or the scheduler. The guest sees the same values in the
  same order whatever the render thread's timing, so slices, preemption and
  logical time are reproducible.
- **Errors.** A backend error stops at the next backend call (or the next
  read), not at the call that caused it. Every backend error is a stop
  either way; only the reported entry differs.

## Interfaces

- `d3d::ThreadedBackend<B>` (`crates/platform/d3d/src/threaded.rs`) is a `Backend`
  that moves `B` to the `d3d-render` thread. `finish`, `with_inner` and
  `with_surface` run something after the queue and wait (tests, reports).
  `with_jitter(seed)` delays the render thread by seeded random amounts
  (tests). `Waits` (`landed_counts`, `pending_counts`) counts the waits and
  the counts answered without the queue.
- `Backend::judge`, `draw_owned`, `try_pixel_count`, `depth_alias_texture`
  and `depth_level_texture` are the trait's hooks for it
  (`crates/platform/d3d/src/backend.rs`); a wrapper forwards them or the backend
  falls back to the synchronous behaviour.
- After each present and each batch the render thread moves the pixel counts
  the GPU has written (`Backend::try_pixel_count`, in report order up to
  the first not back) into a shared map. `GetVisibilityTestResult` polls it
  and answers `D3DERR_TESTINCOMPLETE` for a test ended at most
  `VISIBILITY_PENDING_SWAPS` `Swap`s ago whose count has not landed; the
  miss hands the pending batch over so the count lands. An older test, or
  every test with `Device::set_visibility_poll(false)`, waits for the queue.
- Depth levels (shadow maps) are lazy like colour levels and sampled
  through a texture the render thread makes on the GPU
  (`Backend::depth_level_texture`, [d3d.md](d3d.md), "Lazy depth levels").

## Rules and limits

- The judge stays in step with `GfxBackend::draw` and `Nv2aShaders::prepare`.
  Both share `route`, `program_key` and `translate`, so a skip reason added
  there is judged too. A failure added anywhere else is a mismatch, which
  stops the run with a named error rather than diverging. Translation runs
  twice per new program (judge and compile), and the key is built on both
  threads for every draw.
- `upload_texture`, `write_surface` and `mirror_write` copy their bytes on
  the guest's thread, because the trait passes references. Draws move.
- A backend error stops the guest at the next backend call.
- RTT write-backs the guest asks for (`BlockOnResource`, the busy check,
  `BlockOnTime`, push-buffer callbacks) wait for the queue.
- At most two presents are in flight; under load that limit is what holds
  the guest.

## Evidence

- Test: `crates/platform/d3d/tests/threaded.rs` runs one device sequence through a
  backend and through the render thread. The sequence clears an RTT level,
  samples it in place, has the guest store into it, writes it back with
  `BlockOnResource`, then runs `EndVisibilityTest`, `SetFence`,
  `BlockOnTime`, the busy check, `GetVisibilityTestResult`, `Swap` and
  `KickOff`, over three frames. Both paths give the same observed values
  (entry results, the level's bytes after each wait, 8 KiB of device words),
  the same backend calls, device stats, events and guest memory, and on the
  GPU the same back-buffer bytes
  (`the_render_thread_makes_the_same_backend_calls`,
  `the_render_thread_draws_the_same_pixels`).
- Test: the render thread lagging by seeded random amounts gives the same
  results, which is the determinism check
  (`render_thread_timing_changes_nothing_the_guest_sees`).
- Test: an untranslatable draw between two drawn ones is skipped with the
  same event, stats and pixels
  (`an_untranslatable_draw_is_skipped_the_same_way`); a wrong speculation
  stops the next call (`a_wrong_speculation_stops_the_next_call`); reads
  wait for the work queued before them
  (`reads_wait_for_the_work_before_them_and_textures_are_freed`).
- Test: a sequence several batches long gives the synchronous backend's
  calls in its order and the same answers
  (`batched_commands_run_in_order_and_reads_see_everything_before_them`).
- Test: depth-alias textures made on the render thread give the same
  read-backs, uploads, draws, skips and stats
  (`crates/platform/d3d/tests/depth_textures.rs`).
- Test: a native producer whose pass retains its packet commits without
  waiting for the render thread, which draws the same packet the guest's
  own draw resolves to when the recording is refused
  (`crates/platform/d3d/tests/admission_scope.rs`,
  `a_native_fan_with_a_retained_packet_commits_without_waiting_for_the_render_thread`).
- Test: the persistent pipeline cache builds the same pipelines behind the
  render thread.

## Measurements

The numbers (fps, guest-thread CPU, waits, byte-identical frames) are in
[notes/render-thread.md](notes/render-thread.md), with their dates, commits
and methods. The native particle and UI round trips and their removal are
measured on a Steam Machine in
[steammachine-perf.md](../perf/notes/steammachine-perf.md).
`cargo bench -p d3d --bench pushbuf` and `--bench draw_main`
time the guest's side of a draw through `ThreadedBackend`.

## Not verified

- A quiet-machine fps comparison with the render thread on and off on the
  current renderer; the recorded runs were single runs on a shared machine
  and predate the native renderer.

## Related

- [d3d.md](d3d.md): the device model, fences, visibility tests, lazy levels.
- [graphics.md](graphics.md): `gfx` and the `GfxBackend` it serves.
- [notes/render-thread.md](notes/render-thread.md): measurements.
- [gameplay-budget.md](../perf/notes/gameplay-budget.md): the frame budget
  these waits show up in.
