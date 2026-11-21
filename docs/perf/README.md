# Performance

Status: Built. The tools and the design pages below are current; the budgets are dated notes.

The target is the retail game running correctly at 30 presented frames
per wall second (33.3 ms a frame) in menus and in levels, with margin
under that line, and a headless sim tick under the budget in
[sim-budget.md](sim-budget.md). The design pages here state how the code
works now; every measurement is a dated entry under [notes/](notes/), with
its commit and method. Re-measure before acting on one.

## Baseline

| Area | Where the number is | What it is |
|---|---|---|
| Waterworks sim tick, four local bots | [sim-budget.md](sim-budget.md) | 14.12 ms/tick against a 7.48 ms target, with the guest functions that consume it |
| Who owns each sample of that sim | [sim-attribution.md](sim-attribution.md) | the owner of every main-thread sample and what each rewrite phase saves |
| An in-level frame | [notes/gameplay-budget.md](notes/gameplay-budget.md) | where the time of a frame went, and the worst frames, at the dates recorded there |
| A main-menu frame | [notes/menu-budget.md](notes/menu-budget.md) | the same for the main menu |

## Which page answers what

| Question | Page |
|---|---|
| How do I profile, trace or bench something? | [profiling.md](profiling.md): `h2 profile`, `h2 bench`, the timeline, test and bench knobs |
| How do generated code and the runtime touch guest memory, call and exit? | [guest-memory.md](guest-memory.md): the inline RAM path, direct calls, the exit call, register homes, runner transfers |
| What did each of those cost or save? | [notes/guest-memory.md](notes/guest-memory.md) |
| Which guest functions consume CPU in an unrendered Waterworks sim, and who calls them? | [sim-budget.md](sim-budget.md) |
| Which subsystem owns each sample of that sim, and what does each rewrite phase save? | [sim-attribution.md](sim-attribution.md) |
| Where does an in-level frame's time go? | [notes/gameplay-budget.md](notes/gameplay-budget.md) |
| Where does a main-menu frame's time go? | [notes/menu-budget.md](notes/menu-budget.md) |
| Where does a frame go on a Steam Machine, and why does the music drop out? | [notes/steammachine-perf.md](notes/steammachine-perf.md) |
| What does the D3D device cost per draw? | [notes/device-cost.md](notes/device-cost.md) |
| What structural changes would close the gameplay gap? | [2026-11-16-frame-time.md](../reviews/2026-11-16-frame-time.md): ranked changes (a frozen review; the design pages hold what was done) |
| What did we try and drop? | [notes/not-adopted.md](notes/not-adopted.md) |

Read profiling.md first, then the budget for the scene you care about.
What is planned is in [../reviews/2026-11-20-architecture.md](../reviews/2026-11-20-architecture.md).

## From `h2 play` to the main menu

- **Before guest entry.** With a current module, play opens the window and devices.
  Boot maps the XBE, loads the native library, checks its ABI and registers the
  entry and resume tables. One recorded run spent 2.5 s in this interval for
  18,854 entries and 64,140 resumes. That timer includes unread-target setup.
  Fallback analysis overlaps boot when a target list exists. Boot then mounts
  the VFS, prepares map offers and the campaign catalogue, and installs HLE
  and feature hooks. The bot nav scan already runs on a worker.
  [Recorded startup log](../../crates/apps/cli/src/coop_verdict/host-at-menu.log)
- **First VFS access to mainmenu.** Boot reads installed campaign PNGs. The
  first stat, directory listing or read that needs the rewritten map builds
  its picture tags. The reported ten-card rebuild took about 1 s. Subsequent
  boots hash the actual source map and ordered cards, then restore the verified
  map and tag IDs from `<data>/cache/campaign-cards`.
- **Guest loading screen.** The title formats N: (including mainmenu) at boot.
  Shared slots on Z: survive subject to the title’s checks. On a miss, the
  guest copies and checksums the payload, flushes it and publishes the header.
  A fresh-data **level** boot at `26f45a90` drew 1,744 loading frames for
  838 MiB before batching. Foreground payload copies for slots 0–2 can now run
  128 steps between presents, previously 16. The title still loads tags and
  uploads resources. Its movie wrapper controls Bink frame waits, audio and
  skipping. [Cache copy measurements](../runtime/cache-slots.md#copy-speed-on-the-loading-screen)

For a menu A/B, start from equivalent data roots with the same module, disc,
content and movie settings. Capture the startup log and use `h2 agent loads`
with frame stats enabled. Compare the boot stall's duration, presents,
`read_bytes` and `file_io_us` with and without `H2_FEATURES=-map-copy.boot`.
The agent's interval starts when the agent starts. Time launch separately to
include the earlier host setup. [Load timeline](../runtime/agent-api.md#load-timeline)

## Experiments not adopted

[notes/not-adopted.md](notes/not-adopted.md) lists the experiments that
lost, with the result of each. The Mach exception write-watch prototype,
no longer in the tree, is written up in
[notes/fastmem-proto.md](notes/fastmem-proto.md).

## Elsewhere

Performance work that lives with its area:

- [build-perf.md](../lifter/notes/build-perf.md): build times and the caches
- [x87-native.md](../lifter/x87-native.md), [sse-native.md](../lifter/sse-native.md),
  [x87-transcendentals.md](../lifter/x87-transcendentals.md): floating point
- [render-thread.md](../graphics/render-thread.md),
  [pipelined-render.md](../graphics/pipelined-render.md),
  [occlusion-culling.md](../graphics/occlusion-culling.md),
  [layer-draw.md](../graphics/layer-draw.md): rendering
- [high-framerate.md](../game/high-framerate.md): 60 and 120 fps
