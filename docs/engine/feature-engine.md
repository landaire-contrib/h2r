# Feature engine

Status: Partly built. The registry, the hook chains, the switches, the
bridges, the async entries, the typed views, the settings store and the `ui`
model with our own list class are built. The subsystem facades `loader`,
`camera` and `render`, content packs, the tape and the run fingerprint are
designs ([below](#not-built)).
Code: `crates/hooking/hooks/src` (`lib.rs`, `plan.rs`, `install.rs`, `switches.rs`,
`bridge.rs`, `leaves.rs`, `entry.rs`, `locale.rs`, `arena.rs`,
`snapshot.rs`), `crates/rewrites/engine/src` (`points.rs`, `entry.rs`, `view.rs`,
`testing.rs`), `crates/formats/settings/src`, and `crates/apps/boot/src/registration.rs`.
Tests: `crates/hooking/hooks/tests/plan.rs`, `crates/hooking/hooks/tests/install.rs`,
`crates/apps/boot/tests/feature_plan.rs`, `crates/apps/boot/src/registration/tests.rs`,
`crates/rewrites/engine/tests/views.rs`.
Switch: `H2_FEATURES` (`id`, `-id`, `impl.<name>=on|off`), listed in
[switches.md](../runtime/switches.md).

An opt-in enhancement (a setting, a UI screen or row, a game variant, a
render stage, a Live extra) is a `Feature`. It declares what it attaches to
through a `Builder`: events on the frame and game loops, our strings, or hooks
on named guest points. Nothing is installed while features build. The
registry turns every declaration into one `Plan`: one ordered chain per guest
address, or a list of conflicts naming both parties. One rule must hold: a
guest address has one override, and the plan places it.

Evidence labels: **Code** read from this repository (the path is given),
**Test** shown by a named test, **Design** our choice not yet built.
Nothing on this page was established by running the game.

## How it works

1. Boot builds one `feature::Registry` from `Switches::from_env`
   (`crates/hooking/hooks/src/switches.rs`) and adds every feature in
   `register_core` and `register_late`
   (`crates/apps/boot/src/registration.rs`).
2. `Registry::add` asks the feature's `Manifest` (`id`, `title`,
   `default_on`, `doc`), checks its switch, and runs `Feature::build`
   against a `Builder`. A switched-off feature declares nothing. A feature
   whose `build` fails is left out with the reason.
3. `Registry::build(&dyn ImageAddrs)` calls `plan::build`
   (`crates/hooking/hooks/src/plan.rs`): it resolves every point to an address,
   groups hooks by address, orders each kind, reports conflicts, and
   allocates a shadow entry for every chain that runs something after the
   body.
4. `Plan::install(&Runtime)` (`crates/hooking/hooks/src/install.rs`) maps the
   plan's guest pages and registers one override per address, or refuses the
   whole plan before touching anything.
5. Boot logs the plan: a summary line, or with `H2_FEATURE_PLAN=1` one
   `feature plan:` line per feature and per point (`Plan::text`,
   `Plan::startup_text`). Each hook prints `feature: <POINT> <kind>
   <feature>:<label> active` the first time it runs, under the same
   variable (`feature::plan_verbose`).

### Named points

`engine::points` (`crates/hooking/guest-layout/src/points.rs`) is the one table of named
guest locations. A `Point` has a name, the address in the base (1.0) image,
a `Sig`, a `Class` (`Required` or `Optional`), a `hot` flag and the page that
records the evidence. A `Global` names a guest word or structure read by more
than one module. The `points!` and `globals!` macros declare them:

```text
points! { ALL; NAME @ 0x...: [fn(arg: loc, ..) -> loc ret n] | [opaque], Hot, Optional; "docs/page.md"; }
```

`Sig::Fn` gives where each argument and the result live (`eax`, `ecx`,
`xmm0`, `stack n`, ...) and the bytes the callee pops. `Sig::Opaque` is a
signature nobody has read: observers and `after` hooks only. `Sig::Site`
(built with `Point::new`, not the macro) is an instruction boundary inside a
function, a researched resume point rather than a call. Code elsewhere refers to the name (`points::GAME_TICK.base`),
never to a second copy of the number. A test fails when one address has two
names (`plan::Conflict::TwoNames`).

Addresses and argument locations are research notes. No game data is in the
table.

## Hooks

One registry places every override: implementations, bridges and raw hooks.
A feature that uses `Builder::raw` is a sign that an extension point is
missing.

### Kinds

| Kind | `feature::Kind` | May write guest state | Runs the original | More than one per point |
|---|---|---|---|---|
| observer before / after | `ObserveBefore`, `ObserveAfter` | no by convention (`Obs`: registers, runtime, clock) | always | yes |
| `before` | `Before` | arguments, memory; `Flow::Return` answers in place of the rest | unless it returns | yes, ordered |
| implementation | `Implement` | everything | never | no |
| `after` | `After` | result, memory | already ran | yes, ordered |

Typed access comes from the signature: `Obs::arg_u32(name)`,
`Call::arg_u32(name)`. A `before` hook may answer with an async body
(`Call::answer_async`, see
[Calling code that is still recompiled](#calling-code-that-is-still-recompiled)).
A hook marked `HookRef::context_only` acts only on the registers, stack and
memory of the context it runs on, so a chain made only of such hooks may run
on the pipelined render thread (`Chain::render_thread`,
[pipelined-render.md](../graphics/pipelined-render.md)). A hook may declare
resume sites (`resumes`, `generated_resumes`) for an implementation that
re-enters at base-image return addresses.

### Hook ordering

At one address the chain is, outermost first: observers before, `before`
hooks, the implementation (or the generated original), `after` hooks in
reverse, observers after (`plan::order`, `install::place`). Within a kind,
hooks sort by `Order` (`First`, `Default`, `Last`), then by declared
`after_feature` (`HookRef::after_feature`: this hook sits inside that
feature's), then by feature id. Two mutating hooks (`before` or `after`) of
different features in one `Order` class with no `after_feature` between them
are a conflict, so boot refuses the plan and the game stops at boot
(`crates/apps/boot/src/registration.rs`). A hook on a `hot` point makes the plan
warn: an overridden entry leaves the direct-call fast path; its calls
run in place through the host-call thunk, one runner step each
(`crates/runtime/runtime/src/direct.rs`).

**Test:** `crates/hooking/hooks/tests/plan.rs`
(`unordered_mutating_hooks_are_refused_and_an_order_resolves_them`,
`order_classes_and_after_feature_sort_the_chain`);
`crates/apps/boot/src/registration/tests.rs`
(`the_plan_builds_with_every_combination_of_optional_features`) builds the
plan boot builds, so a clash fails `cargo test`.

### Conflicts

`Registry::build` reports every conflict and resolves none by guessing
(`plan::Conflict`):

- two implementations of one point (`TwoImplementations`);
- two `before` or two `after` hooks with no order (`Unordered`);
- an `after_feature` cycle (`Cycle`);
- a `before` on an opaque point (`BeforeOnOpaque`);
- one address with two point names (`TwoNames`);
- two features with one id (`DuplicateFeature`).

At `install`, an override already on one of the plan's addresses that the
plan did not place refuses the whole plan before anything is mapped or
registered (`InstallError::Occupied`).

### Version key

`Registry::build` takes `&dyn ImageAddrs`: `Base` for the 1.0 image, `Remap`
for synthetic guests. A point that resolves to nothing drops an optional hook
with a note, and turns off the feature of any other hook with the reason. At
install, a point with no generated original turns its features off the same
way. Each point expects a shape at its address (a function start, a call,
a frame prologue; `engine::point_shapes`); install checks it against the
loaded image, and a point that is not what it says turns its features off
with its name ([guest-layout.md](guest-layout.md)). A title update maps the table through `ImageAddrs`
([title-updates.md](../runtime/title-updates.md) section 3.3).
Structure layouts for a title update would go through `engine::view`
([Guest data access](#guest-data-access)); no title-update map exists yet.

### Installing a chain

`install::place` registers one override per address, named
`plan:<POINT>[features]`. When something runs after the body, the chain pops
the caller's return address, calls the body through a shadow entry from the
arena, and a continuation runs the `after` hooks and observers and returns to
the caller. During the body the stack slot holds the continuation token, the
same in every run with the same plan. When the call cannot be set up the body
runs alone.

### Resources

`Builder::guest_alloc(len, perms)` hands out page-granular guest memory from
one reserved range (`crates/hooking/hooks/src/arena.rs`, `ARENA_START`
to `ARENA_END`, inside the kernel's HLE reservation), mapped at install.
Shadow entries and `Builder::entry` pages come from the same range.
`Builder::snapshot(SnapshotPolicy)` says what a feature holds outside guest
RAM; a feature that says nothing is `Unaudited` and blocks machine snapshots
([snapshots.md](../runtime/snapshots.md)).

## Switches

Read from `H2_FEATURES` (`crates/hooking/hooks/src/switches.rs`): a comma-separated
list of `id` (on), `-id` (off) or `id=on|off`. A feature is on unless its
manifest says `off_by_default` or the list switches it. An `impl.<name>`
entry switches an implementation: `Switches::implementation(name, default)`
falls back from `impl.engine.tags.<name>` to `impl.engine.tags`, then
`impl.engine`. The native renderer's one switch, `impl.render.native`, is
looked up exactly (`switches::NATIVE`). Every `impl.*` switch is a row of
`feature::switches::table` (default, owner, age, removal); one not there
does not parse ([guest-layout.md](guest-layout.md)).
Switches are listed once in [switches.md](../runtime/switches.md).

`Switches::spec` writes every switch set, sorted, so the same switches give
the same text however `H2_FEATURES` spelled them (a machine snapshot's build
key).

## Calling code that is still recompiled

A Rust body cannot call a guest function as a plain Rust call: one host
thread drives every guest thread, and a guest call that blocks or is
preempted must unwind to the scheduler. A body that calls guest code is an
`async` block over a `Cx` (`crates/rewrites/engine/src/entry.rs`):

- `cx.call(GuestCall)` awaits a guest call and returns the callee's
  registers; `cx.read` and `cx.write` take guest memory for the closure only,
  so no guard is held across an await and the future is `Send + 'static`.
- The installer drives the future with `ExecCtx::call_guest`, parking it in
  the continuation (`feature::entry`). The stack is put back after every
  call, and the caller's EBX, ESI, EDI, EBP and ESP on return.
- `Builder::entry(label, f)` places a body at a fresh address on the
  feature's executable arena page, for a loader, a vtable slot or a callback
  the guest is handed. `Raw::implement_async` makes one a chain's body.
- `engine::entry::testing::run` drives a body in a test against fake
  callees on a `FakeGuest` and records the calls.
- A future parked across a guest call holds a continuation slot
  (`crates/runtime/runtime/src/continuation.rs`) and blocks whole-machine forks while
  pending, so deeply recursive entries stay sync or stay recompiled.

**Test:** `crates/hooking/hooks/tests/install.rs`
(`an_entry_awaits_guest_calls_and_returns_as_stdcall`,
`an_async_implementation_is_a_chains_body`), and the core's tests in
`crates/rewrites/engine/src/entry.rs`.

## Bridges

`Builder::frame` and `Builder::game` (`crates/hooking/hooks/src/bridge.rs`) give
a feature Rust events without naming an address. Each is a read-only
observer after the guest function returned, so any number share one point:

| Event | Point | When |
|---|---|---|
| `frame().on_tick` | `GAME_TICK` | after each simulated tick |
| `game().on_start` | `GAME_ACTIVATE` | activation returned success (AL non-zero) |
| `game().on_end` | `GAME_END` | the game-end path returned |

Live telemetry, lockstep's tick hash and the level boot's loaded mark
listen on them. **Test:** `crates/hooking/hooks/tests/install.rs`
(`two_tick_observers_both_run_after_the_tick`,
`game_start_reaches_every_listener`).

## Locale sources

`Builder::locale().source(label, f)` adds a string source: `f` maps a packed
id to the guest address of its text. The Rust `locale_string_id_find` asks
the sources first (`locale().serve()`), so with it on our strings need no
hook. When nothing native serves them (`impl.engine.locale=off`, or the
implementation is missing), `Registry::build` adds one `before` on
`LOCALE_FIND` owned by the `locale` bridge
(`crates/hooking/hooks/src/locale.rs`, `plan::place_locale`). The decision is made
after every feature declared. `locale_string_id_exists` does not consult
sources.

**Test:** `crates/hooking/hooks/tests/plan.rs`
(`string_sources_are_served_by_the_rust_lookup_or_one_bridge_hook`,
`the_bridge_is_decided_after_every_feature_declared`).

## Leaves

`engine-leaves` (`crates/hooking/hooks/src/leaves.rs`) places the 22 Rust
implementations in `engine::abi::OVERRIDES` (datum arrays, tag lookup, locale
lookup) as implementations. Each is switched by
`impl.engine.<group>.<name>`, falling back to `impl.engine.<group>` and
`impl.engine`; the groups are `locale`, `tags` and `datums`. Only `locale` is
on by default: the locale point is overridden for our strings anyway, so the
Rust lookup adds no dispatch there, while each datum or tag override takes
one of the hottest functions off the direct-call path. All are context-only
and stateless for snapshots. The differential evidence for the 22 functions
is in [notes/feature-engine.md](notes/feature-engine.md) and
[README.md](README.md); nothing has measured the datum and tag leaves in a
running game.

**Test:** `crates/hooking/hooks/tests/plan.rs`
(`only_the_locale_leaves_are_on_by_default`).

## Guest data access

**One memory trait.** `engine::GuestMemory` is object-safe and
`Result`-returning, implemented for `runtime::Guest` (feature `runtime-glue`)
and for the sparse `engine::testing::FakeGuest` (faults on unmapped access).
Reads from Rust are not charged to the guest's access count. Observers see
the runtime's `Guest` (`Obs::guest`).

**Views.** `engine::record!` declares a fixed-size guest structure: a `Copy`
handle on an address with typed getters and `set_` setters, an uppercase offset
constant per field, and a builder that writes one on a `FakeGuest`. The grammar
is `field @ 0xOFF: Type`; field types are the integers, `f32`, `Vec3` and
`Option<DatumHandle>`. Nothing is read until a getter is called and nothing is
cached, so each offset is a named research point beside the doc it came from
(`crates/hooking/guest-layout/src/view.rs`). Shared game structures (objects, units,
players, game globals) and the globals table are in `crates/hooking/guest-layout`
([guest-layout.md](guest-layout.md)); `record!` takes their offsets by name.

| View | Built on | Used by |
|---|---|---|
| `Datums<T>` | `engine::datum`: `get` is `datum_try_and_get`, `get_absolute` is `datum_get_absolute`, so a view has the game's semantics | telemetry |
| `Tags` | `engine::tags`: `entry`, `get` (`tag_get_checked`, parents included), `scenario` | tag readers |
| `players`, `objects` | `Datums`: `Player` names, `objects::get` giving type and body | telemetry |
| `view::ui` | `Screen`, `ListHandler`, `Item`, `PressRecord`, slot numbers ([ui.md](../formats/ui.md), "Screen and list classes") | our list class |
| `engine::render::light`, `layer` | layouts beside their evidence | the renderer |

**Test:** `crates/rewrites/engine/tests/views.rs` on `FakeGuest`, and
`crates/rewrites/engine/tests/differential.rs`, which checks `Datums` and `Tags`
against the original's answers on real layouts.

## Settings

`crates/formats/settings` depends on `toml` only; `cli`, `app`, `boot`, `d3d` and
`gfx` can all depend on it. A setting is one `Def`
(`crates/formats/settings/src/def.rs`): `key` (`table.key` in `settings.toml`),
`label`, `help`, `kind` (`Bool`, `Int { valid, steps, unit }`, `Choice`),
`default`, `env`, `flag`, `aliases`, `parse_env`, `apply` (`Live`, `NextMap`,
`Restart`), `scope` (`Host`, `View`, `Sim`), `tier` (`User`, `Advanced`,
`Diagnostic`) and `menu`. Boot's definitions are
`crates/extensions/game/src/setting_defs.rs`.

`Store` (`crates/formats/settings/src/store.rs`) is `load`, `handle`, `get`, `set`,
`step`, `state`, `force`, `on_change`, plus per-profile values. Rules:

- Precedence: flag, environment, file, default. A value from a flag or the
  environment, or one the run forces (`Store::force`), is locked: the row
  shows it and a press does nothing.
- `steps` separates what the file may hold from what a row steps through.
- A bad value is a `Problem` naming the key, reported once; the next source
  is used.
- One writer: validate, write the whole file atomically keeping unknown keys,
  then report.
- `Restart` and `NextMap` settings keep running and saved values apart; the
  row shows "(restart)".
- A `Setting<T>` read is one atomic load, so code that holds guest memory can
  read settings. `on_change` runs after the store's lock is released, and the
  `ui` model calls `set` only after dropping guest memory.

| Surface | How |
|---|---|
| In-game menu | `ui::Screen::rows_from_settings` over `Tier::User` defs with a `menu` |
| CLI | `h2 settings list [--all] / get / set`, `h2 play --set key=value` (`crates/apps/cli/src/settings_cmd.rs`) |
| Environment | every `Def` with `env` |

## UI

`crates/extensions/game/src/ui.rs` is the `ui` subsystem's Rust model: our list screens
and the rows we add to retail lists, as data over the settings `Store`. It is
pure: no guest memory, and a press goes only through the store.

- `Screen`: a list of `Row`s; `rows_from_settings` takes every user setting
  whose `Def` names the screen, in its order.
- `Extension`: rows added after a retail list's own (`List::Controls`,
  `List::Settings`, `List::GameSettings`, `List::Screen(id)`).
- `RowKind`: `Setting(key)` (label and value; a press steps it) or
  `Open(screen)`.
- `Model`: `count`, `row`, `texts` (label and value string ids, interned by
  text and served through the locale source) and `press`, which answers a
  `Chosen`: `Stepped`, `Open`, `Push`, `Act`, `Retail` or `Nothing`.
- A `Setting` row needs one string per value it can show; the model
  enumerates them from `Kind`.

A screen opened by the retail loader is a screen class: a loader, a
constructor and a list handler whose row-text and row-chosen functions hold
the screen's behaviour. The generic parts (widget tree, layout, draw list,
transitions, pad navigation) are shared retail code. The retail structures
are read in [ui.md](../formats/ui.md) and
[settings-menu.md](../game/settings-menu.md).

### UI step b

`boot::list_class` (`crates/extensions/game/src/list_class.rs`) is our own list screen
class: a Controls-shaped screen (the Controls tag's layout, built by the
Controls constructor) whose screen and list handler point at our copies of the
Controls vtables. Five entries of ours, async entries at arena addresses,
replace the class's behaviour: loader, row text (list slot 20), row chosen
(the press record's function), pane set-up (screen slot 18) and loader slot
(screen slot 26). The loader does what the Controls loader does (allocate,
construct, push) and adopts the screen between construct and push. Nothing
patches a retail function for our screens, so no "is this list ours" test
exists anywhere.

- A class can carry a description text (`ListClass::description`), a layout of
  its own (`ListClass::layout`, `copy_tag`), an action (`ListClass::action`,
  `Chosen::Act`), one line per row (`ListClass::lines`) and a picture.
- Classes register their loaders by screen id (`list_class::loader_of`); a
  press on an `Open` row calls the screen-request function with the pressing
  player's bit and that loader.
- The rows' datum array is built at the class's count
  (`list_class::array_count`, asked by `layout_glue`'s datum array constructor
  hook).
- The list scrolls past six rows through the Controls list's six item widgets.
- In a game the Controls tag is not loaded (it is in the main menu's map
  only), so the loader runs the constructor's steps on the in-game Controller
  Settings screen's tag instead (`construct_on`, `UI_GAME_KIND`).

Built on it: the Video screen, the Skulls screen (`skulls_screen`), the
campaign picker (`campaign_menu`), the Live sign-in screen (`signin_screen`)
and the co-op lobby, friends and join screens (`coop_live`).
**Test:** the plan tests in `crates/extensions/game/src/list_class.rs`.

### UI step c

Rows of ours on a retail list are data on the plan
(`crates/extensions/game/src/retail_rows.rs`): an `Extension` answers a retail list's
extension rows from the `ui` model with `before` hooks at the list's row text
and row chosen, and a retail row goes on to the original. The Settings list
(the Video row), the Controls list (the speaker row) and Edit Profile
(the Voice Chat row) use it. A list told apart by index
can have ours among its rows (`ui::Extension::at`): a retail row shown past
ours goes to the original with its own row's handle (`retail_rows::moved`).

The retail Controls row text is ported (`crates/extensions/game/src/controls_text.rs`,
feature `controls-text`, an implementation). Every string id is read at run
time from its instruction in the loaded image and named by instruction address;
nothing is copied. Our rows' hooks run around it as they did around the
original. Not ported: the Button Layout list's handlers. The shared list hooks
(the datum array constructor, the pane, the screen tag, the text set), the
layouts, the Live menu's Custom Game Browser row, the added item widgets and
the camera's field of view are still raw hooks in
`crates/extensions/game/src/layout_glue.rs`.

## 7. Rasterizer front-end

The render front-end is replaced by the native renderer
([native-renderer.md](../graphics/native-renderer.md)), whose stages are
features in `crates/rewrites/render-bridge/src/native_*`; its one switch is
`impl.render.native`. The reads of the retail call tree are in
[render-loop.md](../graphics/render-loop.md). `render-timing`
(`crates/rewrites/render-bridge/src/render_timing.rs`, off by default) puts observers on both
sides of the four orchestrators, `render_frame_and_present` and the per-light
pass, and reports the front-end's guest time as the frame stats' `render`
phase with an exclusive split per stage
([gameplay-budget.md](../perf/notes/gameplay-budget.md)).
`guest_layout::points::scene_effects::instruction_entries` lists the retail
instruction boundaries those stages hook mid-function. Discovery for both
image tiers (`recomp::analyze_cached`, used by `h2 build`, `h2 emit` and the
fast tier) makes each one an entry, whatever features the builder has, so
a module built on one machine serves the native stages on another.

## State

A subsystem's state moves through three stages as it is rewritten:

1. **Guest layout.** The state stays in guest memory in the original layout and
   Rust works through views. Always the first stage, and the only one for
   structures that other recompiled code reads.
2. **Mirrored.** Rust owns the state and writes the structures back at the
   boundary.
3. **Rust-owned.** Nothing outside reads it. Only reachable when the readers
   have been rewritten too.

The simulation's rewrite follows these stages ([sim-rewrite.md](sim-rewrite.md)).

## Determinism

Anything on the guest thread changes the run a little even when it writes
nothing: an override costs dispatch transfers, a wrapped call leaves a
continuation token where the return address was (the `lockstep` crate's
header: compare runs that all have it on, or all off), and a Rust entry does
not make the charged accesses the original made, so a perfect port can still
shift where a time slice ends.

Settings carry the class (`settings::Scope`):

| Class | Meaning | Examples |
|---|---|---|
| `Host` | never on the guest thread, never writes guest memory | output resolution, frame rate, vsync, bindings |
| `View` | on the guest thread or writes guest memory, presentation only | UI screens, strings, FOV, HUD position, LOD distance, the tick hash |
| `Sim` | changes what the simulation computes or is given | game variants, the game-start path, button layouts |

- Observers do not write guest memory by convention (`Obs`).
- A snapshot refuses a plan with an `Unaudited` feature
  ([snapshots.md](../runtime/snapshots.md)).
- A per-call differential test is the proof for a port; run-level comparison
  uses the simulation hash.
- Design: a fingerprint of the enabled View and Sim features, the plan and the
  View and Sim setting values, carried by lockstep traces and films, so two
  runs are compared only when it matches. It is not built
  ([live-coop.md](../live/live-coop.md)).

## Rules and limits

- One override per address, placed only by the plan. An address overridden
  elsewhere refuses the plan.
- No game bytes, tables, strings or constants go into a `Def`, a point, a view
  fixture or a test; tests use synthetic guests.
- `before` that returns early skips the rest of the chain, observers after
  included. `after` hooks run inside out.
- Hooks on `hot` points leave the direct-call fast path; the plan warns.
- Observers see the runtime's `Guest`, not `&dyn GuestMemory`.
- `Builder::raw` and `Sig::Opaque` stay first-class; do not add a model field
  to `ui` without a second user.

## Not built

Design only. No type, crate or command named here exists.

- **Subsystem facades.** `loader` (a map patch before the tag table is
  published), `camera` (modifiers and modes), `render` (a view stage and a frame
  observer), and `Subsystem`/`Entry` values that give a subsystem a Port or
  Native grade. Port grade keeps the guest memory and registers of the original
  call for call and is checked by the differential harness; Native keeps only the
  structures the rest of the game reads.
- **Content packs.** Data that changes what the game loads: a manifest naming
  things on the user's own disc with hashes to check it applies, a `ContentKind`
  validated on a copy and then applied, identity by the SHA-256 of the canonical
  JSON form ([forge-design.md](../game/forge-design.md), "The document"). A pack is data, never code, and
  carries no game bytes.
- **Recording and replay.** Every place something from outside reaches the guest
  is a named boundary (pad reads, device changes, network, random, Live answers,
  epoch, a setting change) and the HLE code there asks a tape for the answer:
  off, record or replay. `xapi::tap` does this for pad reads;
  [theater.md](../game/theater.md) lists the rest. A film or lockstep trace is a
  tape plus a header with the image stamp and the fingerprint.
- `h2 features plan` (the text is `Plan::text`, logged at boot),
  `h2 settings doc`, `Toggle::Live` and the `.at(site)` call-site hooks.
- Game options, frame loop, camera, map loading as Rust subsystems. The
  simulation's rewrite is [sim-rewrite.md](sim-rewrite.md).

## Evidence

- **Test.** The differential harness agrees with the lifted original on every
  call of the 22 leaves and on the game path, and the views agree with the
  original: [notes/feature-engine.md](notes/feature-engine.md).
- **Code.** `crates/apps/boot/tests/feature_plan.rs`
  (`the_full_plan_builds_and_reads_as_expected`) pins the plan text for the
  default configuration.

## Not verified

- That a point's signature is right, that a screen looks right and that a
  patched map plays: only a game run shows them.
- Whether any camera path writes memory the simulation reads.
- The cost of the datum and tag leaves and of chains on hot points.
- A list screen opened from one of ours past six rows on a second screen.
- Whether the render view can be rebuilt at a present, which would make render
  scale `Live`.

## Related

- [engine-map.md](engine-map.md): the retail engine by subsystem, with addresses.
- [sim-rewrite.md](sim-rewrite.md): the simulation as Rust subsystems.
- [README.md](README.md): the function contracts behind `engine-leaves`.
- [settings-menu.md](../game/settings-menu.md), [ui.md](../formats/ui.md): the
  settings screens and the retail UI classes.
- [notes/feature-engine.md](notes/feature-engine.md): the evidence.
