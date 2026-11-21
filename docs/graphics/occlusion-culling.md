# Occlusion culling of objects

Status: Research. The read below is current; the Hi-Z culling built on it
was not adopted and is deleted ([not-adopted.md](notes/not-adopted.md)).
Code: `crates/rewrites/engine/src/render_loop/occlusion/guest.rs` (the
addresses), `crates/hooking/guest-layout/src/points.rs` (`OBJECT_RENDER_RECORDS`,
`OBJECT_RENDER_SETUP`, `SCENE_TRANSPARENTS`).

The game's render front-end is CPU-bound guest code: each draw costs
about 8 to 14 µs on the game thread before it reaches the GPU. This page
is the read of how the game picks the objects it draws each frame, where
a per-object skip can go so a hidden object costs almost nothing, and how
the game scales the W-buffer. The native renderer's stages, `FrameScene`'s
camera and the scene step read the same addresses.

Evidence labels as in [feature-engine.md](../engine/feature-engine.md):
**Code** (read in the disassembly, `h2 disasm`), **Test**, **S**
(supposed). No game data: addresses and offsets are research points.

## The read: how a view's objects are chosen (Code unless marked)

All of this runs in `render_view_scene` `0x0002ba10`, **before**
`render_scene_passes`:

```text
render_view_scene 0x0002ba10
  0x0002b990
    0x0003f2c0   ECX the camera 0x004b9da0, EAX the projection 0x004b9e14,
                 EDI the view's player unit; ret 4
      0x000441b0   the view frustum (0x1bc bytes)
      0x001320f0   visibility: EAX 0 = the main struct 0x00547f88
                   (EAX 1 = 0x0054aa68); stores it to [0x004c0b80]; ret 0x1c
        0x00132b80   sets the mode: 0 portal flood 0x00173130 from the
                     start cluster (clusters with clipped frusta), 1 sphere
                     0x00132d60, 2 everything 0x001733e0
        0x00132220   gather: per visible cluster, both object
                     cluster-reference partitions -> 0x00133050 per object
      0x0003f33c..   appends the view's player unit to the object list
                     if it is not there
      0x0002cb30   resets the render record counters
      0x00133390   records for the BSP clusters
      0x00133520   records for the instanced geometry
      0x00133720   records for the objects  <- OBJECT_RENDER_RECORDS
        0x00134240   reorders the object list
        per listed object: 0x00133790      <- OBJECT_RENDER_SETUP
      0x00044720
render_scene_passes 0x0002bcd0
  0x00040e90   "visibility" at site 0x0002bd2d: consumes the records:
               0x00041c80 object attachments (lens flares, light volumes,
               liquids, recursing into children), 0x00044ac0 walks the
               0x004ba138 records; effects 0x00176bb0/0x00176cb0/0x00174220,
               item lists 0x000420a0/0x00042760
  layers: 0x00044550 -> 0x0003bc30 -> per entry handler -> shader draw
          0x0001bbf0, geometry 0x00041040 (reads the 0x004ba138 records)
```

So `0x00040e90` (0.9 ms a frame in the census) is not where objects are
chosen; it consumes what `0x0003f2c0` built.

### The gather and the per-object test

- `0x00132220`: for each visible cluster `i < [[vis]+0xa6c]` (cluster
  index the word at `[vis]+0xa6e+0x1a·i`), walks partition 1 (heads at
  `[0x004de2e0][cluster]`, nodes in the datum array `[0x004de2e4]`: node
  `+4` the object handle, `+8` the next) and partition 2 (`[0x004de2d4]`,
  `[0x004de2d8]`), and calls `0x00133050` for each object. Objects in the
  sky cluster are force-added (`0x00132681`).
- `0x00133050` (EAX the visibility struct, stack `(handle, cluster
  ordinal, skip-frustum byte, force byte)`, `ret 0x10`): dedups by a
  per-pass stamp (`[0x004de300 + 4·index]` against `[0x004de2fc]`), reads
  the object's sphere (body `+0x30` centre, `+0x3c` radius in the main
  view; `+0x50`/`+0x5c` otherwise), tests it (`0x00132e60`: the cluster's
  portal-clipped frusta through `0x00165010`/`0x00181800`, or a sphere
  test) and at `0x0013312d` (`test al, al / je 0x0013329a`) accepts or
  rejects. Accepted: `0x00132fd0`, the flags word `0x00132b10`,
  `0x00134300` (body `+4` bit 31), the list append `0x00163c80` and the
  stamp `0x000bec70`.
