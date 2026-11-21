//! Named guest locations: the one table of hook points and globals.
//!
//! Every guest function a feature hooks and every global more than one
//! module reads is named here once, keyed by its address in the base (1.0)
//! image, with the page that records the evidence. Code elsewhere refers to
//! the name (`points::GAME_TICK.base`), never to a second copy of the
//! number. This is the anchor list docs/runtime/title-updates.md section 3.3 asks
//! for; a title update maps these through `ImageAddrs` (docs/engine/feature-engine.md
//! "Version key").
//!
//! No game data here: addresses and argument locations are research notes,
//! and each carries a doc reference.
//!
//! ```text
//! points! {
//!     /// The game tick.
//!     GAME_TICK @ 0x0013_7fe0: [fn() ret 0], hot; "docs/engine/engine-map.md";
//!     /// Not read yet.
//!     SOMETHING @ 0x0012_3450: [opaque]; "docs/x.md";
//! }
//! ```

pub mod render_aspect;

/// Vertical pixels per projection unit in the current retail view.
pub const PIXEL_SCALE: u32 = 0x004b_9ed0;

/// Where one argument or the result lives at the function's entry or exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loc {
    Eax,
    Al,
    Ecx,
    Edx,
    Ebx,
    Esi,
    Edi,
    Xmm0,
    /// The n-th dword argument on the stack, 0-based: `[esp + 4 + 4n]` at
    /// entry (the return address is at `[esp]`).
    Stack(u8),
}

/// A point's calling convention, as far as it has been read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sig {
    /// Not read: observers and after-hooks only, never typed arguments.
    Opaque,
    /// An instruction boundary inside a function (a researched resume
    /// point), not a call: no arguments, and a before-hook may only
    /// continue or resume a researched transfer, never return a value.
    Site,
    Fn {
        args: &'static [(&'static str, Loc)],
        ret: Option<Loc>,
        /// Bytes the callee pops (`ret n`).
        pop: u16,
    },
}

impl Sig {
    pub fn is_opaque(&self) -> bool {
        matches!(self, Sig::Opaque)
    }
    /// The location of the named argument.
    pub fn arg(&self, name: &str) -> Option<Loc> {
        match self {
            Sig::Opaque | Sig::Site => None,
            Sig::Fn { args, .. } => args.iter().find(|(n, _)| *n == name).map(|(_, l)| *l),
        }
    }
}

/// Whether an image without this point can still run the features on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// A hook on it is required: missing, its feature is turned off.
    Required,
    /// A hook on it is dropped with a note when missing.
    Optional,
}

/// Flags the `points!` macro takes after the signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flag {
    /// Called thousands of times a frame: a hook leaves the direct-call fast
    /// path, and the plan warns.
    Hot,
    /// [`Class::Optional`].
    Optional,
}

/// A named guest function.
#[derive(Debug, PartialEq, Eq)]
pub struct Point {
    pub name: &'static str,
    /// Address in the base (1.0) image.
    pub base: u32,
    pub sig: Sig,
    pub class: Class,
    pub hot: bool,
    /// The page that records the evidence.
    pub doc: &'static str,
}

impl Point {
    pub const fn new(name: &'static str, base: u32, sig: Sig, doc: &'static str) -> Self {
        Point {
            name,
            base,
            sig,
            class: Class::Required,
            hot: false,
            doc,
        }
    }
    pub const fn with_flags(mut self, flags: &[Flag]) -> Self {
        let mut i = 0;
        while i < flags.len() {
            match flags[i] {
                Flag::Hot => self.hot = true,
                Flag::Optional => self.class = Class::Optional,
            }
            i += 1;
        }
        self
    }
}

/// Beginning and end of the index build within the sorted dispatcher.
pub const TRANSPARENT_ORDER_BEGIN: Point = Point::new(
    "TRANSPARENT_ORDER_BEGIN", 0x0001_e381, Sig::Site, "docs/graphics/native-renderer.md",
);
pub const TRANSPARENT_ORDER_READY: Point = Point::new(
    "TRANSPARENT_ORDER_READY", 0x0001_e3bc, Sig::Site, "docs/graphics/native-renderer.md",
);

/// Return sites on the retail depleting damage report's kill-credit path.
/// Used to identify that caller's stack layout at PLAYER_KILLED entry.
pub mod kill_credit {
    pub const RETURN_TO_ATTRIBUTION: u32 = 0x001e_a465;
    pub const RETURN_TO_DAMAGE_REPORT: u32 = 0x000d_9acf;
}

/// Return addresses for the retail lobby leave and party leader queries.
pub mod party_leave {
    pub const LOBBY_BACK_COUNT: u32 = 0x0025_11c5;
    pub const PAUSE_LEAVE_COUNT: u32 = 0x0023_29c8;
    pub const LEADER_ROW_LABEL: u32 = 0x002b_8415;
    pub const LEADER_ROW_BUILD: u32 = 0x002b_8459;
    pub const LEADER_ROW_PRESS: u32 = 0x002b_84c0;
    pub const LEADER_AUTO_SELECT: u32 = 0x0019_b4f2;
    pub const LEADER_APPOINT: u32 = 0x0019_a1a0;
}

/// The object section loop's call to the shared record writer.
pub const OBJECT_RECORD_APPEND: Point = Point::new(
    "OBJECT_RECORD_APPEND", 0x0013_41d0, Sig::Site, "docs/graphics/native-renderer.md",
);
/// Return from that call, routed through the generated resume table.
pub const OBJECT_RECORD_APPENDED: u32 = 0x0013_41d5;

/// The geometry submitter's transparent-only append branch and epilogue.
pub const TRANSPARENT_GEOMETRY_ENTRY: Point = Point::new(
    "TRANSPARENT_GEOMETRY_ENTRY", 0x0004_1381, Sig::Site, "docs/graphics/native-renderer.md",
);
pub const TRANSPARENT_GEOMETRY_QUEUED: Point = Point::new(
    "TRANSPARENT_GEOMETRY_QUEUED", 0x0004_1477, Sig::Site, "docs/graphics/native-renderer.md",
);

/// Entry points of the UI render walk. The feature asks image analysis
/// to expose the interior sites before lifting.
pub mod ui_render {
    use super::{Point, Sig};
    pub use super::{
        UI_FADE_DRAW as FADE_DRAW, UI_HUD_DRAW as HUD_DRAW, UI_LOADING_DRAW as LOADING_DRAW,
        UI_OVERLAY_RECT_DRAW as OVERLAY_RECT_DRAW,
    };

    const DOC: &str = "docs/graphics/native-renderer.md";
    pub const BACKDROP_WASH: Point =
        Point::new("UI_BACKDROP_WASH", 0x0025_4b95, Sig::Site, DOC);
    pub const BACKDROP_WASH_TAIL: u32 = 0x0025_4dc0;
    pub const BACKDROP_TINT: Point =
        Point::new("UI_BACKDROP_TINT", 0x0025_4e28, Sig::Site, DOC);
    pub const BACKDROP_TINT_TAIL: u32 = 0x0025_4ee0;
    pub const BACKDROP_COMPOSITE: Point =
        Point::new("UI_BACKDROP_COMPOSITE", 0x0025_5083, Sig::Site, DOC);
    pub const BACKDROP_COMPOSITE_TAIL: u32 = 0x0025_5191;
    pub const MODEL_WIDGET_DRAW: u32 = 0x002b_0baa;
    pub const MODEL_VIEW_CALL: Point =
        Point::new("UI_MODEL_VIEW_CALL", 0x002b_0e12, Sig::Site, DOC);
    pub const MODEL_VIEW_CONTINUATION: u32 = 0x002b_0e17;
    pub const MODEL_VIEW_RETURN: Point =
        Point::new("UI_MODEL_VIEW_RETURN", 0x002b_0e1b, Sig::Site, DOC);
    pub const MODEL_GEOMETRY_CALL: Point =
        Point::new("UI_MODEL_GEOMETRY_CALL", 0x0004_d621, Sig::Site, DOC);
    pub const MODEL_GEOMETRY_RETURN: u32 = 0x0004_d626;
    pub const MODEL_GEOMETRY_EMITTER: u32 = 0x0004_daa0;
    pub const MODEL_TEXTURE_COMMIT: u32 = 0x0001_cf50;
    pub const ITEM_CONTINUATION: u32 = 0x0023_5448;
    pub const OUTLINE_VERTICES: Point =
        Point::new("UI_OUTLINE_VERTICES", 0x0001_ed98, Sig::Site, DOC);
    pub const OUTLINE_VERTICES_TAIL: u32 = 0x0001_ee3a;
    pub const OUTLINE_SUBMIT_RETURN: u32 = 0x0023_5e55;
    pub const LAYERS_RETURN: Point = Point::new("UI_LAYERS_RETURN", 0x0014_7cb9, Sig::Site, DOC);
    pub const ITEM_CALL: Point = Point::new("UI_ITEM_CALL", 0x0023_5445, Sig::Site, DOC);
    pub const ITEM_RETURN: Point = Point::new("UI_ITEM_RETURN", 0x0023_544d, Sig::Site, DOC);
    pub const HUD_RETURN: Point = Point::new("UI_HUD_RETURN", 0x0013_96b9, Sig::Site, DOC);
    pub const HUD_HIDDEN_RETURN: Point =
        Point::new("UI_HUD_HIDDEN_RETURN", 0x0013_96c6, Sig::Site, DOC);
    pub const FADE_RETURN: Point = Point::new("UI_FADE_RETURN", 0x0004_0dbf, Sig::Site, DOC);
    pub const OVERLAY_RECT_RETURN: Point =
        Point::new("UI_OVERLAY_RECT_RETURN", 0x0013_c1db, Sig::Site, DOC);
    pub const LOADING_RETURN: Point = Point::new("UI_LOADING_RETURN", 0x0002_906d, Sig::Site, DOC);
    pub const LOADING_EMPTY_RETURN: Point =
        Point::new("UI_LOADING_EMPTY_RETURN", 0x0002_9075, Sig::Site, DOC);
    pub const LOADING_SPINNER_READY: Point =
        Point::new("UI_LOADING_SPINNER_READY", 0x0002_a1f4, Sig::Site, DOC);
    pub const LOADING_SPINNER_TAIL: u32 = 0x0002_a2dc;
    pub const LOADING_BITMAP_READY: Point =
        Point::new("UI_LOADING_BITMAP_READY", 0x0002_ae60, Sig::Site, DOC);
    pub const LOADING_BITMAP_TAIL: u32 = 0x0002_af87;
    /// Return from the common inline quad uploader to the overlay/bitmap helper.
    pub const RECT_SUBMIT_RETURN: u32 = 0x0005_2ce7;
    pub const SCREEN_QUAD_READY: Point =
        Point::new("UI_SCREEN_QUAD_READY", 0x0003_63c4, Sig::Site, DOC);
    pub const FADE_FAN_READY: Point = Point::new("UI_FADE_FAN_READY", 0x0004_0d86, Sig::Site, DOC);
    pub const FADE_FAN_TAIL: u32 = 0x0004_0db8;
}

/// The named guest globals live in `guest_layout::global` with the
/// structure layouts; they are re-exported here, so `points::GAME_GLOBALS`
/// and `points::GLOBALS` are unchanged.
pub use guest_layout::global::*;

