//! The `native-hud` feature (docs/graphics/native-post-hud.md, stage 5):
//! its plan (context-only hooks the pipelined render runs on its own
//! thread; none with the master switch off).

use render_bridge::native_hud::{self, NativeHud};
use hooks::plan::Base;
use hooks::{Registry, Switches};

fn plan(switches: &str) -> hooks::plan::Plan {
    let mut registry = Registry::new(Switches::parse(switches).unwrap());
    registry.add(&NativeHud).unwrap();
    registry.build(&Base).unwrap()
}

#[test]
fn the_native_ui_defaults_on_and_the_master_disables_it() {
    for (spec, expected) in [("", true), ("impl.render.native=off", false), ("impl.render=off", true)] {
        assert_eq!(native_hud::on(&Switches::parse(spec).unwrap()), expected);
    }
    assert!(plan("impl.render.native=off").chains.is_empty());
}

#[test]
fn the_ui_hooks_can_run_on_the_render_thread() {
    let plan = plan("");
    let points = [
        "UI_RENDER_LAYERS", "UI_ITEM_CALL", "UI_ITEM_RETURN", "UI_SCREEN_QUAD_READY",
        "UI_LOADING_RETURN", "UI_LOADING_EMPTY_RETURN", "UI_FADE_FAN_READY", "HUD_QUAD_READY",
    ];
    for point in points {
        let chain = plan.chains.values().find(|c| c.point.name == point)
            .unwrap_or_else(|| panic!("placed at {point}"));
        assert!(!chain.needs_shadow(), "{point}: nothing after the body");
        assert!(chain.render_thread().is_ok(), "{point}");
    }
}

#[test]
fn ui_shares_the_loading_and_effect_points_with_other_features() {
    use engine::points::ui_render::LOADING_DRAW;
    use render_bridge::native_particles::{DYNAMIC_QUAD_READY, NativeParticles};
    use render_bridge::render_timing::RenderTiming;

    let mut registry = Registry::new(Switches::parse("render-timing").unwrap());
    registry.add(&NativeHud).unwrap();
    registry.add(&NativeParticles).unwrap();
    registry.add(&RenderTiming::new()).unwrap();
    let plan = registry.build(&Base).unwrap();
    assert!(std::ptr::eq(plan.chains[&LOADING_DRAW.base].point, &LOADING_DRAW));
    assert!(std::ptr::eq(
        plan.chains[&DYNAMIC_QUAD_READY.base].point,
        &DYNAMIC_QUAD_READY,
    ));
    let text = plan.text();
    assert!(text.contains("native_ui_inline_rect"));
    assert!(text.contains("native_effects_walk_quad"));
}
