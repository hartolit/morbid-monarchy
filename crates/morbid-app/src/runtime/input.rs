use bevy::prelude::*;
use monarch_engine::prelude::*;

/// A marker for whatever entity is driving chunk generation (Player, Camera, etc.)
#[derive(Component)]
pub struct FocalPoint;

pub fn setup_focal_point(mut commands: Commands) {
    commands.spawn((
        FocalPoint,
        Camera3d::default(),
        // Pull the camera back and angle it down to see the 3D relief
        Transform::from_xyz(0.0, 150.0, 150.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

pub fn player_movement(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut query: Query<&mut Transform, With<FocalPoint>>,
    time: Res<Time>,
) {
    let Ok(mut transform) = query.single_mut() else {
        return;
    };
    let mut direction = Vec3::ZERO;

    // Movement is mapped to the X/Z plane for Top-Down 3D
    if keyboard.pressed(KeyCode::KeyW) {
        direction.z -= 1.0;
    }
    if keyboard.pressed(KeyCode::KeyS) {
        direction.z += 1.0;
    }
    if keyboard.pressed(KeyCode::KeyA) {
        direction.x -= 1.0;
    }
    if keyboard.pressed(KeyCode::KeyD) {
        direction.x += 1.0;
    }

    if direction.length_squared() > 0.0 {
        let speed = 250.0;
        transform.translation += direction.normalize() * speed * time.delta_secs();
    }
}

pub fn handle_resize_input(
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

    if keyboard.just_pressed(KeyCode::ArrowRight) && new_radius_x > 0 {
        new_radius_x -= 1;
        changed = true;
    }

    if keyboard.just_pressed(KeyCode::ArrowUp) {
        new_radius_y += 1;
        changed = true;
    }

    if keyboard.just_pressed(KeyCode::ArrowDown) && new_radius_y > 0 {
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

pub fn sync_world_focus(
    target: Single<&Transform, With<FocalPoint>>,
    mut focus: ResMut<WorldFocus>,
) {
    // Map the 3D transform back to the engine's conceptual 3D space
    focus.position = target.translation.as_dvec3();
}
