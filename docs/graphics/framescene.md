# FrameScene: a native renderer that owns the frame (design)

Change 7 of the [architecture review](../reviews/2026-11-20-architecture.md)
(section 4.3 and section 5 item 7), and lane 7 of the
[sim review](../reviews/2026-11-20-sim.md) (the presentation snapshot). This
page is the spec: types, threads, the order of work, the tests and gates
for each slice, the order of deletion and the commands that measure it.
Slices 0 to 3 are built; their sections say how. Of slice 4 the entry
census and the structure's layer draws from the plan are built, and its
section says what the replacements wait on. Of slice 5 the extraction of
the fog, the water and the post passes and the fog's and water's
recording on the render thread are built; its section says what the
replacements wait on. Of slice 6 the census half is built: the scene
sink files the guest's D3D work by slot and counts side targets and
readbacks (`frame-scene.sink`); its section says what is not. The rest
is the plan. Lanes write code and tests; the coordinator builds, runs and
measures.

Evidence labels as in [feature-engine.md](../engine/feature-engine.md):
**Code** (read in this repository or in the disassembly), **S**
(supposed), **Estimate**. Addresses and offsets are research points. No
game data.

## 1. Where it stands

- `impl.render.native` is the one render switch and is on by default.
  `=off` is the D3D path, kept as the reference.
- Every native stage is a tenant of the D3D frame. The guest runs its
  render front end on the game thread. At a hook, a stage reads guest
  memory, prepares draws on the game thread and hands the D3D device
  packets that join its open pass (`gfx::RecordedDraw`, after the
  native-cost lane). The stage then makes the guest skip the replaced
  items by editing mask bits and restoring them
  ([native-renderer.md](native-renderer.md)).
- Last clean A/B, before the native-cost lane: Old Mombasa gameplay at
  4x, native 32.35 fps against D3D 34.34, p99 49.7 against 39.5 ms. The
  native-cost lane cut the native passes and command buffers that caused
  most of it; its numbers are not in yet.
- Open on that path: the object walk gives back a part only when what
  it reads does not read (`walk_unsupported`, by cause `walk_part_*`,
  slice 3), the lights stage keeps a light's layers on D3D when a pass
  is refused by name or the additional casters run, and side passes at
  a render scale above 1 are not deferred.
- `native-render` depends on `d3d`, reads guest memory through
  `dyn GuestRead` in about 19 of its modules, and is wrapped in about 30,000
  lines of glue in `boot` and `feature`.
- 60 fps today is E1, reprojection inside `d3d::interp`
  ([high-framerate.md](../game/high-framerate.md)). 120 is not built.

## 2. Decisions

The user's decision is that rendering is one path: the sim runs, then the
native renderer draws, and nothing is drawn twice. The rest follows from
that and from the two reviews.

1. **`FrameScene` is the only thing the renderer reads.** It is owned,
   typed data with no guest addresses and no borrowed guest memory. It is
   built on the sim thread during the frame step that follows the frame's
   ticks, sealed, and published. After the publish nothing reads guest
   memory to draw.
2. **Two grades of content, one renderer.** *Typed* content is extracted
   from engine records (camera, visible clusters, object instances and
   palettes, lights, fog, post parameters) and the guest code that would
   have drawn it does not run. *Carried* content is a guest draw that the
   D3D state model resolved (a `ResolvedDraw`, which already holds no
   guest pointers) and that is stored in the scene at its place in the
   frame order instead of being sent to a backend. The native renderer
   draws both. A stage moves from carried to typed when that saves guest
   time or is needed for smooth motion. Carried content is the fallback
   for everything, which is what lets the renderer own the frame before
   every stage is typed.
3. **The tick does not change, and neither does what it can see.** The
   guest's render front end writes state the tick reads (section 5).
   Every slice keeps those writes, by leaving the guest code that makes
   them running or by making the same writes from Rust in the same
   order. The gate is the gameplay digest series, not pixels.
4. **Stages skip guest work by whole-call replacement, not by mask
   bits.** When a stage is typed and has no remainder, its guest entry
   (a layer draw for one layer, the light pass, the screen effects) is
   implemented as "do the tick-visible writes, return". `skip.rs` and
   the per-draw admissions go.
5. **No new switches.** Each slice lands under `impl.render.native`.
   `=off` stays the whole D3D path until the deletion in section 11, and
   then the switch goes too. Diagnostics are agent API methods.
6. **Interpolation is a property of the renderer.** With two scenes in
   hand the render thread draws any number of frames between them.
   `d3d::interp` (D and E1) is deleted when that works.
7. **The open items are not patched on the tenant path unless they block
   a slice.** Deferring side passes above 1x is not done at all: slice 6
   removes the second copy, and with it the problem. The object-walk
   fallbacks and the refused lights are slice 3, because typed stages
   need them gone to drop their guest entries.
8. **The stage 7a ports and the pipelined-frame worker stay stopped.**
   `FrameScene` replaces the worker. The scene passes are not ported as
   an orchestrator; decision 4 needs only entries.

## 3. The frame, today and after

```text
today (impl.render.native=on)
  guest thread   input | tick | guest render front end ................ | Swap, wait
                                 |hook: read guest, prepare, packets|
  d3d-render                     decode'd draws + native packets -> encode -> submit
  window         composite the newest published frame

after slice 6
  sim thread     input | tick | frame step: owners, extract, carry | publish | wait
  render thread  take (S[n-1], S[n]) -> blend -> cull -> encode -> submit   (display rate)
  window         composite the newest published frame
```

The guest thread becomes the sim thread. It never waits for the GPU. The
render thread never touches guest memory and takes no guest lock.

## 4. The `FrameScene` type

### 4.1 Crates

| Crate | Holds | Depends on |
|---|---|---|
| `frame-scene` (new) | the types below, the builder, the two-scene history, the blend (pure math) | `bytemuck`; `gfx` without its `wgpu` feature, for `ResolvedDraw` |
| `scene-extract` (new) | guest memory to `FrameScene`. The only render code with guest offsets | `frame-scene`, `engine` (points), the layout crate of change 5 when it exists |
| `native-render` | `FrameScene` to wgpu. No `d3d`, no guest reads | `frame-scene`, `gfx`, `shaders`, `tags`, `map`, `wgpu` |
| `gfx` | gains the data types that `d3d::backend` owns today and both sides need: `ResolvedDraw` and its parts, `PixelRect`, the packet and target types (`ExternalTarget`, `ExternalDraws`, `ExternalBundles`, `TargetCopy`), and the NV2A pipeline key (`d3d::nv2a`'s `NativeDrawKey`, `NativeDrawMaterial`, `NativeDrawProgram`, `w_buffered`, `d3d_fog`) | as today |
| `d3d` | the guest-facing device. Depends on `frame-scene` to fill the carried part. Shrinks in section 11 | `gfx`, `shaders`, `frame-scene` |

Until change 5 lands, `scene-extract` keeps offsets in one private
`layout` module, one file per guest record, moved there unchanged from
`native-render` (`camera.rs`, `objects.rs`, `object_nodes.rs`,
`object_walk.rs`, `object_lighting.rs`, `world_ranges.rs`, `light_list.rs`,
`fog.rs`, `shadows.rs`, `transparent.rs`, `gels.rs`, `particles.rs`,
`effect_geometry.rs`, `post.rs`, `hud.rs`, `glow.rs`, `skip.rs`). Moving
them to the layout crate is then a rename.

### 4.2 Shape

Flat arrays and index ranges, so a scene is a handful of allocations that
are recycled, and so the blend can walk it without chasing pointers.

```rust
pub struct FrameScene {
    pub id: SceneId,            // map_epoch, guest frame number, tick after this frame's ticks
    pub stamp: Stamp,           // published: Instant (sim thread, at publish); ticks_run: u8 (0, 1 or 2)
    pub cut: Cut,               // bitset: map or BSP change, revert or state.load, view-set change
    pub views: Vec<View>,       // at most 4 game views plus the alternate view
    pub clusters: Vec<u16>,     // visible cluster ids; views index ranges of it
    pub instances: Vec<Instance>,
    pub parts: Vec<PartDraw>,   // what each instance draws, in the guest's layer order
    pub rows: Vec<Row>,         // [[f32; 4]; 3] palette rows, world space; 48 bytes
    pub lights: Vec<Light>,
    pub shadows: Vec<ShadowMap>,
    pub carried: Carried,       // ordered guest ops per slot (4.4)
    pub uploads: Vec<TextureUpload>, // guest-made textures changed this frame (Bink, glyph pages)
}

pub struct View {
    pub key: ViewKey,           // kind (game, cinematic, alternate), player index, view index
    pub cut: bool,              // this view's camera cut this tick (4.5)
    pub camera: Camera,         // position, forward, up, fov, near, far, viewport: today's RenderCamera
    pub first_person: Option<FirstPerson>, // its own projection, parts and rows in camera space
    pub clusters: Range<u32>,
    pub instances: Range<u32>,  // into FrameScene::instances, in the guest's order
    pub lights: Range<u32>,
    pub world: WorldState,      // texaccum use (a layer draw's material inputs are its LayerPass's)
    pub fog: FogState,          // planar and atmospheric fog words, resolved
    pub water: WaterState,
    pub post: PostParams,       // colour pass, screen effects, glow, fades: values, not draws
    pub slots: [Range<u32>; Slot::COUNT], // into Carried::ops
}

pub struct Instance {
    pub key: InstanceKey,       // object datum handle with its salt; identity across scenes
    pub model: ModelId,         // render model tag datum
    pub lod: u8,                // as the guest's setup chose it
    pub permutations: Permutations, // the guest's per-view permutation bytes, by value
    pub sphere: ([f32; 3], f32),
    pub lighting: Lighting,     // today's object_lighting record, by value
    pub defaults: Defaults,     // material function inputs, evaluated
    pub parts: Range<u32>,
    pub teleported: bool,       // set by extraction when the engine says so, else by distance
}

pub struct PartDraw {
    pub section: u16, pub part: u8, pub layer: u8,
    pub implementation: u8,     // the record's layer-3 implementation
    pub material: MaterialId,   // shader tag datum
    pub rows: Range<u32>,       // palette rows, already remapped for this section
    pub flags: PartFlags,       // stencil marking, strip topology (slice 3)
}
```

`Light`, `ShadowMap`, `FogState`, `WaterState` and `PostParams` take the
fields today's stages read at their hooks (`native_render::lights::Light`
and `LightConstants`, `shadows`, `fog::FogWords`, `post`), by value. The
last three are `frame_scene::late`'s (slice 5).
`ModelId`, `MaterialId` and texture ids are tag datums plus the map
epoch; they name GPU resources the loaders built from the tags, never
guest pointers. `InstanceKey` is opaque to the renderer.