- The list (`[vis+0x14]`): `+0` capacity, `+4` u16 count, `+8` u16
  flags, `+0xc` u32 handles, `+0x10` u32, `+0x14` u16.

### The per-object render setup `0x00133790` (the cost)

Called only from the loop `0x00133746..0x0013377a` in `0x00133720`. ESI
the visibility struct; stack `(list index as a word, byte, byte*, -1, the
caller's argument)`; `ret 0x14`. The handle is `[[ESI+0x14]+0xc+4·index]`.
It computes the view depth (the projection's depth row, globals
`0x004b9e20`/`0x004b9e2c`/`0x004b9e38`/`0x004b9e44`) and pixel size,
picks the LOD (`0x0004baf0`, `0x0004b5d0`) and calls `0x00133ab0`, which
does the node and mirror passes, recurses into children (body `+0x10`
the first, `+0xc` the next sibling) and adds the object's `0x004ba138`
render records through `0x0002cbf0`. Every layer draws from those
records, so an object without records has no draws in any layer.

Its own "not drawn" exit (`0x00133a98`): `or byte [flags + 2·index], 1`,
ECX the flag's address, EDX the flags' base, `ret 0x14`. The caller
reads only AL: non-zero with view kind 1 re-runs the same index.

### Object fields (body = `[[0x004e0300]+0x44 + 12·index + 8]`)

| Offset | Field | Evidence |
|---|---|---|
| `+0x04` | flags (bit 31 sends the accept through `0x00134300`) | Code |
| `+0x0c` | next sibling | Code |
| `+0x10` | first child | Code |
| `+0x14` | parent handle | Code |
| `+0x30` | bounding sphere centre (3 floats), world space | Code |
| `+0x3c` | bounding sphere radius | Code |
| `+0x50`, `+0x5c` | a second sphere for the other views (S: the dynamic-light sphere) | Code; meaning S |
| `+0x64` | position | docs |
| `+0xaa` | type byte | Code |

### Camera and view (while the records are built)

- The render camera `0x004b9da0` (0x74 bytes): position `+0`, forward
  `+0xc`, up `+0x18`, vertical FOV `+0x28`, viewport `+0x30` (i16 top,
  left, bottom, right), near `+0x40` and far `+0x44` (S). The transparent
  sort reads the same position.
