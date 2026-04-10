use bevy::{prelude::*, window::WindowResolution};
use monarch_engine::{
    MonarchEnginePlugin,
    world::{ChunkManager, WorldFocus, events::ResizeSimulationEvent},
};

use crate::render::WorldRenderPlugin;

mod database;
mod render;

pub fn run() {
    let world_db = database::initialize_database();

    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Morbid Monarchy".to_string(),
                        resolution: WindowResolution::new(1024, 1024),
                        resizable: false, // Lock resolution for the 1024x1024 grid prototype
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
        .add_systems(Startup, setup_focal_point)
        .add_systems(
            Update,
            (
                player_movement,
                handle_resize_input,
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
    let mut new_radius = manager.active_radius;
    let mut changed = false;

    // Expand (Keys: +, =, or Numpad +)
    if keyboard.just_pressed(KeyCode::Equal) || keyboard.just_pressed(KeyCode::NumpadAdd) {
        new_radius += 1;
        changed = true;
    }

    // Shrink (Keys: - or Numpad -)
    if (keyboard.just_pressed(KeyCode::Minus) || keyboard.just_pressed(KeyCode::NumpadSubtract))
        && new_radius > 0
    {
        new_radius -= 1;
        changed = true;
    }

    if changed {
        info!("Resizing Simulation: Radius {}", new_radius);

        // Ensure the preload boundary stays comfortably ahead of the new active boundary
        let new_preload = manager
            .preload_radius
            .max(new_radius + manager.preload_trigger + 1);

        writer.write(ResizeSimulationEvent {
            new_active_radius: new_radius,
            new_preload_radius: new_preload,
        });
    }
}

fn sync_world_focus(target: Single<&Transform, With<FocalPoint>>, mut focus: ResMut<WorldFocus>) {
    focus.position = target.translation.as_dvec3();
}
