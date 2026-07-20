use bevy::{
    input::mouse::MouseWheel,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use monarch_engine::prelude::{
    observer::{KinematicObserver, ObserverConfig, ObserverIntent},
    spherical::DynamicRigidSphere,
    *,
};

// TODO: Make this a resource
const DEFAULT_TETHER_DISTANCE: f32 = 15.0;
const OBSERVER_START_Y: f32 = 150.0;
const OBSERVER_MESH_RADIUS: f32 = 1.0;
const OBSERVER_MESH_HEIGHT: f32 = 1.8;
const OBSERVER_LENS_OFFSET_Y: f32 = 0.7;
const OBSERVER_FOV_DEGREES: f32 = 85.0;

const OBSERVER_COLOR_R: f32 = 0.2;
const OBSERVER_COLOR_G: f32 = 0.2;
const OBSERVER_COLOR_B: f32 = 0.25;
const OBSERVER_METALLIC: f32 = 0.5;
const OBSERVER_ROUGHNESS: f32 = 0.5;

const INPUT_MOVEMENT_UNIT: f32 = 1.0;
const ZOOM_SPEED_MULTIPLIER: f32 = 3.0;
const MIN_ZOOM_DISTANCE: f32 = 5.0;
const MAX_ZOOM_DISTANCE: f32 = 100.0;

const SPHERE_ACCELERATION: f32 = 250.0;
const SPHERE_JUMP_THRESHOLD: f32 = 1.0;
const SPHERE_JUMP_IMPULSE: f32 = 35.0;

#[derive(Component)]
pub struct ObserverLens;

#[derive(Component)]
pub struct PossessedSphere;

#[derive(Component)]
pub struct CameraTether {
    pub distance: f32,
}

impl Default for CameraTether {
    fn default() -> Self {
        Self {
            distance: DEFAULT_TETHER_DISTANCE,
        }
    }
}

pub fn setup_observer(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let start_pos = Vec3::new(
        CHUNK_SIZE as f32 / 2.0,
        OBSERVER_START_Y,
        -(CHUNK_SIZE as f32) / 2.0,
    );

    commands
        .spawn((
            KinematicObserver::default(),
            ObserverIntent::default(),
            CameraTether::default(),
            Transform::from_translation(start_pos),
            Mesh3d(meshes.add(Cylinder::new(OBSERVER_MESH_RADIUS, OBSERVER_MESH_HEIGHT))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(OBSERVER_COLOR_R, OBSERVER_COLOR_G, OBSERVER_COLOR_B),
                metallic: OBSERVER_METALLIC,
                perceptual_roughness: OBSERVER_ROUGHNESS,
                ..default()
            })),
            Visibility::default(),
        ))
        .with_children(|parent| {
            parent.spawn((
                ObserverLens,
                Camera3d::default(),
                Projection::Perspective(PerspectiveProjection {
                    fov: OBSERVER_FOV_DEGREES.to_radians(),
                    ..default()
                }),
                Transform::from_xyz(0.0, OBSERVER_LENS_OFFSET_Y, 0.0),
            ));
        });
}