- **Why `+0x28` is vertical (Code, `0x0002fb70`, the projection's
  builder).** It takes `t = tan(+0x28 · 0.5)` with `fptan`, the aspect as
  `(right - left) / (bottom - top)` from `+0x30..+0x36`, and stores the x
  scale as `1 / (t · aspect · x extent)` and the y scale as `1 / (t · y
  extent)` (extents 1 by default): x takes the aspect, so `t` is the
  vertical half-angle's tangent. The Xbox widescreen path conditionally
  multiplies the viewport aspect by the anamorphic factor at `0x0045dfdc`.
  The subsequent test of byte `+5` at `[0x00510c50]` can change the
  vertical tangent or frustum extents independently of this width factor.
  The caller may also supply off-centre extents. The live guest gather
  uses this retail lens. Its per-object setup can advance gameplay RNG,
  so a wider host projection can expose regions for which the guest
  supplied no object records. See [high resolution](high-resolution.md#determinism).
- The projection `0x004b9e14` (built by `0x0002fd90`): world-to-view 4x3
  at `+0`, view-to-world at `+0x34`, four frustum bounds at `+0x68`, pixel
  scales at `+0xb8`/`+0xbc` (`0x004b9ed0` is the LOD pixel scale).
- `0x004ba050` the view index (`render_game_view` stores it), `0x004ba034`
  the frame counter (`render_views` increments it), `0x004b9ed8` the
  view's player unit (S).

### Projected size and level of detail

At 1920 rendered lines, a sphere that spans 10 guest pixels counts as 40
pixels for visual detail. Choosing 1.5x on the Video screen makes that 60.
`setting_defs::lod_pixel_multiplier` multiplies the active target height
divided by 480 by the player's LOD Distance during offline play. Protected
sessions use a fixed guest geometry multiplier independent of display
resolution. See [determinism](high-resolution.md#determinism). The guest projection already
accounts for the viewport and FOV, including split screen. Retail selects
the original pixel size at every resolution. See the
[Video settings](../game/settings-menu.md#video-screen).

- **BSP clusters and placements.** The record reset at `0x0002cb30` samples
  the LOD factor once per view. Guest and native record builders read that
  factor from the view's guest memory, including on a render worker. The
  reset restores any projection saved by an interrupted walk before
  installing the scaled vertical projection for the two structure walks. The guest
  walks and the Rust `world_walk` rewrite read the same value. A placement's
  world-space radius still decides whether to apply its small-object
  factor. The adjusted pixel size controls its fade and zero-alpha cull.
  Each emitted record stores a size byte, saturated at 255.
- **Object models.** The object walk at `0x00133720` restores the original
  projection. A model's detail tier follows its camera distance, not its
  projected size. `0x00133790` passes the camera-to-centre length times
  `0x004b9dcc` to `0x0004bcc0`, and the chooser at `0x0004c450` compares it
  with the model's cutoffs at `hlmt + 0x34`, world units, farthest first.
  The guest chooser and the Rust `native_scene_lists/setup/sections.rs`
  divide that distance by the factor, so at 1920 lines a model 40 units
  away picks the detail retail gives it at 10. Both preserve the transition
  hysteresis and resident-section fallback. The material record gets its
  own scaled size byte. For models with pose-dependent sections
  (`hlmt + 0xc4`), section selection keeps the retail distance. The
  selected sections can clear an object's dirty bit and trigger a shared
  animation update at `0x000bd7b2`. Object lighting uses the original
  projected size because effects read its cache on the game tick.
- **Other view consumers.** Lights and shadow casters compete for a fixed
  budget by relative projected size. Billboard rotation needs both original
  projection axes. Decorator detail and particle fades compare camera
  distance with authored world distances. These consumers keep their
  retail inputs, as do object distance fades and frustum tests.

### What does not go through the object list

The first-person and player chain after the passes (`0x004b9ed8` ->
`0x0002afa0`), effects and particles, the item lists of `0x000420a0` /
`0x00042760` (S: decals and similar), lights. Dynamic lights re-run
visibility with `0x0054aa68` and `0x00133720`; object shadows
(`0x000335e0` -> `0x00033400` picks up to 32 casters from the main list,
`0x00032350` draws them).

## Where to skip

**`OBJECT_RENDER_SETUP` `0x00133790`, main struct only** (ESI
`0x00547f88` and `[ESI+0x2a78] == 0`): a `before` hook that, for an
occluded object, does the "not drawn" exit in Rust (flag bit 0, AL 0,
ECX/EDX as the original) and returns with `ret 0x14`. Nothing of the
object's setup runs: no LOD, nodes, children or records, so none of its
layer draws, shader begins or geometry submissions. The object stays in
the list, so the attachment pass and the shadow caster choice see it as
they see an object too small to draw.

The alternative, rejecting at the gather's accept (`0x0013312d`), also
drops the object from the list: no lens flares or light volumes on it and
no shadow from it onto visible ground. Not used.

A skip is keyed by handle, not list index (`0x00134240` reorders the list). The
light views (`0x0054aa68`) and other view kinds are never touched.

## The W scale (Code, confirmed by the probe)

The device's `+0x944` (1 / w far) is not the scale Halo's draws use. D3D
derives it in `SetTransform(D3DTS_PROJECTION)` (`0x3f7ff0`), and the game
never calls that, so it stays at the identity projection's 1. The game
scales w itself:

- `render_game_views` stores the global near and far (`0x00485ad4`,
  `0x00485ad8`) in the render camera's `+0x40` and `+0x44`
  (`0x002236d0`, `0x002236e7`).
- `0x00012a50` and `0x00013450` build the program clip range from the
  same globals: `[near / far · zs, far / far · zs]`, `zs` the float at
  `0x0045dd40` (D24's full scale), clamped to `[0, zs]`.
- `0x00012d72` writes it to the device's program clip shadow `+0x1C10` /
  `+0x1C14` and sets `+0x1C18` bit 1 (the game owns it), so D3D's own
  `[944 · 93C · 948, 948]` is never pushed.

So w reaches the W-buffer scaled by `zs / far`: the stored depth is
`floor(view_depth · zs / far) / zs`, so a stored `d` in `[0, 1]` is a view
depth of about `d · far`. The device's `+0x944` stays at 1. The deleted
culling's probe confirmed it in Old Mombasa: with `+0x944` as the scale,
1929 of 2048 grid cells read nearer than half a world unit; rescaled by
the camera's far plane they read as street depths.