Bounds come from the guest's own tables (850 render records, 2,560 layer
entries, **Code**, `object_walk.rs`), so a scene's typed part is a few
hundred kilobytes at most (**Estimate**).

### 4.3 What is extracted, and where from

| Content | Source on the sim thread | Grade at the end | Slice |
|---|---|---|---|
| Views and cameras | the view records `render_game_views` `0x002234e0` builds (`RenderCamera::read`) | typed | 1 |
| Visible clusters, placements | the Rust scene lists (`feature::native_scene_lists`: camera visibility) | typed | 1 |
| Object instances, parts, palette rows, lighting | the Rust object setup and render records (`native_scene_lists::setup`, `objects::record_rows`, `object_lighting`) | typed | 1 |
| First person | the layer-3 caller in `0x0003fdf0` (return `0x00040136`): its items, each record's mode at `+4`, the owner's lighting | typed | 3 |
| World layers 1, 3, 5, 7, 8, 20 state | each layer draw's material inputs at its begin (`scene_extract::layout::layer_inputs`: the render time, the view-wide engine inputs, the defaults) and each object item's lighting, which carries the lightmap cycle | typed | 1 (layer 3), 3 and 4 (rest) |
| Model decorators | the layer-3 list's items of build `0x0004de20`, each instance's `v11..v15` as the draw `0x0004e260` sets them ([native-decorators.md](native-decorators.md)) | typed | 4 |
| Lights and their geometry | the Rust light order and gathered light geometry (`native_scene_lists::lights`, `native_lights::walk`) | typed | 3 |
| Shadow maps | the shadow layers' casters and projections (`native_shadows`) | typed | 3 |
| Fog (layer 16), water (layer 6) | the device-resolved fog words; the layer gates and items (`scene_extract::late`, the glue) | typed | 5 |
| Post: colour pass, screen effects, glow | the arguments of `0x00013870` and its callees (`scene_extract::late::post`), the strip rows of `0x0002d565` | typed | 5 |
| Gels | the template and quad sites | carried, then typed | 6, 9 |
| Transparents | the sorted list `0x0001e370` dispatches | carried, then typed | 6, 9 |
| Decals, particles, contrails, effects | the producers' draws | carried, then typed | 6, 9 |
| Sky, layer 2, layers 15, 18, 19, 21 | their draws | carried | 6 |
| Lens flares and their visibility tests | the flare writer `0x00020bb0` and reader `0x00020770` | typed (a native query) | 6 |
| HUD, menus, loading screens, movies | the UI dispatch `0x00147af1`, the fallback view `0x002232a0` | carried | 6 |

"The Rust scene lists" matters: `feature::native_scene_lists` already
builds the visibility lists, object setup and render records in Rust and
publishes them into guest memory for the guest's consumers. Extraction
takes the same results before they are written out. When a list's last
guest consumer is gone (its stage is typed and has no remainder), the
write to guest memory stops, except for the words section 5 lists.

### 4.4 The carried part

```rust
pub struct Carried { pub ops: Vec<GuestOp> }

pub enum GuestOp {
    Draw { draw: ResolvedDraw, camera: CameraUse },
    Clear { target: TargetRef, rect: PixelRect, what: ClearMask, values: ClearValues },
    Bind { colour: TargetRef, depth: TargetRef },
    Copy { from: TargetRef, to: TargetRef, rect: PixelRect }, // framebuffer snapshots, scene copies
}

pub enum TargetRef { SceneColour, SceneDepth, Aux(AuxId), Texture(TextureId) }
pub enum CameraUse { Screen, View { view: u8, block: ViewBlock }, Unknown }
```

`Slot` names a position in the frame, in the guest's order: `Opaque`,
`AfterOpaque`, `AfterLights`, `Transparents`, `AfterPost`, `Overlay`
(HUD and UI), and `Outside` (frames that never enter a game view). The
D3D device fills the slot the frame step declared current (slice 6).

`Aux` targets are the guest's own side targets (a blur chain in a menu,
a gel target) for a stage that is still carried. The renderer makes one
texture for each at the display scale and plays the ops in order.
Nothing in a scene can be read back by the guest (section 8.3).

`CameraUse::View` marks a draw whose vertex program's view block
(`VIEW_CONSTANTS`) was rebuilt on the host from the view's camera and
matched the uploaded words bit for bit. That is high-framerate.md's
rule for design D, applied at capture: only such a draw may be redrawn
with a blended camera. `Unknown` is drawn as the tick drew it.

### 4.5 Who writes, who reads

- **Writer: the sim thread only**, through `SceneBuilder`, during the
  frame step. Typed parts are written at fixed points (section 5.1).
  The builder seals a `View` when its last point has run, and the scene
  at the present. A sealed scene is immutable (`Arc<FrameScene>`).
- **Readers: the render thread**, and through it the agent API
  (`render.scene`, `render.frame`). The editor and `viewer` become
  readers later by building scenes without a guest (out of scope here;
  it is why `frame-scene` has no guest or GPU dependency).
- **Cuts.** Extraction sets `View::cut` on a scripted camera cut (by
  observing the scripted camera path `0x0016c840`), on a respawn, and
  by a position and angle threshold as the backstop. `FrameScene::cut`
  is set on a map or BSP change, a checkpoint revert, an agent
  `state.load` and a change in the set of views.
- **Recycling.** The render thread returns a scene's buffers to the sim
  thread through a free list when it drops the scene. No allocation in
  the steady state.
- **Back to the sim: nothing that changes guest memory.** The render
  thread sends counters, capture results and query results on a
  `RenderFeedback` channel. Lens-flare pixel counts are the one value
  the guest reads; they are answered by polling, as
  `GetVisibilityTestResult` is, and under the same rule for
  deterministic runs (`d3d::Device::set_visibility_poll(false)`, which
  the deterministic tests set: the answer waits).
  Any later feature that needs a render result in the sim applies it at
  a tick boundary and records it in films as an input.

## 5. The frame step and what the tick can see

### 5.1 The step

Once per guest frame, after the frame's ticks, the guest calls
`render_frame_and_present` `0x00223240` (**Code**,
[render-loop.md](render-loop.md)). That call is the frame step. It stays
guest-driven: the guest builds the view records and runs its view loop.
What changes is what runs under it.

| Point (research points, **Code** unless marked) | Runs on the sim thread |
|---|---|
| `render_game_views` `0x002234e0`, `0x00223080` per view | guest, unchanged: the view records are camera state other code reads |
| `render_views` `0x0002b5d0` entry | `SceneBuilder::begin(frame)`; the game-time bracket runs in the guest as now |
| per view, visibility `0x00040e90` and the gathers | the Rust scene lists, with the guest's portal traversal and light frustum setup as their inputs |
| first `LAYER_DRAW` `0x00044550` of the view (layer 1) | extract the view: camera, clusters, instances, parts, rows, world state. This is where today's world and object hooks already assume the lists are final |
| each typed stage's entry | replaced: tick-visible writes, then return (decision 4) |
| each carried stage | guest, unchanged; its D3D draws land in the current slot (slice 6) or go to the D3D backend (before it) |
| the layer-3 caller in `0x0003fdf0` | extract first person |
| `render_screen_effects` `0x00013870` | extract `PostParams`; replaced when post is typed |
| frame tail `0x00013b90`, `0x00013cd0`; present `0x00013f10` | guest, unchanged (the present counter is tick-visible); then `seal` and `publish` |

"Tick end" in the review means this step. The guest computes visibility
and object setup inside it, so the snapshot cannot be taken earlier
without porting those, and does not need to be: it all runs on the sim
thread before the publish.

### 5.2 Writes the tick reads

