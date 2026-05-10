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

// ============================================================================
// Internal Tunable Constants
// ============================================================================

/// Minimum distance squared threshold to prevent division by zero during vector normalization.
const EPSILON_DIST: f32 = 0.000001;

/// Fallback collision normal used when two entities overlap perfectly at the exact same coordinate.
const FALLBACK_NORMAL: Vec3 = Vec3::new(1.0, 0.0, 0.0);

/// Damping multiplier applied to relative velocities during elastic cluster impulse resolution.
const CLUSTER_DAMPING: f32 = 0.99;

/// Half of a single grid cell's width/height, used to locate the analytical center of a cell.
const HALF_CELL: f32 = 0.5;

/// Decay rate per second of accumulated terrain compaction when an entity is ungrounded or airborne.
const COMPACTION_DECAY_RATE: f32 = 2.0;

// ============================================================================
// Component Definitions
// ============================================================================

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

impl DynamicRigidSphere {
    pub fn new(mass: f32, radius: f32) -> Self {
        Self {
            velocity: Vec3::ZERO,
            mass,
            radius,
            accumulated_compaction: 0.0,
        }
    }
}

// ============================================================================
// Kinematics Simulation System
// ============================================================================

/// Decoupled 3D integration pass applying ballistic integration, inter-entity collision,
/// analytical Minkowski footprint restitution, and authentic dynamic deformation.
pub fn simulate_rigid_sphere_kinematics(
    mut spheres: Query<(&mut Transform, &mut DynamicRigidSphere)>,
    mut grid: ResMut<ActiveWorldGrid>,
    time: Res<Time>,
    config: Res<EntityPhysicsConfig>,
) {
    let dt = time.delta_secs();
    let bounds_min = grid.window_origin;
    let bounds_max = grid.window_origin + IVec2::new(grid.width, grid.height);

    // Unconstrained Ballistic Integration
    for (mut transform, mut sphere) in spheres.iter_mut() {
        sphere.velocity += config.gravity * dt;
        sphere.velocity.x *= config.air_resistance;
        sphere.velocity.z *= config.air_resistance;

        transform.translation += sphere.velocity * dt;
    }

    // Inter-Entity Overlap Resolution (Sphere-to-Sphere)
    let mut combinations = spheres.iter_combinations_mut();
    while let Some(
        [
            (mut transform_a, mut sphere_a),
            (mut transform_b, mut sphere_b),
        ],
    ) = combinations.fetch_next()
    {
        let delta = transform_a.translation - transform_b.translation;
        let dist_sq = delta.length_squared();
        let min_dist = sphere_a.radius + sphere_b.radius;

        if dist_sq < min_dist * min_dist {
            let dist = dist_sq.sqrt();
            let collision_normal = if dist > EPSILON_DIST {
                delta / dist
            } else {
                FALLBACK_NORMAL
            };

            let overlap = min_dist - dist;

            let inv_mass_a = 1.0 / sphere_a.mass;
            let inv_mass_b = 1.0 / sphere_b.mass;
            let total_inv_mass = inv_mass_a + inv_mass_b;

            // Positional Correction: push overlapping geometry apart instantly
            let correction = overlap / total_inv_mass;
            transform_a.translation += collision_normal * (correction * inv_mass_a);
            transform_b.translation -= collision_normal * (correction * inv_mass_b);

            // Elastic Impulse Resolution: transfer momentum along the collision normal
            let relative_velocity = sphere_a.velocity - sphere_b.velocity;
            let velocity_along_normal = relative_velocity.dot(collision_normal);

            if velocity_along_normal < 0.0 {
                let impulse =
                    -(1.0 + config.impact_restitution) * velocity_along_normal / total_inv_mass;
                sphere_a.velocity += collision_normal * (impulse * inv_mass_a);
                sphere_b.velocity -= collision_normal * (impulse * inv_mass_b);

                // Gentle damping to settle highly compressed clusters stably
                sphere_a.velocity *= CLUSTER_DAMPING;
                sphere_b.velocity *= CLUSTER_DAMPING;
            }
        }
    }

    // Analytical Footprint Grounding, Restitution & Deformation
    for (mut transform, mut sphere) in spheres.iter_mut() {
        let pos = transform.translation;
        let r = sphere.radius;

        // Map horizontal footprint bounds to grid cell ranges
        let min_gx = (pos.x - r).floor() as i32;
        let max_gx = (pos.x + r).floor() as i32;
        let min_gy = (-(pos.z + r)).floor() as i32;
        let max_gy = (-(pos.z - r)).floor() as i32;

        let mut max_required_y = f32::NEG_INFINITY;
        let mut best_cell_pos = None;
        let mut best_cell_h = 0.0;
        let mut best_contact_offset = Vec3::ZERO;

        // Scan the entire horizontal disk footprint to find the true highest Minkowski contact point
        for gy in min_gy..=max_gy {
            for gx in min_gx..=max_gx {
                let cell_pos = IVec2::new(gx, gy);
                let Some(cell_h) = fetch_floor_height(
                    &grid,
                    cell_pos,
                    bounds_min,
                    bounds_max,
                    config.elevation_scale,
                ) else {
                    continue;
                };

                // Derive the exact world center of the grid cell using the defined half-cell constant
                let x_cell = gx as f32 + HALF_CELL;
                let z_cell = -gy as f32 - HALF_CELL;

                let dx = x_cell - pos.x;
                let dz = z_cell - pos.z;
                let dist_sq = dx * dx + dz * dz;

                if dist_sq <= r * r {
                    let vertical_offset = (r * r - dist_sq).sqrt();
                    let required_y = cell_h + vertical_offset;

                    if required_y > max_required_y {
                        max_required_y = required_y;
                        best_cell_pos = Some(cell_pos);
                        best_cell_h = cell_h;
                        // The vector pointing from the surface contact point to the sphere center
                        best_contact_offset = Vec3::new(-dx, vertical_offset, -dz);
                    }
                }
            }
        }

        let Some(best_pos) = best_cell_pos else {
            // Out-of-bounds or completely airborne over an empty abyss; integrate compaction decay safely
            sphere.accumulated_compaction =
                (sphere.accumulated_compaction - dt * COMPACTION_DECAY_RATE).max(0.0);
            continue;
        };

        // Enforce Absolute Grounding Restitution as the final authoritative boundary
        if pos.y <= max_required_y {
            transform.translation.y = max_required_y;

            // The vector from the exact supporting surface contact point to the sphere center is the perfect analytic normal
            let normal = best_contact_offset.normalize();

            let impact_velocity = sphere.velocity.dot(normal);
            if impact_velocity < 0.0 {
                // Separate true dynamic impacts from static resting contacts
                if impact_velocity.abs() > config.min_bounce_velocity {
                    sphere.velocity -= (1.0 + config.impact_restitution) * impact_velocity * normal;

                    // Only authentic dynamic kinetic impacts above the bounce threshold trigger substrate deformation
                    let kinetic_energy = 0.5 * sphere.mass * impact_velocity.powi(2);
                    if kinetic_energy > config.min_deformation_energy {
                        let resistance = compute_outward_resistance(
                            &grid,
                            best_pos,
                            best_cell_h,
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
                                let mut cell = grid.get_cell(best_pos);
                                let mut remaining_crush = units_to_crush;
                                let mut actual_crushed = 0;

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

                                if actual_crushed > 0 {
                                    grid.set_cell(best_pos, cell);
                                    grid.wake_cell(best_pos);
                                    sphere.accumulated_compaction +=
                                        actual_crushed as f32 * config.force_to_volume_factor;
                                }
                            }
                        }
                    }
                } else {
                    // Grounded at rest: cancel downward momentum perfectly without bouncing or triggering kinetic crushing
                    sphere.velocity -= impact_velocity * normal;
                }

                sphere.velocity *= config.rolling_friction;
            }
        } else {
            sphere.accumulated_compaction =
                (sphere.accumulated_compaction - dt * COMPACTION_DECAY_RATE).max(0.0);
        }
    }
}
