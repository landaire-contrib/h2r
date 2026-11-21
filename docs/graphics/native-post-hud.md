# Native renderer stage 5: post and HUD

Status: Partly built. The colour-pass glow, the scenario screen effects and the native UI and HUD are drawn natively; `0x0002df00`, `0x0002dd30` and layer 17 are not, and parity with D3D is not verified.
Code: `crates/rewrites/render-bridge/src/native_post.rs`, `native_hud.rs`, `crates/extensions/game/src/native_post_glue.rs`, `native_post_load.rs`, `native_hud_glue.rs`, `crates/rewrites/scene-extract/src/layout/post.rs`, `glow.rs`, `screen_look.rs`, `screen_gpu.rs`, `hud.rs`, `ui_gpu.rs`, `crates/rewrites/engine/src/colour_pass.rs`, `glow.rs`, `screen_effects.rs`, `screen_grid.rs`, `function_eval.rs`. Tests: `crates/rewrites/render-bridge/tests/native_post.rs`, `native_hud.rs`, `crates/rewrites/engine/tests/colour_pass_differential.rs`, `glow_differential.rs`, `screen_effects_differential.rs`, `screen_grid_differential.rs`, `crates/platform/d3d/tests/swallow.rs`.
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md).

Part of [native-renderer.md](native-renderer.md).

## Stage 5: post and HUD

What the game draws after a view's scene comes in three groups: the
last passes of `render_scene_passes`, the rest of `render_view_scene`
(a fade, then the HUD and UI), and the end of the frame after every
view. None of them is a layer of the scene list except layer 17. This
section follows 03a's path in the order the game draws it. The census
run's figure for `render_screen_effects` is 0.76 ms and 164 D3D calls a
frame.

### The path (Code, `h2 disasm`)

- **The frame.** `render_frame_and_present` (`0x00223240`) renders the
  views (`0x0002b5d0` → `render_game_view` `0x0002b790` →
  `render_view_scene` `0x0002ba10`), then ends drawing (`0x00013b90`,
  `0x00013cd0`) and presents (`0x00013f10`).
- **One view.** `render_view_scene` runs the visibility and records
  (`0x0002c3b0`, `0x0003fd70`, `0x0002b990`), then `0x00013680`, which
  calls `render_scene_passes` (`0x0002bcd0`, at `0x000136ac`). After it,
  in order: `0x00015370`, `0x00015680` (the view kind `[0x00485898]`),
  `0x001917eb` when `[0x004b9ed8]` is not -1, then `0x00040890`, then
  `0x00147af1` (EAX -1, two words pushed), then the view's end
  `0x00013c20`.
- **The scene passes' tail** (from `0x0002c19a`): the sorted
  transparents `0x0001e370`, `0x00025dc0` (view kind not 2), the player
  chain with `[0x00485a76]` set and `0x0002afa0`, the first person
  (`0x0003fdf0(0)`, its lights `0x000404b0`, `0x0003fdf0(1)`),
  `0x00040870` when BL is set (the transparents again, then
  `[0x004b6284]` copied to `[0x004b6280]`), `[0x00485a76]` cleared, then
  the late passes:
  - `0x0002dd30` when the caller's byte `[esp+0x28]` is set: its own
    target (`[0x00485602]`), render states (`0x003f5cf0`), a stipple
    (`0x003f8cc0`) and quads (`0x0001cf50`, `0x0001c590`, `0x0001c710`,
    `0x00020bb0`);
  - layer 17 when the byte `[esp+0x2c]` is set (S: bloom; 250 passes on
    the disc, from the scene list, gated by `0x004ba024`);
  - `0x00020770`: the occlusion query results (`[0x0051f408]`,
    `[0x0051f409]`, the frame count mod 3, `0x003f8630`);
  - `render_screen_effects` (`0x00013870`,
    `points::RENDER_SCREEN_EFFECTS`), stdcall `(late, colour)`, `ret 8`.
    `colour` is the byte that gates layer 17, and `late` the one that
    gates `0x0002dd30`;
  - the exit `0x00013630`: with `[0x004850c8]` set (a view drawn into a
    target of its own), the target back to 0 (`0x00014bc0`) and the
    depth clip (`0x00015720`).
- **`render_screen_effects`.** Its prologue sets `[0x004670bc]` to 1,
  resets the shader draw state (`0x00016b10` on `0x00485b48`), stores the
  word `[0x00485600]` at `[[0x00485890]+0x1c]`, and runs `0x000245c0`
  with EAX `0x0051ecf0`. Then:
  1. With `colour`, the colour pass `0x000214f0`. Its inputs are
     computed by `0x000137a0` and blended with `[0x004b9fd0]..`, with
     constants `0x407f7cee` and 0.25, and its last argument is "view
     kind is 6". It binds the view's target `[0x004858b8]` as a texture
     (`0x00014f60`), writes a combiner into the PSDEF at `0x00484f68`, and
     draws through `0x0001ccf0`.
  2. The scenario's screen effects. This needs a scenario
     (`[0x004e0350]`), a view kind outside 5..7, both `[0x00485607]` and
     `[0x004850c8]` clear, and `0x00022710` true. It reads a rectangle
     from the words at `0x004b9dd0..`, then walks the scenario block at
     `+0x3d0` (count) and `+0x3d4` (address), entries of `0x24` bytes:
     `+0x14` is a tag datum, and `+0x20`/`+0x22` are function indices
     whose values (`0x00134fc0`) decide whether `0x0002d140` draws the
     entry (a shader through `0x00016610`). S: 03a's map holds one
     `egor`, `effects\screen_effects\blur`.
  3. With `late`, `0x0002df00`. It is shaped like `0x0002dd30`: a
     target, render states, the viewport (`0x003f6840`, `0x003f6f40`,
     `0x003f6390`, `0x003f6190`) and quads.
- **After the scene.** `0x00040890` is a full-screen fade: when the mode
  `[0x00485870]` is not 0, it draws the colour at `0x00485874..`
  (packed by `0x00131e50`), with a blend picked through the jump table
  at `0x00040dc0`. Then `0x00147af1` composes the UI. The HUD render
  (`0x0013954b`, gated by `HUD_SHOWN` `0x0013939b`) is not under it: it
  is the tail jump of `0x001917eb`, which `render_view_scene` calls
  before the fade (Code, `0x00191887`; see "The UI and HUD" below). So
  the HUD is always drawn after `render_screen_effects`, and the post
  passes need no HUD split: the colour pass and the screen effects see
  the scene only, the HUD goes over them, the fade over the HUD, and the
  UI over everything.

