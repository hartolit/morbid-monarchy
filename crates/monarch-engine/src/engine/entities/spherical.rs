use bevy::{
    ecs::{
        component::Component,
        system::{Query, Res, ResMut},
    },
    math::{IVec2, Vec3},
    time::Time,
    transform::components::Transform,
};
use cellular_landscape::prelude::*;

/// Retains strictly the sphere-specific buoyancy constraints.
/// Mass, radius, and velocity have been delegated to GridKinematicBody.
#[derive(Component, Debug, Clone)]
pub struct DynamicRigidSphere {
    pub submerged_buoyancy_lift: f32,
    pub buoyancy_horizontal_speed_threshold: f32,
}

impl Default for DynamicRigidSphere {
    fn default() -> Self {
        Self {
            submerged_buoyancy_lift: 2.5,
            buoyancy_horizontal_speed_threshold: 0.1,
        }
    }
}

pub fn simulate_rigid_sphere_kinematics(
    mut spheres: Query<(&Transform, &mut GridKinematicBody, &DynamicRigidSphere)>,
    mut grid: ResMut<ActiveWorldGrid>,
    time: Res<Time>,
    global_config: Res<GlobalPhysicsConfig>,
) {
    let delta_time = time.delta_secs();

    // Sphere-to-Sphere Rigid Collisions
    let mut combinations = spheres.iter_combinations_mut();
    while let Some([(t_a, mut b_a, _), (t_b, mut b_b, _)]) = combinations.fetch_next() {
        let pos_delta = t_a.translation - t_b.translation;
        let dist_sq = pos_delta.length_squared();
        let min_dist = b_a.radius + b_b.radius;

        if dist_sq < min_dist * min_dist {
            let dist = dist_sq.sqrt();
            let normal = if dist > global_config.epsilon_distance {
                pos_delta / dist
            } else {
                Vec3::X
            };

            let inv_mass_a = 1.0 / b_a.mass;
            let inv_mass_b = 1.0 / b_b.mass;
            let total_inv_mass = inv_mass_a + inv_mass_b;

            let rel_vel = b_a.velocity - b_b.velocity;
            let vel_normal = rel_vel.dot(normal);

            if vel_normal < 0.0 {
                let impulse = -(1.0 + b_a.impact_restitution.max(b_b.impact_restitution))
                    * vel_normal
                    / total_inv_mass;
                b_a.velocity += normal * (impulse * inv_mass_a);
                b_b.velocity -= normal * (impulse * inv_mass_b);
            }
        }
    }

    // Fluid Buoyancy (Engine-specific interaction with the grid surface)
    let physics_api = GridPhysicsApi::new(&mut grid, &global_config);

    for (transform, mut body, sphere) in spheres.iter_mut() {
        // Air resistance
        body.velocity.x *= 0.99;
        body.velocity.z *= 0.99;

        let current_pos = transform.translation;
        let min_grid_x = (current_pos.x - body.radius).floor() as i32;
        let max_grid_x = (current_pos.x + body.radius).floor() as i32;
        let min_grid_y = (-(current_pos.z + body.radius)).floor() as i32;
        let max_grid_y = (-(current_pos.z - body.radius)).floor() as i32;

        let mut max_surrounding_h = f32::NEG_INFINITY;
        for gy in min_grid_y..=max_grid_y {
            for gx in min_grid_x..=max_grid_x {
                if let Some(h) = physics_api.get_floor_height(IVec2::new(gx, gy)) {
                    max_surrounding_h = max_surrounding_h.max(h);
                }
            }
        }

        let is_submerged = max_surrounding_h > current_pos.y;
        if is_submerged {
            let h_speed = (body.velocity.x.powi(2) + body.velocity.z.powi(2)).sqrt();
            if h_speed > sphere.buoyancy_horizontal_speed_threshold {
                body.velocity.y += sphere.submerged_buoyancy_lift * delta_time * h_speed;
            }
        }
    }
}
