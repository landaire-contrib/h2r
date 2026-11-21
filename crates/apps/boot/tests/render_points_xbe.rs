//! The native renderer's hook points are function entries the recompiler
//! discovers in the retail XBE, so a hook can be placed on each. The
//! transparents' geometry handler (`points::TRANSPARENT_GEOMETRY`) is only
//! reached through the entry's function pointer (stored by `mov [esi+0x20],
//! imm32` at `0x000413ff`), so it rests on discovery following code
//! immediates (docs/graphics/native-transparents.md, "Stage 3: transparents").
//! Addresses only; no bytes are kept.
//!
//! `H2_XBE=/path/default.xbe cargo test -p boot --test render_points_xbe -- --ignored`

use engine::points;

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn the_native_renderers_points_are_discovered_functions() {
    let data = std::fs::read(std::env::var_os("H2_XBE").expect("set H2_XBE to a retail default.xbe")).unwrap();
    let xbe = xbe::Xbe::parse(&data).unwrap();
    let analysis = analyze::analyze(&xbe).unwrap();
    let entries = analyze::dispatch::entries(&xbe, &analysis);
    for p in [
        &points::LAYER_DRAW,
        &points::SCENE_TRANSPARENTS,
        &points::TRANSPARENT_GEOMETRY,
        &points::SCENE_CLUSTER_GATHER,
        &points::SCENE_GEOMETRY_GATHER,
        &points::SCENE_OBJECT_ORDER,
        &points::VISIBLE_LIGHT_LIST,
        &points::LIGHT_GEOMETRY_FLAGS,
        &points::LIGHT_EXTRA_CASTER,
        &points::RESET_SCENE_RECORDS,
        &points::OBJECT_RENDER_RECORDS,
        &points::MODEL_LOD,
        &points::SCENE_RECORD_WRITER,
        &points::TRANSPARENT_EFFECT_ENTRY,
        &points::TRANSPARENT_POLYGON_ENTRY,
        &points::OBJECT_RENDER_SETUP,
        &points::RENDER_DYNAMIC_LIGHTS,
        &points::RENDER_LIGHT_PASSES,
        &points::LIGHT_GEOMETRY_DRAW,
        &points::LIGHT_RESTORE_TARGET,
        &points::LAYER_BUILD,
        &points::SCREEN_EFFECT,
        &points::TEMPLATE_PASS,
        &points::TEMPLATE_QUAD,
    ] {
        assert!(
            analysis.functions.contains_key(&p.base),
            "{} ({:#010x}) is not a discovered function",
            p.name,
            p.base
        );
        assert!(entries.contains(&p.base), "{} ({:#010x}) is not an entry", p.name, p.base);
    }
}

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn the_native_scene_list_sites_are_instruction_boundaries() {
    let data = std::fs::read(std::env::var_os("H2_XBE").expect("set H2_XBE to a retail default.xbe")).unwrap();
    let xbe = xbe::Xbe::parse(&data).unwrap();
    let mut analysis = analyze::analyze(&xbe).unwrap();
    let sites = [
        points::OBJECT_RECORD_APPEND.base,
        points::TRANSPARENT_GEOMETRY_ENTRY.base,
        points::TRANSPARENT_GEOMETRY_QUEUED.base,
        points::TRANSPARENT_ORDER_BEGIN.base,
        points::TRANSPARENT_ORDER_READY.base,
    ];
    for site in sites.into_iter().chain([points::OBJECT_RECORD_APPENDED]) {
        assert!(matches!(analysis.claim(site), Some(('I', Some(_)))), "{site:#010x}");
    }
    analysis.expose_instructions(points::scene_effects::instruction_entries());
    for site in sites {
        assert!(analysis.functions.contains_key(&site), "{site:#010x}");
    }
}

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn render_aspect_sites_are_dispatch_blocks_in_the_build_tiers_analysis() {
    let data = std::fs::read(std::env::var_os("H2_XBE").expect("set H2_XBE")).unwrap();
    let xbe = xbe::Xbe::parse(&data).unwrap();
    assert!(recomp::has_retail_render_sites(&xbe));
    let (analysis, _) = recomp::analyze_cached(&xbe, recomp::analysis_cache(&data).as_deref()).unwrap();
    let builder = analyze::cfg::CfgBuilder::new(&analysis, &xbe);
    for pc in points::render_aspect::instruction_entries().chain(points::scene_effects::instruction_entries()) {
        assert!(analysis.functions.contains_key(&pc), "missing render hook entry {pc:#010x}");
        let cfg = builder.build(pc).unwrap();
        assert!(cfg.blocks.contains_key(&pc), "no block for render hook {pc:#010x}");
    }
}
