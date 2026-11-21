//! Interior sites the native render stages hook. Each must be an entry of
//! the generated image, so both image tiers expose them however the builder
//! was configured (a Steam Machine module is built on another machine).
use super::{
    Point, Sig, OBJECT_RECORD_APPEND, TRANSPARENT_GEOMETRY_ENTRY, TRANSPARENT_GEOMETRY_QUEUED,
    TRANSPARENT_ORDER_BEGIN, TRANSPARENT_ORDER_READY, ui_render::*,
};

const RENDERER: &str = "docs/graphics/native-renderer.md";
const PARTICLES: &str = "docs/graphics/native-particles.md";
const DECALS: &str = "docs/graphics/native-decals-research.md";

// The per-record duplicate marker, bitmap and default registers have
// already been set. ESI still names the record about to build a draw.
pub const DECAL_ITEM_GEOMETRY: Point = Point::new("DECAL_ITEM_GEOMETRY", 0x0004_4161, Sig::Site, DECALS);
pub const DECAL_RENDER_END: Point = Point::new("DECAL_RENDER_END", 0x0004_41aa, Sig::Site, DECALS);

// The contiguous-range branch of the standard geometry emitter, after
// state commit. EDI holds the index array, ESI its count, BL its topology.
pub const GEOMETRY_RANGE_READY: Point = Point::new("SCENE_GEOMETRY_RANGE_READY", 0x0004_dad0, Sig::Site, RENDERER);
// All exits from the sorted loop join here, before the guest view cleanup.
pub const SCENE_TRANSPARENTS_END: Point = Point::new("SCENE_TRANSPARENTS_END", 0x0001_e406, Sig::Site, RENDERER);

pub const HUD_QUAD_READY: Point = Point::new("HUD_QUAD_READY", 0x0022_a7f6, Sig::Site, RENDERER);

/// The screen strip's BEGIN is published and its current row is known.
pub const SCREEN_STRIP_READY: Point = Point::new("SCREEN_STRIP_READY", 0x0002_d565, Sig::Site, RENDERER);

// After the allocator succeeds and the caller's preserved ESI/EDI have
// been popped. ESP is still one word below the emitter's entry frame.
pub const PARTICLE_EMIT_READY: Point = Point::new("PARTICLE_EMIT_READY", 0x0023_ed5c, Sig::Site, PARTICLES);
// Publication boundaries, observed before the indicated instruction. These
// keep every hook context-only: no Rust continuation around a guest body.
pub const PARTICLE_PUBLISH: Point = Point::new("PARTICLE_PUBLISH", 0x0023_ef77, Sig::Site, PARTICLES);
pub const PARTICLE_END: Point = Point::new("PARTICLE_END", 0x0023_ef79, Sig::Site, PARTICLES);
// Shared with the UI rectangle hook. The planner keys points by identity.
pub static EFFECT_QUAD_READY: Point = Point::new("EFFECT_QUAD_READY", 0x0004_b252, Sig::Site, PARTICLES);
pub const EFFECT_PUBLISH: Point = Point::new("EFFECT_PUBLISH", 0x0004_b28c, Sig::Site, PARTICLES);
pub const EFFECT_END: Point = Point::new("EFFECT_END", 0x0004_b2a3, Sig::Site, PARTICLES);

/// One researched immediate-mode effect leaf: its BEGIN and its END.
#[derive(Debug)]
pub struct EffectLeaf {
    pub begin: Point,
    pub end: Point,
}

const fn leaf(begin: u32, end: u32) -> EffectLeaf {
    EffectLeaf {
        begin: Point::new("EFFECT_IMMEDIATE_BEGIN", begin, Sig::Site, PARTICLES),
        end: Point::new("EFFECT_IMMEDIATE_END", end, Sig::Site, PARTICLES),
    }
}

pub const EFFECT_BILLBOARD: EffectLeaf = leaf(0x0004_8497, 0x0004_849d);
pub const EFFECT_DEPTH_BILLBOARD: EffectLeaf = leaf(0x0004_8ae5, 0x0004_8aeb);
pub const EFFECT_WEATHER_LAYER: EffectLeaf = leaf(0x0003_8342, 0x0003_8349);
pub const EFFECT_WEATHER_PARTICLES: EffectLeaf = leaf(0x0003_96f7, 0x0003_96f9);
pub const EFFECT_MATERIAL_STRIP: EffectLeaf = leaf(0x0019_fc3e, 0x0019_fc40);

pub const EFFECT_QUERY: Point = Point::new("EFFECT_QUERY", 0x0002_e9b6, Sig::Site, PARTICLES);
pub const EFFECT_LAYERED_A: Point = Point::new("EFFECT_LAYERED_A", 0x0004_1cc0, Sig::Site, PARTICLES);
pub const EFFECT_LAYERED_B: Point = Point::new("EFFECT_LAYERED_B", 0x0004_23c0, Sig::Site, PARTICLES);

/// Every site above, the scene list sites and the UI walk's interior
/// sites, each once.
pub fn instruction_entries() -> impl Iterator<Item = u32> {
    let leaves = [
        &EFFECT_BILLBOARD,
        &EFFECT_DEPTH_BILLBOARD,
        &EFFECT_WEATHER_LAYER,
        &EFFECT_WEATHER_PARTICLES,
        &EFFECT_MATERIAL_STRIP,
    ];
    [
        DECAL_ITEM_GEOMETRY.base,
        DECAL_RENDER_END.base,
        GEOMETRY_RANGE_READY.base,
        SCENE_TRANSPARENTS_END.base,
        HUD_QUAD_READY.base,
        SCREEN_STRIP_READY.base,
        PARTICLE_EMIT_READY.base,
        PARTICLE_PUBLISH.base,
        PARTICLE_END.base,
        EFFECT_QUAD_READY.base,
        EFFECT_PUBLISH.base,
        EFFECT_END.base,
        EFFECT_QUERY.base,
        EFFECT_LAYERED_A.base,
        EFFECT_LAYERED_B.base,
        OBJECT_RECORD_APPEND.base,
        TRANSPARENT_GEOMETRY_ENTRY.base,
        TRANSPARENT_GEOMETRY_QUEUED.base,
        TRANSPARENT_ORDER_BEGIN.base,
        TRANSPARENT_ORDER_READY.base,
        LAYERS_RETURN.base,
        ITEM_CALL.base,
        ITEM_RETURN.base,
        HUD_RETURN.base,
        HUD_HIDDEN_RETURN.base,
        LOADING_RETURN.base,
        LOADING_EMPTY_RETURN.base,
        SCREEN_QUAD_READY.base,
        FADE_FAN_READY.base,
        FADE_RETURN.base,
        OVERLAY_RECT_RETURN.base,
        LOADING_SPINNER_READY.base,
        LOADING_BITMAP_READY.base,
        OUTLINE_VERTICES.base,
        OUTLINE_VERTICES_TAIL,
        BACKDROP_WASH.base,
        BACKDROP_TINT.base,
        BACKDROP_COMPOSITE.base,
        MODEL_VIEW_CALL.base,
        MODEL_VIEW_RETURN.base,
        MODEL_GEOMETRY_CALL.base,
    ]
    .into_iter()
    .chain(leaves.into_iter().flat_map(|l| [l.begin.base, l.end.base]))
}

#[cfg(test)]
mod tests {
    #[test]
    fn instruction_entries_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for address in super::instruction_entries() {
            assert!(seen.insert(address), "duplicate entry {address:#010x}");
        }
        assert!(seen.contains(&super::EFFECT_QUAD_READY.base));
    }
}
