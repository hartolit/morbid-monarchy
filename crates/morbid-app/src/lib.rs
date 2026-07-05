use bevy::prelude::*;
use monarch_engine::{core::entities::observer::resolve_observer_kinematics, prelude::*};

use crate::runtime::{
    dev_tools::DevToolsPlugin,
    input::{
        drive_possessed_sphere, handle_possession_toggle, manage_os_cursor_boundary,
        observer_hardware_ingest, setup_observer, sync_lens_orientation, sync_world_focus,
        update_camera_tether_and_zoom,
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
        .add_systems(Startup, setup_observer)
        .add_systems(
            Update,
            (
                manage_os_cursor_boundary,
                handle_possession_toggle,
                drive_possessed_sphere,
                observer_hardware_ingest,
                sync_lens_orientation,
            )
                .chain(),
        )
        .add_systems(
            Update,
            (
                sync_world_focus.after(sync_lens_orientation),
                update_camera_tether_and_zoom.after(resolve_observer_kinematics),
            ),
        )
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
