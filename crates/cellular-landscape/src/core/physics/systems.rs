use crate::core::physics::{components::*, grid_physics::GridPhysicsApi};
use crate::core::world::grid::ActiveWorldGrid;
use bevy::prelude::*;

/// A completely generic systems pass. It blindly calculates kinematics and continuous
/// collision detection for any entity possessing the GridKinematicBody capability.
pub fn simulate_grid_kinematics(
    mut bodies: Query<(
        &mut Transform,
        &mut GridKinematicBody,
        Option<&GridDeformer>,
    )>,
    mut grid: ResMut<ActiveWorldGrid>,
    time: Res<Time>,
    global_config: Res<GlobalPhysicsConfig>,
) {
    let delta_time = time.delta_secs();
    let mut physics_api = GridPhysicsApi::new(&mut grid, &global_config);

    for (mut transform, mut body, deformer) in bodies.iter_mut() {
        body.velocity += global_config.gravity * delta_time;

        // Execute CCD DDA Sweep generically
        if let Some((hit_pos, normal)) = physics_api.sweep_trajectory(
            transform.translation,
            body.velocity,
            delta_time,
            body.radius,
        ) {
            transform.translation = hit_pos;

            let kinetic_energy = 0.5 * body.mass * body.velocity.length_squared();

            if let Some(def) = deformer {
                if kinetic_energy > def.min_deformation_energy {
                    let cell_pos = bevy::math::IVec2::new(
                        hit_pos.x.floor() as i32,
                        (-hit_pos.z).floor() as i32,
                    );
                    let effective_bottom = hit_pos.y - (body.radius * 2.0);

                    physics_api.clear_surface_organics(cell_pos, effective_bottom);
                    physics_api.crush_bedrock(
                        cell_pos,
                        effective_bottom,
                        kinetic_energy,
                        def.cost_crush_terrain,
                    );
                    physics_api.excavate_granular(cell_pos, effective_bottom);
                }
            }

            let impact_v = body.velocity.dot(normal);
            let body_restitution = 1.0 + body.impact_restitution;
            if impact_v < 0.0 {
                body.velocity -= body_restitution * impact_v * normal;
            }
            body.velocity.x *= body.rolling_friction;
            body.velocity.z *= body.rolling_friction;
        } else {
            transform.translation += body.velocity * delta_time;
        }
    }
}
