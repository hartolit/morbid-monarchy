use bevy::prelude::*;
use monarch_engine::prelude::*;

use crate::runtime::{
    dev_tools::DevToolsPlugin,
    input::{
        apply_camera_transform, center_camera_on_grid, orbit_camera, player_movement,
        setup_focal_point, sync_world_focus, zoom_camera,
    },
    persistence,
    render::WorldRenderPlugin,
};

mod runtime;

pub fn run() {
    let world_db = persistence::initialize_database();
    let startup_seed = 42;

    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Morbid Monarchy".to_string(),
                        ..default()
                    }),
                    ..default()
                })
                .set(ImagePlugin::default_nearest()),
        )
        .add_plugins(MonarchEnginePlugin)
        .add_plugins(WorldRenderPlugin)
        .add_plugins(DevToolsPlugin)
        .insert_resource(world_db)
        .insert_resource(persistence::WorldSeed(startup_seed))
        .init_resource::<persistence::ChunkSaveQueue>()
        .insert_resource(persistence::SaveTimer(Timer::from_seconds(
            2.0,
            TimerMode::Repeating,
        )))
        .add_systems(Startup, (setup_focal_point, center_camera_on_grid).chain())
        // Camera: pan mutates anchor, orbit/zoom mutate angles/distance,
        // then the single transform-derivation pass runs last in this group.
        .add_systems(
            Update,
            (
                player_movement,
                orbit_camera,
                zoom_camera,
                apply_camera_transform,
            )
                .chain(),
        )
        // Engine sync: runs after the camera group has settled.
        .add_systems(Update, (sync_world_focus).after(apply_camera_transform))
        // Persistence: independent of camera, runs every frame.
        .add_systems(
            Update,
            (
                persistence::handle_load_requests,
                persistence::handle_unload_events,
                persistence::process_save_queue,
                persistence::emergency_flush_on_exit,
                persistence::poll_load_tasks,
                persistence::poll_save_tasks,
            ),
        )
        .run();
}