The render census ([pipelined-render.md](pipelined-render.md), "Every
dependency line" and "The new dependency sites") and sim mode
([sim.md](../runtime/sim.md), "What the render writes that the sim
reads") list them. The frame step must leave each one as the guest
would have:

| State | Writer in the guest's frame | Rule for a replaced entry |
|---|---|---|
| The shared random seed `[0x004e7408]` | object code the render reaches (`0x001032a0`) | **Blocking.** A replaced entry makes the same draws from the seed in the same order, or it still calls the guest function that makes them. A sim-mode run already differs from a rendered run here |
| The effects random word | particle draws | the particle producers keep running (they are carried, then typed with the same draws from the word) |
| Particle system visibility and location stamps | the per-view update `0x00174420`, reached from the object and effect draw paths `0x0003fdf0` and `0x00040e90` | the replaced object entry calls `0x00174420` for the same systems in the same order |
| Animation cache use stamps (`+0x14` per entry) | `animation_resource_get` `0x001237e0` | replayed in visit order, as the pipelined prototype does |
| Present counter `0x00485aa0`, frame timing `0x004e63a8` | `0x00013f10`, the frame wait `0x0012b2e0` | guest code keeps running |
| Game-time bracket | `0x00146de0` and `0x00146b80` around `render_views` | guest code keeps running |
| Render frame counter `0x004ba034`, the one-shot byte `0x004e6098` | `render_views`, `0x00223080` | guest code keeps running |
| Texture, geometry and animation residency, the async I/O pool | the render's cache requests | the Rust object setup keeps making the guest's requests (it does today); the renderer's own resources come from the tags and do not depend on them |
| Object visit marks, the object `+0xcc` handle, light render fields | the gathers | not read by the tick (class a); kept while any guest consumer is left, then dropped |

Each slice that replaces an entry adds its row to a table in
`scene-extract`'s doc comment naming the writes it reproduces and the
test that proves each. The gate in every slice is the same: the
gameplay digest series (and the arena hash) of a run is identical with
`impl.render.native=off` and `on` (section 9.3).

A consequence, not a goal of this work: a host that runs the frame step
with extraction turned off would match a rendered client tick for tick,
which sim mode cannot today.

## 6. Threads

| Thread | Does | Never |
|---|---|---|
| **sim** (today's `guest`) | input, ticks, the frame step, publish, the logical frame wait | waits for the GPU, encodes, submits |
| **render** (today's `d3d-render`) | owns the pass graph and every GPU resource the frame uses; takes scenes, blends, culls, encodes, submits, publishes to the window's `TripleBuffer` | reads guest memory, takes the guest memory lock, runs guest code |
| **window** | composites the newest published frame, as now (`app::frame`) | |
| **loaders** | tags to GPU resources at map load and on demand; pipeline compile workers (`gfx`) | |

**Hand-off.** A mailbox of sealed scenes with two modes:

- *Paced* (play): the sim publishes and goes on. The render thread
  keeps the last two scenes it took. If the sim published more than one
  since, the older ones are dropped and counted (`scene_dropped`).
- *Locked* (headless capture, `--unpaced`, lab runs with
  `H2_BOOT_STOP_AT_PRESENT`): publish blocks while two scenes wait.
  Every scene is drawn once, so present-indexed captures and "fps" keep
  today's meaning, and today's two-frames-in-flight bound stays.

**Residency.** Loaders publish a resident set (an atomic bitset per
model and material). Extraction reads it. Before slice 6 an instance
whose model is not resident is left to the guest's own draw, as now.
From slice 6 it is not drawn that frame and is counted
(`scene_unresident`); loaders prefetch every model the scenario's
palettes name at map load so that the count is zero after loading.

**Before slice 6** the D3D backend is still in the frame, so its command
queue is the order. A typed stage is one command in that queue
(`d3d::SceneStage`): its attachments by slot, the plan naming the stage,
its guest view and its ranges, and the stage's typed inputs by value.
The render thread prepares and records it when it reaches it, into the
pass that is open (the packet path the native-cost lane built). The sim
thread does none of that work and does not wait for it. Slices 2 and 5
say which stages queue one and what stays on the sim thread.

**Snapshots and pause.** `state.save` needs no scene. `state.load`
publishes a scene with `cut` set. A paused game publishes nothing; the
render thread holds the last scene.

## 7. 60 and 120 fps from two scenes

The tick stays 30 Hz and bit-identical, so super bounces are untouched
by construction ([high-framerate.md](../game/high-framerate.md), section
4). The rules below are that page's design D, with the scene in place of
the replayed draw list. What D could not do cheaply (re-encode a
thousand guest draws per display frame) does not arise: typed content is
GPU-resident and drawn with indirect draws, so a display frame costs a
cull, a few uniform and palette uploads, and the encode.

**Time.** For a display frame due at wall time `t`, with scenes `A`
(older) and `B` (newer):

```text
alpha = clamp((t - B.published) / (B.published - A.published), 0, 1)
```

One tick of delay, as D. If the next scene is late, hold at `alpha = 1`.
Never extrapolate. If `B.ticks_run == 0` (pause, load) or the two are
more than 45 ms apart or `B.cut` is set, there is no blend: draw `B`.

**Exactness at the ends.** At `alpha >= 1` the renderer uses `B`'s
values themselves, not a blend that happens to equal them. A frame at
`alpha = 1` is therefore the same image as the 30 fps frame of that
tick, which is what the parity checks compare.

**What is blended**, with `B` as the base (its lists, its materials, its
carried ops) and `A` supplying the earlier value:

| Thing | Matched by | Blend | Snap (use `B`) when |
|---|---|---|---|
| View camera | `ViewKey` | position linear, orientation slerp, fov linear; projection rebuilt | `View::cut`, no match |
| Instance palette rows | `InstanceKey`, then part and row index | per row: translation linear, rotation slerp, scale linear | no match, different model or row count, `teleported`, a row moved further than a threshold |
| First person | the view | rows blended in camera space, so the weapon stays locked to the blended camera | weapon or mode change |
| Lights | light datum | position, direction, colour and cone linear | no match |
| Shadow maps | follow their casters | re-rendered from the blended casters when a caster moved; reused otherwise | |
| Visible clusters | | the union of `A`'s and `B`'s sets for the view, culled against the blended frustum | |
| Carried draws, `CameraUse::View` | | the view block is rebuilt from the blended camera; world-space geometry stays where tick `B` put it | |
| Carried draws, `Screen` | | as `B` drew them: 30 Hz, as authored | |
| Carried draws, `Unknown` | | as `B` drew them, and counted (`scene_held_draws`) | |
| Post parameters | | `B`'s | |

The blend runs on the render thread on CPU (a few thousand rows; the
slerp is exact, and a matrix lerp in the shader is not). One palette
buffer is uploaded per display frame.

**What a player sees** is D's care list: particles and decals are
world-anchored and correct under camera motion but move at 30 Hz until
they are typed (slice 9); objects culled at tick `B` can appear a frame
late at the screen edge in a fast turn; water reflections and camouflage
sample tick `B`'s scene copy.

**Pacing.** One frame per vsync at the display's rate, capped by
`graphics.fps` (`H2_FPS`, `h2 play --fps`); 30 means one frame per
scene. Alpha is computed for the predicted display time. On a
variable-refresh display, present when ready up to the cap. The
predictor and its counters come over from `d3d::interp::pacing`.

**Latency.** One tick, as D. Visual aim prediction (rotating the drawn
camera by the turn the next tick will apply) stays optional and later;
`boot::coop_look` already predicts look per rendered view and keeps its
hook.

## 8. Cutting the D3D path, stage by stage

### 8.1 Order

The order is the guest's own pass order where that is free, and "what
costs the most guest time" where it is not.

| Step | Stage | Before | After | Guest entry no longer run | Slice |
|---|---|---|---|---|---|
| 1 | Camera, world layer 3, objects | read guest at the layer hook, game-thread prepare, packets | typed; render-thread prepare | none yet (masks stay while there is a remainder) | 1, 2 |
| 2 | Object-walk remainder | about 14 objects a frame through D3D | typed (`PartFlags`) | the layer-3 geometry handlers for objects | 3 |
| 3 | First person | per-item replacement at issue | typed | the layer-3 call in `0x0003fdf0` | 3 |
| 4 | World layers 1, 5, 7, 8, 20 | recorded at the resolved draw | typed | those layers' draws | 3 |
| 5 | Lights and shadows | admitted per packet; some lights refused | typed, shadows batched in one pass sequence | the light pass `0x00031c80` and the shadow layers | 3 |
| 6 | Opaque block as a whole | mask bits edited and restored | whole-call replacement | `LAYER_DRAW` for layers 1, 3, 5, 7, 8, 20 | 4 |
| 7 | Fog, water | native passes at their layer hooks | typed | layers 16 and 6 | 5 |
| 8 | Post (colour pass, screen effects, glow) | guest code runs, strips replaced | typed | `0x00013870` and its callees | 5 |
| 9 | Everything else in a game view | D3D backend | carried in its slot | none: the guest still produces the draws | 6 |
| 10 | Frames outside a game view (menus, loading, movies) | D3D backend | carried in `Outside` | none | 7 |
| 11 | Transparents, decals, particles, contrails, gels | carried | typed, one at a time by measured guest cost | their producers' emission, then the producers | 9 |

After step 10 no draw reaches a D3D backend. After step 11 the D3D state
model serves only the UI, the HUD and the long tail.

### 8.2 The rule for replacing a guest entry

A stage's guest entry is replaced only when all four hold:

1. **No remainder.** Its refusal counters are zero over the census
   corpus (section 9.1) at the commit before.
2. **Writes accounted for.** The slice's table lists each tick-visible
   write under the entry (from the `render-census` feature, run by the
   coordinator on the corpus) and how it is kept.
3. **Hash parity.** Section 9.3.
4. **Pixel parity.** Section 9.2.

After a replacement, content the stage cannot draw is a named error: a
counter (`scene_dropped_<stage>`), one log line per cause, and a row in
`render.scene`. It is not sent to D3D. From slice 6 the fallback for a
stage that is not ready is to stay carried, which costs guest time but
draws correctly.

### 8.3 What the guest can no longer read

Today the guest can read what it drew: depth aliases, render-to-texture
write-backs, pixel counts. That is why a render scale above 1 keeps a 1x
reference copy. From slice 6 the scene is drawn once, at the display
scale, and the guest cannot read it. Each reader needs an answer before
slice 6:

| Reader | Answer |
|---|---|
| Lens-flare visibility (`0x00020bb0`, `0x00020770`) | a native query pass; counts scaled back to guest pixels; answered by polling (4.5) |
| The depth alias the fog pass samples | fog is typed (step 7) and samples the scene's depth on the GPU |
| Framebuffer snapshots (camouflage, screen effects, water) | `GuestOp::Copy` from `SceneColour`, on the GPU |
| A `LockRect` or write-back of a render target | none. Counted (`scene_readback`); the gate for slice 6 is zero over the corpus. If something real is found, it gets a typed stage or a feedback message |

## 9. Parity

### 9.1 Corpus

- **Gate corpus** (every slice): Old Mombasa (`03a_oldmombasa`) from
  boot, cutscene and gameplay, presents 1 to 5000; one multiplayer map
  with four phantoms (Lockout or Waterworks, the sim benchmarks); the
  main menu with a blurred overlay open; a two-view split-screen start.
  At render scale 1 and 4.
- **Census corpus** (before an entry is replaced, and before slice 6):
  every campaign level and every multiplayer map on the disc, 3,000
  presents each from boot, counters only.

### 9.2 Pixels

Two launches of the same image from boot, one with
`impl.render.native=off`, write PNGs at the same presents
(`H2_BOOT_GFX`, the method of the all-native push, which got identical
frames through present 4500). A new agent method compares them:

- **`render.compare {a, b, diff?}`** (new, slice 0): two PNG paths in,
  `{size, differing, max_channel, mean, first: [x, y]}` out, and a
  difference image at `diff` if asked. `h2 agent frame compare A B`
  runs it locally with no socket. It is added to the agent API because
  every slice here and every later render lane needs the same check;
  until now each lane compared images by hand.
- **`render.capture {tick?, pass?}`** (new, slice 0): the frame of sim
  tick `tick` (default: the next one) as a PNG, with its tick, present
  number and, from slice 1, the scene's summary hash. It waits until
  nothing is unresident. On the D3D path the frame of a tick is the
  present that follows it. This makes captures tick-addressed, which
  present numbers stop being once the render thread free-runs.
- **`render.scene {dump?}`** (new, slice 1): the last sealed scene's
  counts per view (clusters, instances, parts, rows, lights, shadow
  maps, carried ops per slot, by `CameraUse`), drops, unresident and
  held counts; `dump` writes it whole as JSON under the cache
  directory.
- **`render.frame`** (existing) reports D3D passes. From slice 6 it
  reports the native pass graph in the same shape (`passes`, `totals`,
  `snap` by pass number), so `h2 agent frame` keeps working.

**Bar.** Per compared frame: `max_channel <= 8` and `differing` (any
channel off by more than 2) at most 0.01% of the pixels. The all-native
push measured 0 on nine of ten frames and 6 on one. A frame over the
bar fails the slice until explained; an accepted difference is recorded
with its difference image in the slice's report, never by loosening the
bar.

**In-process reference** (slice 4 to slice 8): `render.parity {arm}`
(new) makes the next frame step run the guest's whole front end into
the D3D backend's own targets, and also extracts and draws the same
tick natively into private targets, then compares the two on the
render thread and reports as `render.compare` does. The guest front end
is a superset of the replaced entries' tick-visible writes, so the sim
cannot tell (the hash gate has already shown that). It is armed for one
frame from the agent API and costs one relaxed load a frame otherwise.
It is the one place two paths draw the same tick, it is a diagnostic,
and it is deleted with the D3D backend.

**Interpolated frames.** At `alpha = 1` the frame must equal the 30 fps
frame of that tick exactly (`max_channel == 0`). In-between frames are
checked by counters and by eye, as in high-framerate.md.

### 9.3 The sim

- `h2 lockstep A B` on `H2_LOCKSTEP` traces of the two launches is the
  whole-RAM check. It will differ where only the render writes (visit
  marks, class a), so it localises but does not gate.
- **`sim.hashes {from_tick?}`** (new, slice 0): the `state_hashes_v1`
  rows (gameplay digest, and the arena hash under
  `impl.sim.arena-hash`) of the running game. Sim mode writes this
  series to its report but refuses a GPU, so a rendered run has no way
  to produce it today. The gate compares the series of the two
  launches row by row and names the first differing tick.
- Slice 0 runs this on today's master first. If `native=on` and `=off`
  already differ, that is a bug in the tenant path and is fixed before
  slice 1.
- The super-bounce regression test (high-framerate.md, section 4) runs
  at 30, 60 and 120 once slice 8 lands.

## 10. Slices

One Opus lane each, in this order. Slices 0 to 3 are the first lane of
the review's three, 4 to 7 the second, 8 to 10 the third. Every slice
lands under `impl.render.native` with no new switch or variable. Test
commands are for the coordinator.

### Slice 0: baseline and the three agent methods

- **Code.** `render.compare`, `render.capture`, `sim.hashes`: types in
  `crates/extensions/agent-proto/src/render.rs` and `sim/`, logic in
  `crates/extensions/agent/src/render.rs` (PNG compare as a pure function), host
  glue in `crates/extensions/game/src/agent_glue.rs`, the `h2 agent frame compare`
  verb in the CLI, docs in [agent-api.md](../runtime/agent-api.md).
- **Tests.** Compare: synthetic images (equal, one pixel, size
  mismatch, the bar's edges). Capture: the frame recorder's existing
  rig, by tick. `sim.hashes`: against the sim report writer on a
  synthetic state. `cargo test -p agent -p agent-proto`.
- **Gate.** The coordinator records section 12's baseline after the
  native-cost integration, the hash series for `native=on` against
  `off`, and the census counters. All three go into this page under
  "Measured".

### Slice 1: `frame-scene`, extraction of camera, world and objects

- **Code.** The two new crates. `GuestRead` and the offset-bearing
  modules for the camera, the world ranges and objects move to
  `scene-extract` unchanged. `SceneBuilder` extracts a `View` at the
  world and object hooks that exist. `native-render`'s world and object
  stages take `&View` and `&FrameScene` (`GpuWorld::selection_draws`,
  `LitObjects::draws`, `instances::pack`) and lose their `GuestRead`
  parameters. Still game-thread, still packets, still masks: no
  behaviour change. `render.scene`.
- **Tests.** `scene-extract`: the existing synthetic-memory tests of
  the moved modules, plus round trips (a synthetic guest view to a
  `View` with the expected instances, parts and rows; a refused record
  becomes a remainder, not a part). `frame-scene`: builder sealing,
  range integrity, recycling. `native-render`: its existing GPU tests,
  fed scenes. `cargo test -p frame-scene -p scene-extract -p
  native-render -p render-bridge`.
- **Gate.** Pixels identical to the commit before (`max_channel == 0`)
  on the gate corpus; hashes identical; fps within noise;
  `rg GuestRead crates/rewrites/native-render/src/{gpu,objects,instances,world}*`
  finds nothing.

### Slice 2: typed stages prepare and record on the render thread

- **Code.** The world and object stages queue a scene stage and the
  render thread prepares and records it.
  - **Types.** `TargetCopy`, `ExternalDraws` and `ExternalEncoded` are
    `gfx::external`'s, re-exported at their `d3d::backend` paths, so
    `native-render`'s world and object modules (`gpu`,
    `object_program_gpu`) build their packets without `d3d`.
    `ResolvedDraw` and the NV2A pipeline key stay in `d3d` until slice 6
    carries draws: they are made of `d3d`'s state model (`DrawState`,
    `ShaderInputs`, `HwState`), which would move with them.
  - **The command.** `d3d::SceneStage` in `ThreadedBackend`'s queue.
    `Device::queues_scene_stages(stage)` says whether one is taken now:
    a render thread whose backend records them
    (`Backend::runs_scene_stages`: `GfxBackend`, `ScaledBackend` when
    both copies do, `InterpBackend` while it is not replaying), and the
    stage has not stood down. `Device::reserve_scene_stage` resolves the
    bound targets, the request and the view's constants and W clip, and
    makes here the checks that would refuse it on the render thread (the
    targets resolve, the viewport lies in the target, the slots have
    sizes). The glue then tells the guest to skip and commits
    (`SceneReservation::commit`); dropping the reservation cancels it.
    The render thread maps the attachments and calls
    `Backend::run_scene_stage`, which prepares and records every target
    copy and commits them in order.
  - **What stays at extraction, on the sim thread.** Every check that
    can refuse, because a refusal must leave the items to the guest.
    World (`boot::native_world_glue`): W-buffering, the stage palettes,
    the stencil plan, the inherited cull, and the parts the guest skips
    (frustum against the parts' bounds). Objects
    (`boot::native_objects_glue`): the render-state checks and each
    part's plan (`LitObjects::plan_part_of`, where an item refuses).
  - **Worker preparation.** Map workers upload world and object geometry,
    stage textures and material programs. Pipeline workers build the
    attachment-specific variants. A pending variant leaves the draw on D3D.
  - **Render thread.** `WorldPass::record_draws` and `ObjectsPass::packets`
    bind the prepared resources, write frame-specific uniforms and issue
    the selected parts.
  - **Named errors.** What the render thread cannot record
    (`d3d::scene_stage`) is counted, its cause logged once, and the
    stage stands down: from then on it reserves on the sim thread, where
    a refusal leaves the items to the guest. One frame at most is short
    of the stage's items.
  - **Where nothing is queued.** Without a render thread, while
    interpolation replays, and after a stand-down, the stages reserve on
    the sim thread through `Device::prepare_external_bound`.
  - **Inputs by value.** A stage carries the inputs it reads (the
    world's camera and parts, the objects' planned draws), not an `Arc`
    of its view: a view seals at the present, after its stages are
    queued.
  - **Extraction points.** The view opens at the base layer 3 and the
    objects are extracted at the walk's first geometry handler, as in
    slice 1. The walk's sorted entries live on the stack of the layer-3
    call (`object_walk::read`), so layer 1 cannot read them; opening the
    view at the first `LAYER_DRAW` waits on reading what the item list
    holds at layer 1.
  - **Counters.** `native_<stage>_extract_ns` is the sim thread's
    extraction of a scene stage, `prepare_ns` the game thread's
    preparation alone, `render_prepare_ns` preparation on the render
    thread (`timeline::native::mark_render_thread`),
    `native_<stage>_scene_stages` and `_scene_dropped` the stages queued
    and dropped; `render.scene` adds `scene_stages_<stage>` and
    `scene_dropped_<stage>` for the world and objects.
- **Tests.** `crates/platform/d3d/tests/scene_stage.rs` over the
  `ThreadedBackend` rig with the recording backend: a scene stage
  between two D3D draws on one target records between them, on the
  render thread, and the stage's preparation runs only there; a dropped
  reservation queues nothing; no stage is taken without a render thread,
  while the backend replays, after a stand-down, or with no ranges.
  `timeline`: preparation counted on the render thread is charged as
  `render_prepare_ns`. A remainder is drawn by D3D exactly once: the
  world's skip clears only the parts it draws
  (`an_untranslated_material_keeps_its_d3d_draws_and_a_translated_one_is_skipped`
  in `boot::native_world_glue`), and the object walk consumes only the
  run it queued.
- **Gate.** Pixels and hashes as slice 1. On the gate corpus,
  `native_world_prepare_ns` and `native_objects_prepare_ns` on the game
  thread are zero, `native_objects_worker_round_trips` and
  `native_world_worker_round_trips` are zero, `render-wait` does not
  rise. **Native at least equals D3D** in fps and p99 at 1x and 4x
  over three back-to-back pairs (section 12). If it does not, stop and
  attribute before slice 3.

### Slice 3: no remainder in the opaque block

Research first (`h2 disasm`), then code. Three parts, each its own
commits. The three parts are built.

- **Objects.** The geometry draw `0x0004d0b0` reads four bits of the
  render record's `+0` in layer 3 (**Code**; the module notes of
  `scene_extract::layout::object_walk` give the addresses).
  `object_walk::record_flags` turns them into `PartFlags` or a named
  refusal:
  - **Bit 14, stencil marking** (`PartFlags::STENCIL_MARK`). The
    prologue `0x0004d640` sets stencil mode 2 (`0x00015370`: `ALWAYS`,
    reference `0xff`, masks `0xff`, `REPLACE` on pass) and the
    epilogue `0x0004d720` mode 1 (the same with reference 0); with the
    stencil-off flag bytes set, mode 0 for every item and the bit does
    nothing. The objects stage draws a marked part with
    `object_state::mark` (mode 2, the packet's stencil reference
    `0xff`). A run with a marked part is taken only when the device
    holds mode 1 (`boot::native_objects_glue::plan_walk`), so skipping
    the epilogue leaves the state the guest would. What reads the marks
    is still open (native-renderer.md, "Not verified"): `0x00015370`'s
    other modes test the stencil `EQUAL` and `NOTEQUAL`, and they draw
    on the D3D path over the same depth-stencil, so the native writes
    are what they read.
  - **Bit 9, strip topology** (`PartFlags::STRIP`). The whole-part
    emitter `0x0004daa0` draws a part as a strip when the bit is set and
    the part's triangle-list flag is clear, as a list otherwise. The
    native models decode a part without the list flag as a strip, so
    that part without bit 9 refuses (`Refusal::Topology`).
  - **Bit 6, conditional stream ranges** (`PartFlags::SUBPARTS`). With a
    visible-subpart mask (record `+0x14`, bit `s` of word `s >> 5` for
    the section's subpart `s`) and subparts in the section, the emitter
    draws only the visible subparts of the part: each its own strip with
    strip topology, runs of index-contiguous ones as one list otherwise.
    The scene carries the mask read over the part's own subparts
    (`PartDraw::subparts`, `frame_scene::Subparts`, at most 64). The
    lit models keep, after each part's triangles, each subpart's
    triangles decoded alone in the part's topology
    (`object_program_gpu::lit_subparts`, `LitPart::subparts`), and a
    planned draw issues the chosen ones, adjacent ranges joined, one
    packet per run (`SubpartRanges::drawn`). A list part whose subparts
    are not whole triangles has no native subparts and its chosen draws
    refuse at planning. More than 64 subparts, a negative first subpart
    or a mask that does not read refuse (`Refusal::Subparts`).
  - **Bit 11, the auxiliary state toggle** (`PartFlags::AUX_DEPTH`). The
    prologue calls `0x00012d50` on and the epilogue off. It stores the
    device's program clip range (`+0x1c10`, game-owned through
    `+0x1c18` bit 1, pushed by `0x003f6190`) as `[near · k, far]` from
    the game's clip words (`0x00485adc`, `0x00485ae0`), `k` the
    constant at `0x004407b8` on and 1.0 off; and it builds the
    view-projection rows from the view at `0x0048568c` (its projection
    columns through its 3x4 matrix, times `[0x0045dd40] /
    [0x0048565c]`), all sixteen times `k` on, and uploads them to
    `c[0..3]` when the byte at `0x0055e6bc` differs from the call's.
    Extraction reads both states once per view
    (`scene_extract::layout::aux_depth`, the arithmetic in the
    toggle's order) into `View::aux_depth` (`frame_scene::AuxDepth`).
    An aux-depth part draws with those rows and that clip in place of
    the device's (`object_program_gpu::with_aux_depth`). A run holding
    one is taken only when the device holds the off state (the cached
    byte 0, `c[0..3]` the rows off, the stored and pushed clip the
    range off: `boot::native_objects_glue::aux_depth_at_rest`), so
    skipping the handlers leaves what the epilogue would. When the
    toggle's inputs do not read the part refuses
    (`Refusal::AuxState`).

  Refused parts count by cause as `native_objects_walk_part_aux_state`,
  `_part_subparts` and `_part_topology` (parts, where
  `walk_unsupported` counts objects). `render.scene` reports each view's
  parts by flag (`part_flags`).
- **Lights.** The light passes `0x00031170` draw, per light, layer 10
  (shadow casters, `0x000312ad`), layer 11 (shadow receivers,
  `0x00031348`), the additional casters' pair `0x00030ed0`, layer 14
  (`0x000313dd`), layer 12 (diffuse, `0x0003144a`) and layer 13
  (specular, `0x000314ca`) (**Code**; the module notes of
  `scene_extract::layout::light_passes` give each layer's condition).
  Before each of layers 11, 14, 12 and 13 they set stencil mode 0
  through `0x00015370`, the test off; before layer 10 they set none. So
  no layer reads a stencil an earlier one wrote: a later layer reads
  the shadow map (target 9) and the destination alpha layer 11 writes,
  both kept by the GPU's order. At the passes' entry
  (`hooks::native_lights::begin_light`) `light_passes::read` takes the
  flags the light's constants computed, its lists' masks and the
  additional casters' inputs, `light_passes::steps` gives the layers in
  order, each with its stencil change, target and whether it draws, and
  the light goes into the open view (`frame_scene::SceneLight`,
  `LightStep`; `View::lights`). The resolved-draw path takes each of
  layers 12 and 13 the steps draw, one at a time, with the candidate
  kept from the entry to the next light; the gathered walk batches any
  layer the steps draw, each its own reservation
  (`walk::light_layer_draws`); layers 10, 11 and 14 are the shadow
  stage's. A shadowed light's layers 12 and 13 draw like any other's:
  a pass that samples the shadow maps is refused by `light_look` by
  name. With the additional casters' pair every layer keeps its
  handlers (`lights_walk_fallback_graph`). Drawing the shadow maps as
  one batch at the start of the view's stage is not built.
- **First person and world layers 1, 5, 7, 8, 20.** At each of the
  scene's layer draws 1, 5, 7, 8 and 20 (`0x0002bcd0`; layer 20 returns
  to `0x0002c010`, after layer 8, on the scene's list into the view's
  target after stencil mode 0; `world_layers::layer_site`) and at each
  of the first-person draw `0x0003fdf0`'s layers, the frame step reads
  the current list's items of the layer into a layer pass of the view
  (`scene_extract::extract::layer`, `frame_scene::LayerPass`,
  `LayerItem`; `View::layers`): each geometry item's key, its owner (a
  cluster's section, a placement, or an object by handle), its record's
  model (the first-person mode for a first-person item), an object
  item's palette rows and its owner's lighting;
  the pass keeps the stencil-off flags and whether the texture
  accumulation target binds. Another build callback, or a record, owner
  or rows that do not read, is the pass's remainder. The first person
  is a view of its own (`ViewKind::FirstPerson`) with its own camera.
  The scene's view can open at its layer 1, before layer 3; its
  clusters and world state are read at layer 3 either way
  (`scene_step::open_kind`). The stages still draw these layers from
  their packets, with the constants the device holds; the material
  inputs of section 4.3 are read into the pass in slice 4
  (`frame_scene::LayerInputs`, below).
- **Tests.** One synthetic case per former refusal reason, asserting
  the part or light is in the scene: for objects,
  `each_former_special_record_flag_is_a_part_or_a_named_cause` and
  `a_records_flags_become_part_flags_or_name_their_refusal`
  (`scene_extract::layout::object_walk`),
  `a_marked_or_strip_part_is_in_the_scene_with_its_flags`,
  `a_part_drawing_chosen_subparts_is_in_the_scene_with_its_mask` and
  `an_auxiliary_depth_part_is_in_the_scene_and_its_view_has_what_the_toggle_sets`
  (`scene_extract::extract`),
  `a_marked_part_draws_in_mode_2_whatever_its_implementation_left`
  (`native_render::object_state`),
  `chosen_subparts_draw_their_ranges_joined_where_adjacent` and
  `an_auxiliary_depth_draw_takes_the_toggles_rows_and_clip_and_nothing_else`
  (`native_render::object_program_gpu`); for lights,
  `the_passes_order_is_10_11_14_12_13_and_each_flag_adds_its_layers`,
  `every_step_after_the_casters_turns_the_stencil_off_first` and
  `a_step_draws_only_with_its_layer_in_its_lists_mask`
  (`scene_extract::layout::light_passes`),
  `a_light_drawing_both_sorted_layers_is_the_candidate_of_each_in_turn`
  (`hooks::native_lights`),
  `each_layer_a_light_draws_batches_alone_unless_additional_casters_run`
  (`hooks::native_lights::walk`),
  `a_views_lights_keep_their_steps_in_order_and_count_their_shadows`
  (`frame_scene::builder`); for the layers,
  `a_layers_items_are_in_the_scene_with_their_owners_and_the_rest_a_remainder`
  (`scene_extract::extract`),
  `a_views_layer_draws_keep_their_items_and_rows_and_a_first_person_view_is_its_own`
  (`frame_scene::builder`) and `the_same_layer_number_has_different_view_sites`
  (`scene_extract::layout::world_layers`). Shadow batch: N lights record at most N
  target changes and no pass in between.
- **Gate.** `walk_unsupported`, every lights refusal counter, every
  `refused_*` counter of the five stages and `render.scene`'s
  `layer_remainder` are zero on the census corpus.
  Pixels and hashes on the gate corpus. Shadow passes per frame equal
  the number of shadow maps.

### Slice 4: whole-call replacement for the opaque block

Its entry census is built; its replacements are not, because rule 1 of
8.2 (no remainder) holds for none of its entries. What each still leaves
to the guest's draws (**Code**):

- **Layers 1, 5, 7, 8 and 20, the scene's and the first person's.** Their
  items are in the scene (slice 3), but the native stage draws them from
  the draws the guest resolves under the layer draw
  (`native_render::world_layers_gpu`, the indexed-draw admission in
  `boot::native_world_layers_glue`; the first person's per item at
  issue). With the entry replaced nothing is resolved. What a stage
  needs to draw a layer pass from the scene alone is extracted and
  planned, and the structure's static items draw from the plan
  (`frame-scene.layers`, below).
  - **Material inputs** (`frame_scene::LayerInputs`, read at the layer
    draw's begin by `scene_extract::layout::layer_inputs`, research
    points in its module notes). The render time `0x004858a0`, which
    game-time animated properties divide by their period
    (`0x0001b230`). The engine inputs that do not read the item,
    evaluated as the selectors `0x000336f0`, `0x00033980` and
    `0x00034060` do, from the render context `0x00485600` (copied per
    view by `0x000132d0`), the view vectors, the fog and the light
    block, each with a mask of the ones that read. The defaults the
    lighting inputs fall back on. Every object item of a layer draw
    carries its owner's lighting (`LayerItem::lighting`), the scene's as
    the first person's.
  - **The lightmap cycle.** `0x00022850`, at each view's begin
    (`0x00012a50`), steps the probe coefficients `0x004b8940` and the
    phase `0x005093f0`; their one reader, `0x00022cf0`, is the object
    lighting `0x00023690` under the object setup (`0x0004b5d0`). So the
    cycle reaches a layer's draws only through the object items'
    lighting, read after it; the world's items draw without one.
  - **The plan** (`native_render::layer_pass`). Per item: the passes
    the layer draws for its shader at the first level of detail
    (`0x00016610` draws each implementation with a vertex shader), the
    implementation (the only one, or the item's), the engine inputs
    (the view's, the lighting's from the item's owner, none for the
    structure's, and the shader's own) with the material resolved again
    with them when an implementation reads any, and the values of its
    pixel- and vertex-constant animations at the render time
    (`0x00017550`: time over period, wrapped unless periodic, the
    function `0x0013b390`, mapped onto its range). An item that cannot
    be planned has a named cause (`LayerRefusal::name`): an input the
    scene does not give, an animation driven by an object function or of
    a bitmap or state, a material or pass that does not resolve.
  - **The stage** (`frame-scene.layers`, an option with the native
    renderer on; `render_bridge::native_layers`, its host
    `boot::native_layers_glue`). At the world's load (only with the
    option placed) each world material's passes of each of the five
    layers are compiled as the fog's layer 16 is (`fog_gpu::layer_passes`,
    `FogProgram`), all or nothing per material and layer, and each
    material is resolved once with its instance's properties and input
    scales; `GpuWorld::set_layer` holds one `fog_gpu::FogSet` per layer.
    Before a scene view's layer draw (`layer_site` is `Site::Scene`; the
    first person's layers stay as they are) the bridge extracts the
    layer's pass from guest memory and the host plans it. A planned item
    is drawn when it is a cluster's or a placement's, its draw is static
    (no animation, no per-draw inputs: what the compiled programs hold)
    and the layer's set draws its part with its shader.
    `layer_pass::clear_prefix` clears the layer's bit on the run of such
    items from the list's first, since the native pass records before the
    guest's draw; one external pass (`GpuWorld::layer_draws`, in the D3D
    pass open on the bound attachments) draws them. Everything else keeps
    its D3D draws and so the indexed-draw admission: the objects (their
    recording over the model buffers, `object_program_gpu`, is not
    built), animated or per-draw items, and every item after the first
    one the stage cannot draw. Each ask's outcomes are counted in
    `render.scene` as `layer_plan_<layer>_taken` and
    `layer_plan_<layer>_refused_<cause>`: the plan's causes
    (`LayerRefusal::name`, `per_draw` for a material resolved per draw),
    `object`, `animated`, `stencil` (layers 1 and 3 begin in stencil mode
    1), `palettes`, `device`, `program` (no material of the layer
    compiled), `order` (planned and static, but from the first item the
    layer's set cannot draw on) and `unread` (the layer's pass did not
    read); `layer_plan_asked`, `layer_plan_drawn` and `layer_plan_items`
    count the layer calls asked, those drawn natively and their items.
  - **Tests** (synthetic). `every_view_input_reads_as_its_selector_computes_it`,
    `without_fog_or_a_view_the_selectors_fall_back` and
    `an_input_whose_words_do_not_read_is_not_given`
    (`scene_extract::layout::layer_inputs`); the pass's inputs in
    `a_layers_items_are_in_the_scene_with_their_owners_and_the_rest_a_remainder`
    (`scene_extract::extract`);
    `a_layer_draws_material_inputs_count_in_the_hash`
    (`frame_scene::summary`); for the plan
    (`native_render::layer_pass`),
    `a_static_material_plans_each_pass_with_its_own_inputs`,
    `the_items_implementation_picks_among_several_and_one_without_a_vertex_shader_draws_nothing`,
    `view_and_lighting_inputs_are_given_and_the_material_is_resolved_with_them`,
    `a_structure_item_draws_with_no_lighting_and_an_input_nothing_gives_is_named`,
    `a_game_time_property_wraps_its_time_over_its_period_and_maps_onto_its_range`,
    `constant_animations_take_their_values_and_other_kinds_are_named`,
    `a_scene_layer_plans_its_items_in_order_and_names_the_rest` and
    `the_skip_takes_the_accepted_structure_items_up_to_the_first_it_cannot`;
    for the stage, `the_plans_outcomes_count_by_layer_and_cause`
    (`render_bridge::native_layers`),
    `a_per_draw_refusal_counts_apart_from_a_missing_material` and
    `the_plan_reads_only_what_the_load_resolved`
    (`boot::native_layers_glue`).
- **Layer 3.** The decorators (build `0x0004de20`) are in the scene
  (`View::decorators`, each placement's draw with its instances'
  `v11..v15`), and the decorator stage draws a view's ready ones
  instanced and skips their items; a draw whose material is not ready,
  and a view the stage declines, stays the guest's
  ([native-decorators.md](native-decorators.md)).
  The world keeps the draws of materials it does not translate and of
  parts that lack an input of their program ([native-world.md](native-world.md)),
  and the object walk the parts it refuses (`walk_unsupported`).
- **The light passes and the shadow layers.** Models and rigid
  placements keep their D3D handlers ([native-lights.md](native-lights.md),
  "What a native light draws"), every layer keeps its handlers with the
  additional casters' pair, and the gathered walk's GPU path takes a
  group's live streams, pose, constants and bindings from the first
  geometry handler, which the guest runs. The scene carries what that
  handler supplies (built): at each layer build, the sorted entries the
  walk builds (`render_bridge::native_lights::walk`) go into a group of
  the light (`scene_extract::light_groups::push`,
  `frame_scene::LightGroup`, `LightEntry`; `FrameScene::groups_of`),
  each entry with its stream (section, part, vertex class, index range,
  topology), its pose (world, a placement by index, or an object's node
  rows) and its bindings (shader, pass selector, variant,
  implementation), and layers 10 and 11 with the shadow projection
  block. The additional casters' layers 10 and 11 are groups of their
  own site (`GroupSite::AdditionalCasters`), and each object shadow's
  layers 10 and 11 groups of its `ObjectShadow` in the view
  (`GroupSite::ObjectShadow`). The summary hash
  (`frame_scene::summary_hash`) covers the groups, their entries and the
  object shadows. What the handler does
  beyond binding and drawing (the auxiliary state toggle, state around
  one part, visible subparts, a shader flag, a material animation, a
  template callback) leaves the entry to the guest by named cause.
  `native_render::light_scene::plan` turns a whole group into native
  draws from the scene alone, sorted order and duplicates kept,
  adjacent list ranges joined. A light's layer 12 or 13 whose group is
  whole is drawn from the scene with no handler run, its object entries
  from model parts the light passes build per map with their node rows
  ([native-lights.md](native-lights.md), "Gathered groups in the
  scene"). Not built: a recorder for the shadow layers 10, 11 and 14
  from the scene (the shadow stage records from resolved draws), so
  those layers still run their handlers.

The entry census (`render_bridge::entry_census`, built) measures both
halves of 8.2's first two rules per entry, so the coordinator can tell
from the census corpus which entry is ready. With `frame-scene.census` in
`H2_FEATURES` (and the native renderer on), observers before and after
every `LAYER_DRAW`, `RENDER_LIGHT_PASSES`, `OBJECT_SHADOW` and
`RENDER_SCREEN_EFFECTS` count, per entry: the guest draws handed to the
D3D backend under it (`d3d::device::backend_draws`: the remainder), the
steps of both shared random seeds, and the calls of
`ANIMATION_RESOURCE_GET` (`0x001237e0`) and `PARTICLE_VIEW_UPDATE`
(`0x00174420`) under it (5.2's seed, effects word, animation stamp and
particle stamp rows). A layer draw is named by its site and layer
(`layer.scene.3`, `layer.first_person.1`), or, where the layer table
does not place it, by the entry around it (`layer.light.12`,
`layer.object_shadow.10`). Entries nest, and each counts what happens
under it. `render.scene` reports the totals under `entries`. It is an
option rather than always on because its exit observers give those
entries an after part. Tests: the unit tests of
`render_bridge::entry_census` (draws and seed steps between entry and
exit, unmatched seeds, nesting, missed exits).

An entry whose census is all zero over the corpus is replaced by
"return"; a non-zero write becomes a row of the slice's table, kept by
calling the guest function that makes it or reproduced. Texture,
geometry and animation residency requests (5.2's residency row) are not
counted by the census.

- **Code.** `render_bridge::frame_step`: implementations of `LAYER_DRAW` for
  layers 1, 3, 5, 7, 8 and 20, of the light pass and of the shadow
  layers, each doing the writes of 5.2 and returning. The table of
  writes with their tests. `skip.rs`, the mask plans, `clear_objects`,
  `keep_only`, `restore`, and the world, object, light and shadow
  admissions and their glue are deleted. `render.parity`.
- **Tests.** For each replaced entry, a differential against the
  lifted original on synthetic state (the shared harness of change 6
  when it exists, the per-crate one until then): same bytes in the
  5.2 words, same seed, same particle stamps.
- **Gate.** Hashes identical over the gate corpus and one ten-minute
  multiplayer run. `render.parity` within the bar on 20 armed frames.
  The game thread's `render` phase drops by the layer and light sites'
  time (render-loop.md measured layer 3 at 8.26 ms and the light
  bookkeeping at 1.88 ms on Old Mombasa; **Estimate** for today).

### Slice 5: fog, water and post, typed

The late passes are in the scene, and the fog and the water draw from it
on the render thread. No entry is replaced: as in slice 4, rule 1 of 8.2
waits on the census corpus, and the post stage still draws through the
device's swallow. What is built (**Code**):

- **The types** (`frame_scene::late`). `View::fog` and `View::water` are
  a `LateLayer` (`FogState`, `WaterState`): the layer's own gate (the
  fog's `0x00044582` test, the water's bit 4 of `0x004b9fe0` clear), the
  fog words the device holds at the layer's draw by slot, the layer's
  items in the current list, each material the stage draws with and its
  words resolved over the device's (`LateMaterial`), the structure parts
  the stage draws in item order (`LatePart`), and whether the stage took
  the layer. `View::post` is `PostParams`: what `render_screen_effects`
  was asked to draw (its two stack bytes, the view kind, the scenario's
  screen-effect count and the loop's gates, read as
  `scene_extract::layout::post::PostFrame` reads them), the colour pass
  `0x000214f0`'s seven arguments, and each screen effect `0x0002d140`
  under it (`PostEffect`: its datum, its four values by value, its
  grid). The late passes run after the view's opaque block, after the
  first person's view can open, so the builder writes them into the
  scene's last view of the guest view's key
  (`SceneBuilder::set_late`, `set_post`); they extend no view range. The
  summary hash covers them.
- **Extraction.** The fog's and the water's glue
  (`boot::native_fog_glue`, `native_water_glue`) read the device's fog
  words, resolve each material's words over them and plan the skip;
  then `render_bridge::scene_step::late_layer` writes the view's layer
  (with `scene_extract::late::layer_items`) and hands back the scene's
  copy, and the pass is built from that copy: the parts it draws and
  each material's fog unit. A layer whose gate does not hold, with no
  structure item, or that the glue leaves to the guest is written as
  the guest draws it (`late_kept`). The frame step observes
  `RENDER_SCREEN_EFFECTS` for `PostParams`; `native_post`'s colour-pass
  and screen-effect hooks add their calls (`post_colour`,
  `post_effect`).
- **Recording on the render thread.** With a render thread that
  records scene stages, the fog and the water each queue one
  (`native_fog_glue::commit_late`: `Device::reserve_scene_stage`, as the
  world's in slice 2); the render thread prepares and records the pass,
  the water's scene copy and sampled targets included. Not taken, the
  pass is reserved on the sim thread as before. Either way the load is
  pinned until the masks and the commands commit, and only then does
  `late_drawn` mark the layer taken. Any refusal before the commit
  leaves the layer's items to the guest (the D3D path); a stage the
  render thread cannot record is slice 2's named error and stands the
  stage down.
- **Coverage.** `render.scene` reports each view's `fog` and `water`
  (gate, items, materials, parts, whether the stage took the layer, the
  items the guest keeps, the device words that read) and `post`, and the
  counters `scene_fog_*`, `scene_water_*`, `scene_post_*` and
  `scene_stages_{fog,water}`, `scene_dropped_{fog,water}`. The entry
  census names the late passes' calls `layer.scene.16` and
  `layer.scene.6` (`entry_census::late_site`) and counts `colour_pass`
  and each `screen_effect` under `screen_effects`.
- **Tests** (synthetic). `frame_scene::late`:
  `a_views_late_layers_keep_their_words_and_parts_and_count_what_the_guest_keeps`,
  `a_post_call_keeps_its_colour_pass_and_effects_in_order`,
  `the_late_passes_count_in_the_hash`; `scene_extract::late`:
  `a_late_layers_items_are_those_with_its_bit_in_the_current_list`,
  `a_post_call_reads_its_gates_and_a_screen_effect_its_values`;
  `render_bridge::entry_census`:
  `the_late_passes_fog_and_water_calls_are_the_scenes`,
  `the_post_callees_nest_under_the_screen_effects`; `boot::scene_glue`'s
  `a_scene_summarises_per_view`; `agent_proto`'s
  `a_views_late_passes_print_only_when_they_ran`.
- **Not built.** The replacement of layers 16 and 6 and of
  `render_screen_effects` (8.2: their census over the corpus, the table
  of writes, the differentials); the post stage drawing from
  `PostParams` on the render thread, owning its bloom and effect
  targets, in place of the swallow, which matches the guest's draws as
  they run and replays them on a mismatch, so it needs the guest's
  calls; the deletion of the three stages' glue. Screen effects the
  scope-zoom HUD draws are not put in the scene (`native_post` leaves
  them to the HUD).
- **Gate.** As slice 4. No guest surface other than the back buffer,
  depth and a carried stage's own targets is bound in a game view
  (from `render.frame`).

### Slice 6: the renderer owns game-view frames

The large one. A census comes first: the coordinator runs the census
corpus and reports every guest readback and every side target still
bound in a game view. The slice starts when section 8.3's table covers
the list.

**Built: the carried census** (`frame-scene.sink`, an option with the
native renderer on; **Code**). The D3D backend still draws every frame;
the sink records what the renderer will own.

- **The types** (`frame_scene::carried`). `Slot` and `SlotAt` (the slot
  and the guest view index); `SlotTracker` moves the slot on the frame
  step's events and, inside a game view, only forward. `GuestOp<D>`
  (`Draw`, `Bind`, `Clear`, `Copy`, `Upload`, `Sample`) over
  `TargetRef` (`SceneColour`, `SceneDepth`, `Aux(n)`), generic in the
  draw so this crate stays free of `d3d`. `Carried<D>` holds a frame's
  ops with their `SlotAt`, its side targets (`AuxTarget`: size, format,
  depth) and its readbacks (`Readback`: `Surface`, `Depth`, `Mirror`,
  `PixelCount`); `Carried::summary` counts them by slot and view and
  lists the side targets a game view names (`CarriedSummary`).
- **The slot** (`render_bridge::carried_slots`, placed by the frame
  step's build with the option). `render_frame_and_present`'s entry and
  exit are `Outside`; `render_game_view` `0x0002b790` opens `Opaque`
  with its record's view index (`+4`); the light pass `0x00031c80` is
  `AfterOpaque` and its exit `AfterLights`; the sorted transparents
  `0x0001e370` open `Transparents`, which holds the first person, layer
  17, the flares and `render_screen_effects`; that call's exit is
  `AfterPost`; a game view's exit, the HUD's draw `0x001917eb` and
  `render_views`' exit are `Overlay` once the frame has had a game view.
  Before the slot moves the device decodes what game code wrote into
  the push buffer (`carried_slots::Drain`, the host's
  `Device::sync_push_buffer`), so a draw lands in the slot it was
  written in.
- **The sink** (`d3d::SceneSink`, in boot's `install_for_profile` over
  the whole backend stack, on the sim thread). It forwards every
  `Backend` call and, while the slot reads, files each draw (after a
  `Bind` when the attachments, slot or view change), clear, whole-surface
  copy, texel upload into a target and sampled target (a framebuffer
  snapshot, a render-to-texture level in place, depth bits) at the
  current slot, and each readback (`read_surface`, `read_depth`,
  `mirror_read`, `report_pixel_count`). The scene's colour is the
  surfaces the guest presents (and, with a copy swap, the one copied
  into a presented surface); its depth is the depth bound with that
  colour; anything else is a side target, numbered per frame. A present
  ends the frame: its summary is published (`d3d::scene_sink::last`)
  and its totals counted (`d3d::scene_sink::counters`). Typed stages'
  packets and passes are not carried, nor is a queued native draw
  (`Backend::queue_external_draw`) whether the render thread records it
  or draws its D3D packet.
- **Coverage.** `render.scene` reports, with the option, `carried`
  (the last frame's ops by slot, its side targets with the ops that
  named them by slot, and its readbacks in and outside a game view),
  each game view's `carried` (its ops by slot) and the counters
  `scene_carried_frames`, `scene_carried_draws`, `scene_carried_ops`,
  `scene_side_target_frames`, `scene_readback` and
  `scene_readback_<kind>`. Over the census corpus these are the list
  section 8.3's table must cover.
- **Tests** (synthetic). `frame_scene::carried`:
  `a_game_view_runs_its_slots_in_order_and_the_overlay_follows`,
  `inside_a_view_the_slot_only_moves_forward_and_outside_it_does_not_move`,
  `a_summary_counts_by_slot_and_view_and_names_the_side_targets_of_game_views`;
  `d3d::scene_sink`: `forwards_every_backend_method`,
  `the_sink_names_targets_slots_and_readbacks_and_passes_every_call_on`;
  `d3d`'s `tests/scene_sink.rs` on the device's draw rig:
  `the_sink_carries_the_devices_draws_in_order_by_slot_and_the_backend_still_draws_them`,
  `a_sink_with_no_slot_records_nothing_and_changes_nothing`;
  `render_bridge::scene_step`:
  `the_carried_slots_are_placed_only_with_their_option`;
  `boot::scene_glue`:
  `the_sinks_frame_is_reported_by_slot_and_per_game_view`; `agent_proto`'s
  `a_scenes_carried_part_prints_only_when_the_sink_recorded`.
- **Not built.** Everything below that draws: the sink in place of the
  backend stack in a game view, the carried ops held in the scene
  (`FrameScene::carried`, which waits on `ResolvedDraw` moving to
  `gfx`), `CameraUse` and its classification, `native-render`'s pass
  graph and `GuestDrawPass`, the native flare query, the mailbox, Swap
  without the GPU wait, and `render.frame` over the pass graph. The
  census corpus run that this half exists for is the coordinator's.

- **Code.** `d3d::SceneSink`, one `Backend` implementation that fills
  `Carried` (draw, clear, bind, copy, upload) and classifies
  `CameraUse`. In a game view the device runs over it in place of
  `ThreadedBackend<InterpBackend<ScaledBackend<GfxBackend>>>`. The
  frame step sets the current slot. `native-render` gets the pass
  graph: its own colour and depth at the display scale, the stages in
  order, the carried slots between them, the present to the window's
  sink. Carried draws are recorded by a `GuestDrawPass` that `d3d`
  registers with the renderer (the draw-recording core of
  `gfx_backend.rs` and the view mapping of `scaled.rs`, kept; their
  surface management, not). The native flare query. The mailbox, both
  modes. Swap no longer waits on the GPU. `render.frame` reports the
  pass graph.
- **Tests.** `SceneSink` against the recording backend on the existing
  D3D draw rigs: the same draws in the same order, by slot. Pass
  graph: the stages and slots in the declared order with one pass
  where attachments do not change. Mailbox: paced drops and counts,
  locked blocks at two. A scene with a carried copy from `SceneColour`
  samples what was drawn before it.
- **Gate.** Pixels on the gate corpus against `native=off` (which
  still selects the old stack) and by `render.parity`. Hashes.
  `scene_readback`, `scene_dropped_*` and `scene_unresident` zero over
  the census corpus after loading. One recording per draw at 4x
  (`command_buffers`, `passes`). The sim thread's frame time no longer
  includes `gpu-wait` or `render-wait`. Native at least 15% ahead of
  D3D at 4x on the gate corpus (**Estimate**; the number is set from
  slice 2's measurement).

### Slice 7: frames outside a game view

- **Code.** Menus, loading screens, the fallback view `0x002232a0`,
  the alternate view `0x0002c790` and movies go through `SceneSink`
  into `Outside` and `Overlay`. Bink frames and glyph pages are
  `TextureUpload`s. The old backend stack is no longer constructed
  when `impl.render.native` is on.
- **Tests.** A carried menu frame with a blur chain through `Aux`
  targets; a texture upload consumed by a later draw in the same
  scene.
- **Gate.** Pixels on the menu, loading and movie captures. From boot
  to gameplay and back to the menu, the D3D backend's draw counter
  stays zero.

### Slice 8: 60 and 120 fps

- **Code.** `frame-scene::blend` (pure): matching, the blends and
  snaps of section 7. The render thread's display-rate loop, alpha
  from publish times, the pacer moved from `d3d::interp::pacing`. Cut
  detection in extraction. `graphics.fps` drives it; no switch.
  `d3d/src/interp`, `impl.render.interpolate`, `impl.render.reproject`
  and `impl.render.reproject_ahead` are deleted.
- **Tests.** Blend: identity at 0 and 1 (bitwise at 1), slerp against
  known rotations, every snap rule, unmatched and re-ordered
  instances, the union of cluster sets. Pacer: the existing interp
  pacing tests, moved. A two-scene render at `alpha = 1` equals the
  one-scene render.
- **Gate.** Hash series identical at `--fps 30`, `60` and `120`. The
  super-bounce test passes at all three. `alpha = 1` captures are
  identical to 30 fps captures. Render-thread time per display frame
  under 16.7 ms at 60 and under 8.3 ms at 120 on the gate corpus at
  the user's play scale, p99, or the rate is not offered at that
  scale. `scene_held_draws` reported per level.

### Slice 9: type the carried stages that cost the most

One stage per sub-slice, chosen by the guest time `render-timing`
attributes to its producer: transparents (2.58 ms at the sorted
dispatch when last measured), then particles and contrails
([native-particles.md](native-particles.md)), decals
([native-decals-research.md](native-decals-research.md)), gels. Each
follows 8.2. Typed particles also turn to face the blended camera and
move with their parent at display rate.

### Slice 10: deletion

Section 11, as its own lane, after a soak the user decides.

## 11. Order of deletion

Each row is deleted in the slice named, in that slice's last commit,
after its gate. Line counts are the size today of the files named
(**Code**, `wc -l`, before the native-cost integration), so they are the
ceiling: loaders, material translation and anything a typed stage still
uses move to `native-render` or `scene-extract` and stay.

| When | What goes | About |
|---|---|---|
| Slice 2 | `native-render`'s `d3d` dependency for world and objects: their modules use `gfx::external`; the crate keeps `d3d` for the other stages | |
| Slice 4 | `scene-extract/src/layout/skip.rs`; the mask and admission halves of `boot/src/native_{world,objects,lights,shadows,first_person,world_layers}_glue.rs` and `feature/src/native_{world,objects,lights,shadows,first_person,world_layers}*`; `d3d/src/device/{opaque_world,opaque_vertices}.rs` | 18,000 |
| Slice 5 | `boot/src/native_{fog,water,post}_glue.rs`, `feature/src/native_{fog,water,post}.rs` | 5,000 |
| Slice 6 | `DrawAdmission`, `ExternalPass`, `ExternalPlan`, reservations and `device/admission_scope.rs`; `gfx_backend/native_runs.rs`; the scene-stage command of slice 2; the remaining `boot/src/native_*_glue.rs` and `feature/src/native_*` except the scene lists | 7,800 |
| Slice 7 | nothing yet: the old stack is still what `native=off` selects | |
| Slice 8 | `d3d/src/interp/` and its three switches | 5,600 |
| Slice 10, step 1 | `impl.render.native` itself and everything only `=off` reaches: `d3d/src/threaded.rs`, `scaled.rs` and `scaled/` except the view mapping, the `Backend` implementations other than `SceneSink`, `gfx_backend.rs`'s surface, write-back and present code, `device/{mirror,display,snap}.rs`'s render-target half, lazy depth, alias mirrors, the visibility poll, and the "old-behaviour opt-out" variables of render-switches.md that configure them; `render.parity` | 8,000 |
| Slice 10, step 2 | `impl.render.pipelined_frame`, `feature/src/pipeline_{render,scene,snapshot}.rs`, `H2_PIPELINE_*`; what is left of `engine/src/render_loop` that no typed stage uses | 6,000 |
| Slice 10, step 3 | the `Backend` trait, reduced to what `SceneSink` implements, then folded into `Device` | |
| Later, not scheduled | `viewer`'s and `scenario-edit`'s own world renderers, onto `native-render` through `frame-scene` | |

What stays of `d3d`: the 88 entries and their ABI, resources and locks,
the push-buffer decoder and the state model (they resolve the carried
draws), vertex-shader and combiner translation, the vertex cache,
vblank and the logical frame wait, fences and `KickOff` answered at
once. It stays as long as anything is carried, and the UI can stay
carried indefinitely: it is cheap and it is still drawn once, by the
native renderer.

After step 1 there is no D3D reference in the tree. The reference for
later render work is the last commit before it, tagged
`d3d-reference`; `h2 lab run --rev d3d-reference` builds it on the
laptop. Its captures are game images and stay in local scratch, never
in the repository.

## 12. Measurements

Performance numbers come from the quiet laptop through `h2 lab`
([lab.md](../runtime/lab.md)). Local runs are for parity and smoke
checks. Run each pair back to back, three times, and report medians.

**A/B on the laptop** (every slice; `B` is the slice's branch):

```sh
for arm in off on; do
  h2 lab run --host laptop --branch "$B" --level 03a_oldmombasa --difficulty heroic \
    --frames 5000 --stats-from 3000 --build-env H2_OPT=2 --fresh-data \
    --env H2_RENDER_SCALE=4 --env H2_FEATURES=impl.render.native=$arm \
    --wait --to "$RUSH_KEEP/lab/$B-$arm"
done
h2 lab runs --host laptop
```

Repeat with `--env H2_RENDER_SCALE=1`, and with `--replay fight-1` once
the user has recorded a fight (`h2 lab record --level 03a_oldmombasa
--difficulty heroic --name fight-1`). Read `frames.csv`: fps, p50 and
p99 frame time, and the phases `cpu`, `render`, `decode`, `encode`,
`submit`, `gpu-wait`, `render-wait`, `d3d`.

**Counters on a serve** (the coordinator's `ab.sh`; `H` is the built
`h2`, `IMG` its image directory, `F` empty or `impl.render.native=off`):

```sh
env H2_OPT=2 H2_BOOT_GFX=$TMPDIR/ab-gfx H2_BOOT_GFX_EVERY=never \
    H2_LEVEL=03a_oldmombasa H2_RENDER_SCALE=4 ${F:+H2_FEATURES=$F} \
    $H serve disc/halo2.iso --no-movies --socket $S --out $IMG &
$H agent wait '{"phase":"gameplay"}' 600000 --socket $S --timeout 620
$H agent capture 600 --socket $S --timeout 300 > capture.json
$H agent render.stats --socket $S > render-stats.json
$H agent profile.counters --socket $S > counters.json
$H agent render.scene --socket $S > scene.json          # from slice 1
```

For the entry census (slice 4) add `frame-scene.census` to
`H2_FEATURES` and read `.entries` of `render.scene`: per entry, its
`d3d_draws` (8.2 rule 1) and its seed steps, animation gets and
particle updates (rule 2), with `unbalanced` zero.

Read per frame: `passes`, `command_buffers`, `draws`,
`native_<stage>_{examined,suppressed,refused_*,prepare_ns,encode_ns,worker_round_trips,command_buffers}`,
from slice 2 `native_<stage>_{extract_ns,render_prepare_ns,scene_stages,scene_dropped}`,
`walk_unsupported` and from slice 3 `walk_part_{aux_state,subparts,topology}`, and from slice 1 `scene_dropped_*`,
`scene_unresident`, `scene_readback`, `scene_held_draws`; from slice 5
`render.scene`'s per-view `fog`, `water` and `post` and the counters
`scene_{fog,water}_{views,gate_held,drawn,parts}`; from slice 6, with
`frame-scene.sink` in `H2_FEATURES`, `render.scene`'s `carried` and
`scene_carried_{frames,draws,ops}`, `scene_side_target_frames`,
`scene_readback_{surface,depth,mirror,pixel_count}`.

**Pixels and the sim** (two launches from boot, same image):

```sh
for arm in off on; do
  env H2_OPT=2 H2_LEVEL=03a_oldmombasa H2_FEATURES=impl.render.native=$arm \
      H2_BOOT_GFX=$R/$arm H2_BOOT_GFX_EVERY=500 H2_BOOT_STOP_AT_PRESENT=5000 \
      H2_FRAME_STATS=1 H2_FRAME_STATS_FROM=3000 H2_FRAME_STATS_CSV=$R/$arm.csv \
      H2_LOCKSTEP=$R/$arm.lock \
      $H run disc/halo2.iso --out $IMG --data $R/data-$arm --fresh-data --slices 10000000
done
for f in $R/off/frame-*.png; do
  n=$(basename "$f")
  $H agent frame compare "$f" "$R/on/$n" --diff "$R/diff-$n"
done
$H lockstep $R/off.lock $R/on.lock
```

The hash gate, on two serves started as in the counters block (one per
arm), once both are in gameplay:

```sh
$H agent sim.hashes --socket $S_OFF > off-hashes.json
$H agent sim.hashes --socket $S_ON  > on-hashes.json
diff <(jq -c '.rows[]' off-hashes.json) <(jq -c '.rows[]' on-hashes.json) | head
```

**60 and 120** (slice 8): the laptop A/B with `--env H2_FPS=60` and
`120`, plus on a serve `H2_FPS=120` and `h2 agent render.stats`, which
reports the render thread's time per display frame, frames per scene,
holds and their reasons, and `scene_held_draws`.

**Frame step cost** (slices 4 to 9): add `--env H2_FEATURES=render-timing,impl.render.native=on`
to the laptop run and read the `render` phase and its sites. The target
for the sim thread's frame step on Old Mombasa is under 6 ms after
slice 6 and under 3 ms after slice 9 (**Estimate**; `render` was about
25 ms of the game thread before any native stage).

### Measured

Empty until slice 0. Each slice adds a row: commit, scale, fps and p99
for both arms, the counters above, the hash result.

## 13. Risks and open questions

- **The shared seed.** Section 5.2's first row decides how much guest
  code a typed stage can drop. If the draws from the seed under the
  object draw path cannot be reproduced in order without running most
  of that path, objects stay "typed for drawing, guest for side
  effects" and save less guest time than planned. The alternative is
  to define a new stream, which breaks existing films and needs every
  co-op peer on the same build. That is the user's call; this spec
  assumes exact.
- **Is `native=on` hash-identical to `off` today?** Not known. Slice 0
  answers it.
- **Do two launches stay comparable?** They did to present 4500 in the
  all-native push; agent-api.md notes headless runs drifting apart
  after a few minutes. The gate corpus stays inside 5,000 presents,
  and `render.parity` needs no second launch.
- **Does a snapshot load across the switch?** If `state.load` of a
  snapshot saved with `native=off` works with `on`, parity can start
  mid-level. Not checked; the spec does not rely on it.
- **Readbacks nobody listed.** Slice 6's census exists to find them.
  Film thumbnails and screenshots are owned by the theater session and
  need a `render.capture`-style answer from the native frame.
- **Slice 6 is large.** If it has to be split, the cut is by view kind:
  single-player game views first, split-screen and cinematic views
  after, with the old stack serving the rest in between.
- **Carried draws at 120.** A world-space draw whose view block cannot
  be rebuilt is held at the tick's camera and will judder against the
  blended world. `scene_held_draws` per level says how much slice 9
  has to type before 120 looks right.
- **Change 5 (the layout crate) is not built.** `scene-extract` starts
  with a private `layout` module. If change 5 lands first, slice 1
  uses it directly.
- **E1 was the user's choice for 60 fps.** Slice 8 replaces it with the
  two-scene blend. E1 could be rebuilt on the native frame as a cheap
  mode if the blend does not fit a display frame at some scale; it is
  not planned.
- **Sim mode and dedicated hosts** skip the frame step with stubs
  today. Running the step without extraction on a host is a follow-up
  for the session that owns hosts.
