use bevy::prelude::*;
use monarch_engine::{MonarchEnginePlugin, world::types::WorldFocus};

mod database;

pub fn run() {
    let world_db = database::initialize_database();

    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(MonarchEnginePlugin)
        .insert_resource(world_db)
        .add_systems(Startup, setup_focal_point)
        .add_systems(
            Update,
            (
                database::handle_load_requests,
                database::handle_unload_events,
                database::poll_load_tasks,
                database::poll_save_tasks,
                sync_world_focus,
            ),
        )
        .run();
}

/// A marker for whatever entity is driving chunk generation (Player, Camera, etc.)
#[derive(Component)]
struct FocalPoint;

fn setup_focal_point(mut commands: Commands) {
    // Spawn a dummy target at the origin.
    // In the future, attach this component to your Player or Main Camera.
    commands.spawn((FocalPoint, Transform::from_translation(Vec3::ZERO)));
}

/// Copies the Transform of the focal point into the engine's pure math resource.
fn sync_world_focus(target: Single<&Transform, With<FocalPoint>>, mut focus: ResMut<WorldFocus>) {
    focus.position = target.translation.as_dvec3();
}
