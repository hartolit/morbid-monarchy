use bevy::prelude::*;
use cellular_landscape::prelude::*;

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
            base_speed: 20.0,
            sprint_speed: 60.0,
            jump_impulse: 35.0,
            look_sensitivity: 0.002,
            max_pitch: 1.54,
            structural_radius: 1.0,
            structural_height: 1.8,
        }
    }
}

/// The Observer is now stripped of velocity and physical state.
/// It acts purely as a semantic marker and input accumulator.
#[derive(Component)]
pub struct KinematicObserver {
    pub yaw: f32,
    pub pitch: f32,
    pub is_noclip: bool,
    pub is_grid_attached: bool,
}

impl Default for KinematicObserver {
    fn default() -> Self {
        Self {
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
    mut commands: Commands,
    mut query: Query<(
        Entity,
        &mut Transform,
        &mut KinematicObserver,
        &mut ObserverIntent,
        Option<&mut GridKinematicBody>,
    )>,
    time: Res<Time>,
    tuning: Res<ObserverConfig>,
) {
    let delta_time = time.delta_secs();

    for (entity, mut transform, mut observer, mut intent, mut kinematic_body) in query.iter_mut() {
        // Evaluate State Toggles
        if intent.toggle_grid_attachment {
            observer.is_grid_attached = !observer.is_grid_attached;
        }

        if intent.toggle_noclip {
            observer.is_noclip = !observer.is_noclip;
            // Structural ECS Boundary: Grant or revoke physical rights dynamically
            if observer.is_noclip {
                commands.entity(entity).remove::<GridKinematicBody>();
            } else {
                commands
                    .entity(entity)
                    .insert(GridKinematicBody::new(75.0, tuning.structural_radius));
            }
        }

        // Resolve Lens Orientation
        observer.yaw -= intent.yaw_delta;
        observer.pitch =
            (observer.pitch - intent.pitch_delta).clamp(-tuning.max_pitch, tuning.max_pitch);

        intent.yaw_delta = 0.0;
        intent.pitch_delta = 0.0;
        intent.toggle_noclip = false;
        intent.toggle_grid_attachment = false;

        let horizontal_rotation = Quat::from_rotation_y(observer.yaw);
        transform.rotation = horizontal_rotation;

        // Accumulate Wish Direction
        let mut wish_dir = Vec3::ZERO;
        let active_speed = if intent.is_sprinting {
            tuning.sprint_speed
        } else {
            tuning.base_speed
        };

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

        if observer.is_noclip {
            // Unrestricted mathematical translation (bypassing grid physics entirely)
            if intent.translation_vector.y > 0.0 {
                wish_dir += up;
            }
            if intent.translation_vector.y < 0.0 {
                wish_dir -= up;
            }

            transform.translation += wish_dir.normalize_or_zero() * active_speed * delta_time;
        } else if let Some(mut body) = kinematic_body {
            // Steer the generic GridKinematicBody. The generic landscape system will handle CCD and gravity.
            let grounded_dir = wish_dir.normalize_or_zero();
            body.velocity.x = grounded_dir.x * active_speed;
            body.velocity.z = grounded_dir.z * active_speed;

            if intent.is_jumping && body.velocity.y <= 0.0 {
                body.velocity.y = tuning.jump_impulse;
            }
        }
    }
}