pub fn manage_os_cursor_boundary(
    mut cursor_query: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
) {
    let Ok(mut cursor) = cursor_query.single_mut() else {
        return;
    };

    if mouse.just_pressed(MouseButton::Right) {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    } else if mouse.just_released(MouseButton::Right) {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

pub fn observer_hardware_ingest(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut motion: MessageReader<bevy::input::mouse::MouseMotion>,
    mut query: Query<&mut ObserverIntent>,
    tuning: Res<ObserverConfig>,
    possessed_query: Query<(), With<PossessedSphere>>,
) {
    let Ok(mut intent) = query.single_mut() else {
        for _ in motion.read() {}
        return;
    };

    intent.translation_vector = Vec3::ZERO;
    intent.yaw_delta = 0.0;
    intent.pitch_delta = 0.0;

    intent.toggle_noclip = keyboard.just_pressed(KeyCode::KeyN);
    intent.toggle_grid_attachment = keyboard.just_pressed(KeyCode::KeyG);

    let right_click_held = mouse.pressed(MouseButton::Right);
    for ev in motion.read() {
        if right_click_held {
            intent.yaw_delta += ev.delta.x * tuning.look_sensitivity;
            intent.pitch_delta += ev.delta.y * tuning.look_sensitivity;
        }
    }

    let is_possessing = !possessed_query.is_empty();

    if !is_possessing {
        if keyboard.pressed(KeyCode::KeyW) {
            intent.translation_vector.z -= INPUT_MOVEMENT_UNIT;
        }
        if keyboard.pressed(KeyCode::KeyS) {
            intent.translation_vector.z += INPUT_MOVEMENT_UNIT;
        }
        if keyboard.pressed(KeyCode::KeyA) {
            intent.translation_vector.x -= INPUT_MOVEMENT_UNIT;
        }
        if keyboard.pressed(KeyCode::KeyD) {
            intent.translation_vector.x += INPUT_MOVEMENT_UNIT;
        }

        if keyboard.pressed(KeyCode::Space) {
            intent.translation_vector.y += INPUT_MOVEMENT_UNIT;
        }
        if keyboard.pressed(KeyCode::ControlLeft) {
            intent.translation_vector.y -= INPUT_MOVEMENT_UNIT;
        }

        intent.is_sprinting = keyboard.pressed(KeyCode::ShiftLeft);
        intent.is_jumping = keyboard.pressed(KeyCode::Space);
    }
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

pub fn handle_possession_toggle(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    observer_query: Query<&Transform, With<KinematicObserver>>,
    possessed_query: Query<Entity, With<PossessedSphere>>,
    spheres_query: Query<(Entity, &Transform), With<DynamicRigidSphere>>,
) {
    if !keyboard.just_pressed(KeyCode::KeyF) {
        return;
    }

    if let Ok(possessed_entity) = possessed_query.single() {
        commands
            .entity(possessed_entity)
            .remove::<PossessedSphere>();
        info!("Detached from sphere.");
    } else {
        let Ok(obs_transform) = observer_query.single() else {
            return;
        };
        let obs_pos = obs_transform.translation;

        let mut closest_entity = None;
        let mut closest_dist_sq = f32::MAX;

        for (entity, transform) in spheres_query.iter() {
            let dist_sq = transform.translation.distance_squared(obs_pos);
            if dist_sq < closest_dist_sq {
                closest_dist_sq = dist_sq;
                closest_entity = Some(entity);
            }
        }

        if let Some(entity) = closest_entity {
            commands.entity(entity).insert(PossessedSphere);
            info!("Possessed nearest sphere.");
        }
    }
}

pub fn update_camera_tether_and_zoom(
    mut mouse_wheel: MessageReader<MouseWheel>,
    possessed_query: Query<&Transform, (With<PossessedSphere>, Without<KinematicObserver>)>,
    mut observer_query: Query<(&mut Transform, &KinematicObserver, &mut CameraTether)>,
) {
    let Ok((mut obs_transform, observer, mut tether)) = observer_query.single_mut() else {
        for _ in mouse_wheel.read() {}
        return;
    };

    for ev in mouse_wheel.read() {
        tether.distance -= ev.y * ZOOM_SPEED_MULTIPLIER;
        tether.distance = tether.distance.clamp(MIN_ZOOM_DISTANCE, MAX_ZOOM_DISTANCE);
    }

    if let Ok(sphere_transform) = possessed_query.single() {
        let rotation = Quat::from_rotation_y(observer.yaw) * Quat::from_rotation_x(observer.pitch);
        let offset = rotation * Vec3::Z * tether.distance;
        obs_transform.translation = sphere_transform.translation + offset;
    }
}

pub fn drive_possessed_sphere(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut spheres: Query<&mut DynamicRigidSphere, With<PossessedSphere>>,
    camera_query: Query<&KinematicObserver>,
    time: Res<Time>,
) {
    if spheres.is_empty() {
        return;
    }

    let Ok(camera) = camera_query.single() else {
        return;
    };

    for mut sphere in spheres.iter_mut() {
        let mut input_dir = Vec3::ZERO;

        if keyboard.pressed(KeyCode::KeyW) {
            input_dir.z -= INPUT_MOVEMENT_UNIT;
        }
        if keyboard.pressed(KeyCode::KeyS) {
            input_dir.z += INPUT_MOVEMENT_UNIT;
        }
        if keyboard.pressed(KeyCode::KeyA) {
            input_dir.x -= INPUT_MOVEMENT_UNIT;
        }
        if keyboard.pressed(KeyCode::KeyD) {
            input_dir.x += INPUT_MOVEMENT_UNIT;
        }

        if input_dir.length_squared() > 0.0 {
            input_dir = input_dir.normalize();

            let rot = Quat::from_rotation_y(camera.yaw);
            let move_dir = rot * input_dir;

            sphere.velocity.x += move_dir.x * SPHERE_ACCELERATION * time.delta_secs();
            sphere.velocity.z += move_dir.z * SPHERE_ACCELERATION * time.delta_secs();

            sphere.is_granular_inactive = false;
        }

        if keyboard.just_pressed(KeyCode::Space) && sphere.velocity.y.abs() < SPHERE_JUMP_THRESHOLD
        {
            sphere.velocity.y = SPHERE_JUMP_IMPULSE;
            sphere.is_granular_inactive = false;
        }
    }
}
