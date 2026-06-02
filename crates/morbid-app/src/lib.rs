use bevy::prelude::*;
use monarch_engine::prelude::*;

use crate::runtime::{
    dev_tools::DevToolsPlugin,
    input::{
        manage_os_cursor_boundary, observer_hardware_ingest, setup_observer, sync_lens_orientation,
        sync_world_focus,
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
        // Instantiate the physical intent buffer and sensory lens
        .add_systems(Startup, setup_observer)
        // I/O Boundary: Harvest hardware deltas into the ObserverIntent membrane
        .add_systems(
            Update,
            (
                manage_os_cursor_boundary,
                observer_hardware_ingest,
                sync_lens_orientation,
            )
                .chain(),
        )
        // Engine Sync: Projects the post-integration physical state onto the thermodynamic grid
        .add_systems(Update, sync_world_focus.after(sync_lens_orientation))
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
