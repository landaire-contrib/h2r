//! Our additions to the game: menus and settings screens, skulls, variants,
//! the campaign picker, gameplay hooks and the glue to the platform crates.
//!
//! `boot` is the composition root above this crate and re-exports it.

// Build and load: `crate::fallback`, `crate::tier`, `stamp_path` and the
// rest are `recomp`'s.
pub use recomp::*;
pub use live_telemetry as telemetry;
pub use live_hle::{HostPublisher, LiveConfig};
pub use run_opts::{BetweenSlices, Host, RunOpts, RunReport, VfsReady};

pub mod account_screen;
pub mod agent_frames;
mod agent_spawn_view;
mod spectator_view;
pub mod agent_glue;
pub mod aim_assist;
pub mod attract;
mod audio_loop_agent;
mod audio_loops;
pub mod campaign_cache;
pub mod campaign_cards;
pub mod campaign_info;
pub mod campaign_menu;
pub mod campaigns;
pub mod cheats;
pub mod community_variants;
pub mod controls_text;
pub mod coop_jip;
pub mod coop_latency;
pub mod coop_live;
pub mod coop_look;
pub mod coop_players;
pub mod coop_probe;
pub mod custom_games;
pub mod d3d_diag;
pub mod d3d_glue;
pub mod debug_menu_glue;
pub mod dsound_glue;
pub mod fov_row;
#[cfg(feature = "gpu")]
pub mod render_aspect;
pub mod game_setup_help;
pub mod h1_recordings;
pub mod h1_conversation;
pub mod h1_drive;
pub mod h1_options;
pub mod h1_options_row;
pub mod h1_particle_physics;
pub mod h1_flag;
pub mod h1_glow;
pub mod h1_lightning;
pub mod h1_sound_promotion;
pub mod h1_cinematics;
pub mod h1_script_builtins;
pub mod h1_squad_storage;
pub mod h1_variants;
pub mod h1_ai_control;
pub mod h1_command_atoms;
pub mod health;
pub mod host_audit;
pub mod host_lobby;
pub mod host_metrics;
pub mod host_mem;
pub mod host_perf;
pub mod hot_glue;
pub mod hud;
pub mod hud_swap;
pub mod h1_hud_text;
pub mod icons;
pub mod join_link;
pub mod keyboard_glue;
pub mod layout_glue;
pub mod level_boot;
pub mod list_class;
#[cfg(feature = "gpu")]
pub mod lod_distance;
mod lobby_quality;
mod lobby_status;
pub mod live_glue;
pub mod live_hopper;
pub mod live_notice;
pub mod lobby_options;
mod menu_music;
pub mod loaded_map;
pub mod lockstep_glue;
pub mod map_bases;
pub mod map_copy;
pub mod map_override;
pub mod map_select;
pub mod map_slots;
pub mod mod_slots;
pub mod mods_screen;
pub mod movies_glue;
#[cfg(feature = "gpu")]
pub mod native_decals_glue;
#[cfg(feature = "gpu")]
pub mod native_decorators_glue;
#[cfg(feature = "gpu")]
pub mod native_first_person_glue;
#[cfg(feature = "gpu")]
pub mod native_first_person_load;
#[cfg(feature = "gpu")]
pub mod native_fog_glue;
#[cfg(feature = "gpu")]
pub mod native_gels_glue;
#[cfg(feature = "gpu")]
pub mod native_layers_glue;
#[cfg(feature = "gpu")]
pub mod native_gels_replace;
#[cfg(feature = "gpu")]
pub mod native_hud_glue;
#[cfg(feature = "gpu")]
pub mod native_lights_glue;
#[cfg(feature = "gpu")]
pub mod native_map;
#[cfg(feature = "gpu")]
pub mod native_shadows_draw;
#[cfg(feature = "gpu")]
pub mod native_shadows_scope;
#[cfg(feature = "gpu")]
pub mod native_shadows_glue;
#[cfg(feature = "gpu")]
pub mod native_objects_glue;
#[cfg(feature = "gpu")]
pub mod native_particles_glue;
#[cfg(feature = "gpu")]
pub mod native_particles_admission;
#[cfg(feature = "gpu")]
pub mod native_post_glue;
#[cfg(feature = "gpu")]
pub mod native_post_load;
#[cfg(feature = "gpu")]
pub mod native_residual_glue;
#[cfg(feature = "gpu")]
pub mod native_transparents_glue;
#[cfg(feature = "gpu")]
pub mod native_water_glue;
#[cfg(feature = "gpu")]
pub mod native_world_glue;
#[cfg(feature = "gpu")]
pub mod native_world_layers_glue;
#[cfg(feature = "gpu")]
pub mod opaque_streams;
#[cfg(feature = "gpu")]
pub mod scene_glue;
pub mod new_maps;
#[cfg(feature = "test-maps")]
pub mod test_maps;
pub mod pad_lightbar;
pub mod pad_speaker;
pub mod precision;
#[cfg(feature = "desktop-runtime")]
pub mod render_probe;
#[cfg(feature = "desktop-runtime")]
pub mod render_thread;
pub mod retail_rows;
#[cfg(feature = "gpu")]
pub mod return_to_menu;
pub mod run_plan;
pub mod sampling;
pub mod bot_accuracy;
pub mod bot_glue;
pub mod bot_mark;
pub mod bots_nav_auto;
pub mod clanker_card;
pub mod clankermatch;
pub mod clankers;
pub mod pause_menu;
pub mod profile_voice_row;
pub mod setting_defs;
pub mod sha;
pub mod signin_keyboard;
pub mod signin_screen;
pub mod sim_bots;
pub mod sim_glue;
pub mod skulls;
pub mod skulls_custom;
pub mod skulls_menu;
pub mod skulls_mp;
pub mod skulls_mp_row;
pub mod skulls_screen;
pub mod snapshot_glue;
pub mod snapshot_probe;
pub mod speaker_row;
pub mod speaker_setting;
#[cfg(feature = "local-io")]
pub mod steam_speaker;
pub mod stopgaps;
pub mod sweep_diag;
pub mod theater;
pub mod theater_ui;
pub mod touch_glue;
pub mod trace;
pub mod ui;
mod ui_audit;
pub mod ui_back;
pub mod ui_kit;
mod ui_press;
pub mod ui_stack;
mod ui_transition;
pub mod ui_watch;
pub mod variant_ext;
pub mod variant_options;
pub mod variant_page;
pub mod video_screen;
#[cfg(feature = "local-io")]
pub mod voice_glue;
pub mod voice_settings;
pub mod xapi_glue;
#[cfg(test)]
pub mod string_ids;
pub mod run_opts;
