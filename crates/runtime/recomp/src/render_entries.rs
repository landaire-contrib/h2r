//! Additional entry boundaries shared by fast and Clang emission.

use guest_layout::points::{render_aspect, scene_effects};

/// Whether named instruction sites in the base retail image apply. Other
/// titles and title updates keep their own discovered instruction boundaries.
pub fn has_retail_render_sites(x: &xbe::Xbe) -> bool {
    x.certificate.title_id == 0x4d53_0064 && x.entry == 0x002d_0aee
}

pub(super) fn expose(x: &xbe::Xbe, analysis: &mut analyze::Analysis) {
    if has_retail_render_sites(x) {
        analysis.expose_instructions(render_aspect::instruction_entries().chain(scene_effects::instruction_entries()));
    }
}
