# Native particles and effect geometry

Status: Built. Particles, beam emissions, generated contrails, the four-vertex effect builder, lens reflections, volume and trail billboards, weather layers and lines, and two researched material strips are admitted through `d3d::DrawAdmission`. Pixel parity and timing are not verified.
Code: `crates/rewrites/render-bridge/src/native_particles.rs`, `native_effects.rs`, `crates/extensions/game/src/native_particles_glue.rs`, `native_particles_admission.rs`, `crates/rewrites/native-render/src/particles.rs`, `particle_gpu.rs`, `effect_geometry.rs`, `effect_immediate.rs`. Tests: `crates/rewrites/render-bridge/tests/native_particles.rs`, `crates/rewrites/native-render/tests/particles_gpu.rs`, `crates/platform/d3d/tests/admission_scope.rs`, `external_pass.rs`.
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md).

The guest keeps all particle, beam and contrail simulation, property evaluation and geometry construction. The stage reads the completed geometry and replaces only the draw packet. Part of [native-renderer.md](native-renderer.md).

## How it works

### Particle boundary

Code evidence from the owner's executable, inspected with `h2 disasm`:

- `0x00174420` updates particle/effect state while rendering. It is **never
  skipped** here; neither are property evaluation or emitter iteration.
- `0x002243c0` sets up the particle material, selects the vertex program
  through `0x0023ea80`, then walks particles with callback `0x00224550`.
- `0x00224550` evaluates colour, alpha, rotation, size and sprite windows;
  `0x00224cd2` calls `0x0023ed30` with eight stack words and an EBX pointer.
- `0x0023ed30` writes dynamic attribute registers selected at
  `0x00470a3c..0x00470a58`, then one four-corner quad. The two-float writes
  reset z/w to 0/1. Packed colour is little-endian RGBA, **not** ARGB.
  `0x0023ecd0` initializes the register assignments; we read the live
  assignments and refuse overlaps or register 0 rather than copying that
  table. `0x00470a38` gates emission.
- The inline corner payload is eight words for four vertices. The
  stage admits only a *live declaration proven by the adapter* to be v0
  SHORT4, no other arrays. It expands (0,0), (1,0), (1,1), (0,1), z/w both
  zero. A different declaration refuses; the payload alone is not proof
  of SHORT4. Quad triangulation uses the D3D diagonal (0,1,2), (0,2,3).

`native_render::particles::read_emission` reads the helper inputs without
writing guest memory. The feature's context-only hook always returns
Continue. Keeping the helper running preserves allocator failure behavior,
push-buffer put pointer, shader state and the last attribute values that a
later native or D3D draw inherits. The adapter replaces only the captured
primitive after those effects have occurred. The stage saves submission
work only; it does not remove the per-particle property evaluation.


### Contrails and other effects

- `0x00042b20` enumerates contrail records and channels. `0x00043740`
  queues a sorted entry through `0x0001e5e0`, with material setup
  `0x00043720`, builder `0x00042cf0`, stride 24 and two vertices per point.
  The existing `0x00035b00` sorted handler selects the live declaration and
  invokes `0x0004b220`. That dispatcher is unchanged.
- `0x00042cf0` retains ownership of ribbon construction: camera facing,
  endpoint fading, curve evaluation and point state. The native decoder
  checks its completed bytes, with a live FLOAT3/FLOAT2/D3DCOLOR layout at
  offsets 0/12/20. Register locations come from the live declaration.
  Packed BGRA goes to GPU vertex fetch unchanged. Triangle strips retain
  vertex order, duplicate/degenerate points and strip topology.
- `0x0004b220` receives stride in ECX and mode, primitive index, vertex
  count, builder and context on the stack. Only mode zero is admitted.
  The primitive index is resolved through the live table at `0x0043fdc0`;
  it is not assumed to be the NV2A topology. Its push-size limit is checked
  before allocation. The original callback and allocator still run.
- `0x00052040` calls the same upload helper with builder `0x00052020`,
  which copies four 20-byte FLOAT3/FLOAT2 vertices. These effect quads are
  admitted only with a proven declaration and strip/fan/quad topology.
  No semantic label such as lens flare is inferred from this helper alone.
- Effect-event beams pass through `0x00179e80` → `0x00156b60`; the sorted
  and direct beam render paths reach `0x00157020`, whose call at
  `0x0015743d` uses the same `0x0023ed30` emitter as particles. They share
  the particle decoder and admission without skipping the beam parents.
- Other effect builders and unsupported declarations remain D3D-owned.
  The lens, MGS2 and tdtl leaves and the remaining refusals are catalogued
  below; admission is per draw, never an entire effect-event handler.


