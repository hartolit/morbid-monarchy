use bevy::prelude::*;
use monarch_engine::prelude::*;

use crate::runtime::{input::*, persistence, render::WorldRenderPlugin};

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
                // Prevents anti-aliasing blur on textures
                .set(ImagePlugin::default_nearest()),
        )
        .add_plugins(MonarchEnginePlugin)
        .add_plugins(WorldRenderPlugin)
        .insert_resource(world_db)
        .insert_resource(persistence::WorldSeed(startup_seed))
        .init_resource::<persistence::ChunkSaveQueue>()
        .insert_resource(persistence::SaveTimer(Timer::from_seconds(
            2.0,
            TimerMode::Repeating,
        )))
        .add_systems(Startup, setup_focal_point)
        .add_systems(
            Update,
            (
                player_movement,
                handle_resize_input,
                persistence::handle_load_requests,
                persistence::handle_unload_events,
                persistence::process_save_queue,
                persistence::emergency_flush_on_exit,
                persistence::poll_load_tasks,
                persistence::poll_save_tasks,
                sync_world_focus,
            ),
        )
        .run();
}