#[doc(hidden)]
#[macro_export]
macro_rules! point_loc {
    (eax) => {
        $crate::points::Loc::Eax
    };
    (al) => {
        $crate::points::Loc::Al
    };
    (ecx) => {
        $crate::points::Loc::Ecx
    };
    (edx) => {
        $crate::points::Loc::Edx
    };
    (ebx) => {
        $crate::points::Loc::Ebx
    };
    (esi) => {
        $crate::points::Loc::Esi
    };
    (edi) => {
        $crate::points::Loc::Edi
    };
    (xmm0) => {
        $crate::points::Loc::Xmm0
    };
    (stack $n:literal) => {
        $crate::points::Loc::Stack($n)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! point_sig {
    (opaque) => {
        $crate::points::Sig::Opaque
    };
    (fn ( $($arg:ident : $loc:ident $($n:literal)?),* ) $(-> $ret:ident)? ret $pop:literal) => {
        $crate::points::Sig::Fn {
            args: &[$((stringify!($arg), $crate::point_loc!($loc $($n)?))),*],
            ret: $crate::point_ret!($($ret)?),
            pop: $pop,
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! point_ret {
    () => {
        None
    };
    ($r:ident) => {
        Some($crate::point_loc!($r))
    };
}

/// Declares named [`Point`]s (see the module docs for the grammar) and adds
/// them to a slice named by the first token (`points! { ALL; ... }`).
#[macro_export]
macro_rules! points {
    ($all:ident; $( $(#[doc = $d:literal])* $name:ident @ $addr:literal : [$($sig:tt)*] $(, $flag:ident)* ; $doc:literal ; )*) => {
        $(
            $(#[doc = $d])*
            pub static $name: $crate::points::Point = $crate::points::Point::new(
                stringify!($name),
                $addr,
                $crate::point_sig!($($sig)*),
                $doc,
            )
            .with_flags(&[$($crate::points::Flag::$flag),*]);
        )*
        /// Every point declared in this table.
        pub static $all: &[&$crate::points::Point] = &[$(&$name),*];
    };
}

points! { ALL;
    /// Length of the vector at EAX. The original returns the float in x87 ST(0).
    VECTOR_LENGTH_3D @ 0x0002_4590: [fn(vector: eax) ret 0], Hot; "docs/halo1/objects.md";
    /// Initialize the damage packet at EDX from the damage effect datum on the stack.
    DAMAGE_DATA_INIT @ 0x000d_6660: [fn(packet: edx, effect: stack 0) ret 4]; "docs/halo1/objects.md";
    SIM_UNIT_POSE @ 0x0011_2070: [fn(unit: stack 0, nodes: stack 1, source: stack 2, target: stack 3) ret 16], Hot; "docs/perf/sim-budget.md";
    SIM_ANIMATION_STREAM @ 0x0027_98a0: [opaque], Hot; "docs/perf/sim-budget.md";
    SIM_GRAPH_RESOLVE @ 0x001d_d9d0: [fn(graph: edx, clip: stack 0) ret 4], Hot; "docs/perf/sim-budget.md";
    SIM_MARKER_TRANSFORMS @ 0x000b_8d30: [fn(out: ebx, object: stack 0, name: stack 1, capacity: stack 2) -> eax ret 12], Hot; "docs/perf/sim-budget.md";
    COLLISION_BSP_SEGMENT @ 0x001d_e6d0: [fn(query: stack 0, node: stack 1, lo: stack 2, hi: stack 3) -> al ret 16], Hot; "docs/perf/sim-budget.md";
    /// BSP vector test: one segment through a collision BSP into the result record.
    COLLISION_VECTOR_TEST @ 0x001d_e630: [fn(out: eax, flags: ecx, bsp: edx, limit: xmm0, breakable_count: stack 0, breakable_mask: stack 1, start: stack 2, delta: stack 3) -> al ret 16], Hot; "docs/engine/collision-queries.md";
    /// The instanced-geometry trace in one cluster.
    COLLISION_INSTANCE_TRACE @ 0x0024_4980: [fn(cluster: eax, start: esi, delta: edi, flags: stack 0, filter: stack 1, out: stack 2) -> al ret 12], Hot; "docs/engine/collision-queries.md";
    /// Location of a point in the structure BSP: leaf, cluster, BSP index.
    COLLISION_LOCATION @ 0x0011_bed0: [fn(point: edx, out: esi) ret 0], Hot; "docs/engine/collision-queries.md";
    SIM_COLLISION_QUERY @ 0x0016_97c0: [fn(flags: stack 0, start: stack 1, delta: stack 2, ignore_a: stack 3, ignore_b: stack 4, out: stack 5) -> al ret 24], Hot; "docs/perf/sim-budget.md";
    BASE_OBJECT_IMPORT @ 0x000b_cc20: [fn(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al ret 16], Hot; "docs/perf/sim-budget.md";
    SIM_MOPP_BOX @ 0x002f_f8b0: [fn(query: ecx, state: stack 0, program: stack 1) ret 8], Hot; "docs/perf/sim-budget.md";
    SIM_MOPP_RAY @ 0x0030_09e0: [fn(query: ecx, state: stack 0, program: stack 1, segment: stack 2) ret 12], Hot; "docs/perf/sim-budget.md";
    OBJECT_TRANSFORM_COMPOSE @ 0x0014_2a60: [fn(parent: ecx, child: edx, out: stack 0) -> eax ret 4], Hot; "docs/perf/sim-budget.md";
    OBJECT_POSE_MATRIX @ 0x0014_21f0: [fn(out: stack 0, pose: stack 1) ret 8], Hot; "docs/perf/sim-budget.md";
    /// Root node transform from two axes, scale and an optional parent.
    OBJECT_ROOT_TRANSFORM @ 0x000b_dc40: [fn(forward: eax, up: ecx, position: stack 0, scale: stack 1, mirror: stack 2, parent: stack 3, out: stack 4) ret 20], Hot; "docs/perf/sim-budget.md";
    ANIM_ROTATION_CODEC @ 0x0028_c530: [fn() ret 0], Hot; "docs/perf/sim-budget.md";
    /// Animation channel codecs the node evaluators call per node.
    ANIM_STATIC_ROTATION @ 0x0028_c470: [fn() ret 0], Hot; "docs/perf/sim-budget.md";
    ANIM_FRAME_ROTATION @ 0x0028_c960: [fn() ret 0], Hot; "docs/perf/sim-budget.md";
    ANIM_BLEND_ROTATION @ 0x0028_c790: [fn() ret 0], Hot; "docs/perf/sim-budget.md";
    ANIM_KEYED_TRANSLATION @ 0x0028_cb70: [fn() ret 0], Hot; "docs/perf/sim-budget.md";
    /// Additive-layer node evaluator: blends two samples per channel into the pose.
    ANIM_ADDITIVE_NODES @ 0x0028_0ac0: [fn() ret 0], Hot; "docs/perf/sim-budget.md";
    COLLISION_POINT_TO_LEAF @ 0x0014_a280: [fn(node: eax, bsp: ecx, point: edx) -> eax ret 0], Hot; "docs/perf/sim-budget.md";
    COLLISION_GEOMETRY_TEST @ 0x0011_e5e0: [fn(origin: eax, center: ecx, direction: edx, radius: stack 0) -> al ret 4], Hot; "docs/perf/sim-budget.md";
    /// The HaloScript evaluator shared by argument-evaluating discarded builtins.
    HS_DISCARDED_EVALUATE @ 0x002a_b6c0: [fn(opcode: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/ai-control.md";
    /// How strongly the observing actor prioritizes one candidate.
    AI_TARGET_SCORE @ 0x0026_5d30: [fn(actor: stack 0, interest: stack 1) -> xmm0 ret 8], Hot; "docs/halo1/ai-control.md";
    /// Finalizes the actor's current interest after examining perception records.
    AI_SELECT_INTEREST @ 0x0026_6640: [fn(actor: stack 0) ret 4], Hot; "docs/halo1/ai-control.md";
    /// The actor's contribution to outgoing damage calculation.
    AI_DAMAGE_MODIFIER @ 0x001c_8440: [fn(actor: ecx, multiplier: edx, packet: stack 0) -> al ret 4]; "docs/halo1/ai-control.md";
    /// Selects the uncover behavior from presearch when a tracked target is available.
    AI_PRESEARCH_UNCOVER @ 0x001b_44d0: [fn(actor: stack 0, state: stack 1) -> eax ret 8], Hot; "docs/halo1/ai-control.md";
    /// Ranks uncover from its own behavior descriptor.
    AI_UNCOVER_RANK @ 0x001b_84a0: [fn(actor: stack 0) -> eax ret 4], Hot; "docs/halo1/ai-control.md";
    AI_UNCOVER_ENTER @ 0x001b_8540: [fn(actor: stack 0, record: stack 1) -> al ret 8]; "docs/halo1/ai-control.md";
    AI_UNCOVER_BEGIN @ 0x001b_85a0: [fn(actor: stack 0, record: stack 1) -> al ret 8]; "docs/halo1/ai-control.md";
    AI_UNCOVER_UPDATE @ 0x001b_89d0: [fn(actor: stack 0, record: stack 1) ret 8]; "docs/halo1/ai-control.md";
    AI_UNCOVER_CONTROL @ 0x001b_8ae0: [fn(actor: stack 0, record: stack 1) ret 8]; "docs/halo1/ai-control.md";
    /// Starts a firing burst, setting the actor timer and angular spread.
    AI_FIRE_BURST @ 0x001f_e620: [fn(actor: eax, firing: ebx, weapon: stack 0) ret 4], Hot; "docs/halo1/ai-control.md";
    /// Chooses the pause between bursts and saves it in the actor timer.
    AI_FIRE_PAUSE @ 0x001f_ed70: [fn(actor: eax, firing: ecx) ret 0], Hot; "docs/halo1/ai-control.md";
    /// Stages the actor's movement, aim and trigger commands for its unit.
    AI_SUBMIT_UNIT_CONTROL @ 0x001e_4390: [fn(actor: eax) ret 0], Hot; "docs/halo1/ai-control.md";
    /// One actor's queued behaviors update: walks the 0x40-stride records
    /// at actor +0x90 through the dispatch table at 0x46eeb8.
    AI_UPDATE_BEHAVIORS @ 0x001a_7cc0: [fn(actor: stack 0) ret 4], Hot; "docs/halo1/ai-control.md";
    HS_UNIT_GRENADE_COUNT @ 0x002a_4310: [fn(opcode: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/ai-control.md";
    /// HaloScript's object awareness evaluator, index 305.
    HS_AI_SEE_OBJECT @ 0x002a_53b0: [fn(opcode: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/ai-control.md";
    HS_CONVERSATION_SPAWN_WAVE @ 0x0020_1520: [fn(count: eax, squad: stack 0, location: stack 1) ret 16]; "docs/halo1/ai.md";
    HS_CONVERSATION_UNIT_NEW @ 0x000c_4410: [fn(unit: stack 0) ret 12]; "docs/halo1/ai.md";
    /// Initializes the unit's speech block from its model and variant name.
    UNIT_DIALOGUE_SET_VARIANT @ 0x0011_4ec0: [fn(unit: eax, variant: stack 0) ret 4]; "docs/halo1/ai.md";
    /// Enqueue the selected candidate in the AI dialogue scheduler.
    AI_DIALOGUE_ENQUEUE @ 0x0020_d0c0: [fn(vocabulary: stack 0, time: stack 1, stimulus: stack 2, candidate: stack 3, context: stack 4, parent: stack 5, queued: stack 6) -> al ret 28]; "docs/halo1/ai.md";
    /// Check whether a unit can play a line and resolve its sound.
    UNIT_DIALOGUE_CHECK @ 0x0011_4520: [fn(unit: eax, vocalization: edx, priority: stack 0, speech_time: stack 1, sound: stack 2) -> al ret 12]; "docs/halo1/ai.md";
    /// Installs the prepared utterance into the per-unit speech block.
    UNIT_DIALOGUE_SET_LINE @ 0x0011_4680: [fn(unit: eax, line: ebx) ret 0]; "docs/halo1/ai.md";
    /// Update the unit's playing and queued speech.
    UNIT_DIALOGUE_UPDATE @ 0x0011_4c60: [fn(unit: eax) ret 0], Hot; "docs/halo1/ai.md";
    /// Clears body depletion and restores a reanimating unit.
    UNIT_REANIMATE @ 0x000c_af00: [fn(unit: eax) ret 0]; "docs/halo1/ai.md";
    /// Reports a nonlethal hit to the victim's and aggressor's AI.
    AI_INJURY_NOTICE @ 0x001c_9c80: [fn(victim: eax, owner: edx, category: stack 0, amount: stack 1, direction: stack 2, deferred: stack 3) ret 16]; "docs/halo1/ai.md";
    /// Reports a depleted unit body and the player or unit responsible.
    AI_KILL_NOTICE @ 0x001c_95d0: [fn(victim: stack 0, owner: stack 1, category: stack 2, amount: stack 3) ret 16]; "docs/halo1/ai.md";
    /// Announces a vocalization event from an actor and its perceived object.
    AI_ACTOR_VOCALIZE @ 0x001f_b7e0: [fn(event: eax, actor: ecx, context: edx, object: stack 0, other: stack 1) -> al ret 8]; "docs/halo1/ai.md";
    /// Notifies the AI when native dismount action 29 has been accepted.
    AI_DISMOUNT_NOTICE @ 0x001b_ba20: [fn(actor: eax) ret 0]; "docs/halo1/ai.md";
    /// Enters the leaping variant of the native melee charge.
    AI_MELEE_LEAP_ENTER @ 0x001a_b380: [fn(actor: stack 0, state: stack 1) -> al ret 8]; "docs/halo1/ai.md";
    /// Dispatches the global vocalization stimulus and its two participants.
    AI_DIALOGUE_STIMULUS @ 0x0020_ba60: [fn(event: stack 0, speaker: stack 1, object: stack 2, relation: stack 3, other: stack 4, context: stack 5) -> al ret 24]; "docs/halo1/ai.md";
    /// Enters native postcombat behavior.
    AI_POSTCOMBAT_ENTER @ 0x001b_4240: [fn(actor: stack 0, state: stack 1) -> al ret 8]; "docs/halo1/ai.md";
    HS_CONVERSATION_MAP_DISPOSE @ 0x0020_9290: [fn() ret 0]; "docs/halo1/ai.md";
    /// `game_tick`: the per-tick phases, then the tick count. Called once per
    /// simulated tick by `game_update_ticks`.
    GAME_TICK @ 0x0013_7fe0: [fn() ret 0]; "docs/engine/engine-map.md";
    /// Choose and reserve an available actor starting location.
    AI_CHOOSE_SQUAD_START @ 0x0020_39d0: [fn(squad: eax, available: stack 0, excluded: stack 1) -> eax ret 8]; "docs/halo1/ai-control.md";
    /// Spawn an actor using the chosen starting location index.
    AI_SPAWN_AT_SQUAD_START @ 0x0020_3780: [fn(context: eax, start: ecx, squad: stack 0, flags: stack 1) -> eax ret 8]; "docs/halo1/ai-control.md";
    AI_SQUADS_ALLOCATE @ 0x0020_0930: [fn() ret 0]; "docs/halo1/ai-control.md";
    AI_SQUADS_INITIALIZE @ 0x0020_0b30: [fn() ret 0]; "docs/halo1/ai.md";
    /// The frame's local tick batch and its elapsed seconds output.
    GAME_UPDATE_TICKS @ 0x0013_7f70: [fn(ticks: eax, elapsed: ebx) ret 0]; "docs/halo1/script-builtins.md";
    /// Per-frame gameplay updates after the local simulation batch.
    GAME_UPDATE_FRAME @ 0x0013_7ed0: [fn(elapsed: stack 0) ret 4]; "docs/halo1/script-builtins.md";
    /// The shared HaloScript evaluator for discarded retail commands.
    OBJECT_FUNCTION_IMPORT @ 0x0010_8d90: [fn(object: stack 0, name: stack 1, value: ebx, active: edi) -> al ret 8]; "docs/halo1/script-builtins.md";
    HS_UNIT_FLAG_SET @ 0x002a_3200: [fn(index: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/script-builtins.md";
    HS_UNIT_FLAG_GET @ 0x002a_3a10: [fn(index: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/script-builtins.md";
    HS_AI_LIST_FLAG_SET @ 0x002a_55f0: [fn(index: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/script-builtins.md";
    OBJECT_FUNCTION_VALUE @ 0x000b_ab40: [fn(object: stack 0, name: stack 1, value: stack 2) -> al ret 12]; "docs/halo1/script-builtins.md";
    HS_SHORT_TO_LONG_EVALUATOR @ 0x002a_b760: [fn(index: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/script-builtins.md";
    /// The shared HaloScript evaluator for discarded retail commands.
    HS_UNUSED_EVALUATOR @ 0x002a_b130: [fn(index: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/script-builtins.md";
    HS_LOOK_ANY_DIRECTION @ 0x002a_9340: [fn(index: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/script-builtins.md";
    HS_MOVE_ANY_DIRECTION @ 0x002a_9370: [fn(index: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/script-builtins.md";
    HS_FALSE_EVALUATOR @ 0x002a_5d50: [fn(index: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/script-builtins.md";
    HS_CONSTANT_LONG_EVALUATOR @ 0x002a_c600: [fn(index: stack 0, thread: stack 1, first: stack 2) ret 12]; "docs/halo1/script-builtins.md";
    /// Game activation after the scenario's cache is loaded: options in EAX,
    /// AL 1 once the map and its first BSP are in.
    GAME_ACTIVATE @ 0x0018_f000: [fn(options: eax) -> al ret 0]; "docs/runtime/level-boot.md";
    CACHE_TAGS_LOAD @ 0x0012_28f0: [opaque], Optional; "docs/engine/engine-map.md";
    CACHE_BSP_FIXUP @ 0x0012_2b40: [opaque], Optional; "docs/engine/engine-map.md";
    TAG_LOOKUP @ 0x0012_2c10: [opaque], Optional; "docs/engine/tag_get_checked.md";
    OBJECTS_PRE_PHYSICS @ 0x000b_6f10: [opaque], Optional; "docs/engine/engine-map.md";
    WORLD_PHYSICS @ 0x001c_3d30: [opaque], Optional; "docs/engine/engine-map.md";
    OBJECTS_POST_PHYSICS @ 0x000b_7060: [opaque], Optional; "docs/engine/engine-map.md";
    FIRST_PERSON_POSE_UPDATE @ 0x0016_760c: [opaque], Hot; "docs/perf/sim-budget.md";
    FIRST_PERSON_FILTER_UPDATE @ 0x0016_6d75: [opaque], Hot; "docs/perf/sim-budget.md";
    LIGHT_RECORD_UPDATE @ 0x000c_0c80: [fn(light: stack 0) ret 4], Hot; "docs/perf/sim-budget.md";
    HAVOK_WELD_CONTACTS @ 0x002f_a2f0: [opaque], Hot; "docs/perf/sim-budget.md";
    HAVOK_SOLVER @ 0x0031_c940: [opaque], Hot; "docs/perf/sim-budget.md";
    HAVOK_SUPPORT_VERTICES @ 0x0030_48a0: [opaque], Hot; "docs/perf/sim-budget.md";
    OBJECT_SPATIAL_VISIT @ 0x000b_b050: [fn(lists: stack 0, types: stack 1, location: stack 2, center: stack 3, radius: stack 4, out: stack 5, capacity: stack 6) ret 28], Hot; "docs/perf/sim-budget.md";
    OBSERVER_RESULT @ 0x0017_1d90: [fn(observer: esi) ret 0], Hot; "docs/perf/sim-budget.md";
    OBJECT_SPATIAL_PORTAL @ 0x0014_a370: [fn(portal: eax, center: ecx, bsp: edx, radius: stack 0) -> al ret 4], Hot; "docs/perf/sim-budget.md";
    OBJECT_CHANGE_COLORS @ 0x000b_e8e0: [fn(object: stack 0) -> al ret 4], Hot; "docs/perf/sim-budget.md";
    OBJECT_PRE_UPDATE @ 0x000b_c470: [fn(object: stack 0) -> al ret 4], Hot; "docs/perf/sim-budget.md";
    OBJECT_POST_UPDATE @ 0x000b_c820: [fn(object: stack 0) ret 4], Hot; "docs/perf/sim-budget.md";
    /// Per-object node matrices: orientation block, root transform, skeleton walk.
    OBJECT_NODE_MATRICES @ 0x000b_d090: [fn(object: stack 0) ret 4], Hot; "docs/perf/sim-budget.md";
    UNIT_AIM_UPDATE @ 0x000c_7840: [opaque], Hot; "docs/perf/sim-budget.md";
    UNIT_UPDATE @ 0x000c_6b30: [fn(unit: stack 0) -> al ret 4], Hot; "docs/perf/sim-budget.md";
    OBJECTS_FINALIZE @ 0x000b_7150: [opaque], Optional; "docs/engine/engine-map.md";
    /// Type update dispatch: slot `+40` of every ancestry level; `al` is the OR of the results.
    OBJECT_TYPE_UPDATE @ 0x0010_8c60: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Type post-physics dispatch: slot `+44` of every ancestry level.
    OBJECT_TYPE_POST_UPDATE @ 0x0010_8cd0: [fn(object: edi) ret 0], Hot; "docs/engine/object-update.md";
    /// Type pose dispatch: slot `+6c` of every ancestry level.
    OBJECT_TYPE_POSE @ 0x0010_9050: [fn(object: ebx, a: stack 0, b: stack 1, c: stack 2) ret 12], Hot; "docs/engine/object-update.md";
    /// Type after-root dispatch: slot `+70`, called with the root node matrix.
    OBJECT_TYPE_AFTER_ROOT @ 0x0010_90d0: [fn(object: ebx, matrix: stack 0) ret 4], Hot; "docs/engine/object-update.md";
    /// Set AWAKE on an object and its parent chain.
    OBJECT_WAKE @ 0x000b_7360: [fn(object: eax) ret 0], Hot; "docs/engine/object-update.md";
    /// The root ancestor of an object.
    OBJECT_ROOT @ 0x000b_af80: [fn(object: ecx) -> eax ret 0], Hot; "docs/engine/object-update.md";
    /// Wake an object (its root ancestor) for the post-physics walk.
    OBJECT_WAKE_FOR_POST @ 0x000b_ba20: [fn(object: eax) ret 0], Hot; "docs/engine/object-update.md";
    /// Wake the bipeds, vehicles and creatures in an object's subtree.
    OBJECT_WAKE_CHILDREN @ 0x000b_c7b0: [fn(object: stack 0) ret 4], Hot; "docs/engine/object-update.md";
    /// Wake what an early mover's sphere touches.
    OBJECT_EARLY_MOVER_WAKE @ 0x0010_9580: [fn(mover: stack 0) ret 4], Hot; "docs/engine/object-update.md";
    // Slice 3 of docs/engine/sim-rewrite-objects.md: function values and change colours.
    /// Evaluate one function export of an object's definition.
    OBJECT_FUNCTION_EVALUATE @ 0x000b_a8c0: [fn(record: eax, object: stack 0, value: stack 1) -> al ret 8], Hot; "docs/engine/object-update.md";
    /// The `unit` level's function import (slot `+4c`).
    UNIT_FUNCTION_IMPORT @ 0x000c_6b90: [fn(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al ret 16], Hot; "docs/engine/object-update.md";
    /// A unit import helper: a fraction from the unit's body `+334`/`+338`.
    UNIT_IMPORT_FRACTION @ 0x000d_1210: [fn(unit: eax) -> xmm0 ret 0], Hot; "docs/engine/object-update.md";
    /// A unit import helper: a value from the unit's animation state.
    UNIT_IMPORT_ANIMATION @ 0x000d_1410: [fn(unit: eax) -> xmm0 ret 0], Hot; "docs/engine/object-update.md";
    /// The `bipd` level's function import (slot `+4c`).
    BIPED_FUNCTION_IMPORT @ 0x000d_e3b0: [fn(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al ret 16], Hot; "docs/engine/object-update.md";
    /// The `vehi` level's function import (slot `+4c`).
    VEHICLE_FUNCTION_IMPORT @ 0x000f_6d10: [fn(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al ret 16], Hot; "docs/engine/object-update.md";
    /// The `weap` level's function import (slot `+4c`).
    WEAPON_FUNCTION_IMPORT @ 0x000f_f5f0: [fn(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al ret 16], Hot; "docs/engine/object-update.md";
    /// The `proj` level's function import (slot `+4c`).
    PROJECTILE_FUNCTION_IMPORT @ 0x000f_be70: [fn(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al ret 16], Hot; "docs/engine/object-update.md";
    /// The `devi` level's function import (slot `+4c`).
    DEVICE_FUNCTION_IMPORT @ 0x0010_6d70: [fn(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al ret 16], Hot; "docs/engine/object-update.md";
    /// The `mach` and `bloc` levels' function import (slot `+4c`).
    MACHINE_FUNCTION_IMPORT @ 0x0011_bd60: [fn(object: stack 0, name: stack 1, value: stack 2, active: stack 3) -> al ret 16], Hot; "docs/engine/object-update.md";
    AI_UPDATE @ 0x001c_83c0: [opaque], Optional; "docs/engine/engine-map.md";
    SCRIPT_UPDATE @ 0x0020_9310: [opaque], Optional; "docs/engine/engine-map.md";
    EFFECT_PART_CREATE @ 0x0017_a8a0: [opaque], Optional; "docs/engine/engine-map.md";
    PARTICLE_SYSTEM_UPDATE @ 0x0024_7140: [opaque], Optional; "docs/engine/engine-map.md";
    /// `animation_resource_get`: EAX a graph's resource record; loads it
    /// through the animation cache when needed and stamps the cache entry's
    /// use (`+0x14`). The tick requests (`0x001236f0`), the render gets.
    ANIMATION_RESOURCE_GET @ 0x0012_37e0: [opaque], Optional, Hot; "docs/engine/engine-map.md";
    /// The particle systems' per-view update the render reaches from the
    /// object and effect draw paths: their visibility and location stamps.
    PARTICLE_VIEW_UPDATE @ 0x0017_4420: [opaque], Optional; "docs/graphics/native-particles.md";
    CAMERA_DIRECTOR @ 0x0015_54b0: [opaque], Optional; "docs/engine/engine-map.md";
    CAMERA_OBSERVER @ 0x0016_f280: [opaque], Optional; "docs/engine/engine-map.md";
    CAMERA_FIRST_PERSON @ 0x0023_c110: [opaque], Optional; "docs/engine/engine-map.md";
    /// The per-frame UI update: one float argument, the elapsed time.
    UI_UPDATE @ 0x0014_79c3: [fn(elapsed: stack 0) ret 4], Optional; "docs/engine/engine-map.md";
    /// Updates the menu music gate and returns true when the attract movie is due.
    UI_ATTRACT_TIMER @ 0x0022_3976: [fn() -> al ret 0]; "docs/audio/bink-hle.md";
    /// Starts the attract movie through the retail Bink wrapper.
    UI_ATTRACT_START @ 0x0022_3a21: [fn() ret 0]; "docs/audio/bink-hle.md";
    /// Closes the finished or dismissed movie and resets the idle clock.
    UI_MOVIE_RETURN @ 0x0015_65a0: [fn() ret 0]; "docs/audio/bink-hle.md";
    UI_RENDER_LAYERS @ 0x0014_7af1: [fn(user: eax, fallback_user: stack 0, bounds: stack 1) ret 8], Optional; "docs/engine/engine-map.md";
    UI_HUD_DRAW @ 0x0019_17eb: [fn() ret 0], Optional; "docs/graphics/native-renderer.md";
    UI_FADE_DRAW @ 0x0004_0890: [fn() ret 0], Optional; "docs/graphics/native-renderer.md";
    UI_OVERLAY_RECT_DRAW @ 0x0013_c050: [fn(bounds: ecx, color: eax) ret 0], Optional; "docs/graphics/native-renderer.md";
    UI_LOADING_DRAW @ 0x0002_89e0: [fn() -> al ret 0], Optional; "docs/graphics/native-renderer.md";
    NETWORK_PRE_UPDATE @ 0x0008_df50: [opaque], Optional; "docs/engine/engine-map.md";
    NETWORK_POST_UPDATE @ 0x0008_dfc0: [opaque], Optional; "docs/engine/engine-map.md";
    /// The game-end path; the end latch is game globals `+0x1128`.
    GAME_END @ 0x0013_89c0: [opaque]; "docs/live/live-gamestate.md";
    /// A player was killed: `(attacker player, victim player, flag, packed
    /// damage)` on the stack. The flag is not a betrayal flag (it is set in
    /// free-for-all too). The low byte of the fourth argument is the
    /// credited damage reporting type and modifier.
    PLAYER_KILLED @ 0x0015_ce70: [opaque]; "docs/live/live-gamestate.md";
    /// One weapon barrel fires: stdcall `(weapon object datum, barrel
    /// index)`. One call is one shot (likely).
    WEAPON_FIRE @ 0x0010_4150: [fn(weapon: stack 0, barrel: stack 1) ret 8], Optional; "docs/live/live-gamestate.md";
    /// Map a packed reporting code to its multiplayer weapon statistics bucket.
    WEAPON_STAT_BUCKET @ 0x0019_6ef0: [fn(code: ecx) -> eax ret 0], Optional; "docs/runtime/sim.md";
    /// Object deletion before the salted header and body are freed.
    OBJECT_DELETE @ 0x000b_8540: [fn(object: stack 0) ret 4], Optional; "docs/game/skulls.md";
    /// Attach a weapon to a unit, or detach when the unit is -1.
    ITEM_SET_UNIT @ 0x0010_ca80: [fn(item: eax, unit: stack 0) ret 4], Optional; "docs/game/bots.md";
    /// Successful power-up application before the consumed equipment is deleted.
    POWERUP_TAKEN @ 0x000a_89c0: [fn(unit: ecx, definition: edx) ret 0], Optional; "docs/game/bots.md";
    /// A damage report is applied to its victim: the report in EDI, the
    /// victim object datum on the stack. Called once per damage instance.
    DAMAGE_REPORT @ 0x000d_9640: [fn(report: edi, victim: stack 0) ret 4], Optional; "docs/live/live-gamestate.md";
    /// A damage instance's shield step (`0xd7b80` calls it once, at
    /// `0xd80de`): stdcall `(object datum, ?, damage data, record)`. It
    /// takes the shield's share of the amount at `record+0x1c` (divided by
    /// the object's shield vitality) and leaves what is left there.
    SHIELD_DAMAGE @ 0x000d_9110: [fn(object: stack 0, record: stack 3) ret 16], Optional; "docs/game/game-options.md";
    /// A damage instance's body step (`0xd7b80` calls it once, at
    /// `0xd8184`): the record in EDI, stdcall `(object datum, ?, damage
    /// data)`. Reads the amount at `record+0x1c` through `0xd9020`.
    BODY_DAMAGE @ 0x000d_8cb0: [fn(record: edi, object: stack 0) ret 12], Optional; "docs/game/game-options.md";
    /// Wheel and tread controllers after the driver inputs have been read.
    VEHICLE_TREAD_UPDATE @ 0x000f_1cd0: [fn(object: ebx, contacts: stack 0) ret 4]; "docs/halo1/role-audit.md";
    UNIT_CONTROL_REFRESH @ 0x000c_e0c0: [fn(object: edi) ret 0]; "docs/halo1/role-audit.md";
    VEHICLE_GRAVITY_DELTA @ 0x000f_7ed0: [fn(object: eax, delta: esi) ret 0]; "docs/halo1/role-audit.md";
    VEHICLE_CONTACT_FORCES @ 0x0020_6090: [fn(contacts: stack 0, force: stack 1, torque: stack 2) -> al ret 12]; "docs/halo1/role-audit.md";
    VEHICLE_CONTACT_SOLVE @ 0x0020_56e0: [fn(object: stack 0, contacts: stack 1, fraction: stack 2, force: stack 3, torque: stack 4) ret 20]; "docs/halo1/role-audit.md";
    VEHICLE_WHEEL_UPDATE @ 0x000f_1f80: [fn(object: stack 0, contacts: stack 1) ret 8]; "docs/halo1/role-audit.md";
    /// An object's damage update: stdcall `(object datum)`, AL whether
    /// anything changed. Called once per object per tick by
    /// `object_update` (`0xbc470`, its only caller): stun timers, shield
    /// recharge, and the body's regeneration (`0xd63de`..`0xd64b5`).
    OBJECT_DAMAGE_UPDATE @ 0x000d_5de0: [fn(object: stack 0) -> al ret 4]; "docs/halo1/health.md";
    /// Update one effect's native particle systems for this event step.
    EFFECT_STEP_EMITTERS @ 0x0017_9020: [fn(effect: stack 0, elapsed: stack 1, dt: stack 2) ret 12]; "docs/halo1/objects.md";
    /// Dispatch one native movement operator over an emitter's particle list.
    PARTICLE_MOVEMENT_STAGE @ 0x002b_8da0: [fn(stage: eax, head: esi, dt: stack 1) ret 12]; "docs/halo1/objects.md";
    /// Cloth geometry update, per-node integration and material lookup.
    CLOTH_UPDATE @ 0x0011_68a0: [fn() ret 0]; "docs/halo1/objects.md";
    CLOTH_INTEGRATE @ 0x0011_8140: [fn(cloth: eax) ret 0]; "docs/halo1/objects.md";
    CLOTH_SHADER @ 0x0011_6ac0: [fn(cloth: stack 0) -> eax ret 4]; "docs/halo1/objects.md";
    /// Queue an attachment endpoint with its animated marker transform (0x41bb3).
    LIGHTNING_QUEUE @ 0x0004_29a0: [fn(owner: stack 0, tag: stack 1) ret 24]; "docs/halo1/objects.md";
    /// Submit 0x18-byte light-volume samples (0x4f792).
    LIGHT_VOLUME_SPRITES @ 0x0004_f7e0: [fn(bitmap: ecx, samples: stack 0, count: stack 1) -> al ret 32]; "docs/halo1/objects.md";
    /// Initialise a particle's local position and velocity at birth.
    PARTICLE_INITIALISE @ 0x0024_9170: [fn(emitter: stack 0, particle: edi) ret 8]; "docs/halo1/objects.md";
    /// Begin, update and remove a command on the Halo 2 AI scheduler.
    AI_COMMAND_BEGIN @ 0x0025_8cf0: [fn(actor: stack 0, unit: stack 1, state: stack 2, command: stack 3) -> eax ret 16]; "docs/halo1/ai.md";
    AI_COMMAND_UPDATE @ 0x0025_9430: [fn(actor: stack 0, unit: stack 1, state: stack 2, command: stack 3) -> eax ret 16]; "docs/halo1/ai.md";
    AI_COMMAND_REMOVE @ 0x0025_8540: [fn(actor: ecx, command: stack 0) ret 4]; "docs/halo1/ai.md";
    OBJECT_ANIMATION_BLEND @ 0x000b_a350: [fn(unit: esi, duration: stack 0) ret 4]; "docs/halo1/ai.md";
    /// Finish the character physics mode's velocity calculation.
    CHARACTER_MOTION_STATE @ 0x001e_5b50: [fn(input: eax, state: ecx, output: esi) ret 0]; "docs/halo1/ai.md";
    AI_COMMAND_TARGETS @ 0x0025_a380: [fn(actor: stack 0) ret 4]; "docs/halo1/ai.md";
    /// Clear HS threads and instantiate scenario globals and persistent scripts.
    HS_NEW_MAP @ 0x0020_90a0: [fn() ret 0]; "docs/halo1/scripts.md";
    /// Attach the new scenario's syntax nodes (`0x16b6b0`) and strings,
    /// then tail-jump to `HS_NEW_MAP`.
    HS_MAP_ATTACH @ 0x0020_a750: [fn() ret 0]; "docs/halo1/scripts.md";

    /// HS global write-back notification (EAX is the variable index).
    HS_GLOBAL_STORED @ 0x0020_a490: [fn(index: eax) ret 0]; "docs/halo1/ai.md";
    /// Restore an object's body to full (EAX the object datum), AL 1 when
    /// it did (refused when full or depleted). Its only caller is the
    /// powerup pickup's health case (`0x151f73`).
    BODY_RESTORE @ 0x000d_6790: [fn(object: eax) -> al ret 0]; "docs/halo1/health.md";
    /// `locale_string_id_find`: the string for the packed id in EDI, its
    /// address in EAX.
    LOCALE_FIND @ 0x001a_02d0: [fn(id: edi) -> eax ret 0], Hot; "docs/engine/locale_string_id_find.md";
    /// `locale_string_id_exists`: whether the packed id has a string (AL).
    LOCALE_EXISTS @ 0x001a_0310: [opaque]; "docs/engine/locale_string_id_find.md";
    /// `ice_cream_flavor_stock`'s worker: turns skull ESI on (byte ESI of
    /// the 15-byte array at `0x4f55dc`, only when it was 0), then the HUD
    /// message and sound.
    SKULL_STOCK @ 0x001d_f3e0: [fn(index: esi) ret 0]; "docs/game/skulls.md";
    /// Queue new game options (EAX = the options; `+0` the mode, 1 a
    /// campaign). The menu's campaign start and level boot both end here.
    OPTIONS_QUEUE @ 0x0018_e790: [fn(options: eax) ret 0]; "docs/game/skulls.md";
    /// `render_frame_and_present`: renders the game views (or the fallback
    /// screen) and presents. Once a frame, from the main loop.
    RENDER_FRAME @ 0x0022_3240: [fn() ret 0]; "docs/engine/engine-map.md";
    /// `render_game_views`: builds the per-player view records and cameras,
    /// then calls `render_views`.
    RENDER_GAME_VIEWS @ 0x0022_34e0: [opaque]; "docs/engine/engine-map.md";
    /// `render_views`: begin-frame setup, the loop over view records, end of
    /// drawing.
    RENDER_VIEWS @ 0x0002_b5d0: [fn(count: eax, context: ecx, arg0: stack 0, arg1: stack 1, views: stack 2) ret 12]; "docs/engine/engine-map.md";
    /// `render_game_view`: one game view (camera, render state, scene
    /// passes, UI composition).
    RENDER_GAME_VIEW @ 0x0002_b790: [fn(view: stack 0, flags: stack 1) ret 8]; "docs/engine/engine-map.md";
    /// The alternate view path selected by the render view record.
    RENDER_ALTERNATE_VIEW @ 0x0002_c790: [opaque]; "docs/graphics/render-loop.md";
    /// End-of-frame rendering after the view loop, before the final blit.
    RENDER_FRAME_TAIL @ 0x0001_3b90: [opaque]; "docs/graphics/native-renderer.md";
    /// `render_scene_passes`: the scene pass sequence of one view.
    RENDER_SCENE_PASSES @ 0x0002_bcd0: [opaque]; "docs/graphics/e3-lighting.md";
    /// Resets the record counters immediately before the two structure walks.
    RESET_SCENE_RECORDS @ 0x0002_cb30: [fn() ret 0]; "docs/graphics/occlusion-culling.md";
    /// Main view's visible structure cluster and placement record walks.
    WORLD_CLUSTER_RECORDS @ 0x0013_3390: [fn(visibility: ebx) ret 0]; "docs/graphics/native-renderer.md";
    /// One visible row entering the gather. A cold neighbour can require
    /// the surrounding walk to stay in the guest.
    WORLD_VISIBILITY_FLAGS @ 0x0013_32f0: [fn(visibility: stack 0) -> eax ret 4]; "docs/graphics/native-renderer.md";
    WORLD_PLACEMENT_RECORDS @ 0x0013_3520: [fn(visibility: stack 0) ret 4]; "docs/graphics/native-renderer.md";
    /// Sets the render stencil mode, including after the scene passes return.
    RENDER_STENCIL_MODE @ 0x0001_5370: [opaque]; "docs/graphics/native-renderer.md";
    /// The additional shadow-caster pass (layer 10/11 pair after the main lights).
    ADDITIONAL_SHADOW_CASTERS @ 0x0003_0ed0: [opaque]; "docs/graphics/native-shadows.md";
    /// Target selection helper. The light tail restores its view target here
    /// after the last geometry, including on a light with no active layer.
    LIGHT_RESTORE_TARGET @ 0x0001_4bc0: [opaque]; "docs/graphics/native-renderer.md";
    /// The scene call that walks the object shadow list.
    OBJECT_SHADOW_LIST @ 0x0003_35e0: [opaque]; "docs/graphics/native-shadows.md";
    /// Gathers the object shadow candidates.
    GATHER_OBJECT_SHADOWS @ 0x0003_3400: [opaque]; "docs/graphics/native-shadows.md";
    /// One object's shadow (caster and receiver layers).
    OBJECT_SHADOW @ 0x0003_2350: [opaque]; "docs/graphics/native-shadows.md";
    /// One dynamic decal callback: `0x00163cd0` calls it in stored list order
    /// after `0x00043a40` selects the mode and its state. Stack `(list, unused)`,
    /// `ret 8`; the only native decal replacement point.
    DECAL_RENDER @ 0x0004_4040: [fn(list: stack 0, unused: stack 1) ret 8]; "docs/graphics/native-decals-research.md";
    /// `render_dynamic_lights`: the visible lights, each gathered and drawn
    /// with `RENDER_LIGHT_PASSES` (`render_scene_passes` calls it after
    /// layer 7).
    RENDER_DYNAMIC_LIGHTS @ 0x0003_1c80: [opaque]; "docs/graphics/native-renderer.md";
    /// `render_light_passes`: draws one dynamic light's passes (layers 10,
    /// 11, 14, 12, 13 on the light's list). EAX the `ligh` datum, ECX a
    /// context, stack `(light datum, &position, shadow byte, weight)`,
    /// `ret 0x10`; its only caller is `RENDER_DYNAMIC_LIGHTS`.
    RENDER_LIGHT_PASSES @ 0x0003_1170: [fn(definition: eax, context: ecx, light: stack 0, position: stack 1, shadow: stack 2, weight: stack 3) ret 16]; "docs/graphics/e3-lighting.md";
    /// `render_screen_effects`: the view's post passes, the last call of
    /// `render_scene_passes` (`0x0002c322`, after layer 17 and
    /// `0x00020770`): the colour pass over the view's target (`0x000214f0`,
    /// when `colour` is set), the scenario's screen effects (`0x0002d140`
    /// per entry) and `0x0002df00` (when `late` is set). Stack `(late byte,
    /// colour byte)`, `ret 8`.
    RENDER_SCREEN_EFFECTS @ 0x0001_3870: [fn(late: stack 0, colour: stack 1) ret 8]; "docs/graphics/native-renderer.md";
    /// `render_screen_effects`' colour pass, a glow (`0x000214f0`): the
    /// view's target down-sampled into target 7, blurred between 7 and 8
    /// (`0x000211a0`) and blended back. Stack: the seven words
    /// `0x000138b3`..`0x000139b5` push (`engine::colour_pass::Arguments`),
    /// `ret 0x1c`.
    COLOUR_PASS @ 0x0002_14f0: [fn(a0: stack 0, a1: stack 1, a2: stack 2, a3: stack 3, a4: stack 4, a5: stack 5, a6: stack 6) ret 28]; "docs/graphics/native-renderer.md";
    /// One of the scenario's screen effects (`0x0002d140`, from the loop of
    /// `render_screen_effects`): EAX the `egor` datum, stack `(&values,
    /// columns, rows, &corners)` (`engine::screen_effect::plan`), `ret
    /// 0x10`.
    SCREEN_EFFECT @ 0x0002_d140: [fn(datum: eax, values: stack 0, columns: stack 1, rows: stack 2, corners: stack 3) ret 16]; "docs/graphics/native-renderer.md";
    /// Screen convolution blur, also called by the colour pass. From
    /// 0x0002da96, stack arguments 0 and 1 are the steps and tap spread.
    SCREEN_CONVOLUTION_BLUR @ 0x0002_11a0: [fn(source: eax, steps: stack 0, spread: stack 1, growth: stack 2, dest: stack 3, other: stack 4, last: stack 5, combine: stack 6, arg7: stack 7, arg8: stack 8, arg9: stack 9) -> eax ret 40]; "docs/graphics/native-renderer.md";
    /// A unit's integrated-light update, called at the end of its tick.
    UNIT_LIGHT_UPDATE @ 0x000c_50a0: [fn(unit: eax) ret 0]; "docs/halo1/hud.md";
    /// A shader template's pass set up (`0x00016610`, called by
    /// `0x0002d140` at `0x0002d4b7` per pass): stack `(shader, layer,
    /// implementation, selector, target, 0)`, AL whether the pass draws
    /// (`engine::screen_effect::template_draws`; `0x0002d4be` skips the
    /// pass when it does not), `ret 0x18`. Post trace and native gels hook it.
    TEMPLATE_PASS @ 0x0001_6610: [fn(shader: stack 0, layer: stack 1, implementation: stack 2, selector: stack 3, target: stack 4, flags: stack 5) -> al ret 24], Optional; "docs/graphics/native-renderer.md";
    /// Full-target template quad; its gel caller returns to 0x1675a.
    TEMPLATE_QUAD @ 0x0001_5ec0: [fn(target: stack 0) ret 4], Optional; "docs/graphics/native-gels.md";
    /// A batch of screen quads begins (`0x00035b90`): stdcall `(record)`,
    /// `ret 4`. The record (`native_render::hud::Record`) names the
    /// bitmaps, address modes, filter, blend mode and colours; the begin
    /// sets that state and the combiner. Text (`0x0001f070`) is drawn this
    /// way, one quad per glyph.
    SCREEN_QUADS_BEGIN @ 0x0003_5b90: [fn(record: stack 0) ret 4]; "docs/graphics/native-renderer.md";
    /// One screen quad (`0x000363a0`): stdcall `(&corners)`, four corners
    /// of `0x14` bytes (x, y, u, v, colour), pushed as one inline
    /// `Begin(7)`/`End` with no D3D entry. `ret 4`.
    SCREEN_QUAD @ 0x0003_63a0: [fn(corners: stack 0) ret 4], Hot; "docs/graphics/native-renderer.md";
    /// The batch's end (`0x00036560`): no arguments, `ret`.
    SCREEN_QUADS_END @ 0x0003_6560: [fn() ret 0]; "docs/graphics/native-renderer.md";
    /// One HUD shader quad (`0x0022a664`): ECX the four corners, EAX the
    /// coordinate rectangle, stack `(bitmap datum, bitmap index, shader)`,
    /// `ret 0xc`. A shader draw (`0x0001bbf0`) and one
    /// `D3DDevice_Begin(7)`..`End`.
    HUD_SHADER_QUAD @ 0x0022_a664: [fn(corners: ecx, rect: eax, bitmap: stack 0, index: stack 1, shader: stack 2) ret 12], Hot; "docs/graphics/native-renderer.md";
    /// Draws the current layer (`[0x00467130]`) of the current list: layer
    /// setup, then `LAYER_ENTRIES` for the layers in its mask.
    LAYER_DRAW @ 0x0004_4550: [opaque]; "docs/graphics/layer-draw.md";
    /// The layer's end: every `LAYER_DRAW` call in `RENDER_LIGHT_PASSES`
    /// is followed by one; it ends the layer's state and sets the current
    /// layer (`[0x00467130]`) to 0.
    LAYER_END @ 0x0004_45d0: [opaque]; "docs/graphics/native-renderer.md";
    /// Resolve one material for the record-to-scene-item walk. The main
    /// caller tests AL before advancing its part cursor in EDI.
    WORLD_PART_MATERIAL @ 0x0004_5ce0: [fn(record: ecx, scratch: eax, part: stack 0, region: stack 1) -> al ret 8], Hot; "docs/graphics/native-renderer.md";
    /// The standard geometry item's build callback. AL admits its sort slot.
    WORLD_GEOMETRY_BUILD @ 0x0004_cbf0: [fn(shader: stack 0, arg: stack 1, lod: stack 2, layer: stack 3, selector: stack 4, part: stack 5, entry: stack 6) -> al ret 28], Hot; "docs/graphics/layer-draw.md";
    /// BSP geometry emitter. AL selects masked subparts, CL selects strips.
    WORLD_PART_EMIT @ 0x0004_daa0: [fn(section: stack 0, part: stack 1, bitmap: stack 2) ret 12], Hot; "docs/graphics/native-renderer.md";
    /// LAYER_ENTRIES calls the item's geometry handler with seven stack words.
    /// Its continuation reloads EAX from the loop counter before using it.
    LIGHT_GEOMETRY_DRAW @ 0x0004_d0b0: [fn(shader: stack 0, arg: stack 1, variant: stack 2, layer: stack 3, selector: stack 4, part: stack 5, entry: stack 6) ret 28], Hot; "docs/graphics/layer-draw.md";
    /// The layer draw's setup call (ESI = `0x00485b48`).
    LAYER_SETUP @ 0x0001_6b10: [opaque]; "docs/graphics/layer-draw.md";
    /// Draws a list's entries for a layer: builds them, then calls each
    /// entry type's handler. EDI the list, EBX the layer, stack 0 an
    /// argument (`[0x00467134]`), `ret 4`.
    LAYER_ENTRIES @ 0x0003_bc30: [fn(list: edi, layer: ebx, arg: stack 0) ret 4]; "docs/graphics/layer-draw.md";
    /// Builds a layer's entries (0x18 bytes each) into the caller's buffer;
    /// the count in EAX. Stdcall `(list, arg, layer, buffer)`.
    LAYER_BUILD @ 0x0003_bab0: [fn(list: stack 0, arg: stack 1, layer: stack 2, buffer: stack 3) -> eax ret 16]; "docs/graphics/layer-draw.md";
    /// The engine's qsort: ECX the count, EAX the base, stack `(size,
    /// compare, context)`, `ret 0xc`. The layer build sorts its entries
    /// with it (comparators `0x0003bb80`, `0x0003bbd0` for layer 5).
    LAYER_SORT @ 0x0013_da70: [opaque]; "docs/graphics/layer-draw.md";
    /// The engine's index sort (u16 elements): EAX the count, ECX the base,
    /// stack `(unused, compare, context)`, `ret 0xc`. The transparents'
    /// draw order sorts with it (comparator `0x0001e700`).
    TRANSPARENT_SORT @ 0x0013_df80: [opaque]; "docs/graphics/render-loop.md";
    /// The shader draw's render-state list: EBX the draw context, stack 0
    /// the list word's address (`count << 9 | first`), `ret 4`. Applies
    /// each entry through `SetRenderState_Simple`, the D3D shadow and
    /// dirty bits, or a per-state setter.
    RENDER_STATES @ 0x0001_a170: [fn(context: ebx, list: stack 0) ret 4]; "docs/graphics/render-loop.md";
    /// Populate visibility lists from the current view's cluster references.
    SCENE_CLUSTER_GATHER @ 0x0013_2220: [fn(vis: stack 0) ret 4]; "docs/graphics/native-renderer.md";
    /// Publish clusters and their instanced geometry in visibility order.
    SCENE_GEOMETRY_GATHER @ 0x0013_4390: [fn(vis: stack 0) ret 4]; "docs/graphics/native-renderer.md";
    /// Move objects with the model-link property to the front of the list.
    SCENE_OBJECT_ORDER @ 0x0013_4240: [fn(vis: stack 0) ret 4]; "docs/graphics/native-renderer.md";
    /// Collect, rank and fade the current view's dynamic lights.
    VISIBLE_LIGHT_LIST @ 0x0003_1910: [fn(output: stack 0) -> eax ret 4]; "docs/graphics/native-renderer.md";
    /// Mark caster and receiver flags on a light's visibility lists.
    LIGHT_GEOMETRY_FLAGS @ 0x0003_1660: [fn(light: eax, definition: ecx, vis: esi, owner: stack 0, root: stack 1, point: stack 2, shadow: stack 3) ret 16]; "docs/graphics/native-renderer.md";
    /// Append the optional owner or owning unit as a dynamic light caster.
    LIGHT_EXTRA_CASTER @ 0x0003_1c20: [fn(shadow: al, owner: ecx, vis: stack 0) ret 4]; "docs/graphics/native-renderer.md";
    /// Select a model detail tier. The scaled camera distance is in the low
    /// float of XMM4. The return value and fraction drive the per-view hysteresis.
    MODEL_LOD @ 0x0004_c450: [fn(distances: ecx, fraction: ebx, object: stack 0) -> al ret 4]; "docs/graphics/occlusion-culling.md";
    /// Write one BSP or model render record, with its associated cache state.
    SCENE_RECORD_WRITER @ 0x0002_cbf0: [fn(section: eax, selector: ecx, node_count: edx, pixels: stack 9) -> eax ret 84]; "docs/graphics/native-renderer.md";
    /// The object render records for a visibility struct: EAX the struct
    /// (`0x00547f88` the main view's), stack 0 an argument passed on,
    /// `ret 4`. Reorders the struct's object list (`0x00134240`), then
    /// calls `OBJECT_RENDER_SETUP` per listed object.
    OBJECT_RENDER_RECORDS @ 0x0013_3720: [fn(vis: eax, arg: stack 0) ret 4]; "docs/graphics/occlusion-culling.md";
    /// One listed object's render setup (view depth, LOD, nodes, children,
    /// the `0x004ba138` records): ESI the visibility struct, stack
    /// `(list index, byte, byte*, -1, arg)`, AL a retry flag, `ret 0x14`.
    OBJECT_RENDER_SETUP @ 0x0013_3790: [fn(vis: esi, index: stack 0) -> al ret 20]; "docs/graphics/occlusion-culling.md";
    /// The sorted transparents of a scene pass (`render_scene_passes`
    /// calls it after the opaque layers and before the first-person
    /// chain).
    SCENE_TRANSPARENTS @ 0x0001_e370: [opaque]; "docs/graphics/occlusion-culling.md";
    /// Queue an effect callback with inline payload and view depth.
    TRANSPARENT_EFFECT_ENTRY @ 0x0001_e4e0: [fn(size: ecx, position: edx, handler: stack 0, payload: stack 1) -> al ret 8]; "docs/graphics/native-renderer.md";
    /// Queue the polygon effect callback with seven extra words.
    TRANSPARENT_POLYGON_ENTRY @ 0x0001_e5e0: [fn(size: ecx, position: edx) -> al ret 32]; "docs/graphics/native-renderer.md";
    /// A sorted transparent geometry entry's handler: the transparents call
    /// it through the entry's `+0x20` with the entry's `+0x24` (shader
    /// datum, then the render record argument), stdcall, `ret 4`. Runs the
    /// opaque path's build and draw over the template's layer-15 passes.
    TRANSPARENT_GEOMETRY @ 0x0004_1020: [fn(data: stack 0) ret 4]; "docs/graphics/native-renderer.md";
    /// Shader draw begin: per draw.
    SHADER_DRAW_BEGIN @ 0x0001_bbf0: [fn(shader: edi, mode: stack 0, pass: stack 1, flags: stack 2, index: stack 3, scale: stack 4) ret 20], Hot; "docs/formats/shader-instances.md";
    /// `submit_structure_or_model_geometry`: per draw.
    GEOMETRY_SUBMIT @ 0x0004_1040: [opaque], Hot; "docs/formats/structure-bsp.md";
    /// A shader parameter's value for this draw: its sources through
    /// `[0x00485a58]`, the CRT `fmod`, then `FUNCTION_EVALUATE`.
    SHADER_PARAMETER @ 0x0001_b230: [opaque], Hot; "docs/graphics/layer-draw.md";
    /// A texture binding's source by type (a jump table).
    TEXTURE_SOURCE @ 0x0001_9530: [opaque], Hot; "docs/graphics/layer-draw.md";
    /// An animated bitmap's frame for a texture binding.
    BITMAP_FRAME @ 0x0001_43c0: [opaque], Hot; "docs/graphics/layer-draw.md";
    /// A tag function's value (two inputs on the stack).
    FUNCTION_EVALUATE @ 0x0013_b390: [opaque], Hot; "docs/graphics/layer-draw.md";
    /// The CRT's x87 `fmod` (`_CIfmod`: ST1 mod ST0), through the CRT's
    /// transcendental dispatcher.
    CRT_FMOD @ 0x0037_3b6a: [opaque], Hot; "docs/graphics/layer-draw.md";
    /// The top-level Settings list's row text (thiscall on the list, the
    /// item and a word).
    SETTINGS_ROW_TEXT @ 0x002b_1d6e: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The top-level Settings list's row chosen (thiscall on the list, the
    /// player chain and a pointer to the row).
    SETTINGS_ROW_CHOSEN @ 0x002b_1da9: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The campaign pause menu's row text (screen 18, `dialog_pause`):
    /// switches on the item's row, 0 to 4; any other row gets the null id.
    PAUSE_ROW_TEXT @ 0x0023_1f6c: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The campaign pause menu's row chosen: rows 0 to 3 have their own
    /// press, and every other row runs row 4's.
    PAUSE_ROW_CHOSEN @ 0x0023_1fe3: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The in-game Controller Settings loader (screen 200).
    PAUSE_CONTROLS_LOADER @ 0x0023_1db5: [fn(request: stack 0) -> eax ret 4]; "docs/game/settings-menu.md";
    /// The multiplayer pause menu's list handler constructor (screen 195,
    /// `mp_menu`): ESI the handler, one stack argument. Makes the rows'
    /// datum array (room for six) and adds a datum per row it shows, the
    /// row's action word at datum `+2`.
    MP_MENU_LIST_CTOR @ 0x0023_2708: [fn(list: esi, a: stack 0) ret 4]; "docs/game/settings-menu.md";
    /// The multiplayer pause menu's row text: looks the row's action up in
    /// a table of six.
    MP_MENU_ROW_TEXT @ 0x0023_284e: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The multiplayer pause menu's row chosen: actions 0 to 4 have their
    /// own press, and every other action runs action 5's.
    MP_MENU_ROW_CHOSEN @ 0x0023_28b5: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The Controls list's row text: the label and value of rows 0 to 5.
    CONTROLS_ROW_TEXT @ 0x002c_8548: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The Controls list's row chosen (rows past 4 open the vibration
    /// dialog).
    CONTROLS_ROW_CHOSEN @ 0x002c_8719: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The campaign difficulty list's row text.
    DIFFICULTY_ROW_TEXT @ 0x002c_9ce7: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/game/skulls.md";
    /// The campaign difficulty list's row chosen: clamps the row to 0..3,
    /// stores the difficulty and starts the campaign.
    DIFFICULTY_ROW_CHOSEN @ 0x002c_9d38: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/game/skulls.md";
    /// The variant editor's category list (screen 178,
    /// `variant_editing_options`) row text: looks the row's word up in a
    /// table of six; an unknown word leaves the text as it was.
    VARIANT_OPTIONS_ROW_TEXT @ 0x002b_7581: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/game/skulls.md";
    /// The variant editor's category list row chosen: words 0 to 5 open a
    /// category's screen; any other word returns having done nothing.
    VARIANT_OPTIONS_ROW_CHOSEN @ 0x002b_75e8: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/game/skulls.md";
    /// The Edit Profile list's row text (screen 38, `edit_profile_menu`):
    /// switches on the item's row index, 0 to 5 (Controls, Appearance,
    /// Voice Output, Voice Masking, Online Status, Subtitles); any other
    /// row gets the null id.
    EDIT_PROFILE_ROW_TEXT @ 0x002b_7990: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The Edit Profile list's row chosen: rows 0 to 5 by index (the
    /// handle checked through `0x16bae0` first); any other row does
    /// nothing.
    EDIT_PROFILE_ROW_CHOSEN @ 0x002b_79f9: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The Game Setup dialog's list handler constructor (screen 23,
    /// `squad_settings_dialog`): ESI the handler, one stack argument. Fills
    /// the "squad setting list" rows (action word at datum `+2`) for the
    /// lobby's kind.
    SQUAD_SETTINGS_LIST_CTOR @ 0x002b_b4d2: [fn(list: esi, a: stack 0) ret 4]; "docs/halo1/multiplayer.md";
    /// The Game Setup list's row text: looks the row's action up in a table
    /// of nine; an unknown action leaves the text as it was.
    SQUAD_SETTINGS_ROW_TEXT @ 0x002b_b74f: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/halo1/multiplayer.md";
    /// The Game Setup list's row chosen: jumps through a ten-entry table by
    /// the row's action with no bounds check.
    SQUAD_SETTINGS_ROW_CHOSEN @ 0x002b_b801: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/halo1/multiplayer.md";
    /// The Game Setup dialog's update (vtable `0x45c524`): sets the
    /// description, the "current" label and value, and the picture for
    /// the focused row's action (a jump table of ten at `0x2bbea8`; any
    /// other action gets none of them and image 0), then the base update.
    SQUAD_SETTINGS_UPDATE @ 0x002b_bbf6: [fn(screen: ecx) ret 0]; "docs/game/ui-builder.md";
    /// Whether the OptiMatch playlist list shows hopper `index` (0..16):
    /// the range, two gates and the descriptor's validity (tail `jmp` to
    /// `0x001934f0`). Its one caller is the list's item builder
    /// `0x002b1ff4` (return address `0x002b2069`), which stores the index
    /// in each row's datum at `+4`, so a skipped hopper leaves no hole.
    HOPPER_LISTED @ 0x0019_2db0: [fn(index: ecx) -> al ret 0]; "docs/game/bots.md";
    /// Reads one hopper rotation into the caller's block. The weighted map
    /// picker calls it separately from the validator and playlist UI.
    HOPPER_BLOCK_READ @ 0x0019_43d0: [fn(index: eax, block: stack 0) -> al ret 4]; "docs/live/live-matchmaking.md";
    /// The hopper Quickmatch searches: local player `player`'s saved
    /// playlist (`[0x54e9dc + player*0xc70]`), or -1 when the player has
    /// none (flag bit 4 of `[0x54e8e0 + player*0xc70]` clear). Its one
    /// caller is the Live menu's Quickmatch press (`0x00231056`), which
    /// starts nothing on -1.
    QUICKMATCH_HOPPER @ 0x0019_02de: [fn(player: ecx) -> eax ret 0]; "docs/game/bots.md";
    /// Saves `hopper` as local player `player`'s playlist for Quickmatch.
    /// Its one caller is the OptiMatch playlist list's press
    /// (`0x002b21e3`).
    PLAYLIST_REMEMBER @ 0x0019_02fc: [fn(player: stack 0, hopper: stack 1) ret 8]; "docs/game/bots.md";
    /// The Xbox Live menu's "build" method (vtable `0x00458b48`, slot
    /// `0x00458bec`): resets the search globals, fills the list, then puts
    /// the cursor on the row in `[0x00510a18]` when it is 0 to 3 (`0x0024c591`,
    /// the list at screen `+0x818`).
    LIVE_MENU_CURSOR @ 0x0023_1170: [fn(screen: ecx) ret 0]; "docs/game/bots.md";
    /// The multiplayer level list's data array (`[0x4ee4e4]`, 8-byte
    /// elements, the map id at `+4`), or 0 without UI level data. The
    /// Select Map list's constructor (`0x2c9928`, list handler in ESI)
    /// calls it at `0x2c9980` and copies every element into its own list.
    MP_LEVEL_ARRAY @ 0x0019_c6a0: [fn() -> eax ret 0]; "docs/game/map-select.md";
    /// Refreshes the UI level lists (no direct caller: the function table at
    /// `0x43ca94`, at each map load): the arrays cleared, the content
    /// packages' `patch.lvl` (`0x19c120`, main menu only), the level select's
    /// list (`0x19c6d0`), then the multiplayer array (`0x19c7e0`).
    MP_LEVEL_REFRESH @ 0x0019_c190: [fn() ret 0]; "docs/game/map-select.md";
    /// A multiplayer list slot's name in the current language (stdcall:
    /// `slot + 0xc + 0x40 * language`). The Select Map row text calls it at
    /// `0x2c9afc`.
    MP_LEVEL_NAME @ 0x002a_fd70: [fn(slot: stack 0) -> eax ret 4]; "docs/game/map-select.md";
    /// The Select Map list's row text (list handler in ECX): the row's map
    /// id, its slot, then [`MP_LEVEL_NAME`] for the text.
    MAP_LIST_ROW_TEXT @ 0x002c_9ac4: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/game/map-select.md";
    /// The Select Map list's row chosen (list handler in ECX, at its
    /// screen's `+0x614`): the map, then message 3 to its own screen.
    MAP_LIST_ROW_CHOSEN @ 0x002c_9b17: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/game/map-select.md";
    /// Open the lobby's Select Map dialog from a screen request.
    SELECT_MAP_LOAD @ 0x002b_b2db: [fn(request: stack 0) -> eax ret 4]; "docs/game/map-select.md";
    /// Open the lobby's Game Setup dialog from a screen request.
    GAME_SETUP_LOAD @ 0x002b_bacb: [fn(request: stack 0) -> eax ret 4]; "docs/game/ui-screens.md";
    LOBBY_VARIANT_LIST_LOAD @ 0x002b_1b85: [fn(request: stack 0) -> eax ret 4]; "docs/game/ui-screens.md";
    LOBBY_LEVEL_LIST_LOAD @ 0x002b_136d: [fn(request: stack 0) -> eax ret 4]; "docs/game/ui-screens.md";
    LOBBY_DIFFICULTY_LOAD @ 0x002b_b432: [fn(request: stack 0) -> eax ret 4]; "docs/game/ui-screens.md";
    /// The Select Map screen's destructor (screen vtable `0x45c650` slot 0).
    MAP_SCREEN_FREE @ 0x002b_b384: [fn(screen: ecx, flags: stack 0) -> eax ret 4]; "docs/game/map-select.md";
    /// Whether the HUD draws (AL); 0 in a campaign with Blind on.
    HUD_SHOWN @ 0x0013_939b: [fn() -> al ret 0]; "docs/game/skulls.md";
    HUD_TEXT_SETUP @ 0x0013_edb0: [fn(font: esi, colour: edi, shadow: stack 3) ret 16], Optional; "docs/halo1/text.md";
    HUD_TEXT_MEASURE @ 0x0013_ee20: [fn(text: ecx, font: stack 0) -> al ret 4], Optional; "docs/halo1/text.md";
    HUD_TEXT_BOUNDS @ 0x0024_ce9d: [fn(text: stack 0) ret 4], Optional; "docs/halo1/text.md";
    HUD_MESSAGE_LOOKUP @ 0x0013_925f: [fn(key: edi, output: ecx) ret 0], Optional; "docs/halo1/text.md";
    HUD_GLYPH_FIND @ 0x0014_0b20: [fn(font: eax, unit: ecx) -> eax ret 0], Optional; "docs/halo1/text.md";
    HUD_GLYPH_CREATE @ 0x0014_0b80: [fn(font: stack 0, unit: stack 1) -> eax ret 8], Optional; "docs/halo1/text.md";
    HUD_GLYPH_LOAD @ 0x0014_1020: [fn(glyph: eax, flags: edx) -> al ret 0], Optional; "docs/halo1/text.md";
    HUD_GLYPH_PIXELS_READY @ 0x0014_0870: [fn(glyph: eax, flags: stack 0) -> al ret 4], Optional; "docs/halo1/text.md";
    HUD_GLYPH_PIXELS @ 0x0014_0960: [fn(glyph: eax, output: stack 0) -> al ret 4], Optional; "docs/halo1/text.md";
    HUD_INTERACTION_VISIBLE @ 0x0024_c98c: [fn(user: eax, visible: stack 0) ret 4], Optional; "docs/halo1/text.md";
    PLAYER_PICKUP_SCAN @ 0x0015_08e0: [fn(player: stack 0) ret 4], Optional; "docs/halo1/text.md";
    PLAYER_PICKUP_CANDIDATE @ 0x0015_0e30: [fn(player: stack 0, target: stack 1) ret 8], Optional; "docs/halo1/text.md";
    PLAYER_PRESS_INTERACT @ 0x0015_1c90: [fn(player: eax, action: stack 0) -> al ret 4], Optional; "docs/halo1/text.md";
    HUD_MATERIAL_CONSTANTS @ 0x0001_8ee0: [fn(material: stack 0) ret 4], Optional; "docs/halo1/text.md";
    HUD_MATERIAL_PIXEL_SHADER @ 0x0001_b050: [fn(material: esi) ret 0], Optional; "docs/halo1/text.md";
    HUD_MATERIAL_READY @ 0x0001_cd90: [fn() ret 0], Optional; "docs/halo1/text.md";
    HUD_WIDGET_INPUTS @ 0x0013_ad48: [fn(weapon: ecx, hud: edx, user: stack 0, inputs: stack 1, output: stack 2) ret 12], Optional; "docs/halo1/hud.md";
    HUD_WIDGET_STRING @ 0x0022_a871: [fn(widget: ebx, hud: stack 0, values: stack 1, output: stack 2, font: stack 3) ret 16], Optional; "docs/halo1/hud.md";
    HUD_WIDGET_EFFECTS @ 0x0022_bcaf: [fn(effects: eax, y_basis: ebx, origin: stack 0, x_basis: stack 1) ret 8], Optional; "docs/halo1/hud.md";
    HUD_WIDGET_SPRITE @ 0x0022_ae21: [fn(widget: ecx, bounds: eax, user: stack 0, image: stack 1, width: stack 2, height: stack 3) ret 16], Optional; "docs/halo1/hud.md";
    HUD_WIDGET_GATE @ 0x0013_ac87: [fn(state: eax, weapon: ecx, word_c: edx, word_a: stack 0, word_b: stack 1, word_d: stack 2) -> al ret 12], Optional; "docs/editor/hud-editor.md";
    HUD_SENSOR_DRAW @ 0x001a_2823: [fn(screen: eax, user: ecx) ret 0], Optional; "docs/halo1/hud.md";
    HUD_TEXT_TAIL @ 0x0015_f120: [fn(player: eax, out: esi, capacity: stack 0, mode: stack 1) -> al ret 8], Optional; "docs/halo1/text.md";
    HUD_TEXT_DRAW @ 0x0001_fa30: [fn(rect: eax, text: ecx) ret 0], Optional; "docs/halo1/text.md";
    HUD_TEXT_SUBSTITUTE @ 0x0022_d2ee: [fn(text: stack 0, capacity: stack 1) ret 8], Optional; "docs/halo1/text.md";
    /// Draws the first-person weapons of one local user (stdcall, four
    /// arguments, EAX the result); skipped whole in a campaign with Blind on.
    FIRST_PERSON_RENDER @ 0x0016_5f16: [fn(user: stack 0, a: stack 1, b: stack 2, c: stack 3) -> eax ret 16]; "docs/game/skulls.md";
    /// A weapon object made from its scenario placement (stdcall: the
    /// object datum, the placement); its rounds are the placement's
    /// `+0x4c`, halved in a campaign with Famine on.
    WEAPON_FROM_PLACEMENT @ 0x000f_da00: [fn(object: stack 0, placement: stack 1) ret 8]; "docs/game/skulls.md";
    /// Open a screen: build a 0x20-byte request for the player mask and
    /// call `loader` with it (stdcall).
    OPEN_SCREEN @ 0x0014_9f1e: [fn(mask: stack 0, loader: stack 1) ret 8]; "docs/formats/ui.md";
    /// Allocate a UI object of `size` bytes (stdcall); 0 when out of memory.
    UI_ALLOC @ 0x001a_47fd: [fn(size: stack 0) -> eax ret 4]; "docs/formats/ui.md";
    /// The Controls screen loader (stdcall, the request).
    CONTROLS_LOADER @ 0x002b_7714: [fn(request: stack 0) -> eax ret 4]; "docs/formats/ui.md";
    /// The Controls screen constructor: the screen in EDI, the request's
    /// `+4`, `+8` and `+2` (word) on the stack; the screen in EAX.
    CONTROLS_SCREEN_CTOR @ 0x002b_7752: [fn(screen: edi, a: stack 0, b: stack 1, player: stack 2) -> eax ret 12]; "docs/formats/ui.md";
    /// Queue a constructed screen on the channel named by the request.
    SCREEN_PUSH @ 0x0014_7f6d: [fn(screen: edi, request: ebx) ret 0]; "docs/formats/ui.md";
    /// The generic screen event handler; B and Back close by channel.
    SCREEN_BACK_EVENT @ 0x0023_00ea: [fn(screen: ecx, event: stack 0) -> al ret 4]; "docs/game/ui-screens.md";
    /// Game Setup handles X and delegates other events to SCREEN_BACK_EVENT.
    GAME_SETUP_EVENT @ 0x002b_bed0: [fn(screen: ecx, event: stack 0) -> al ret 4]; "docs/game/ui-screens.md";
    /// Unlink and free a screen on a UI channel.
    UI_CHANNEL_FREE @ 0x0023_51ea: [fn(channel: ecx, screen: stack 0) ret 4]; "docs/game/ui-screens.md";
    /// Play the UI sound by index (Back is 4).
    UI_SOUND @ 0x0023_6299: [fn(sound: eax) ret 0]; "docs/game/ui-screens.md";
    /// Screen vtable slot 18 of the Controls class: set up the current pane
    /// (header, then the pane's widgets).
    SCREEN_SETUP_PANE @ 0x0023_047e: [fn(screen: ecx, arg: stack 0) ret 4]; "docs/formats/ui.md";
    /// Set a text widget's string id (the widget in EDI).
    TEXT_SET @ 0x0025_3b1a: [fn(text: edi, id: stack 0) ret 4]; "docs/formats/ui.md";
    /// The pane builder's text step: make a text widget from a text block
    /// definition (`0x2c` bytes), a child of the screen in EBX, and give it
    /// the block's string; the widget in EAX, or 0.
    PANE_TEXT_ADD @ 0x0022_f9bf: [fn(screen: ebx, index: stack 0, block: stack 1) -> eax ret 8]; "docs/formats/ui.md";
    /// The pane builder's bitmap step: make a bitmap widget from a bitmap
    /// block definition (`0x38` bytes), a child of the screen in ESI; the
    /// widget in EAX, or 0.
    PANE_BITMAP_ADD @ 0x0022_fa30: [fn(screen: esi, block: stack 0) -> eax ret 4]; "docs/formats/ui.md";
    /// Builds the wide-name history rows for Recent Players and returns the list.
    RECENT_PLAYERS_BUILD @ 0x002b_393c: [fn(list: ebx, request: stack 0) -> eax ret 4]; "docs/game/bots.md";
    /// Retail's lossy wide-to-ASCII conversion, used when packing a Live selection.
    PLAYER_NAME_TO_ASCII @ 0x0013_fb90: [fn(source: eax, dest: ecx, capacity: edx) ret 0]; "docs/game/bots.md";
    /// The two retail player-options constructors share the current selected-player record.
    PLAYER_OPTIONS_CHECK_FRIEND @ 0x002b_70a3: [fn(request: eax, source: esi) -> eax ret 0]; "docs/game/bots.md";
    PLAYER_OPTIONS_OPEN @ 0x002b_71a6: [fn(request: eax, source: esi) -> eax ret 0]; "docs/game/bots.md";
    /// List skin by index in the current interface, or null.
    UI_LIST_SKIN @ 0x0014_837a: [fn(index: stack 0) -> eax ret 4]; "docs/formats/ui.md";
    PANE_SHADER_ADD @ 0x002b_af38: [fn(screen: esi, block: ebx) -> eax ret 0]; "docs/formats/ui.md";
    EMBLEM_WIDGET_DRAW @ 0x002b_11c9: [fn(widget: ecx, context: stack 0) ret 4]; "docs/game/bots.md";
    EMBLEM_WIDGET_SET @ 0x002b_12ca: [fn(widget: eax, team: edx, slot: ecx, profile: stack 0) ret 4]; "docs/game/bots.md";
    /// The current interface's screen tag for a screen id (stdcall): the
    /// `wgit` tag's datum handle, or -1.
    SCREEN_TAG_BY_ID @ 0x0014_8098: [fn(id: stack 0) -> eax ret 4]; "docs/formats/ui.md";
    /// The focused item's row handle of the list in EAX, or -1.
    LIST_FOCUSED_ROW @ 0x0024_c447: [fn(list: eax) -> eax ret 0]; "docs/formats/ui.md";
    /// Transform a UI quad in place from UI coordinates to screen coordinates.
    /// The list's arrow draw at `0x24bda2` calls it with its arrow quad in ESI.
    UI_QUAD_TO_SCREEN @ 0x0023_618e: [fn(quad: esi, scale: stack 0, transform: stack 1) -> eax ret 8]; "docs/formats/ui.md";
    /// An item widget's `index`-th text widget of kind 6 (thiscall on the
    /// item; the game pushes 0, the index, 6), or 0.
    ITEM_TEXT @ 0x0022_edb8: [fn(item: ecx, kind: stack 0, index: stack 1, zero: stack 2) -> eax ret 12]; "docs/audio/playstation-audio.md";
    /// The network session's game start: builds game options from the
    /// session in EDX and starts the game (`0x18e8b0`); AL 1 when it
    /// started. Refuses a campaign session with more than one peer
    /// (`0x6eea5`).
    SESSION_GAME_START @ 0x0006_ec80: [fn(session: edx, flag: stack 0) -> al ret 4]; "docs/live/live-coop.md";
    /// The scoreboard badge's visibility (AL) and connection bar count
    /// (written to the out pointer).
    SCOREBOARD_CONNECTION_BARS @ 0x0006_8a90: [fn(stats: ebx, bars: stack 0) -> al ret 4], Optional; "docs/game/bots.md";
    /// Whether a machine (its 6-byte id at EAX) is in the game, AL: 1 with
    /// no simulation or for this console's machine, else its simulation
    /// view's `+0x78` (0 with no view). The player update (`0x14d2a0`, its
    /// only caller) copies it into each player's flag bit 0, and only
    /// players with it spawn (`0x14d3a0`).
    MACHINE_IN_GAME @ 0x0006_86e0: [fn(machine: eax) -> al ret 0]; "docs/game/bots.md";
    /// Gather one stored remote action into this tick's simulation update.
    SIMULATION_SLOT_ACTION @ 0x0008_4990: [fn(slot: stack 0, action: stack 1) -> al ret 8]; "docs/game/bots.md";
    /// Clear and default a game-options block (ESI). The session start
    /// calls it right after its peer and player checks.
    OPTIONS_NEW @ 0x0013_8110: [fn(options: esi) ret 0]; "docs/runtime/level-boot.md";
    /// Whether the squad session is joinable (Live presence, AL); false
    /// for a campaign session (`0x63e7a`).
    SESSION_JOINABLE @ 0x0006_3e00: [fn() -> al ret 0]; "docs/live/live-coop.md";
    /// The host's game-start session parameters (session in EBX, state 5
    /// only); for a campaign session it sets the peer limit to 1.
    SESSION_GAME_LIMITS @ 0x0006_e720: [fn(session: ebx) -> al ret 0]; "docs/live/live-coop.md";
    /// Lobby: set the squad session's map (host only): stdcall `(campaign
    /// id)`, the map id in ESI, AL 1 when set.
    LOBBY_SET_MAP @ 0x0019_a6f2: [fn(map: esi, campaign: stack 0) -> al ret 4]; "docs/live/live-coop.md";
    /// Lobby: set the squad session's campaign difficulty (AX; `0xffff`
    /// none) and, for a difficulty, clear the variant (synchronous
    /// protocol). AL 1 when set.
    LOBBY_SET_DIFFICULTY @ 0x0019_a76d: [fn(difficulty: eax) -> al ret 0]; "docs/live/live-coop.md";
    /// Lobby: put the squad lobby in a mode (stdcall; 0 co-op campaign,
    /// 2 custom game): the Xbox Live menu's Custom Game row calls it with 2
    /// once its session is made (`0x23101b`), the lobby's "Switch To: Co-Op
    /// Game" with 0 (`0x2bba30`).
    LOBBY_MODE_SET @ 0x0019_9a03: [fn(mode: stack 0) ret 4]; "docs/live/live-coop.md";
    /// Available Games' Create row (local and Live flags both zero for System Link).
    BROWSER_CREATE_SQUAD @ 0x0019_9df9: [fn(local: stack 0, live: stack 1) -> al ret 8]; "docs/live/dedicated-host.md";
    /// Per-frame reconciliation of the local UI screen and network life cycle.
    UI_SYNC_NETWORK_SCREEN @ 0x0019_9b08: [fn() ret 0]; "docs/live/dedicated-host.md";
    /// The menu leaves the squad and target, immediately or gracefully.
    UI_LEAVE_SESSIONS @ 0x0019_9e2e: [fn(immediate: stack 0) ret 4]; "docs/live/dedicated-host.md";
    /// Lobby: why the game cannot start yet (stdcall, an optional float
    /// out); 9 ready, 7 the teams or co-op check (`0x19a4be`: co-op needs
    /// exactly two players, both on this console), 8 and 10 the load and
    /// countdown states. Its status line picks the text (`0x250008`).
    LOBBY_START_STATE @ 0x0019_a2ce: [fn(progress: stack 0) -> eax ret 4]; "docs/live/live-coop.md";
    /// The start state's tail after a co-op check that passed: a countdown
    /// running (`>= 0`, state 10), else `LOBBY_ALL_READY` (AL: 9, else 8).
    LOBBY_COUNTDOWN @ 0x0006_4a60: [fn() -> eax ret 0]; "docs/live/live-coop.md";
    /// Despite the name, not a readiness count: AL 1 when this console
    /// leads the party (session `+0x72d8` equals `+0x50`, in states 3 to
    /// 8). Start state 8's text is "Waiting for ... to start the game"
    /// (docs/game/bots.md §4.2, "The start check").
    LOBBY_ALL_READY @ 0x0005_92f0: [fn() -> al ret 0]; "docs/live/live-coop.md";
    /// Refresh the pregame lobby's game type picture and option lines
    /// (EAX the screen, stack 0 the current game variant or null).
    LOBBY_VARIANT_PANEL @ 0x0025_01c0: [fn(screen: eax, variant: stack 0) ret 4]; "docs/game/ui-builder.md";
    /// The pregame lobby's input handler (ECX the screen, the event on the
    /// stack, AL handled, `ret 4`): event `+0` the kind (5 a button), `+4`
    /// the controller, `+8` the button (0 A, `0xc` Start). A or Start from
    /// a controller with a profile reaches Start Game (`0x250c30`,
    /// `0x25106c`); from one without, the profile chooser (`0x250cc3`).
    PREGAME_LOBBY_INPUT @ 0x0025_0b1e: [fn(screen: ecx, event: stack 0) -> al ret 4]; "docs/game/bots.md";
    /// The squad membership's peer count. Only leave callers treat a
    /// leader with exclusively phantom opponents as alone.
    PARTY_PEER_COUNT @ 0x0019_9f34: [fn() -> eax ret 0]; "docs/game/bots.md";
    /// Whether the player slot in AX is occupied in the squad membership.
    PARTY_PLAYER_VALID @ 0x0019_a951: [fn(player: eax) -> al ret 0]; "docs/game/bots.md";
    /// The lobby Back or pause Leave Game has committed to leaving.
    PARTY_LEAVE @ 0x0019_9e3c: [fn(controller: stack 0) ret 4]; "docs/game/bots.md";
    /// The player accepted the matchmaking lobby's Back confirmation.
    MATCHMAKING_CANCEL_CONFIRMED @ 0x0025_2150: [fn(controller: stack 0) -> al ret 4]; "docs/game/bots.md";
    /// The pregame lobby's list item handler (ECX the screen, `ret 8`),
    /// set for its rows at `0x24fb40`: stack 0 a record whose `+0` is the
    /// input event (`+4` the controller), stack 1 the row (a word: 0 Start
    /// Game, which asks the start state at `0x251051` and starts the
    /// countdown on 9). The list takes A before the screen does, so A on
    /// Start Game never reaches `PREGAME_LOBBY_INPUT` (the run
    /// 20261119-005001 started its game with no `bots: start state` line).
    LOBBY_ROW_PRESS @ 0x0025_0fb3: [fn(screen: ecx, item: stack 0, row: stack 1) -> al ret 8]; "docs/game/bots.md";
    /// The per-tick player update (from `game_tick`, `0x13801f`): every
    /// player; a first spawn pending (flags byte `+2` bit 3) goes to
    /// `PLAYER_SPAWN`; the co-op respawn (`0x14cdd0`) runs inside it.
    PLAYER_UPDATE @ 0x0014_d3a0: [fn(update: stack 0) ret 4]; "docs/live/live-coop.md";
    /// Spawn one player (handle): unit type, a starting location from
    /// `PLAYER_SPAWN_LOCATION`, the unit; AL 0 when no location was free.
    /// Clears the first-spawn bit (`0x14f6c1`) when it spawned.
    PLAYER_SPAWN @ 0x0014_f2e0: [fn(player: stack 0) -> al ret 4]; "docs/live/live-coop.md";
    /// The best free starting location for a player (handle in ECX): AX the
    /// index into `scnr+0x100`, `0xffff` when every spot is blocked.
    PLAYER_SPAWN_LOCATION @ 0x0014_ef00: [fn(player: ecx, unit: stack 0, a: stack 1, b: stack 2) -> eax ret 12]; "docs/live/live-coop.md";
    /// Retail co-op placement: move a player (EBX) next to a position (ESI,
    /// three floats) beside an anchor unit (stack). The co-op respawn's
    /// own call is at `0x14cffd`. AL 1 when placed.
    PLAYER_PLACE_NEAR @ 0x0014_e970: [fn(player: ebx, position: esi, anchor: stack 0) -> al ret 4]; "docs/live/live-coop.md";
    /// Teleport an object to a cutscene flag: the body of HaloScript
    /// `object_teleport` (`0x2a2517`) and of
    /// `volume_teleport_players_not_inside` (`0x29f641`).
    SCRIPT_TELEPORT @ 0x0029_ffb0: [fn(object: stack 0, flag: stack 1, a: stack 2, b: stack 3) ret 16], Optional; "docs/live/live-coop.md";
    /// Whether the game may save a checkpoint now: the body of
    /// HaloScript `game_safe_to_save` (`0x2a9bb8`). A run of checks, among
    /// them every player's unit (`0x14df40`, `0x14deb0`) and "some player
    /// dead" (`players_globals+5`); AL 1 when safe.
    SAVE_SAFE @ 0x0022_6190: [fn() -> al ret 0]; "docs/live/live-coop.md";
    /// The Live join lifecycle. Its update promotes the target squad or records
    /// the title's reason for declining the join.
    LIVE_JOIN_UPDATE @ 0x0006_fd10: [fn(context: ecx) -> al ret 0], Optional; "docs/live/live-custom-games.md";
    /// The host's answer to a join request (session in EAX, request in
    /// EDI): 0 accepts, 3 refuses unless the session state (`S+0x741c`)
    /// is 5 (the lobby), 2 for an invite-only session, 4 when full.
    SESSION_JOIN_CHECK @ 0x0006_2fe0: [fn(session: eax, request: edi) -> eax ret 0], Optional; "docs/live/live-coop.md";
    /// Sync-server: start sending the game state to a joining client's
    /// view (EAX): compress the block and queue the join and state pieces
    /// ahead of its updates. AL 1 when queued.
    SIM_JOIN_SEND_STATE @ 0x0008_62e0: [fn(view: eax) -> al ret 0], Optional; "docs/live/live-coop.md";
    /// Sync-client: the synchronous-join message (view in EDI, the
    /// server's update number on the stack). AL 1 when taken.
    SIM_JOIN_RECEIVE @ 0x0008_5dc0: [fn(view: edi, update: stack 0) -> al ret 4], Optional; "docs/live/live-coop.md";
    /// Sync-client: the last (empty) game-state piece: decompress what
    /// arrived (`length` bytes) into the game-state block. AL 1 when it
    /// came to `0x3fe000` bytes.
    SIM_STATE_RECEIVED @ 0x0006_aa20: [fn(world: edi, length: stack 0) -> al ret 4], Optional; "docs/live/live-coop.md";
    /// Apply one update record (stack): player entries, the player-set
    /// change (`+0xDD0`, mask `+0xDD4`), events, the save flag (`+0x403C`).
    SIM_UPDATE_APPLY @ 0x0006_83a0: [fn(update: stack 0) ret 4], Optional; "docs/live/live-coop.md";
    /// Synchronous: send this console's player actions for the tick to
    /// the machine that builds the updates (message `0x28`, `0x180`
    /// bytes, up to four local players).
    SIM_ACTIONS_SEND @ 0x0008_5fc0: [opaque], Optional; "docs/live/live-coop.md";
    /// Encode a synchronous 0x28 after the sender has admitted it to the channel.
    SIM_ACTIONS_ENCODE @ 0x000b_2790: [fn(stream: stack 0, size: stack 1, actions: stack 2) ret 12], Optional; "docs/live/live-coop.md";
    /// Synchronous: a peer's actions arrived (from the `0x28` handler
    /// `0x949d0`).
    SIM_ACTIONS_RECEIVE @ 0x0008_5f50: [opaque], Optional; "docs/live/live-coop.md";
    /// Synchronous: send one tick's update record to one peer's view
    /// (message `0x27`; queued instead while that view is joining).
    SIM_UPDATE_SEND @ 0x0008_60b0: [opaque], Optional; "docs/live/live-coop.md";
    /// Synchronous: an update arrived; run the ticks it allows (from the
    /// `0x27` handler `0x94990`).
    SIM_CATCH_UP @ 0x0006_98e0: [opaque], Optional; "docs/live/live-coop.md";
    /// Save the game state (checkpoint): the save callbacks, then the
    /// writer `0x2151f0`.
    GAME_STATE_SAVE @ 0x0012_3e20: [opaque], Optional; "docs/runtime/checkpoints.md";
    /// Revert the game state to a saved slot.
    GAME_STATE_REVERT @ 0x0012_3f60: [opaque], Optional; "docs/runtime/checkpoints.md";
    /// Restore the whole game state from a saved image (header checked).
    GAME_STATE_RESTORE @ 0x0012_4380: [opaque], Optional; "docs/runtime/checkpoints.md";
    /// Switch the structure BSP (prepare, dispose, load, publish).
    BSP_SWITCH @ 0x0011_c1b0: [opaque], Optional; "docs/runtime/level-boot.md";
    /// HaloScript `cinematic_start`.
    CINEMATIC_START @ 0x002a_9740: [opaque], Optional; "docs/runtime/level-boot.md";
    /// HaloScript `cinematic_stop`.
    CINEMATIC_STOP @ 0x002a_9760: [opaque], Optional; "docs/runtime/level-boot.md";
    /// The level select's list: one campaign's map ids from the start-level
    /// list, sorted, into the data array (stdcall `(array, campaign id)`).
    /// 1.0 asks for campaign 1 only (`0x19c190`).
    CAMPAIGN_LEVEL_LIST @ 0x0019_c6d0: [fn(array: stack 0, campaign: stack 1) ret 8]; "docs/halo1/campaign-picker.md";
    CAMPAIGN_OPTIONS_LIST @ 0x002b_14dc: [fn(list: ebx, player: stack 0) -> eax ret 4]; "docs/halo1/campaign-picker.md";
    /// Campaign Options list slot 19: the number of item widgets to link.
    /// Retail returns the active row count from the list's datum array.
    CAMPAIGN_OPTION_ITEM_COUNT @ 0x002b_160c: [fn(list: ecx) -> eax ret 0]; "docs/halo1/campaign-picker.md";
    CAMPAIGN_OPTION_TEXT @ 0x002b_1613: [fn(list: ecx, item: stack 0, arg: stack 1) ret 8]; "docs/halo1/campaign-picker.md";
    CAMPAIGN_OPTION_CHOSEN @ 0x002b_166d: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/halo1/campaign-picker.md";
    PROFILE_LAST_MAP @ 0x0019_10d9: [fn() -> eax ret 0]; "docs/halo1/campaign-picker.md";
    PROFILE_CHECKPOINT_SET @ 0x0019_048c: [fn(slot: stack 0) ret 4]; "docs/halo1/campaign-picker.md";
    CAMPAIGN_CHECKPOINT_POLL @ 0x002b_178e: [fn(list: ecx) ret 0]; "docs/halo1/campaign-picker.md";
    CAMPAIGN_OPTION_HELP @ 0x002b_18b7: [fn(list: eax) -> eax ret 0]; "docs/halo1/campaign-picker.md";
    CHECKPOINT_QUEUE_READ @ 0x002a_da70: [fn(path: eax) -> eax ret 20]; "docs/halo1/campaign-picker.md";
    CHECKPOINT_QUEUE_WRITE @ 0x002a_dec0: [fn(path: eax) -> eax ret 20]; "docs/halo1/campaign-picker.md";
    /// The menu's campaign start: options from the selection globals
    /// `0x54e7c0`-`0x54e7c8`, the level looked up by `(campaign, map)`.
    CAMPAIGN_START @ 0x0023_643f: [fn() ret 0]; "docs/halo1/campaign-picker.md";
    /// The next level of a campaign: the smallest map id above `map` with
    /// that campaign id in the start-level list (stdcall, EAX, -1 none).
    /// The game-won path (`0x18e960`) and `0x163610` ask it.
    NEXT_LEVEL @ 0x0019_c440: [fn(campaign: stack 0, map: stack 1) -> eax ret 8]; "docs/halo1/campaign-picker.md";
    /// The start-level record of `(campaign, map)`: the first with both ids
    /// in the loaded globals' start-level list (stdcall, EAX, 0 none). Some
    /// callers pass campaign 1 whatever the map: the level select's press
    /// (`0x2c5118`), which faults on 0, and Continue (`0x199ab7`).
    CAMPAIGN_LEVEL_LOOKUP @ 0x0019_c270: [fn(campaign: stack 0, map: stack 1) -> eax ret 8]; "docs/halo1/campaign-picker.md";
    /// A finished level raises one slot's profile progress for a difficulty
    /// to map id ESI (stdcall `(slot, difficulty)`).
    PROGRESS_RAISE @ 0x0019_043f: [fn(map: esi, slot: stack 0, difficulty: stack 1) ret 8]; "docs/halo1/campaign-picker.md";
    /// The last level started (map id EAX) into every signed-in profile and
    /// `0x510840`.
    LAST_LEVEL_SET @ 0x0019_040d: [fn(map: eax) ret 0]; "docs/halo1/campaign-picker.md";
    /// The level select screen's constructor (thiscall; the lobby's passes
    /// 1 in its second argument). Sets `0x54e7c0` to 1.
    LEVEL_SELECT_NEW @ 0x002c_4ea0: [fn(screen: ecx, a: stack 0, lobby: stack 1) -> eax ret 8]; "docs/halo1/campaign-picker.md";
    /// The highest map id the signed-in profiles finished, any difficulty
    /// (EAX, -1 for none).
    PROGRESS_MAX @ 0x0019_0565: [fn() -> eax ret 0]; "docs/halo1/campaign-picker.md";
    /// Fill a screen request (ECX): the player mask, two words and the
    /// loader; `OPEN_SCREEN` and the menus' own opens call it.
    SCREEN_REQUEST @ 0x0014_9f49: [fn(request: ecx, mask: stack 0, a: stack 1, b: stack 2, loader: stack 3) ret 16]; "docs/halo1/campaign-picker.md";
    /// One local player's control update (look input to the player's
    /// action): stdcall, the local user index first of five stack words,
    /// the player's local controller index (player `+0x24`) in EDX. Its
    /// only caller is `0x186ab0` (`0x186b16`), which `0x1870a0` runs for
    /// each bound local user. Reads slowdown and follow at
    /// `0x18622b` and calls the aim-assist search `0x1a4900` at `0x1861a1`.
    PLAYER_CONTROL_UPDATE @ 0x0018_5be0: [fn(user: stack 0, controller: edx) ret 0x14]; "docs/game/aim-assist.md";
    /// Gather script action-test flags and consume the controls claimed by a script.
    PLAYER_ACTION_TEST_UPDATE @ 0x0018_7b30: [fn(packet: ecx, actions: eax) ret 0]; "docs/halo1/script-builtins.md";
    /// The aim-assist parameters of a unit's held weapons, from the weapon
    /// tags (aim cone and pull angles and ranges, zoom applied):
    /// unit in ECX, the 0x24-byte result at ESI (kept to the return), zoom
    /// level on the stack, AL whether there is one. Callers: the local
    /// aim-assist search `0x1a4900` (`0x1a49a1`) and the shot aim
    /// `0x1a4c00` (`0x1a4cbf`, from the barrel fire `0x104150`).
    AIM_ASSIST_PARAMS @ 0x001a_50a0: [fn(unit: ecx, out: esi, zoom: stack 0) -> al ret 4]; "docs/game/aim-assist.md";
    /// The players screen's (`0x1e`) button event for a controller with no
    /// profile: the screen pushed, the event record in ESI (`+4` the
    /// controller, `+8` the button: 0 A, `0xc` Start). A or Start starts
    /// the profile pick (`0x1905bf`). Called from the screen's handler
    /// `0x252ad7`.
    PLAYERS_SCREEN_UNJOINED @ 0x0025_2a67: [fn(screen: stack 0, event: esi) -> al ret 4]; "docs/game/bots.md";
    /// The players screen's (`0x1e`) per-frame update (vtable `0x45a490`
    /// slot 3, thiscall, plain `ret`; from the window update `0x2358b9`):
    /// for each port it lays out the pane. A port is "joined" (name, no
    /// prompt) when its controller handle (`0x4e61cc`, `0x18f8b8`) is
    /// non-zero and its slot has a profile (`+0` bit `0x10`); with a
    /// controller and no profile it says "PRESS A TO JOIN" (`0x0f00028a`),
    /// with none "CONNECT CONTROLLER" (`0x1100028b`). Ends in the children's
    /// update `0x22e391`.
    PLAYERS_SCREEN_UPDATE @ 0x0025_2554: [fn(screen: ecx) ret 0]; "docs/game/bots.md";
    /// Signs a profile into a local player slot: the slot index, the
    /// 0x1e0-byte profile block it copies to the slot's `+0x18`, and the
    /// profile file (-1 for none, so nothing is saved). Sets the slot's
    /// `0x10` flag and builds the controls table (`0x190c34`).
    PROFILE_SIGN_IN @ 0x0018_fb34: [fn(slot: stack 0, block: stack 1, file: stack 2) ret 12]; "docs/game/bots.md";
    /// Signs the profile out of local player slot EAX: closes its saved
    /// file when it has one (`+0x1f8` not -1, `0x2172a0`), zeroes the
    /// 0x1e0-byte block at `+0x18`, clears the slot's `0x10` flag and sets
    /// `+0x1f8` to -1. Does nothing when the flag is clear. Callers
    /// `0x190d76`, `0x24b7c8`.
    PROFILE_SIGN_OUT @ 0x0018_fc08: [fn(slot: eax) ret 0]; "docs/game/bots.md";
    /// Signs local player slot EAX out altogether, as the profile UI's
    /// sign-out (`0x24b807`) and a failed host logon's (`0x190186`, at
    /// `0x1901e7`) do: leaves Xbox Live (`0x190da5`: clears the `0x20` flag
    /// and the account record at `+0x470`), frees the slot's local user
    /// (`+4`, `0x64690` zeroes its `0xd0`-byte entry at `0x4cd904`), signs
    /// the profile out (`PROFILE_SIGN_OUT`), then zeroes the flags, the
    /// player id at `+8` and the name at `+0xc1c` and sets `+4` and `+0x14`
    /// to -1. With its local user gone the squad session's local-player
    /// pass (`0x65770`, status 0) takes the slot's player out of the
    /// session. Callers `0x18f5c6`, `0x1901e7`, `0x1906be`, `0x24b807`,
    /// `0x24ba16`, `0x24ba5a`.
    SLOT_SIGN_OUT @ 0x0019_0d4b: [fn(slot: eax) ret 0]; "docs/game/bots.md";
    /// Allocates a one-shot sound after native playback admission.
    SOUND_IMPULSE_ALLOCATE @ 0x0012_6000: [fn(source: stack 0, channel: stack 1, tag: stack 2, rule: stack 3) ret 16]; "docs/halo1/sounds.md";
    /// Walks a block of predicted resources in the requested prediction mode.
    PREDICT_RESOURCES @ 0x0016_e5e0: [fn(mode: edi, resources: stack 0) -> al ret 4], Optional; "docs/perf/sim-budget.md";
    /// Sets source reverb and filter parameters on a DirectSound submix buffer.
    SOUND_SUBMIX_FX @ 0x0022_11e0: [fn(submix: eax, room: stack 0) ret 4], Optional; "docs/perf/sim-budget.md";
    /// The sound frame service, once a frame after the render: among
    /// other things, one listener per mapped local user (`0x125c93`).
    SOUND_UPDATE @ 0x0012_5b40: [fn() ret 0]; "docs/engine/engine-map.md";
    /// The sound listeners' update (stdcall, one argument, `ret 4`): for
    /// each local user mapped (`[0x4e8c20] + 0xc`) it sets the user's
    /// listener from its camera and marks it active (the byte at
    /// `[SOUND_GLOBALS] + 0x8e + 0x48 * user`). An unmapped user's active
    /// byte is left alone.
    /// Called from `SOUND_UPDATE` and from the per-frame effect update.
    SOUND_LISTENERS_UPDATE @ 0x0012_7400: [fn(arg: stack 0) ret 4]; "docs/game/bots.md";
    /// Whose own sound is this (stdcall, `ret 12`): when `*kind` is 1 (a
    /// positioned sound), walks the four local users (`[0x4e8c20] + 0xc`)
    /// and, for the first mapped one in first person (`0x155760`) whose
    /// player's unit is `object` or carries it (`0xbafb0`), sets `*kind`
    /// to 0 (a plain sound, no position: heard in that user's ears) and
    /// `*user` (when not null) to the user. Called when a sound starts on
    /// an object: `0x1890e5`, `0x18935f`, `0x189441`, `0x18cf76`,
    /// `0x18d4dc`, on the game thread (the object and effect updates and the
    /// per-frame update `0x18c060`).
    SOUND_OWNER_USER @ 0x0018_d4f0: [fn(object: stack 0, kind: stack 1, user: stack 2) ret 12]; "docs/game/bots.md";
    /// Per-type file bound called by the cache header validator (0x1228b8).
    CACHE_TYPE_LIMIT @ 0x0021_4ed0: [fn(kind: ecx) -> eax ret 0]; "docs/halo1/sounds.md";
    /// Apply the unit control packet (EAX unit datum, ECX control). The
    /// recording player calls this after its event decoder, at 0x1fb630.
    UNIT_APPLY_CONTROL @ 0x000c_6de0: [fn(unit: eax, control: ecx) ret 0]; "docs/halo1/campaign.md";
    /// The map cache copy machine: one state step per call (submit a
    /// 1 MiB read or write job, take its result, checksum, publish), AL
    /// returned and ignored by its callers. They step it only while the
    /// job-done byte `0x55bd0c` is set: the loading-screen loop `0x163890`
    /// (once a frame, at `0x1639bd`), the main loop `0x12b450` and the
    /// flush loop `0x213fa0` (back to back, nothing drawn).
    CACHE_COPY_STEP @ 0x0021_41f0: [fn() -> al ret 0]; "docs/runtime/cache-slots.md";
    // Slice 11 of docs/engine/sim-rewrite-objects.md: the small type updates.
    /// Item update: slot `+40` of `item` (weapons, equipment and garbage inherit it).
    ITEM_UPDATE @ 0x0010_b4d0: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Scenery update: slot `+40` of `scen`.
    SCENERY_UPDATE @ 0x0010_a2f0: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Device update: slot `+40` of `devi` (machines, controls and light fixtures inherit it).
    DEVICE_UPDATE @ 0x0010_6790: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Machine update: slot `+40` of `mach`.
    MACHINE_UPDATE @ 0x0011_52e0: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Machine slot `+48`: whether any of the machine's entries has its bit set in `bits`; 1 to `[out]` on a hit.
    MACHINE_SLOT_48 @ 0x0011_5a40: [fn(object: stack 0, bits: stack 1, out: stack 2) -> al ret 12], Hot; "docs/engine/object-update.md";
    /// Projectile update: slot `+40` of `proj`; calls the projectile step `0xf8eb0`.
    PROJECTILE_UPDATE @ 0x000f_8de0: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Crate update: slot `+40` of `bloc`.
    CRATE_UPDATE @ 0x0011_bce0: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Creature update: slot `+40` of `crea`.
    CREATURE_UPDATE @ 0x0011_8700: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Creature post-physics update: slot `+44` of `crea`.
    CREATURE_POST_UPDATE @ 0x0011_8b80: [fn(object: stack 0) ret 4], Hot; "docs/engine/object-update.md";
    // Slice 11: the projectile step.
    /// Projectile step: one tick of a projectile's flight (homing, drag, gravity, collision, up to ten hits), called by `PROJECTILE_UPDATE`.
    PROJECTILE_STEP @ 0x000f_8eb0: [fn(object: stack 0, velocity: stack 1) ret 8], Hot; "docs/engine/object-update.md";
    // Slice 4 of docs/engine/sim-rewrite-objects.md: the post-update shell.
    /// The bounding spheres from the root node matrix, and the spatial partition relink; `al` 1 when they were stored.
    OBJECT_BOUNDS @ 0x000b_def0: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// The attachment refresh: light lists, the object's lights, then children and (with `recurse`) siblings.
    OBJECT_ATTACHMENTS @ 0x000b_ef30: [fn(object: stack 0, lights: stack 1, lists: stack 2, recurse: stack 3, keep: stack 4) ret 20], Hot; "docs/engine/object-update.md";
    /// Hands the node matrices to the slot `+74` dispatcher when the model has them.
    OBJECT_NODES_NOTIFY @ 0x000b_e1d0: [fn(object: eax) ret 0], Hot; "docs/engine/object-update.md";
    /// The physics component's transform and velocities into the object.
    OBJECT_COMPONENT_TRANSFORM @ 0x000b_c5e0: [fn(object: stack 0) ret 4], Hot; "docs/engine/object-update.md";
    /// Set position, forward and up (any may be null); `al` 0 when a position was outside the world.
    OBJECT_SET_TRANSFORM @ 0x000b_7430: [fn(object: stack 0, position: stack 1, forward: stack 2, up: stack 3, connect: stack 4, refresh: stack 5, notify: stack 6, value: stack 7, keep: stack 8) -> al ret 36], Hot; "docs/engine/object-update.md";
    /// Set linear and angular velocity (either may be null); `quiet` skips the replication mark.
    OBJECT_SET_VELOCITY @ 0x000b_7740: [fn(object: eax, linear: ecx, angular: edi, quiet: stack 0) ret 4], Hot; "docs/engine/object-update.md";
    /// The object's position in world space, through its parent's node matrix; returns `out`.
    OBJECT_WORLD_POSITION @ 0x000b_9dd0: [fn(object: eax, out: esi) -> eax ret 0], Hot; "docs/engine/object-update.md";
    /// The object's forward and up in world space (either output may be null).
    OBJECT_WORLD_BASIS @ 0x000b_9fc0: [fn(object: eax, forward: esi, up: edi) ret 0], Hot; "docs/engine/object-update.md";
    /// The root ancestor's linear and angular velocity (either output may be null).
    OBJECT_ROOT_VELOCITY @ 0x000b_a1d0: [fn(object: eax, linear: ecx, angular: stack 0) ret 4], Hot; "docs/engine/object-update.md";
    // Slice 5 of docs/engine/sim-rewrite-objects.md: lights.
    /// The tick's light walk: every live light's record update, relink and expiry.
    LIGHTS_UPDATE @ 0x000c_1400: [fn() ret 0], Hot; "docs/engine/object-update.md";
    /// A light record's modulation step (brightness, colour and the two outputs at `[out]`), called by `LIGHT_RECORD_UPDATE`.
    LIGHT_RECORD_PHASE @ 0x000c_0b00: [fn(colour: eax, block: ecx, light: stack 0, input: stack 1, brightness: stack 2, out: stack 3) ret 16], Hot; "docs/engine/object-update.md";
    /// Place an active light and add it to the light lists.
    LIGHT_LINK @ 0x000c_2d00: [fn(light: stack 0) ret 4], Hot; "docs/engine/object-update.md";
    /// Remove an active light from the light lists.
    LIGHT_UNLINK @ 0x000c_3220: [fn(light: eax) ret 0], Hot; "docs/engine/object-update.md";
    /// Expire a light: unlink it, update its marker parent's flag when `notify`, free the datum.
    LIGHT_EXPIRE @ 0x000c_3260: [fn(light: stack 0, notify: stack 1) ret 8], Hot; "docs/engine/object-update.md";
    // Slice 6a of docs/engine/sim-rewrite-objects.md: the unit update's shell and movement.
    /// Unit movement: the aim and look directions turned toward the desired ones (`UNIT_UPDATE`'s third call).
    UNIT_MOVEMENT @ 0x000c_60c0: [fn(unit: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Clamp a unit's direction (`look` 0: aim, 1: look) to its yaw and pitch limits; whether it clamped.
    UNIT_CLAMP_DIRECTION @ 0x000c_ba50: [fn(unit: eax, direction: stack 0, look: stack 1) -> al ret 8], Hot; "docs/engine/object-update.md";
    /// Whether the unit is free of seven states and of its held weapons' test (`0x100880`).
    UNIT_IDLE_TEST @ 0x000c_85c0: [fn(unit: eax) -> al ret 0], Hot; "docs/engine/object-update.md";
    // Slice 9 of docs/engine/sim-rewrite-objects.md: the weapon update.
    /// Weapon update: slot `+40` of `weap`, after the item update.
    WEAPON_UPDATE @ 0x000f_dfb0: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    // Slice 10 of docs/engine/sim-rewrite-objects.md: the vehicle update.
    /// Vehicle update: slot `+40` of `vehi`.
    VEHICLE_UPDATE @ 0x000e_fde0: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// The vehicle update's gate: `al` 1 when damage flags `+10a` bit 2 is
    /// set and the definition's vehicle type (`+1f0`) is 5, or 4 with
    /// definition `+218` equal to 1; the update then skips its physics.
    VEHICLE_UPDATE_GATE @ 0x000f_1320: [fn(object: eax) -> al ret 0], Hot; "docs/engine/object-update.md";
    /// Sets the first word of the vehicle update's 16 contact records
    /// (`d8` bytes each, from `buffer + bc0`) to none.
    VEHICLE_CONTACTS_RESET @ 0x000f_06c0: [fn(buffer: eax) ret 0], Hot; "docs/engine/object-update.md";
    // Slice 6b of docs/engine/sim-rewrite-objects.md: the unit update's state
    // children (`UNIT_CONTROL_REFRESH`, `ce0c0`, is named above).
    /// Unit control: desired aim and look, timed bits, the two linked objects (`UNIT_UPDATE`'s first call).
    UNIT_UPDATE_CONTROL @ 0x000c_49b0: [fn(unit: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Unit state: timers, unit states, held weapons (`UNIT_UPDATE`'s second call).
    UNIT_UPDATE_STATE @ 0x000c_58f0: [fn(unit: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// Powered seats: calls `UNIT_CONTROL_REFRESH`, then moves each seat's power (`UNIT_UPDATE`'s fourth call).
    UNIT_POWERED_SEATS @ 0x000c_5eb0: [fn(unit: eax) -> al ret 0], Hot; "docs/engine/object-update.md";
    // Slice 6c of docs/engine/sim-rewrite-objects.md: the rest of the unit
    // update (`UNIT_DIALOGUE_UPDATE` and `UNIT_LIGHT_UPDATE` are named above).
    /// The unit's timers (`+2e8`, `+2ca` with the deferred injury notice,
    /// `+1f4`); `al` whether any was running (`UNIT_UPDATE`'s fifth call).
    UNIT_TIMERS @ 0x000c_6740: [fn(unit: eax) -> al ret 0], Hot; "docs/engine/object-update.md";
    /// Active camouflage: the levels at `+2b0` and `+2b4` moved toward body
    /// `+134` bits 3 and 4, and the Assassins skull's AI camouflage.
    UNIT_CAMO_UPDATE @ 0x000c_6810: [fn(unit: eax) ret 0], Hot; "docs/engine/object-update.md";
    /// The unit's team `+138`: the player's, the actor's, or for a vehicle
    /// its seated units'.
    UNIT_TEAM_UPDATE @ 0x000c_6990: [fn(unit: eax) ret 0], Hot; "docs/engine/object-update.md";
    /// Start a seat iterator (`0x18` bytes at `iterator`) at the unit's top
    /// biped or vehicle ancestor.
    UNIT_SEATS_BEGIN @ 0x000d_0620: [fn(unit: eax, iterator: ebx) ret 0], Hot; "docs/engine/object-update.md";
    /// The iterator's next biped or vehicle (its handle at `+10`, seat at
    /// `+14`, parent at `+c`); `al` whether there was one.
    UNIT_SEATS_NEXT @ 0x000d_0690: [fn(iterator: edx) -> al ret 0], Hot; "docs/engine/object-update.md";
    /// The unit's animation state (`0xa0e` bytes; the vehicle update calls
    /// it): picks and starts the unit's two animation names and steps the
    /// limits block's three channels; `al` from its last two callees and
    /// the channels.
    UNIT_ANIMATION_STATE @ 0x0011_1650: [fn(unit: stack 0, record: stack 1) -> al ret 8], Hot; "docs/engine/object-update.md";
    // Slice 7a of docs/engine/sim-rewrite-objects.md: the biped update.
    /// Biped update: slot `+40` of `bipd`.
    BIPED_UPDATE @ 0x000d_d360: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// The first object up the parent chain from `object` (itself
    /// included) whose body `+4` bit 26 is clear; none when the chain ends
    /// first. The biped update's first call, on its parent.
    OBJECT_CARRIER @ 0x000b_af40: [fn(object: eax) -> eax ret 0], Hot; "docs/engine/object-update.md";
    /// The biped's counter at body `+39d`, counted down; at 1 in state 3 it
    /// starts an animation (`10f430`). `al` 1 when it counted.
    BIPED_TIMER_UPDATE @ 0x000e_2d80: [fn(object: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// A seconds value times the tick rate, rounded by `fistp` (`ecx`
    /// returns the same integer).
    GAME_SECONDS_TO_TICKS @ 0x0014_69f0: [fn(seconds: stack 0) -> eax ret 4], Hot; "docs/engine/object-update.md";
    /// An action request for the unit: eight words on the stack, the action
    /// first, handed to `0xe6900`.
    BIPED_ACTION @ 0x000e_68c0: [fn(action: ecx, object: stack 0) ret 4], Hot; "docs/engine/object-update.md";
    /// The animation kind (0, 1, or 2 for any other) of an animation name
    /// (string ID).
    BIPED_ANIMATION_KIND @ 0x000d_d300: [fn(name: ecx) -> al ret 0], Hot; "docs/engine/object-update.md";
    /// `eax` 1 when an unparented biped in state 1 or 3 has been in the air
    /// (body `+348` bit 0) long enough to land, else 0.
    BIPED_AIRBORNE_TEST @ 0x000e_4050: [fn(object: eax) -> eax ret 0], Hot; "docs/engine/object-update.md";
    /// After the fall counter's time, the root velocity against a speed;
    /// above it, `0xe4d90` with kind 2.
    BIPED_FALL_UPDATE @ 0x000e_45e0: [fn(object: esi) ret 0], Hot; "docs/engine/object-update.md";
    /// The animation kind's counter at body `+39b` and, for two animation
    /// names, `0xe4d90` with kind 3 and a weight.
    BIPED_ANIMATION_STATE_UPDATE @ 0x000e_4c50: [fn(object: edi) ret 0], Hot; "docs/engine/object-update.md";
    /// The look toward the target (body `+13c`): a head marker, the aim
    /// clamped to the look limits, and body `+3cc..3d4` stepped toward the
    /// result.
    BIPED_LOOK_UPDATE @ 0x000d_e190: [fn(object: eax) ret 0], Hot; "docs/engine/object-update.md";
    // Slice 7b of docs/engine/sim-rewrite-objects.md: the biped
    // post-physics update.
    /// Biped post-physics update: slot `+44` of `bipd`.
    BIPED_POST_UPDATE @ 0x000d_d990: [fn(object: stack 0) ret 4], Hot; "docs/engine/object-update.md";
    /// The step of the crouch fraction (body `+2c4`) this tick: the
    /// definition's rate (an actor may scale it) times the tick length,
    /// toward 1 or 0 by the control bit (body `+134` bit 23).
    BIPED_CROUCH_STEP @ 0x000d_d7d0: [fn(object: eax) -> xmm0 ret 0], Hot; "docs/engine/object-update.md";
    /// The height the crouch step moves the biped this tick into `out`,
    /// `al` 1 when there is one (landing or forced to crouch, the step not
    /// zero).
    BIPED_CROUCH_HEIGHT @ 0x000d_d8e0: [fn(object: edi, out: stack 0) -> al ret 4], Hot; "docs/engine/object-update.md";
    /// `al` 1 in state 6 with body `+3f8` set and `+348` bit 13.
    BIPED_CROUCH_FORCED @ 0x000e_0dc0: [fn(object: eax) -> al ret 0], Hot; "docs/engine/object-update.md";
    /// A collision query below an airborne biped and the stuck counter at
    /// body `+3ab`.
    BIPED_GROUND_PROBE @ 0x000e_30c0: [fn(object: ebx) ret 0], Hot; "docs/engine/object-update.md";
    /// `al` the flag bit of the surface the biped stands on (body `+354`),
    /// 0 with a contact object.
    BIPED_GROUND_FLAG @ 0x000d_df60: [fn(object: eax) -> al ret 0], Hot; "docs/engine/object-update.md";
    /// The ground and jump counters at body `+39c` and `+39e`, and the
    /// jump `0xe4770` when the control asks for it; `al` 1 when it jumped.
    BIPED_JUMP_UPDATE @ 0x000e_4680: [fn(object: edi) -> al ret 0], Hot; "docs/engine/object-update.md";
    /// Fills the physics step's input record (`0x1e68a0`) and, by the
    /// state, its fields `+44`, `+48`, `+4c`.
    BIPED_PHYSICS_INPUT @ 0x000d_d0d0: [fn(object: stack 0, record: stack 1, flag: stack 2) ret 12], Hot; "docs/engine/object-update.md";
    /// The landing's kind and ticks at body `+34c..34f`, from a speed in
    /// `xmm2` against the definition's bands.
    BIPED_LANDING_UPDATE @ 0x000e_4330: [fn(object: eax) ret 0], Hot; "docs/engine/object-update.md";
    /// The contact object at body `+394` and its counter `+398`.
    BIPED_CONTACT_UPDATE @ 0x000e_2fa0: [fn(object: eax, contact: ebx, unused: stack 0) ret 4], Hot; "docs/engine/object-update.md";
}

/// The point at `base`, if the table names one.
pub fn by_base(base: u32) -> Option<&'static Point> {
    ALL.iter().copied().find(|p| p.base == base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_address_is_named_once() {
        let mut seen = std::collections::BTreeMap::new();
        for p in ALL {
            assert!(seen.insert(p.base, p.name).is_none(), "{} twice", p.name);
            assert!(p.doc.starts_with("docs/"), "{}: doc {}", p.name, p.doc);
        }
        for g in GLOBALS {
            assert!(seen.insert(g.base, g.name).is_none(), "{} twice", g.name);
        }
    }

    #[test]
    fn the_macro_reads_signatures_and_flags() {
        assert_eq!(
            GAME_TICK.sig,
            Sig::Fn {
                args: &[],
                ret: None,
                pop: 0
            }
        );
        assert_eq!(GAME_ACTIVATE.sig.arg("options"), Some(Loc::Eax));
        assert_eq!(WEAPON_FIRE.sig.arg("barrel"), Some(Loc::Stack(1)));
        assert_eq!(WEAPON_FIRE.class, Class::Optional);
        assert!(LOCALE_FIND.hot && !GAME_TICK.hot);
        assert!(PLAYER_KILLED.sig.is_opaque());
        assert_eq!(by_base(0x0013_7fe0).map(|p| p.name), Some("GAME_TICK"));
    }
}