### Bound admission contract

`boot::native_particles_glue::install(&device)` installs a concrete
`ImmediateAdmission` host and `DrawAdmission` implementation. With the switch off it installs neither, which avoids extra D3D snapshot work.

The original six hooks run with the walks disabled and use no Rust continuation:
particle entry, publication (`0x0023ef77`) and epilogue (`0x0023ef79`), and
dynamic entry, post-builder/pre-publication (`0x0004b28c`) and epilogue
(`0x0004b2a3`). At entry the decoder captures the producer inputs. Before
publication, the adapter reads the completed dynamic bytes and drains
only previously published commands, with no admission active. It then
arms exactly one packet. The epilogue drains that published packet and
unconditionally clears the scope. Allocation failure publishes nothing
and is cancelled. A mismatched first draw consumes the scope too: a later
identical quad cannot become owned. Pending interrupt callbacks remain
queued and refuse the scope. No shared sorted dispatcher is modified.

`DrawSnapshot::draw` supplies the actual declaration, streams, defaults,
shaders, constants, state and clipping. Particle corners and every effective
register must agree bitwise with the independent emission decoder.
Generated geometry must agree byte-for-byte with the completed callback
output, and its topology/count/declaration must agree independently.

A small pure helper in `d3d::nv2a::resolved` uses the existing translator's
program-key derivation and state rules for uniforms and samplers. It keeps
the final-combiner latch, inherited defaults, fog, alpha test, signed
channels, mip bias, W clipping and offset, stipple and per-copy window
shift. W clipping remains enabled even when the fragment does not write
depth. Combiner constants are uniforms, not pipeline-cache keys. Neither
this helper nor the native renderer writes the shared upload ring.

`ExternalTarget::stages` supplies the resident effective texture views,
including D3D's palette, mip, format and signed-channel handling. Each
copy gets immutable vertex/index/uniform buffers. `prepare_copy` prepares
that copy, then `record_ranges` records exactly one range into the supplied
isolated encoder. The handoff seals both reference/display recordings
before committing either; a refusal on either discards both and lets D3D
draw. The handoff also owns target-write, RTT and alias bookkeeping.

Explicit assumptions and limits:

- Publication points above belong to this executable's helper bodies;
  they are not a global next-shape matcher. Guest simulation writes at
  `0x00174420` and all material/property/builder work continue unchanged.
- Attachment/stage views are held by the handoff for the reservation, on
  the render thread when it is queued. Native identities use the host device plus a unique call
  epoch and copy, rather than guessing a map or attachment generation.
  Prepared draws never survive that reservation. Pipeline caches contain
  no guest resource handles and are keyed by their shader/state contents.
- The seam does not expose a player-view ordinal. Diagnostics/counters use
  view bucket 4 (other); each draw's actual viewport, constants and scissor
  still come from its own snapshot. No stale per-view matrices are cached.
- Only programmable vertex shaders, one scissor rectangle, a depth-backed
  target and the researched vertex declarations are admitted. Shadow
  sampling, query-counted draws, glyph filtering, unknown shader behavior
  and missing sampled stages refuse. Integer uniform render scale and
  single-sample attachments are the handoff/backend contract here.
- The stage replaces D3D's GPU recording and keeps guest geometry construction and resolved-draw capture. Uploads are owned per recording so cancellation stays correct. The feature's armed count is not replacement coverage; the stage counters are.


### Additional immediate effect leaves

The adapter also owns narrowly scoped immediate draws. These use the same
`DrawAdmission` and two-copy reservation as particles. On each leaf's END
boundary it drains previous published work with no owner, arms one draw,
then drains/cancels at the next instruction. It never skips the leaf or its
parent. For the two shared billboard emitters the saved return address must
also match a researched caller; another caller gives `UnownedCaller`.

| Producer | Researched path | Admission |
| --- | --- | --- |
| Lens-flare reflections | `0x0002e9e0`, call `0x0002f247` to `0x000480a0` | Four-vertex triangle fan; saved caller `0x0002f24c` |
| Light volumes (`MGS2`) | Sorted handler `0x0004f010` → `0x0004f7e0` → `0x000480a0` or `0x000484b0` | Four-vertex triangle fan; saved callers `0x0004fddd` / `0x0004fd9d` |
| Light trails (`tdtl`) | Sorted handler `0x000508d0` → `0x0004f7e0` | Same billboards, preserving every trail update and material change |
| Weather layer | `0x00037a60` | Four-vertex quad strip |
| Weather particle lines | `0x000391f0`, resolved LineList | Complete line pairs, bounded at 65,536 vertices |
| Weather particle points | Same helper, resolved PointList | `WeatherPointRasterization`: point size/expansion is not represented by this renderer |

