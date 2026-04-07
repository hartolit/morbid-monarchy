use bevy::prelude::*;
use monarch_engine::{MonarchEnginePlugin, world::types::WorldFocus};

mod world_io;

pub fn run() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(MonarchEnginePlugin)
        .add_systems(Startup, setup_focal_point)
        .add_systems(
            Update,
            (
                world_io::handle_load_requests,
                world_io::handle_unload_events,
                world_io::poll_load_tasks,
                world_io::poll_save_tasks,
                sync_world_focus,
            ),
        )
        .run();
}

/// A marker for whatever entity is driving chunk generation (Player, Camera, etc.)
#[derive(Component)]
struct FocalPoint;

fn setup_focal_point(mut commands: Commands) {
    // Make sure the world_data directory exists on boot
    let _ = std::fs::create_dir_all("world_data");

    // Spawn a dummy target at the origin.
    // In the future, attach this component to your Player or Main Camera.
    commands.spawn((FocalPoint, Transform::from_translation(Vec3::ZERO)));
}

/// Copies the Transform of the focal point into the engine's pure math resource.
fn sync_world_focus(target: Query<&Transform, With<FocalPoint>>, mut focus: ResMut<WorldFocus>) {
    if let Ok(transform) = target.get_single() {
        focus.position = transform.translation.as_dvec3();
    }
}
