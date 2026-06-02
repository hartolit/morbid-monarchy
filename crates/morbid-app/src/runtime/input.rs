use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use monarch_engine::{
    engine::entities::observer::{KinematicObserver, ObserverConfig, ObserverIntent},
    prelude::*,
};

#[derive(Component)]
pub struct ObserverLens;

pub fn setup_observer(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let start_pos = Vec3::new(CHUNK_SIZE as f32 / 2.0, 150.0, -(CHUNK_SIZE as f32) / 2.0);

    commands
        .spawn((
            KinematicObserver::default(),
            ObserverIntent::default(),
            Transform::from_translation(start_pos),
            Mesh3d(meshes.add(Cylinder::new(1.0, 1.8))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.2, 0.2, 0.25),
                metallic: 0.5,
                perceptual_roughness: 0.5,
                ..default()
            })),
            Visibility::default(),
        ))
        .with_children(|parent| {
            parent.spawn((
                ObserverLens,
                Camera3d::default(),
                Projection::Perspective(PerspectiveProjection {
                    fov: 85.0_f32.to_radians(),
                    ..default()
                }),
                Transform::from_xyz(0.0, 0.7, 0.0),
            ));
        });
}

pub fn manage_os_cursor_boundary(
    mut cursor_query: Query<&mut CursorOptions, With<PrimaryWindow>>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
) {
    let Ok(mut cursor) = cursor_query.single_mut() else {
        return;
    };

    if keys.just_pressed(KeyCode::Escape) {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }

    if mouse.just_pressed(MouseButton::Left) && cursor.grab_mode == CursorGrabMode::None {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
}

pub fn observer_hardware_ingest(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut motion: MessageReader<bevy::input::mouse::MouseMotion>,
    mut query: Query<&mut ObserverIntent>,
    cursor_query: Query<&CursorOptions, With<bevy::window::PrimaryWindow>>,
    tuning: Res<ObserverConfig>,
) {
    let Ok(cursor) = cursor_query.single() else {
        for _ in motion.read() {}
        return;
    };

    if cursor.grab_mode == bevy::window::CursorGrabMode::None {
        for _ in motion.read() {}
        return;
    }

    let Ok(mut intent) = query.single_mut() else {
        for _ in motion.read() {}
        return;
    };

    // Erase translational and toggle state to prevent cross-frame intent leaking
    intent.translation_vector = Vec3::ZERO;
    intent.toggle_noclip = keyboard.just_pressed(KeyCode::KeyN);
    intent.toggle_grid_attachment = keyboard.just_pressed(KeyCode::KeyG);

    for ev in motion.read() {
        intent.yaw_delta += ev.delta.x * tuning.look_sensitivity;
        intent.pitch_delta += ev.delta.y * tuning.look_sensitivity;
    }

    if keyboard.pressed(KeyCode::KeyW) {
        intent.translation_vector.z -= 1.0;
    }
    if keyboard.pressed(KeyCode::KeyS) {
        intent.translation_vector.z += 1.0;
    }
    if keyboard.pressed(KeyCode::KeyA) {
        intent.translation_vector.x -= 1.0;
    }
    if keyboard.pressed(KeyCode::KeyD) {
        intent.translation_vector.x += 1.0;
    }

    if keyboard.pressed(KeyCode::Space) {
        intent.translation_vector.y += 1.0;
    }
    if keyboard.pressed(KeyCode::ControlLeft) {
        intent.translation_vector.y -= 1.0;
    }

    intent.is_sprinting = keyboard.pressed(KeyCode::ShiftLeft);
    intent.is_jumping = keyboard.pressed(KeyCode::Space);
}

pub fn sync_lens_orientation(
    observer_query: Query<&KinematicObserver, Without<ObserverLens>>,
    mut lens_query: Query<&mut Transform, With<ObserverLens>>,
) {
    let Ok(observer) = observer_query.single() else {
        return;
    };
    let Ok(mut lens_transform) = lens_query.single_mut() else {
        return;
    };

    lens_transform.rotation = Quat::from_rotation_x(observer.pitch);
}

pub fn sync_world_focus(
    query: Query<(&Transform, &KinematicObserver)>,
    mut focus: ResMut<WorldFocus>,
) {
    let Ok((transform, observer)) = query.single() else {
        return;
    };

    if observer.is_grid_attached {
        focus.position = bevy::math::DVec3::new(
            transform.translation.x as f64,
            -transform.translation.z as f64,
            0.0,
        );
    }
}