Boundary pairs (before END / after END) are `0x00048497/0x0004849d`,
`0x00048ae5/0x00048aeb`, and `0x00038342/0x00038349`. Weather particles
write END inline: observe before its put publication at `0x000396f7`, then
finish at `0x000396f9`. Early-out paths never arm. The saved-caller stack
offsets are `+0x24` and `+0x30` for the two billboard emitters.

Admission requires a single 256-byte immediate stream with unique Float4
attributes at `16 * register`, matching the resolved shader input mask,
and position register zero present. `ImmediateDeclaration` and
`ImmediateGeometry` identify malformed/unproved inputs. Written bits are
copied exactly; unwritten attributes come from the resolved vertex defaults.
The weather quad-strip winding is `(0,1,3),(0,3,2)`. Line primitives use the
same shader/state translation as D3D and cannot enter the triangle pipeline.
All shader, texture, target, scissor and state preflight restrictions above
still apply. Failure on either copy retains the original draw.

This assumes the resolved immediate stream is the authoritative output of
the guest's geometry calculation. Unlike the particle emission decoder,
this path does not independently re-evaluate billboard/weather arithmetic.

The handoff prepares every live copy before recording any. Particle passes retain one prepared entry per Reference/Display copy
and remove only the matching entry when recording. This is required at
2x and whenever both views are awake.

### Effect-handler census and remaining refusals

This census follows both calls to the common sorted-entry allocator
`0x0001e2d0` and producers that allocate entries directly. Entries are
0x70 bytes with handler at +0x20 and data at +0x24. It is not a blanket
claim that every runtime callback is known. The shared sorted dispatcher
is unchanged; each owned leaf executes at its original position.

| Handler / producer | Evidence and disposition |
| --- | --- |
| Particle `0x002243c0` | Material and particle iteration reach the existing `0x0023ed30` decoder. Parent and `0x00174420` writes remain guest-owned. |
| Sorted beam `0x00156e10` | Producer `0x00156b60` also queues direct-layer callback `0x00156fe0`. Both call `0x00157020`. Its only geometry emission is `0x0015743d` → `0x0023ed30`; there is no separate unresolved beam draw in this helper. Property evaluation and material/program selection still run. |
| Sorted dynamic `0x00035b00` | Producer `0x0001e5e0`, used by contrails. Exact builder `0x00042cf0` admitted; others give `UnownedDynamicBuilder` or `DynamicDeclarationOrMode`. |
| MGS2 `0x0004f010` | Producer `0x00042850`; reached by effect parts, object attachments and lights. Evaluated volume elements feed shared billboard helper `0x0004f7e0`, admitted per leaf. |
| tdtl `0x000508d0` | Producer `0x000429a0`; reached by attachments/lights. Curve/state updates precede `0x0004f7e0`; they remain guest-owned. |
| Geometry `0x00041020` | Producer `0x00041040`; the transparents stage owns this handler and its `0x0004cbf0`/`0x0004d0b0` callbacks. No particles-stage hook is installed here. |
| Layered geometry `0x00041cc0` / `0x000423c0` | Producers `0x000420a0` / `0x00042760`. Multiple geometry calls and target/layer changes: `OtherLaneLayeredGeometry`. These observations never suppress a body. A single effect reservation does not establish ownership of their complete layered sequence. |
| Material callbacks `0x00244610` / `0x002be650` | Generic producer `0x00041490`, material callback `0x000d4bc0`, geometry callbacks `0x00244470` / `0x002be5d0`. Both reach `0x0019f680`: admits its completed triangle strip, preserving ring/outline construction, material evaluation and degenerates. |
| Other callbacks queued by `0x00041490` | Handler pointer comes from stack +0x18 at entry; data-driven registration through `0x000d4d85` may supply others. `UnownedMaterialCallback` for a handler outside the two researched wrappers. No guessed semantic type or ownership. |
| Wrapper `0x00048e40` | Producer `0x00047870`; sole statically found caller `0x001160b3` supplies callback `0x0024dc50`, a no-op return: `NoGeometryCallback`. The wrapper's material changes still run. A different runtime callback gives `UnownedMaterialCallback`. |
| Four-vertex builder `0x00052020` | Called through `0x00052040` and the existing dynamic-upload adapter; covered without assigning an unsupported effect-family label. |

The material strip's publication/finish pair is
`0x0019fc3e/0x0019fc40`. At publication, saved caller at stack +0x40 must
be `0x002445fa` or `0x002be647`. Require a resolved triangle strip with an
even count of at least four vertices and the same validated immediate
register declaration as billboards. The guest still publishes END and
restores its render state afterwards. Other callers give `UnownedCaller`.