### Hook point and order

The colour pass (`COLOUR_PASS`, `0x000214f0`) and each screen effect
(`SCREEN_EFFECT`, `0x0002d140`) run their guest bodies. The entry
hook records the caller, stack frame and plan. The return
continuation closes the plan before the guest caller resumes. Under
`impl.render.pipelined_frame`, these hooks require the game thread.

### What the native post draws

- **The colour pass**, `0x000214f0` (Code). It is two passes, a glow:
  1. The view's target (`[0x004858b8]`) bound to all four stages
     (`0x00014f60`; their sampler states clamp and linear, stored into
     `D3D__TextureState` with the dirty bits), and a four-tap combiner
     (PSDEF at `0x00484f68`, constant colour `pack(c, second x3)` at
     `0x00484f9c`) drawn by `0x00034a90` into target 7, then blurred by
     `0x000211a0` (EAX 7; not traced), which returns the texture.
  2. That texture at stage 0, alpha blending on (`0x000222d0`: states
     59 = 1, 62 = 1 or `0x307` by the sixth argument, 63 = the seventh
     argument clear, 74 = `0x8006`), a one-stage combiner with
     `pack(x, third x3)` at `0x00484f90`, drawn by `0x00034a90` into the
     view's target; then the target is set back (`0x00014bc0`).

  Between them, `0x000211a0` blurs target 7 (Code):
  - `0x00020f30` first writes the blur's combiner (eight stages with the
    fifth argument above 0, else three; `0x00131ed0` packs its colour,
    and the local `y` sets a `<< 24` word) and selects it (the device's
    `+0x924`, `+0x928`, `+0x92c` = the block, `0x003fa3d0`);
  - then up to `ceil(first argument)` steps (about 3.99: four), each
    binding the last result to stages 0..3 (sampler words as above) and
    drawing four taps into the other buffer (7 and 8 in turn), with
    spread `fraction * scale`, the scale growing by
    `1 - g + g * [0x0045dc0c]` per step (`g` the fourth argument, 0.25):
    a Kawase-style blur. It returns the last buffer drawn. At infinity
    (or past 2^24, where `i + 1` is `i`) the loop never ends; the port
    stops after `engine::glow::MAX_BLUR_STEPS` (64) with
    `Error::Unbounded`, so the glue leaves such a call to D3D.

  The quad draw `0x00034a90` (eleven arguments, `ret 0x2c`): the target
  (`0x00014bc0` unless -1), `0x000142f0` with the second argument unless
  -1, the colour write mask (third, `0x1010101`), Z off (or mode 2 with
  the first byte), Z bias 0, Z write off, cull none, stencil off, with
  the byte two more simple states (S: alpha test; `0x203`/`0x204` by the sign of the sixth
  argument); then per tap (the ninth argument: 4 or 1) an offset from the
  word tables at `0x00485af4`/`0x00485b04` over the source's size
  (`0x0001dd60`), or the screen rectangle `0x004b9dd0..`, and the
  spread (seventh); the quad through `0x0001c590` and `0x00034420`.

  Its numbers are built: `engine::colour_pass` (the arguments and
  parameters), `engine::glow` (`pass_a`, `pass_b`, `blur_combiner`: the
  three 60-word blocks; `blur`: each step's source, target and spread,
  and the result). The native pass is these three stages over a copy of
  the view's colour target, through the stage 3 combiner compiler with
  these blocks.
- **The screen effects**, each entry `0x0002d140` would draw: its shader
  compiled as stage 3 compiles layer 15, drawn with the rectangle and
  the entry's function values. Blur needs the scene as a texture, which
  the colour pass's copy already gives.
- **`0x0002df00`** is not drawn natively; a call with `late` set keeps its D3D passes.

### The UI and HUD (Code, `h2 disasm`)

