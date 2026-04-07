use bevy::prelude::*;
use monarch_engine::{MonarchEnginePlugin, world::types::WorldFocus};

use crate::render::WorldRenderPlugin;

mod database;
mod render;

pub fn run() {
    let world_db = database::initialize_database();

    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(MonarchEnginePlugin)
        .add_plugins(WorldRenderPlugin)
        .insert_resource(world_db)
        .add_systems(Startup, setup_focal_point)
        .add_systems(
            Update,
            (
                player_movement,
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
    commands
        .spawn((FocalPoint, Transform::from_translation(Vec3::ZERO)))
        .with_child(Camera2d);
}

fn player_movement(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut query: Query<&mut Transform, With<FocalPoint>>,
    time: Res<Time>,
) {
    let Ok(mut transform) = query.single_mut() else {
        return;
    };
    let mut direction = Vec3::ZERO;

    if keyboard.pressed(KeyCode::KeyW) {
        direction.y += 1.0;
    }
    if keyboard.pressed(KeyCode::KeyS) {
        direction.y -= 1.0;
    }
    if keyboard.pressed(KeyCode::KeyA) {
        direction.x -= 1.0;
    }
    if keyboard.pressed(KeyCode::KeyD) {
        direction.x += 1.0;
    }

    if direction.length_squared() > 0.0 {
        let speed = 500.0; // Pixels per second translation
        transform.translation += direction.normalize() * speed * time.delta_secs();
    }
}

fn sync_world_focus(target: Single<&Transform, With<FocalPoint>>, mut focus: ResMut<WorldFocus>) {
    focus.position = target.translation.as_dvec3();
}