Lens flares use a separate queue: `0x0002dba0` → `0x000209b0`, populated
from sky, object attachments, lights and effect parts. Pass `0x0002dd30`
invokes callback `0x0002e540` through `0x00020bb0` with query work enabled;
its END at `0x0002e9b6` is explicitly `VisibilityQuery`. The queue helper
brackets this callback with z-pass query packets and retains its readback
lifecycle. Reflection pass `0x0002df00` invokes `0x0002e9e0` without those
queries; its visible billboards use the admitted leaf above. Neither queue
nor visibility-history logic is replaced. The central handoff also rejects
query-counted draws before asking admission owners.

Weather is a BSP weather-palette path, not one of these effect sort-list
handlers. The palette is `sbsp+0x84`, stride 0x88, weather datum +0x24;
cluster weather selection is +0x76. The loop around `0x00036e32` updates
weat block +8/+12 (stride 0x3a8) through `0x00037260`, then draws it at
`0x00037a60`. Block +0/+4 (stride 0x8c) updates through `0x00038450` and
draws at `0x000391f0`. All updates, exclusions and cluster selection stay
in the guest. That latter draw selects PointList or LineList from its mode
field (+0x6c); admission uses the actual resolved primitive. Rain/snow
names are not a reliable mode decoder: the repository's Live weather
viewer infers those names from palette/tag labels. Admission therefore covers
rain/snow **line draws**, while any rain/snow **point draws** explicitly
refuse with `WeatherPointRasterization`. It makes no claim that all snow
is one mode or all rain another.

Other named admission refusals are `MissingColorTarget`,
`MissingDepthTarget`, `UnsupportedClip`, `GlyphFiltering`,
`UnresolvedState`, plus renderer `State`, `Shader`, `Texture`, `Target`,
`Scissor`, `Geometry` and `Identity`. Preparation uses the live shader,
textures and state on each copy; it never substitutes an approximate
material. Diagnostics are bounded and are not replacement receipts.

### Walking completed emission inputs

The particle walk tries the direct emitter at
`0x0023ed5c` after the allocator succeeds. The native issue uses the live
declaration and the same carried attributes as the inline quad. A
successful reservation bypasses the emitter's guest packet and preserves
the final default registers. A refused reservation restores the
defaults and the guest emits the quad in place. Behind the render thread
the reservation is queued with the resolved quad as its D3D packet and
succeeds once queued: the render thread records the particle or draws
that packet ([render-thread.md](render-thread.md), "Keeping guest-visible
semantics exact").

The effects walk admits the effect quad at
`0x0004b252` from the builder's source and contrails at `0x0004b28c`
from the builder's output. The return frame and caller arguments must
still describe the same dynamic call. The quad's known builder only
copies the four vertices. Contrails run their original builder before
native issue. The effect leaves and query paths keep their documented
admission and fallback boundaries. The effect quad, contrails and the
guest-issued leaves are queued behind the render thread the same way.

## Rules and limits

- Coverage comes from the `particles` stage counters in `H2_FRAME_STATS` (suppressed, and positive reference and display ranges in view bucket `other`). Resource and range refusals are named counters, not receipts.
- The feature test `crates/rewrites/render-bridge/tests/native_particles.rs` requires twenty-one render-safe observer points and none on `0x00174420`.
- `crates/rewrites/native-render/tests/particles_gpu.rs` draws a canonical particle and a packed ribbon strip into one submission with invented microcode and checks colour readback and refusal of a changed attachment identity. It skips without a GPU adapter. Pure tests cover dynamic register assignment, colour channel order, z/w reset, NaN and signed zero, missing memory, alias and overflow refusal, clip and depth mismatch and malformed geometry; `effect_immediate::` tests cover winding, caller allowlisting, defaults and point and odd-line refusals.

## Not verified

- Pixel parity of colour, depth and particle evolution against D3D over the first 4,000 presents of the Old Mombasa level, including a contrail-bearing projectile and the four-vertex effect builder.
- Combat and explosion sections at render scale 1 and 2 and two views, with a receipt on each copy: sprite blending, near clipping, alpha test, additive particles and D3D-only neighbours.
- Sorted and direct beams, a lens with changing occlusion, a light volume or trail, both material-strip callbacks, and a weather BSP in line and point modes.
- Query results and weather and beam simulation state equal to the D3D baseline; refused point and layered-geometry draws keep their D3D receipts.
- Timings: warm p50 and p95 with diagnostics off.

## Related

[native-renderer.md](native-renderer.md), [native-transparents.md](native-transparents.md), [native-decals-research.md](native-decals-research.md).
