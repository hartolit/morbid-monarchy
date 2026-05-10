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
    utils::spatial_hash,
    world::{
        cell::{GranularMat, SurfaceMat, WorldCell},
        grid::ActiveWorldGrid,
    },
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
                            let mut available_energy = kinetic_energy - required_energy;

                            let mut harvested_granular_vol = 0u32;
                            let mut dominant_granular_mat = None;
                            let mut actual_crushed_terrain = 0u32;

                            // ========================================================================
                            // Sequential Harvesting, Compaction & Biologic Destruction Sweep
                            // ========================================================================
                            // Iterates strictly row-major (gy outer loop, gx inner loop) over the flat grid
                            // memory layout to maintain total hardware prefetcher saturation.
                            for gy in min_gy..=max_gy {
                                for gx in min_gx..=max_gx {
                                    let cell_pos = IVec2::new(gx, gy);
                                    if cell_pos.x < bounds_min.x
                                        || cell_pos.x >= bounds_max.x
                                        || cell_pos.y < bounds_min.y
                                        || cell_pos.y >= bounds_max.y
                                    {
                                        continue;
                                    }

                                    let x_cell = gx as f32 + HALF_CELL;
                                    let z_cell = -gy as f32 - HALF_CELL;
                                    let dist_sq =
                                        (x_cell - pos.x).powi(2) + (z_cell - pos.z).powi(2);

                                    if dist_sq <= r * r {
                                        let vertical_offset = (r * r - dist_sq).sqrt();
                                        // The analytical lower boundary of the penetrating sphere at this exact cell
                                        let sphere_bottom = pos.y - vertical_offset;

                                        let mut cell = grid.get_cell(cell_pos);
                                        let mut modified = false;

                                        // Effortlessly obliterate biologic growth (foliage) if the sphere crushes the surface layer
                                        if cell.surface_mat() == SurfaceMat::SURFACE_FOLIAGE {
                                            let floor_base_h = (cell.elevation() as f32
                                                + cell.granular_vol() as f32
                                                + cell.fluid_vol() as f32)
                                                * config.elevation_scale;
                                            let surface_top_h = floor_base_h
                                                + (1.0f32.max(cell.surface_state() as f32))
                                                    * config.elevation_scale;

                                            if sphere_bottom < surface_top_h {
                                                cell.set_surface_mat(SurfaceMat::EMPTY);
                                                cell.set_surface_state(0);
                                                modified = true;
                                            }
                                        }

                                        let elev = cell.elevation();
                                        let g_vol = cell.granular_vol();
                                        let current_total_h =
                                            (elev as f32 + g_vol as f32) * config.elevation_scale;

                                        // Strictly clamp carving to the sphere's actual bottom profile to prevent unnatural abyss holes
                                        if current_total_h > sphere_bottom {
                                            let elev_h = elev as f32 * config.elevation_scale;

                                            // Harvest loose granular matter intersecting the sphere
                                            if g_vol > 0
                                                && available_energy >= config.cost_displace_granular
                                            {
                                                let target_g_vol = if sphere_bottom <= elev_h {
                                                    0
                                                } else {
                                                    ((sphere_bottom - elev_h)
                                                        / config.elevation_scale)
                                                        .floor()
                                                        .max(0.0)
                                                        as u16
                                                };

                                                if g_vol > target_g_vol {
                                                    let desired_removal = g_vol - target_g_vol;
                                                    let max_affordable = (available_energy
                                                        / config.cost_displace_granular)
                                                        .floor()
                                                        as u16;
                                                    let actual_removal =
                                                        desired_removal.min(max_affordable);

                                                    if actual_removal > 0 {
                                                        if dominant_granular_mat.is_none()
                                                            && cell.granular_mat()
                                                                != GranularMat::EMPTY
                                                        {
                                                            dominant_granular_mat =
                                                                Some(cell.granular_mat());
                                                        }
                                                        cell.set_granular_vol(
                                                            g_vol - actual_removal,
                                                        );
                                                        if cell.granular_vol() == 0 {
                                                            cell.set_granular_mat(
                                                                GranularMat::EMPTY,
                                                            );
                                                        }
                                                        harvested_granular_vol +=
                                                            actual_removal as u32;
                                                        available_energy -= actual_removal as f32
                                                            * config.cost_displace_granular;
                                                        modified = true;
                                                    }
                                                }
                                            }

                                            // Permanently fracture solid terrain layer if still protruding and massive energy remains
                                            if elev > 0
                                                && available_energy >= config.cost_crush_terrain
                                            {
                                                let current_elev_h = cell.elevation() as f32
                                                    * config.elevation_scale;
                                                if current_elev_h > sphere_bottom {
                                                    let excess_h = current_elev_h - sphere_bottom;
                                                    let needed_crush =
                                                        (excess_h / config.elevation_scale).ceil()
                                                            as u16;
                                                    let max_affordable = (available_energy
                                                        / config.cost_crush_terrain)
                                                        .floor()
                                                        as u16;
                                                    let actual_crush = needed_crush
                                                        .min(cell.elevation())
                                                        .min(max_affordable);

                                                    if actual_crush > 0 {
                                                        cell.set_elevation(
                                                            cell.elevation() - actual_crush,
                                                        );
                                                        available_energy -= actual_crush as f32
                                                            * config.cost_crush_terrain;
                                                        actual_crushed_terrain +=
                                                            actual_crush as u32;
                                                        modified = true;
                                                    }
                                                }
                                            }
                                        }

                                        if modified {
                                            grid.set_cell(cell_pos, cell);
                                            grid.wake_cell(cell_pos);
                                        }
                                    }
                                }
                            }

                            if actual_crushed_terrain > 0 {
                                sphere.accumulated_compaction +=
                                    actual_crushed_terrain as f32 * config.force_to_volume_factor;
                            }

                            // ========================================================================
                            // Symmetric Allocation-Free Deposition Sweep
                            // ========================================================================
                            // Eliminates both dynamic heap allocations and directional sweep bias.
                            // Executes an initial read-only capacity scan to compute isotropic fill ratios,
                            // followed by a deterministic spatial distribution sweep backed by spatial hashing.
                            if harvested_granular_vol > 0 {
                                let deposit_mat =
                                    dominant_granular_mat.unwrap_or(GranularMat::GRANULAR_DIRT);
                                let r_outer = r * config.rim_expansion_factor;

                                let rim_min_gx = (pos.x - r_outer).floor() as i32;
                                let rim_max_gx = (pos.x + r_outer).floor() as i32;
                                let rim_min_gy = (-(pos.z + r_outer)).floor() as i32;
                                let rim_max_gy = (-(pos.z - r_outer)).floor() as i32;

                                // Read-only scan to accumulate total valid rim capacity
                                let mut total_rim_capacity = 0u32;

                                for gy in rim_min_gy..=rim_max_gy {
                                    for gx in rim_min_gx..=rim_max_gx {
                                        let cell_pos = IVec2::new(gx, gy);
                                        if cell_pos.x < bounds_min.x
                                            || cell_pos.x >= bounds_max.x
                                            || cell_pos.y < bounds_min.y
                                            || cell_pos.y >= bounds_max.y
                                        {
                                            continue;
                                        }

                                        let x_cell = gx as f32 + HALF_CELL;
                                        let z_cell = -gy as f32 - HALF_CELL;
                                        let dist = ((x_cell - pos.x).powi(2)
                                            + (z_cell - pos.z).powi(2))
                                        .sqrt();

                                        if dist > r && dist <= r_outer {
                                            let cell = grid.get_cell(cell_pos);
                                            let current_mat = cell.granular_mat();

                                            // Deposit exclusively into matching material or empty granular columns
                                            if current_mat == GranularMat::EMPTY
                                                || current_mat == deposit_mat
                                            {
                                                let desired_capacity = if dist <= r * 1.2 {
                                                    config.max_rim_deposit_per_cell
                                                } else {
                                                    1
                                                };
                                                let available_slot = WorldCell::MAX_GRANULAR_VOL
                                                    .saturating_sub(cell.granular_vol());
                                                let capacity = available_slot.min(desired_capacity);
                                                total_rim_capacity += capacity as u32;
                                            }
                                        }
                                    }
                                }

                                // Sequential write scan distributing exact fractional volume isotropically via spatial hashing
                                if total_rim_capacity > 0 {
                                    let fill_ratio = (harvested_granular_vol as f32)
                                        / (total_rim_capacity as f32);

                                    for gy in rim_min_gy..=rim_max_gy {
                                        for gx in rim_min_gx..=rim_max_gx {
                                            let cell_pos = IVec2::new(gx, gy);
                                            if cell_pos.x < bounds_min.x
                                                || cell_pos.x >= bounds_max.x
                                                || cell_pos.y < bounds_min.y
                                                || cell_pos.y >= bounds_max.y
                                            {
                                                continue;
                                            }

                                            let x_cell = gx as f32 + HALF_CELL;
                                            let z_cell = -gy as f32 - HALF_CELL;
                                            let dist = ((x_cell - pos.x).powi(2)
                                                + (z_cell - pos.z).powi(2))
                                            .sqrt();

                                            if dist > r && dist <= r_outer {
                                                let mut cell = grid.get_cell(cell_pos);
                                                let current_mat = cell.granular_mat();

                                                if current_mat == GranularMat::EMPTY
                                                    || current_mat == deposit_mat
                                                {
                                                    let desired_capacity = if dist <= r * 1.2 {
                                                        config.max_rim_deposit_per_cell
                                                    } else {
                                                        1
                                                    };
                                                    let available_slot =
                                                        WorldCell::MAX_GRANULAR_VOL
                                                            .saturating_sub(cell.granular_vol());
                                                    let capacity =
                                                        available_slot.min(desired_capacity);

                                                    if capacity > 0 {
                                                        let exact_fill =
                                                            (capacity as f32) * fill_ratio;
                                                        let base_deposit =
                                                            exact_fill.floor() as u16;
                                                        let remainder_prob =
                                                            exact_fill - (base_deposit as f32);

                                                        // Micro-sloshing: use a deterministic spatial hash to resolve remainder probability perfectly isotropically
                                                        let rand_val =
                                                            (spatial_hash(cell_pos, grid.tick)
                                                                % 1000)
                                                                as f32
                                                                / 1000.0;
                                                        let extra = if rand_val < remainder_prob {
                                                            1
                                                        } else {
                                                            0
                                                        };
                                                        let actual_deposit =
                                                            (base_deposit + extra).min(capacity);

                                                        if actual_deposit > 0 {
                                                            cell.set_granular_mat(deposit_mat);
                                                            cell.set_granular_vol(
                                                                cell.granular_vol()
                                                                    + actual_deposit,
                                                            );
                                                            grid.set_cell(cell_pos, cell);
                                                            grid.wake_cell(cell_pos);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
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