**Where they are drawn.** `render_view_scene` (`0x0002ba10`) calls
`0x001917eb` at `0x0002bc93` when `[0x004b9ed8]` (the view's user) is
not -1: it asks `0x0014de70` and `0x00159170` for the user's unit,
draws per-user overlays (`0x0019188c`, `0x00192129`, `0x0024da7d`,
`0x0024cf66`), and tail-jumps to the HUD render `0x0013954b`. Then the
fade `0x00040890`, then `ui_render_layers` `0x00147af1` (also called by
the alternate view `0x0002c790` at `0x0002c884` and by `0x0002c8a0` at
`0x0002c9bc`). `0x00147af1` takes the user in EAX (-1 every user, the
word at `[ebp+8]` when EAX is -1), stores it at `0x0054d598`, returns
early when `0x00155f60` says so, then for an index 0 to 4 calls the
render slot (`+0x14`) of three layer objects (`0x0054d62c + 0x40 *
index`, `0x0054d76c + 0x38 * index`, `0x0054d884 + 0x38 * index`); index
4 also draws the screen managers around them (`0x00234f8c` on
`0x0054d5c4` and `0x0054dba8`, `0x0023583e` on `0x0054d9e0`,
`0x00234f8c` on `0x0054d99c`). Those collect and sort the screen's widgets
(`0x002353a5`, docs/formats/ui.md, "Draw order") and call each
widget's draw. Last, a fade-in by the clock at `0x0054d5b8` against
`0x0054d5c0` (3 s and 4 s windows).

**The draw primitives under them.** Every draw the HUD and the menus
make that was followed ends in one of three:

| Primitive | What it pushes | Who calls it |
|---|---|---|
| Screen quads: begin `0x00035b90` (record of `0x98` bytes), quad `0x000363a0`, end `0x00036560` (`points::SCREEN_QUADS_BEGIN`, `SCREEN_QUAD`, `SCREEN_QUADS_END`) | The begin sets up to three bitmaps (`0x00012310`, address mode wrap or clamp per stage, point or linear filter), colour writes without alpha (`SET_COLOR_MASK`), depth test off, the blend by mode (`0x000142f0`: mode 10 off, else three 12-entry tables of source factor, destination factor and equation), two vertex constant blocks (a pixel-centred screen transform over the rectangle at `0x00485648`, and a second block from the record's flips and pairs), and, with stage 0 bound and `+0x97` clear, a fixed combiner written into the PSDEF at `0x00484f68` with the record's colours packed in (`0x00131e50`). Each quad commits the state (`0x0001cf50`) and pushes one inline `Begin(7)`, four vertices of diffuse (`SET_VERTEX_DATA4UB`), coordinate and position (`SET_VERTEX_DATA2F`), and `End`: no D3D entry. | Text: `0x0001f070` (from `0x0001fa50`, 22 call sites, and `0x0001fb10`, the HUD's `0x0022aca8`), over the glyph cache `[0x004b62ac]`, one quad per glyph through the layout `0x0013f0e0`, its callback `0x0001f490` and `0x0001f3a0`. A full-screen bitmap at `0x00156960`. |
| HUD shader quad `0x0022a664` (`points::HUD_SHADER_QUAD`) | The HUD's bitmap into `0x004e6978`/`0x004e697c` (datum, index: the shader's engine texture), the shader draw `0x0001bbf0` (EDI the shader), then `D3DDevice_Begin(7)`, per corner `SetVertexData2f(3, u, v)` and `SetVertexData4f(-1, x, y, z, w)` (z and w the float at `0x0045dd40`), `D3DDevice_End`. The coordinates are the rectangle at EAX `(u0, u1, v0, v1)`, times the bitmap's size when it is linear. | The HUD widgets (`0x0022b397` to `0x0022bca4`), the UI's shader bitmaps (`0x002b12ae`), `0x001e936a` to `0x001e95b6`, `0x0020077f`, `0x0024eca0` to `0x0024f556`. |
| Other immediate draws (`D3DDevice_Begin` callers) | Not traced: `0x002548f0` (`0x00254e28`, `0x00255083`, with `0x00014bc0` target switches and `0x00014f60` texture binds: S, the menus' blurred backdrop), the fade's `0x00040d86`, and `0x00029b00`, `0x00037a60`, `0x0003bf20`, `0x00046ea0`. | |

So text is one batch per string with a quad per glyph, each a separate
D3D draw; that is the work a native batch removes (one draw per run of
glyphs, the glyph cache mirrored as a texture). The HUD's bitmaps are
shader draws: native, they need the shader's program (the stage 3
compiler) with the HUD's engine bitmap, in screen space.

**The end of the frame** (for the present's owner; Code, not changed
here). `0x00013b90` calls `0x0001fd20`, then, when `0x0001dc40` and
`0x0001de20` say so, copies the view's target `[0x004858b8]` through
`0x000351a0`, then `0x0001fd00`, `0x0001fcd0` and `0x00022070`.
`0x00013cd0` (`ret 4`) commits the state (`0x0001cf50`), writes push
words itself (`[0x00407488]`) and calls the D3D entry at `0x003fb520`.
`0x00013f10` (`ret 8`) locks a surface for the optional readback
(`0x003fdb20`, `0x003fad50`, `0x003fdbc0`; bitmaps `0x001358c0`,
`0x00135a30`), calls `0x00013d80`, and swaps at `0x000140fc`
(`0x003f9d00`, between two `0x003f9df0`).

#### Native UI

The guest selects the visible screens and widgets, sorts them, and sets
up layout, animation, text shaping, bitmap selection and shaders. At the
sorted virtual draw call `0x00235445`, the hook reads the ordinal, list
record, widget and vtable and verifies that they name the called entry.
The scope closes at `0x0023544d`, reached by fall-through from the call's
return address. List entries keep their original order, depth bounds and
pre/post callbacks. `UI_RENDER_LAYERS` scopes the surrounding screen
managers and layer callbacks. This covers the retail and UI-kit menus
through the same runtime objects. The per-user HUD scope starts at
`0x001917eb` and ends at either return of its tail-called HUD function.
The loading draw at `0x000289e0` also has a scope through both return
paths. The fade and solid rectangle helpers own their separate scopes.

| Producer | What Rust reads | Guest instructions skipped after a successful reservation |
|---|---|---|
| Screen quads, including glyphs and full-screen bitmaps | The four corners at `0x000363c4`, after the engine and XDK state commits. The original writes RGBA to register 9, UV to 3 and position to 0. | The inline Begin, vertex writes and End, and the push-pointer updates for those words. A wrap or pending fence keeps the original. |
| Shader bitmaps and HUD widgets | The four completed positions and UVs at `0x0022a7f6`, after the shader setup. | Begin and vertex setter calls through End, resuming at `0x0022a854`. |
| Full-screen fade | The two signed-short extents at `0x00040d86`, with the fade's selected combiner and blend already set. | Begin, four position setters and End. The fade epilogue runs at `0x00040db8`. |
| Menu fade rectangles, cinematic bars and split-screen dividers | The completed quad copy at the `0x0004b252` uploader with return address `0x00052ce7`. The callback, mode, topology and copy length must agree. | The copy callback and inline packet publication. The common uploader's caller runs its original state tail. |
| Loading spinners and background/sweep bitmaps | The spinner's two sets of completed UVs at `0x0002a1f4`. The bitmap at `0x0002ae60` supplies its two rectangles and UV divisors and Rust evaluates the multiplications with the live MXCSR. | The immediate-mode submission of each fan. The loading animation, clears, target binds and presentation remain in guest order. |
| Menu backdrop wash, tint, and composite | The wash uses the live tint and sample spread at `0x00254b95`. The tint at `0x00254e28` reads the active title's immediate operands. The composite at `0x00255083` reads the selected center and half extent. | Each fan's geometry builder through End. Material and target changes keep their original order. |
| Model widgets | At the sorted call, Rust checks the selector and definition. For an active view it verifies the selected object and light handles, camera scale, animated offsets, field of view and viewport against the camera arguments. It walks the live mesh section, material group and visibility mask at `0x0004d621`. | The entire virtual draw for an inactive widget or an empty name list. An accepted material group skips the indexed geometry builder at `0x0004daa0` and resumes at `0x0004d626`. |
| Debug/widget outlines | Five signed-short points and the converted colour at `0x0001ed98`, with the caller and the open line-strip Begin verified. | The packed colour and vertex loop. The guest End closes the empty stream and restores line state. |

Active model widgets still use the guest's scenario-object updates, view
and layer construction, and the material and transform uploads. When
textures change, the same guest commit runs before native recording and
updates the applied mirror. A refused range lets the guest emitter run
at that draw position. Separate masked strips and disjoint triangle-list
ranges reserve together, in guest order, and require that all draws can
use the first draw's texture views. Each material group commits
independently, so an active model widget can contain both native and guest
groups. The menu backdrop
currently reserves each fan independently. The outline keeps the
guest's rectangle transformation and state setup.

Uncovered HUD and widget helpers still build their draw packets.
The scoped recorder can consume these packets with the resolved D3D
draw available as ordered fallback. The host debug menu is already
drawn by the host over the presented frame. See
[The debug menu](../runtime/debug-menu.md).

**Draw state.** Native packets take their vertex and combiner programs,
textures, flat shading, alpha comparison, fog and stipple from the
same NV2A inputs as D3D. The GPU pipeline takes depth test/write,
stencil operations and reference, fill, winding, clip, colour writes and
blend from the live draw. Point or wireframe fill requires the matching
device feature. The 1× and display copies have separate clip transforms
and the display uses the existing glyph shader option when enabled.
Native UI uses the live textures, including the glyph cache. The guest lays
out the string before `native_hud::ui::screen_quad` reads the corners, so a
leading glyph quad with zero alpha keeps its draw position and the next glyph
uses the guest's pen advance and kerning.
Ordered native packets can join the same GPU pass as adjacent compatible
draws.

**Fallback.** Each early producer observes the state point of the original
operation, including the distinction between API Begin and inline pushed
state. Register carry is installed only on commit. Every single-draw pass
retains its resolved D3D packet for ordered fallback. Behind the render
thread the producer's draw is queued with that packet and commits at once:
the render thread records it or draws the packet, which is the draw the
guest would have issued ([render-thread.md](render-thread.md), "Keeping
guest-visible semantics exact"). A model draw with sibling ranges, or any
draw on the synchronous backend, is validated against every required target
copy and commits before the hook changes control flow; a refusal there
leaves the guest to issue its original draw. Outstanding push callbacks, visibility queries, replay,
unavailable resources or attachments, unsupported shaders or methods,
and GPU preparation failure all preserve the original path. In particular,
the built-in diffuse shader, backend-specific shadow textures and enabled
shader overrides use the original renderer.

`H2_FRAME_STATS=1` adds `native_ui_*` counters, including widget visits
and list validation refusals (`items`, `item_refused`), early
geometry submissions skipped (`builds_skipped`, `walk_suppressed`),
and native ranges committed on each copy. `fallback_*` distinguishes
geometry, order, target, state, shader, texture, upload and reservation.
A preparation refusal may accompany a failed early reservation.
`guest_builder_draw` counts draws that still arrive as guest packets
and `model_guest_builder_draw` counts those inside a model widget.
`backdrop_fallback_*` and `outline_fallback_*` distinguish a missing
entry, incompatible inputs and failed reservations.
`model_fallback_widget` counts widgets with a recorded refusal.
`model_fallback_*` gives the entry, camera, caller, geometry, range
limit, texture commit or reservation reason. The standard per-copy
resource, validation and replay counters still apply. The generated
image must expose the instruction sites for the native walk.

### Post admission

Each call either commits
one complete replacement or draws all its originals. An effect whose
plan requires an unsupported input or an unavailable live target
uses D3D for that call.

| Effect | Native admission |
|---|---|
| Colour pass glow | The window, combiners, target sizes and blur steps must plan successfully. The live quads and attachments must match. |
| Screen strips into target 0 or target 1 | All drawn layer-15 passes must have loaded programs and supported bindings. A target 1 read requires an earlier write in the same call. |
| Kept convolution | It emits no draws. Its result names the pass's target. |
| Blur and wide convolutions | The quads or grids must plan successfully and match the live attachments and colour state. |
| Template pass skipped by `0x00016610` | The host leaves that pass and its convolutions out of the plan. An unreadable gate refuses the call. |
| `0x0002df00`, `0x0002dd30` and layer 17 | D3D. The fade, UI and HUD are the native UI, above. |

### The post hooks

`feature::native_post`: The glow draws after
   layer 17 and before the screen effects. The entry hooks pass
   guest arguments, MXCSR and the x87 control word to the host. The
   guest runs all its setters and its tail with either draw path.

### The call transaction

`d3d::Device::swallow`. Arming drains
   previously queued pushes. The active guest frame is identified by
   its entry, entry ESP, caller's return address and live return
   continuation. D3D entries must still be inside that frame. Each
   matching draw supplies a primitive, count, emitter and colour
   slot. A layer-15 strip must come from the `D3DDevice_End` return
   site `0x0002d8b2`. Glow quads and convolution draws use the pushed
   path.

   The device resolves each original at its usual draw boundary and
   retains the packet, binding handles, textures and stream bytes.
   The first draw into a slot pins the host surface and its guest
   resource identity. Target switches and direct RTT samples can
   proceed while their bindings preserve access to the held
   surfaces. Pixel observations or resource operations that depend
   on those writes trigger replay before the operation. A competing
   external stage or draw scope also triggers replay before it prepares
   its own recording. Gel comparison uses a read-only observer that
   runs after the last matched guest draw.

   The return continuation drains any remaining pushed draws from
   the call and requires the full planned sequence. A mismatch,
   extra draw, interruption or early return replays the packets in
   order. The last matched draw supplies the primary target and
   viewport even if the guest tail restored its state. Native
   recording uses the live slot surfaces and sizes. The backend
   keeps each copy's commands cancellable until all copies have
   recorded the expected ranges. A failed encode discards those
   commands and replays the originals.

   The call validates live packet state and attachments each time.
   Under render scaling, a replacement that uses a reference-only
   intermediate together with a live display target falls back.
   Start A/B runs at `H2_RENDER_SCALE=1`.

   Tests in `d3d/tests/swallow.rs` compare the original packets at
   every mismatch position and across target switches, and check
   same-shape draws from a different End caller. GPU tests compare
   both colour attachments after a failed encode and check that a
   successful replacement writes the intermediate back to the guest.
   The screen renderer's GPU test covers masked writes and pixels
   preserved between partial passes into a BGRA intermediate.

### The colour pass

Its numbers are ported: `engine::colour_pass`
   (`arguments`: the caller's half with `0x000137a0`'s curves;
   `parameters`: the callee's override record and clamps;
   `pack_colour`: `0x00131e50`; `Parameters::colours`: the two combiner
   colours), bit-equal to the lifted original
   (`engine/tests/colour_pass_differential.rs`), and the glow's:
   `engine::glow` (`engine/tests/glow_differential.rs`: the three
   combiner blocks, and the blur's steps with the device calls stubbed).
   The host (`boot::native_post_glue`) uses them through the swallow
   (the call transaction, above).

   1. **The quad** (Code). `0x0001c590` loads the vertex shader
      the rasterizer globals name for screen quads
      (`[[0x00485a80] + 0x5c] + 0x64`, entry 0; `LoadVertexShaderProgram`
      when Halo's cache at `0x0051f0f0` holds another), which passes
      every input through: `oPos = v0`, `oT0..oT3 = v1..v4`,
      `oD0 = v5`, `oD1 = v6`. So positions are the target's pixels.
      `0x00034420` (EAX 0: rectangle, callback `0x00034100`, 0, the tap
      record) runs the draw-state reset `0x00016b10` again, sets the
      four stages' dirty bits and zeroes their `D3DTSS_ALPHAKILL` words,
      commits the textures (`0x0001cf50`: `SetTexture` for each stage
      whose `0x0051f3d8` word differs from `0x0051f3c8`'s, which it
      updates), the vertex input (`0x0001c710`) and the deferred state
      (`0x00400fa0`), then emits one four-vertex fan (`Begin(7)`) with corners
      `(0, 0)`, `(0, 1)`, `(1, 1)`, `(1, 0)`, attributes 15 down to 0 from
      the callback as inline `SET_VERTEX_DATA4F`. The callback: attribute
      0 is the rectangle's lerp (`0x000350e0`, z and w from the near and
      far at `0x00485ad4`); attribute `k + 1`, for `k` below the tap
      count, the same lerp moved by the spread times a sign
      (`[0x0045dbbc]` with `k & 1` in x and `k & 2` in y,
      `[0x0045dbc4]` otherwise) and scaled by the stage's bound size
      over the target's. With the quad draw's tenth argument clear (all
      three passes) the rectangle is Halo's window (`0x004b9dd0`: top,
      left, bottom, right shorts), and the eleventh clear means no
      window offset, so a fragment's base coordinate is its own pixel
      and each tap reads the source in texels. The down-sample's four
      taps at spread 0.5 land half way between texel centres: a 4 by 4
      box. `0x0001dd60` reads Halo's target table (`0x004b4b58`, stride
      `0x98`: width `+0x80`, height `+0x84`, surface `+0x88`, a hiding
      byte `+0x95`; targets 0, 3 and `0x18` default to 640 by 480).
      Targets 7 and 8 are made by `0x0001df20` at 160 by 120,
      `D3DFMT_LIN_A8R8G8B8` (`0x0001d6f0`'s linear format `0x12`), one
      level. `0x00014f60` binds a target to a stage: its size into
      `0x00485af4`/`0x00485b04` and its texture into `0x0051f3d8`
      (through the jump table at `0x00015114` by the target's kind).
   2. **The native glow:** `native_render::glow`.
      `read_frame` reads the window, the tap signs and the sizes;
      `Plan::quad` gives each quad its corners and per-stage offsets and
      scales; `VERTEX` is the screen shader for them; `program` compiles
      a block through `shaders::psh` with its stages linear (`tex_scale`
      one over the guest size) and fog off; `GpuGlow::encode` draws a
      plan's quads into live targets 7 and 8 and the view's colour,
      using each attachment's format and loading its existing
      pixels. The composite uses the blend of
      `0x000222d0`'s states (`Blend`: source `ONE` or
      `INVDESTCOLOR` by the sixth argument, destination `ONE` unless
      the seventh's byte is set, alpha alike). The down-sample and
      blur draw with blending off (the Blending item below), and S: the D3D path's pixel centres
      match a quad whose corners are the window's (the frame A/B shows
      it). Tests: `glow::tests` (the taps of the down-sample and the
      composite, the blend, a block's module, the frame's words).
   3. **Blending.** The game's code runs; only its quads are dropped. `0x00034a90`'s `0x000142f0(10)` sets
      `NV097_SET_BLEND_ENABLE` (`0x00040304`) to 0 for the down-sample
      and the blur (Code), so they draw unblended, as `GpuGlow` does.

### The screen effects

`0x0002d140`, with the function values (`0x00134fc0`) read at the call (Code, `h2 disasm`).
   - **The loop** (`0x00013a0d..0x00013b72`): the window `0x004b9dd0`
     as four floats twice (the corners), then per entry of the scenario
     block (`+0x3d0` count, `+0x3d4` address, `0x24` bytes): skipped when
     its tag datum `+0x14` is -1 or both function indices `+0x20`/`+0x22`
     are -1; otherwise each index not -1 is evaluated by `0x00134fc0`
     (ECX the index; the scenario's function block `[0x004e6740]`,
     `0x20` bytes each, through the tag function evaluator `0x0013b390`)
     into two floats, and with either non-zero
     `0x0002d140(values, 2, 2, corners)` runs with EAX the datum (`ret
     0x10`).
   - **`0x0002d140`** copies the four value words to `0x004c19b0..bc`
     and returns at once for a datum of -1, an empty window, or a target
     override `[0x004858b8]` set. The datum is an `egor` tag; its shader
     reference (`+0x44`, a template through `0x00137bd0` when its group is
     `MTRP` or `3trp`) gives the implementation count. Then per entry of
     its pass block (`+0x88` count, `+0x8c` address, `0xac` bytes):
     - the pass's implementation index (`+0x08`, -1 the loop index) and
       one of four bytes `+0x0c..0x0f` chosen by which of the two values
       are zero (-1: skipped);
     - its target by `+0x60` (the jump table at `0x0002db68`: the view
       at full size, or a half or quarter of the window, `0x0001ddd0`);
     - `0x00016610(shader, 15, index, byte, ...)`, the shader's layer-15
       pass (a template pass, as the world's and the transparents'), then
       the quad pushed by hand: `0x417fc` (`SET_BEGIN_END`, 6) and per
       vertex up to four `SET_VERTEX_DATA4F` texture coordinates (the
       kinds at `+0x50`, jump table `0x0002db78`: from the corners, the
       window over its size, or scaled by `[0x0045dbbc]`/`[0x0045dc20]`)
       and the position (`0x101a00`, z and w `[0x0045dd40]`), S: a strip of
       `2 x 2` vertices for the call's `(2, 2)`; `0x003fb480` ends it;
     - then per entry of its convolution block (`+0xa4` count, `+0xa8`
       address, `0x5c` bytes): a size from the target's (`0x0001dd60`)
       times `+0x58`, at least 64 and its area capped at `0x12c00` by a
       square root (`0x00322b0c` rounds), targets `0x19`/`0x1a` (first)
       or `0x1b`, and with flag bit 2 `0x00021360` (ported:
       `engine::glow::wide`) else the blur `0x000211a0` (ported:
       `engine::glow::blur`), its result in
       `[0x00485a78 + 4 i]`.
     - At the end the targets 1, `0x19`, `0x19` and `0x1b` are set back,
       `[0x004670bc]` set, the draw state reset (`0x00016b10`) and
       `0x00485a78`/`7c` cleared.
   - **What the native side does.** The draws are swallowed as the glow's are (the hand-pushed strip is an inline `Begin(6)` of four vertices; the blurs are `0x00034a90` quads). The pieces are: (a) the plan, which entries draw (the function values, `0x00134fc0` over `0x0013b390`, with its own differential test), and per pass the implementation, target and texture coordinates (pure, from the tag); (b) the layer-15 template pass through `program::translate_pass` (the world's and lights' path) with the engine inputs the screen-effect pass reads; (c) `0x00021360` beside `blur`; (d) targets `0x19` to `0x1b` as convolution targets like 7 and 8.
   - **which entries draw** (`engine::screen_effects`):
     `function_value` (`0x00134fc0` and `0x00134fe0`: the state's input
     plus its `+0x1c`; without a parent (`+0x16` -1 or past the count)
     the function through `0x0013bb90`, its output range unless a flag in
     `0xf0`; with one (itself without a parent, else 0) the parent's
     value times its own raw value, then its range), the evaluator being
     `engine::function_eval`; `full_screen_single_view`
     (`0x00022710`: the viewport `0x00485648` is 0, 0, 480, 640 and at
     most one view `[0x004ba04c]`); `gated`, `corners` and `drawn` (the
     loop: each entry's datum and four values). Tests: unit tests, and
     `engine/tests/screen_effects_differential.rs` (`0x00134fc0` over
     random functions of every type with parents, and the loop from
     `0x000139ba` with `0x0002d140` stubbed, against the lifted
     original, bit for bit).
   - **the evaluator** (`engine::function_eval`, `0x0013b390`
     instruction for instruction): SSE in the original's operand order,
     so a NaN keeps the sign and payload the original's does (the blend
     is `second * r + (1 - r) * first`, type 9's `(1 - r) * first +
     second * r`); shape `0x0017ca10` and wave `0x0017c900` over the
     image's tables with the CRT `fmod(v, 1.0)` (`0x00373b6a`: `fprem`
     for finite non-zero, `fldz` for a zero, a quieted NaN, the
     indefinite for an infinity) and an x87 `fistp`; type 9 and spline
     2 through the CRT `pow` (`0x00372d48`) on the x87 model: `fyl2x`
     and `0x00327f84`'s `frndint`/`f2xm1`/`fscale`, negative bases by
     `0x00372f31`'s integer and odd test, zero bases, NaNs (`faddp`),
     infinities (`_powhlp` `0x003738a8`), the range check `0x0032806d`
     (scaled by 2^1536 or 2^-1536) and `_handle_exc` (`0x003246b5`):
     overflow to the rounding mode's infinity or largest value,
     underflow to a denormal by truncation or zero, the others kept.
     Spline 2's exponent is `rsqrtss` of one segment's squared length
     times the halving-trick root of the other's, as the original
     computes it. Control words with an x87 exception unmasked are
     refused. Tests: unit tests, and the differential above with
     `_getptd` (`0x00326079`) stubbed so the CRT's error paths run
     lifted.
   - **what each draws** (`engine::screen_effect::plan`, one call
     of `0x0002d140` as an `Effect`): the shader and layer 15's
     implementation count (the template datum the shader's `+0x24`
     points at, its definition's `+0x5c` element: the word at
     `[+4] & 0x1ff`, then the word that many plus 15 into `[+0xc]`, its
     high 7 bits; `Err(Data)` for `MTRP`/`3trp`, which go through a
     virtual call). Per pass that draws: its index, implementation and
     selector (`0x00016610`'s third and fourth arguments; skipped as the
     original skips them), its target kind and target 1's size (window
     width and height, halved or quartered toward zero), the strips (per
     row of corners, per column the corner and the one below; four
     texture coordinates by kind, 0 the corner's uv, 1 the window
     fraction, 2 pixels from the window's top left, 3 the corner's x and
     y, 4 the vector at `[[0x004687c0]]`, z 0 and w 1 except kind 4; plus
     the offsets `+0x5c` when `+0x58` is above 0, u and v times
     `[0x004b9de4]` when the set's flag bit 0 is set; the position raw,
     from the window's top left, or that times `[0x0045dbbc]` or
     `[0x0045dc20]`) and `Pass::pushed`, the exact words; and its
     convolutions (kept, the blur with `glow::blur`'s draws, or
     `0x00021360`'s registers and arguments), each with its size (the
     area cap: x87 `fild`, `fdivr`, `fsqrt`, `_ftol2` toward zero, both
     64 when either side is under 64). At the end the original sets
     target `0x19` twice, the second time to `0x1a`'s saved size (as it
     does; a native draw need not copy it). Tests: unit tests, and
     `effect_plans_match_the_lifted_original` (`0x0002d140` lifted, its
     callees stubbed and recorded: the template passes, every size set,
     the convolution calls and the words pushed must be the plan's).
   - **`0x00021360`** (`engine::glow::wide`), the blur's
     sibling (Code): EAX the source, ECX the draw count, XMM0 a scale,
     then stack words passed to each draw, the two buffers, the last
     target, a byte that first sets the combiner `0x00020f30` (XMM0 0,
     XMM3 its eighth word), `ret 0x20`. `s = scale * [0x0045e4e0]`; per
     draw (count above 0) the source is bound to stages 0..3 (`0x00014f60`,
     the same sampler states as the blur) and `0x00034d90` draws into the
     destination with EDI pointing at `s`, `s * [0x0045e1f4]`,
     `s * [0x0045e4dc]` and `0 - s`, the buffers ping-pong as the blur's,
     and `s` grows by `[0x0045dc0c]` (the blur's spread term). The result
     and the copy into the last target are the blur's. `MAX_WIDE_TAPS`
     (64) bounds it. `0x00034d90` is `0x00034a90`'s sibling: the
     target's size (`0x0001dd60`, -1 the bound one `[0x004858b8]`), the
     target's rectangle or the window by a byte argument, the per-stage
     size ratios (`0x00485af4`/`0x00485b04` over the target's) and EDI's
     four floats where `0x00034a90` takes a spread; the grid's strips give the four moves of each tap. Tests: unit test
     `the_wide_convolution_ping_pongs_with_a_growing_scale`, and
     `glow_differential::wide_matches_the_lifted_original` (stubbed as
     the blur's: draws, offsets, the quad's stack words, combiner, copy
     and result); the plan now follows a convolution after a wide one.
   - **`0x00034d90`'s grid** (`engine::screen_grid`): the grid
     each step of `0x00021360` draws, 30 strips of 21 x 2 vertices
     (`0x00034770` with the attribute function `0x00034100`). Each tap is
     the rectangle at (u, v) pulled toward its centre by its EDI float,
     weighted, when the radii (words 5 and 6) differ, by where the
     vertex's distance from the centre (x87 `fsqrt`) falls between them,
     times the stage's bound-over-target ratio; the position through
     `0x000350e0`. A radial (zoom) blur. `strips()` and `pushed()` give
     the exact words. Tests: unit tests and
     `screen_grid_differential::the_grid_matches_the_lifted_original`.
   - **The screen-effect hooks** (`feature::native_post`). The entry
     hands boot's host EAX and the four stack words. The host plans
     the drawn passes and refuses unsupported inputs before arming.
     The return hooks commit the staged recording or replay the call.

   - **the template pass's look** (`native_render::screen_look`):
     `screen_look` takes the egor's shader resolved and an
     layer pass and implementation and gives that pass of layer 15 (at
     the first level of detail; its vertex shader, stages as the shader's bitmaps
     or engine textures the draw supplies, stage and render states,
     pixel shader, constants), or why it stays on D3D (no layer, no such
     pass or implementation, animated, engine inputs beyond the
     instance's, engine-driven states, unset or default textures, a
     lightmap, states the native pass does not fit). `compile_screen`
     translates it through `program::translate_pass` over the strips'
     inputs, `v0` the position and `v1`..`v4` the four texture
     coordinates (`0x00101a00`, `0x00101a10 + 16 s`).
   - **The load** (`boot::native_post_load`): every `egor` tag's shader (the definition's `+0x44`)
     resolved, each implementation of each of its layer-15 passes
     looked at and translated with its vertex shader's classification-0
     program (below), and the stages' bitmaps decoded; refusals counted by reason, one line on
     stderr. The post glue runs it itself, on a thread of its own at the
     first screen-effect call on a map (`native_post_glue::EffectsSlot`),
     with or without the native world.
   - **the engine textures** (`0x00019530`, the jump table at
     `0x00019b68` by id - 1; `native_render::screen_look::engine_texture`):
     1, 9, 14 and 15 the rasterizer data's bitmaps (`+0xc`, `+4`, `+0x14`);
     render targets through `0x00014f60`: 2 and 3 target 1, 6 target 3, 8
     target 10, 13 target 9, 17 `0x13`, 19 `0x15`, 20 `0x16`, 21 13, 22
     `0x11`, 23 `0x17`; 5 the view's bound target `[0x004858b8]`; 24 and
     25 the target the word `[0x00485a18 + 4 id]` names (`0x19`..`0x1b`
     sized through `0x0001dd60`: the screen effects' own targets); 4
     clears the stage; 10 and 12 the light's; 16, 18, 26 and 27 chosen at
     run time; 7 and 11 null entries. A template pass binding the light's,
     a run-time one or an invalid id is refused
     (`ScreenRefusal::EngineTexture`); the rest the draw supplies.
   - **which pass, implementation and program**
     (`0x00016610(shader, layer, pass, implementation, target, flag)`,
     Code): the layer's passes are the template's word at its layer
     index (low 9 bits the first, high 7 the count, which is
     `screen_effect::Effect::implementations`); `pass` -1 draws them all,
     else that one (`screen_effect::Pass::implementation` names a layer
     pass), each with implementation `implementation`
     (`Pass::selector`; `0x132` bytes each, the vertex shader at `+0x100`,
     -1 not drawn). After `0x0001bbf0`, `0x0001c590` loads that vertex
     shader with classification 0 (`0x0001c2e0`, `0x1c` bytes per
     classification), so the load translates only the first program
     (`screen_look::SCREEN_CLASSIFICATION`), per (layer pass,
     implementation): `effects\screen_effects\blur` has two layer-15
     passes (`copy_fb_to_texaccum`, `copy_texaccum_to_fb`), so a layer
     with several passes is accepted. What the real egors
     (`blur`, `desaturate`) showed the look must take: their passes set
     `ZENABLE` 0 and no ZWRITEENABLE (`pass_state` takes the depth
     buffer off as enough; a screen pass testing or writing depth is
     refused, `ScreenRefusal::Depth`), and `screen_xform_4` moves `v5`,
     `v6` and `v7.w` into the colours and fog, registers the strips do
     not push: `compile_screen` gives them the uniforms' defaults only
     for a pixel side that reads no colour or fog
     (`screen_look::reads_vertex_colours`, which also counts a missing
     final combiner as reading them). Its WGSL writes no depth
     (`PassParts::w_buffer` off). The engine constants are the call's
     (below).
   - **The GPU pass** (`native_render::screen_gpu`). It translates
     the layer-15 program at the target's guest size with the
     device's depth scale. Engine stages read the view, target 1 or
     a convolution result. A stage reading the view takes an
     ordered copy before the pass. Each attachment uses its live
     format and loads existing contents. Partial strips, colour
     masks and consecutive writes preserve the surrounding pixels.

   - **the end of the call draws nothing**
     (`0x0002dae8..0x0002db64`): `0x0001ddd0` writes the sizes of
     targets 1, `0x19` and `0x1b` back into the target table (`+0x80`,
     `+0x84`, the surface words `+0x14`, `+0x28`) from what
     `0x0002d26e..0x0002d344` saved on entry (0 for a target without a
     surface; `0x19` is written twice, the second time with `0x1a`'s
     saved size, and `0x1a` never), `0x00016b10` applies the last pass's
     restore states and clears the draw-state cache at `0x00485b48`,
     `[0x004670bc]` is set and `0x00485a78`/`7c` cleared. The game runs
     it on both paths. The backend records into the physical
     surface used by each original draw, even when the tail changes
     the guest size fields.
   - **Target 1** (`screen_gpu::Dest`). A pass of kind 1..3 uses
     slot 2 at the window's size, a half or a quarter. Engine
     textures 2 and 3 can sample that output in later passes of the
     call. Reading target 1 before the call draws into it or while
     the same pass writes it refuses the replacement.

   - **the call's engine constants**
     (`screen_look::ScreenConstants`): `0x0002d140` copies its four
     values to `0x004c19b0`, and the scalar evaluator `0x000336f0` (its
     table at `0x000338d4`) returns `[0x004c19b0]` for scalar 41 and
     `[0x004c19b4]` for 42 (`screen_look::SCREEN_SCALARS`), so a pass
     reading those is supplied. The blur's
     `copy_texaccum_to_fb` reads 41 into `c0.a` (pixel mode 3), the mix
     of its two stages, and sets `c18`..`c21` from its stages' sizes
     (vertex modes 14 and 15 of stages 1 and 2: `[w, 0, 0, 0]`,
     `[0, h, 0, 0]`), which carry its coordinates into texels. Per call
     the pass gets each combiner constant reading a value packed as
     `0x00019ca0` packs it (the binding's transform, then
     `pack_engine_colour`, into its lanes) and each vertex constant
     reading a value or an engine stage's size evaluated as
     `0x00019be0` evaluates it (`tags::material::EngineVertexConstant`,
     from the `spas`, keeping what no later binding of the register
     writes), the size the guest size of the target the stage binds
     (`screen_gpu::stage_sizes`: the view's, target 1's as the call drew
     it). The rest stay as resolved at load: the instance's scalar 21,
     the pixel side's sizes (`0x00019ca0` takes 1 by 1), a bitmap stage's
     size. A vertex register the resolver took from the engine that no
     external of the implementation names is refused
     (`ScreenRefusal::EngineVertexConstants`).
   - **the blur's convolution** (`screen_gpu::ScreenConvolution`,
     `native_post_glue::screen_convolution`): `0x0002d140` calls
     `0x000211a0` with its seventh word 1 and the eighth to tenth 0, so
     the combiner is `0x00020f30`'s at strength 0 and `y` 0
     (`engine::glow::blur_combiner`), and each quad goes through
     `0x00034a90` with its tenth argument 0: the window's rectangle in
     the target's pixels, four taps, each stage's coordinates scaled by
     its source's size over the target's (`0x00034bf7`..`0x00034c27`),
     the quad `glow::quad` gives the glow's, drawn by the glow's
     `glow::QuadDrawer`. The quads bind the live levels
     `0x19`..`0x1b` (slots 3..5). Engine textures 24 and 25 resolve
     the results in the order the call produces them. A convolution
     whose colour state or source differs keeps the original draws.

   - **the wide convolution** (`0x00021360`; `screen_gpu::ConvolutionGrid`,
     `native_post_glue::wide_grid`, `grid_of`). `0x0002d140` passes it
     the words `+0x50`, `+0x54`, the convolution targets, -1, 1, 0, 0, so its
     combiner is the blur's (`0x00020f30` with XMM0 0 and XMM3 its eighth
     word, 0), its stages are bound as the blur's (clamp, linear), and
     each draw is `0x00034d90` (Code, `0x00021455..0x00021485`) with the
     same screen vertex shader (`0x0001c590`) and states as the quad's:
     the window's rectangle (word 7's low byte clear), depth `1.0`, the
     two words its radii, EDI the draw's offsets, each stage's ratio the
     size `0x00014f60` bound the source at (`0x0001dd60` of it: the
     pass's target as the call sized it, `screen_effect::target_size`,
     or a convolution target at the pair's size) over the target's. The glue
     makes each draw's 30 strips of 42 vertices with
     `engine::screen_grid::strips` (the original's arithmetic, under the
     call's MXCSR and x87 control word), the taps in texels of that bound
     size; `glow::QuadDrawer::draw_grid` draws them through
     `glow::GRID_VERTEX` (attribute 0 the position in the target's
     pixels, 1..4 the taps; z and w ignored as the quad's are: no depth
     test and w the same at every vertex) and the blur's combiner with
     its stages scaled by the bound size. The swallow takes each strip
     as `SET_BEGIN_END` 6 of 42 (`0x00034770`) into the target's slot.
     A grid that cannot be made keeps the call on D3D
     (`Missing::Wide`), as does a missing frame; a grid with a strip
     under a triangle is refused (`ScreenRefused::NoStrips`). A wide convolution is armed whenever the native post is.
   - **Diagnostics.** The menu and gameplay call `0x0002d140` with different bound targets and grid sizes, so a menu A/B with no committed screen replacement shows identical images. `H2_FRAME_STATS` includes the native post examined, suppressed, refused and recorded-range columns.

## Rules and limits

- Each post call commits one complete replacement or draws all its originals. A plan with an unsupported input or an unavailable live target uses D3D for that call.
- Under render scaling, a replacement that uses a reference-only intermediate together with a live display target falls back.
- `0x0002df00` and `0x0002dd30` (same shape as the screen-effect passes) and layer 17 (S: bloom) are not drawn natively. The end of the frame (`0x00013b90`, `0x00013cd0`, the present `0x00013f10`) is D3D.
- Under `impl.render.pipelined_frame` the post hooks require the game thread.

## Not verified

- Pixel identity of the same view and call with `impl.render.native` on and off (fixed save, view and time, captured through `H2_BOOT_GFX` or the agent API's `render.frame`): the glow and screen replacements commit with no unexpected refusals or replays, and every changed intermediate, its next D3D consumer and the final image match.
- The cost of `render_screen_effects` against a D3D measurement.
- That the D3D path's pixel centres match a quad whose corners are the window's (S).

## Related

[native-renderer.md](native-renderer.md), [native-transparents.md](native-transparents.md), [formats/ui.md](../formats/ui.md), [d3d.md](d3d.md), [debug-menu.md](../runtime/debug-menu.md).
