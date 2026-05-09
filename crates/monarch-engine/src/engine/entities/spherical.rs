use bevy::{
    ecs::{
        component::Component,
        system::{Query, Res, ResMut},
    },
    math::{IVec2, Vec3},
    time::Time,
    transform::components::Transform,
};

use crate::engine::{
    entities::{
        EntityPhysicsConfig,
        utils::{compute_outward_resistance, fetch_floor_height},
    },
    world::grid::ActiveWorldGrid,
};

#[derive(Component, Debug, Clone)]
pub struct DynamicRigidSphere {
    pub velocity: Vec3,
    pub mass: f32,
    pub radius: f32,
    pub accumulated_compaction: f32,
}

impl Default for DynamicRigidSphere {
    fn default() -> Self {
        Self {
            velocity: Vec3::ZERO,
            mass: 50.0,
            radius: 5.0,
            accumulated_compaction: 0.0,
        }
    }
}

/// Decoupled 3D integration pass applying Newtonian physics, normal reflections, and analytic CA deformation.
pub fn simulate_rigid_sphere_kinematics(
    mut spheres: Query<(&mut Transform, &mut DynamicRigidSphere)>,
    mut grid: ResMut<ActiveWorldGrid>,
    time: Res<Time>,
    config: Res<EntityPhysicsConfig>,
) {
    let dt = time.delta_secs();
    let bounds_min = grid.window_origin;
    let bounds_max = grid.window_origin + IVec2::new(grid.width, grid.height);

    for (mut transform, mut sphere) in spheres.iter_mut() {
        // 1. Unconstrained Ballistic Integration (Completely detached from grid limits)
        sphere.velocity += config.gravity * dt;
        sphere.velocity.x *= config.air_resistance;
        sphere.velocity.z *= config.air_resistance;

        let intended_pos = transform.translation + sphere.velocity * dt;
        let grid_pos = IVec2::new(
            intended_pos.x.floor() as i32,
            (-intended_pos.z).floor() as i32,
        );

        let Some(floor_height) = fetch_floor_height(
            &grid,
            grid_pos,
            bounds_min,
            bounds_max,
            config.elevation_scale,
        ) else {
            transform.translation = intended_pos; // Airborne out-of-bounds; integrate freely
            sphere.accumulated_compaction = (sphere.accumulated_compaction - dt * 2.0).max(0.0);
            continue;
        };

        let contact_y = floor_height + sphere.radius;

        // 2. Sloped Surface Collision & True Reflection
        if intended_pos.y <= contact_y {
            let penetration = contact_y - intended_pos.y;

            // Derive local surface normal vector via central differences
            let h_px = fetch_floor_height(
                &grid,
                grid_pos + IVec2::new(1, 0),
                bounds_min,
                bounds_max,
                config.elevation_scale,
            )
            .unwrap_or(floor_height);
            let h_nx = fetch_floor_height(
                &grid,
                grid_pos + IVec2::new(-1, 0),
                bounds_min,
                bounds_max,
                config.elevation_scale,
            )
            .unwrap_or(floor_height);
            let h_py = fetch_floor_height(
                &grid,
                grid_pos + IVec2::new(0, 1),
                bounds_min,
                bounds_max,
                config.elevation_scale,
            )
            .unwrap_or(floor_height);
            let h_ny = fetch_floor_height(
                &grid,
                grid_pos + IVec2::new(0, -1),
                bounds_min,
                bounds_max,
                config.elevation_scale,
            )
            .unwrap_or(floor_height);

            let normal =
                Vec3::new(h_nx - h_px, 2.0 * config.elevation_scale, h_py - h_ny).normalize();
            transform.translation = intended_pos + normal * penetration;

            let impact_velocity = sphere.velocity.dot(normal);
            if impact_velocity < 0.0 {
                // Execute true Newtonian reflection vector mechanics
                if impact_velocity.abs() > config.min_bounce_velocity {
                    sphere.velocity -= (1.0 + config.impact_restitution) * impact_velocity * normal;
                } else {
                    // Grounded: cancel downward momentum and apply surface rolling friction along gradients
                    sphere.velocity -= impact_velocity * normal;
                }
                sphere.velocity *= config.rolling_friction;

                let kinetic_energy = 0.5 * sphere.mass * impact_velocity.powi(2);

                // Analytic Kinetic Deformation
                if kinetic_energy > config.min_deformation_energy {
                    let resistance = compute_outward_resistance(
                        &grid,
                        grid_pos,
                        floor_height,
                        bounds_min,
                        bounds_max,
                        &config,
                    );
                    let required_energy = (config.min_deformation_energy * resistance)
                        + sphere.accumulated_compaction;

                    if kinetic_energy >= required_energy {
                        let net_energy = kinetic_energy - required_energy;
                        let units_to_crush =
                            (net_energy * config.force_to_volume_factor).floor() as u16;

                        if units_to_crush > 0 {
                            let mut cell = grid.get_cell(grid_pos);
                            let mut remaining_crush = units_to_crush;
                            let mut actual_crushed = 0;

                            // Prioritize spilling over from granular volume to base elevation
                            let g_vol = cell.granular_vol();
                            if g_vol > 0 {
                                let crush = g_vol.min(remaining_crush);
                                cell.set_granular_vol(g_vol - crush);
                                remaining_crush -= crush;
                                actual_crushed += crush;
                            }

                            if remaining_crush > 0 {
                                let elev = cell.elevation();
                                let crush = elev.min(remaining_crush);
                                if crush > 0 {
                                    cell.set_elevation(elev.saturating_sub(crush));
                                    actual_crushed += crush;
                                }
                            }

                            // Commit memory writes, cell waking, and compaction buildup strictly when physical deformation occurs
                            if actual_crushed > 0 {
                                grid.set_cell(grid_pos, cell);
                                grid.wake_cell(grid_pos);
                                sphere.accumulated_compaction +=
                                    actual_crushed as f32 * config.force_to_volume_factor;
                            }
                        }
                    }
                }
            }
        } else {
            transform.translation = intended_pos;
            sphere.accumulated_compaction = (sphere.accumulated_compaction - dt * 2.0).max(0.0);
        }
    }
}
