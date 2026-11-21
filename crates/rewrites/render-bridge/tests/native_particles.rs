use render_bridge::native_particles::{NativeParticles, DYNAMIC_QUAD_READY, EMIT_READY};
use hooks::plan::Base;
use hooks::{Registry, Switches};

#[test]
fn the_switch_off_and_no_parent_update_skip() {
    let mut r = Registry::new(Switches::parse("impl.render.native=off").unwrap());
    r.add(&NativeParticles).unwrap();
    assert!(!r.build(&Base).unwrap().text().contains("native_particles"));
    let mut r = Registry::new(Switches::defaults());
    r.add(&NativeParticles).unwrap();
    let plan = r.build(&Base).unwrap();
    assert_eq!(plan.chains.len(), 22);
    assert!(plan.chains.contains_key(&EMIT_READY.base));
    assert!(!plan.chains.contains_key(&native_render::particles::EMIT_QUAD), "the walk replaces the emit observer");
    assert!(plan.chains.contains_key(&DYNAMIC_QUAD_READY.base));
    assert!(plan
        .chains
        .contains_key(&native_render::effect_geometry::DYNAMIC_DRAW));
    let scene = engine::points::scene_effects::instruction_entries().collect::<Vec<_>>();
    for chain in plan.chains.values().filter(|c| c.point.name.starts_with("EFFECT_IMMEDIATE_")) {
        assert!(scene.contains(&chain.point.base));
    }
    assert_eq!(plan.chains.values().filter(|c| c.point.name.starts_with("EFFECT_IMMEDIATE_")).count(), 10);
    for chain in plan.chains.values() {
        assert_ne!(chain.point.base, native_render::particles::RENDER_UPDATE);
        assert!(chain.render_thread().is_ok());
        assert!(!chain.needs_shadow());
    }
}
