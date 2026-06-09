use crate::engine::{
    entities::GlobalPhysicsConfig, physics::grid_physics::GridPhysicsApi,
    world::grid::ActiveWorldGrid,
};
use bevy::prelude::*;

#[derive(Resource, Debug, Clone, Copy)]
pub struct ObserverConfig {
    pub base_speed: f32,
    pub sprint_speed: f32,
    pub jump_impulse: f32,
    pub look_sensitivity: f32,
    pub max_pitch: f32,
    pub structural_radius: f32,
    pub structural_height: f32,
}

impl Default for ObserverConfig {
    fn default() -> Self {
        Self {
            base_speed: 200.0,
            sprint_speed: 800.0,
            jump_impulse: 120.0,
            look_sensitivity: 0.002,
            max_pitch: 1.54,
            structural_radius: 2.5,
            structural_height: 10.0,
        }
    }
}

#[derive(Component)]
pub struct KinematicObserver {
    pub velocity: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub is_noclip: bool,
    pub is_grid_attached: bool,
}

impl Default for KinematicObserver {
    fn default() -> Self {
        Self {
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            is_noclip: true,
            is_grid_attached: true,
        }
    }
}

#[derive(Component, Default)]
pub struct ObserverIntent {
    pub translation_vector: Vec3,
    pub yaw_delta: f32,
    pub pitch_delta: f32,
    pub is_sprinting: bool,
    pub is_jumping: bool,
    pub toggle_noclip: bool,
    pub toggle_grid_attachment: bool,
}

pub fn resolve_observer_kinematics(
    mut query: Query<(&mut Transform, &mut KinematicObserver, &mut ObserverIntent)>,
    mut grid: ResMut<ActiveWorldGrid>,
    time: Res<Time>,
    global_config: Res<GlobalPhysicsConfig>,
    tuning: Res<ObserverConfig>,
) {
    let delta_time = time.delta_secs();
    let physics = GridPhysicsApi::new(&mut grid, &global_config);

    for (mut transform, mut observer, mut intent) in query.iter_mut() {
        if intent.toggle_noclip {
            observer.is_noclip = !observer.is_noclip;
        }
        if intent.toggle_grid_attachment {
            observer.is_grid_attached = !observer.is_grid_attached;
        }

        observer.yaw -= intent.yaw_delta;
        observer.pitch =
            (observer.pitch - intent.pitch_delta).clamp(-tuning.max_pitch, tuning.max_pitch);

        intent.yaw_delta = 0.0;
        intent.pitch_delta = 0.0;
        intent.toggle_noclip = false;
        intent.toggle_grid_attachment = false;

        let horizontal_rotation = Quat::from_rotation_y(observer.yaw);
        transform.rotation = horizontal_rotation;

        let mut wish_dir = Vec3::ZERO;
        let active_speed = if intent.is_sprinting {
            tuning.sprint_speed
        } else {
            tuning.base_speed
        };

        if observer.is_noclip {
            let forward = (horizontal_rotation * Vec3::NEG_Z).normalize_or_zero();
            let right = (horizontal_rotation * Vec3::X).normalize_or_zero();
            let up = Vec3::Y;

            if intent.translation_vector.z < 0.0 {
                wish_dir += forward;
            }
            if intent.translation_vector.z > 0.0 {
                wish_dir -= forward;
            }
            if intent.translation_vector.x < 0.0 {
                wish_dir -= right;
            }
            if intent.translation_vector.x > 0.0 {
                wish_dir += right;
            }
            if intent.translation_vector.y > 0.0 {
                wish_dir += up;
            }
            if intent.translation_vector.y < 0.0 {
                wish_dir -= up;
            }

            observer.velocity = wish_dir.normalize_or_zero() * active_speed;
            transform.translation += observer.velocity * delta_time;
        } else {
            let forward = transform.forward().normalize_or_zero();
            let right = transform.right().normalize_or_zero();

            if intent.translation_vector.z < 0.0 {
                wish_dir += forward;
            }
            if intent.translation_vector.z > 0.0 {
                wish_dir -= forward;
            }
            if intent.translation_vector.x < 0.0 {
                wish_dir -= right;
            }
            if intent.translation_vector.x > 0.0 {
                wish_dir += right;
            }

            let grounded_dir = wish_dir.normalize_or_zero();

            observer.velocity.x = grounded_dir.x * active_speed;
            observer.velocity.z = grounded_dir.z * active_speed;
            observer.velocity.y += global_config.gravity.y * delta_time;

            if intent.is_jumping && observer.velocity.y <= 0.0 {
                observer.velocity.y = tuning.jump_impulse;
            }

            transform.translation += observer.velocity * delta_time;

            let center_grid = bevy::math::IVec2::new(
                transform.translation.x.floor() as i32,
                (-transform.translation.z).floor() as i32,
            );

            let structural_floor = physics.get_floor_height(center_grid).unwrap_or(0.0);
            let contact_boundary = structural_floor + tuning.structural_height;

            if transform.translation.y < contact_boundary {
                transform.translation.y = contact_boundary;
                if observer.velocity.y < 0.0 {
                    observer.velocity.y = 0.0;
                }
            }
        }
    }
}
