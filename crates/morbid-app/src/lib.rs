use bevy::prelude::*;
use monarch_engine::{
    MonarchEnginePlugin,
    world::{ChunkManager, WorldFocus, events::ResizeSimulationEvent},
};

use crate::render::WorldRenderPlugin;

mod database;
mod render;

pub fn run() {
    let world_db = database::initialize_database();
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
        .insert_resource(database::WorldSeed(startup_seed))
        .init_resource::<database::ChunkSaveQueue>()
        .insert_resource(database::SaveTimer(Timer::from_seconds(
            2.0,
            TimerMode::Repeating,
        )))
        .add_systems(Startup, setup_focal_point)
        .add_systems(
            Update,
            (
                player_movement,
                handle_resize_input,
                database::handle_load_requests,
                database::handle_unload_events,
                database::process_save_queue,
                database::emergency_flush_on_exit,
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
    commands.spawn((
        FocalPoint,
        Camera2d,
        // Zoom in 2x so each grid cell takes up an 8x8 block of pixels on screen
        Transform::from_translation(Vec3::ZERO), //.with_scale(Vec3::splat(0.125)),
    ));
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

/// Listens for +/- keys to dynamically resize the simulation grid
fn handle_resize_input(
    keyboard: Res<ButtonInput<KeyCode>>,
    manager: Res<ChunkManager>,
    mut writer: MessageWriter<ResizeSimulationEvent>,
) {
    let mut new_radius_x = manager.active_radius_x;
    let mut new_radius_y = manager.active_radius_y;
    let mut changed = false;

    if keyboard.just_pressed(KeyCode::ArrowLeft) {
        new_radius_x += 1;
        changed = true;
    }

    if (keyboard.just_pressed(KeyCode::ArrowRight)) && new_radius_x > 0 {
        new_radius_x -= 1;
        changed = true;
    }

    if keyboard.just_pressed(KeyCode::ArrowUp) {
        new_radius_y += 1;
        changed = true;
    }

    if (keyboard.just_pressed(KeyCode::ArrowDown)) && new_radius_y > 0 {
        new_radius_y -= 1;
        changed = true;
    }

    if changed {
        info!(
            "Resizing Simulation: radius_x {} radius_y {}",
            new_radius_x, new_radius_y
        );

        writer.write(ResizeSimulationEvent {
            new_active_radius_x: new_radius_x,
            new_active_radius_y: new_radius_y,
        });
    }
}

fn sync_world_focus(target: Single<&Transform, With<FocalPoint>>, mut focus: ResMut<WorldFocus>) {
    focus.position = target.translation.as_dvec3();
}
