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
    entities::{EntityPhysicsConfig, utils::fetch_floor_height},
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
/// continuous embedded bulldozer carving, authoritative grounding restitution, and tight rim deposition.
pub fn simulate_rigid_sphere_kinematics(
    mut spheres: Query<(&mut Transform, &mut DynamicRigidSphere)>,
    mut grid: ResMut<ActiveWorldGrid>,
    time: Res<Time>,
    config: Res<EntityPhysicsConfig>,
) {
    let delta_time = time.delta_secs();
    let bounds_min = grid.window_origin;
    let bounds_max = grid.window_origin + IVec2::new(grid.width, grid.height);

    // Unconstrained Ballistic Integration
    for (mut transform, mut sphere) in spheres.iter_mut() {
        sphere.velocity += config.gravity * delta_time;
        sphere.velocity.x *= config.air_resistance;
        sphere.velocity.z *= config.air_resistance;

        transform.translation += sphere.velocity * delta_time;
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
        let position_delta = transform_a.translation - transform_b.translation;
        let distance_squared = position_delta.length_squared();
        let minimum_distance = sphere_a.radius + sphere_b.radius;

        if distance_squared < minimum_distance * minimum_distance {
            let distance = distance_squared.sqrt();
            let collision_normal = if distance > EPSILON_DIST {
                position_delta / distance
            } else {
                Vec3::X
            };

            let overlap = minimum_distance - distance;
            let inverse_mass_a = 1.0 / sphere_a.mass;
            let inverse_mass_b = 1.0 / sphere_b.mass;
            let total_inverse_mass = inverse_mass_a + inverse_mass_b;

            let positional_correction = overlap / total_inverse_mass;
            transform_a.translation += collision_normal * (positional_correction * inverse_mass_a);
            transform_b.translation -= collision_normal * (positional_correction * inverse_mass_b);

            let relative_velocity = sphere_a.velocity - sphere_b.velocity;
            let velocity_along_normal = relative_velocity.dot(collision_normal);

            if velocity_along_normal < 0.0 {
                let impulse =
                    -(1.0 + config.impact_restitution) * velocity_along_normal / total_inverse_mass;
                sphere_a.velocity += collision_normal * (impulse * inverse_mass_a);
                sphere_b.velocity -= collision_normal * (impulse * inverse_mass_b);

                sphere_a.velocity *= CLUSTER_DAMPING;
                sphere_b.velocity *= CLUSTER_DAMPING;
            }
        }
    }

    // Continuous Footprint Carving, Authoritative Grounding & Tight Rim Deposition
    for (mut transform, mut sphere) in spheres.iter_mut() {
        let mut current_position = transform.translation;
        let sphere_radius = sphere.radius;

        let min_grid_x = (current_position.x - sphere_radius).floor() as i32;
        let max_grid_x = (current_position.x + sphere_radius).floor() as i32;
        let min_grid_y = (-(current_position.z + sphere_radius)).floor() as i32;
        let max_grid_y = (-(current_position.z - sphere_radius)).floor() as i32;

        // Compute continuous mechanical crushing power available this frame.
        // Massive spheres exert an immense continuous static weight load (m * g) alongside kinetic energy,
        // allowing them to plow embedded trenches effortlessly like a heavy brush.
        let mut available_energy = 0.5 * sphere.mass * sphere.velocity.length_squared()
            + sphere.mass * config.gravity.length() * config.force_to_volume_factor * 10.0;

        let mut harvested_granular_volume = 0u32;
        let mut dominant_granular_material = None;
        let mut actual_crushed_terrain = 0u32;

        // ========================================================================
        // Continuous Embedded Bulldozer Carving Pass
        // ========================================================================
        // Evaluated BEFORE snapping the sphere up to the resting floor. This allows the sphere's
        // deeply embedded underside profile to forcefully clear foliage, sand, and soft rock out of its path.
        for grid_y in min_grid_y..=max_grid_y {
            for grid_x in min_grid_x..=max_grid_x {
                let cell_pos = IVec2::new(grid_x, grid_y);
                if cell_pos.x < bounds_min.x
                    || cell_pos.x >= bounds_max.x
                    || cell_pos.y < bounds_min.y
                    || cell_pos.y >= bounds_max.y
                {
                    continue;
                }

                let cell_center_x = grid_x as f32 + HALF_CELL;
                let cell_center_z = -grid_y as f32 - HALF_CELL;
                let delta_x = cell_center_x - current_position.x;
                let delta_z = cell_center_z - current_position.z;
                let distance_squared = delta_x * delta_x + delta_z * delta_z;

                if distance_squared <= sphere_radius * sphere_radius {
                    let vertical_offset = (sphere_radius * sphere_radius - distance_squared).sqrt();
                    // The exact analytical underside profile of the embedded sphere at this cell
                    let sphere_bottom = current_position.y - vertical_offset;

                    let mut cell = grid.get_cell(cell_pos);
                    let mut modified = false;

                    // Effortlessly obliterate biologic growth (foliage) if crushed
                    if cell.surface_mat() == SurfaceMat::SURFACE_FOLIAGE {
                        let floor_base_height = (cell.elevation() as f32
                            + cell.granular_vol() as f32
                            + cell.fluid_vol() as f32)
                            * config.elevation_scale;
                        let surface_top_height = floor_base_height
                            + (1.0f32.max(cell.surface_state() as f32)) * config.elevation_scale;

                        if sphere_bottom < surface_top_height {
                            cell.set_surface_mat(SurfaceMat::EMPTY);
                            cell.set_surface_state(0);
                            modified = true;
                        }
                    }

                    let elevation = cell.elevation();
                    let granular_volume = cell.granular_vol();
                    let current_total_height =
                        (elevation as f32 + granular_volume as f32) * config.elevation_scale;

                    // If the terrain exceeds the sphere's underside, forcefully plow it away
                    if current_total_height > sphere_bottom {
                        let elevation_height = elevation as f32 * config.elevation_scale;

                        // Harvest loose granular sand/dirt
                        if granular_volume > 0 && available_energy >= config.cost_displace_granular
                        {
                            let target_granular_volume = if sphere_bottom <= elevation_height {
                                0
                            } else {
                                ((sphere_bottom - elevation_height) / config.elevation_scale)
                                    .floor()
                                    .max(0.0) as u16
                            };

                            if granular_volume > target_granular_volume {
                                let desired_removal = granular_volume - target_granular_volume;
                                let max_affordable =
                                    (available_energy / config.cost_displace_granular).floor()
                                        as u16;
                                let actual_removal = desired_removal.min(max_affordable);

                                if actual_removal > 0 {
                                    if dominant_granular_material.is_none()
                                        && cell.granular_mat() != GranularMat::EMPTY
                                    {
                                        dominant_granular_material = Some(cell.granular_mat());
                                    }
                                    cell.set_granular_vol(granular_volume - actual_removal);
                                    if cell.granular_vol() == 0 {
                                        cell.set_granular_mat(GranularMat::EMPTY);
                                    }
                                    harvested_granular_volume += actual_removal as u32;
                                    available_energy -=
                                        actual_removal as f32 * config.cost_displace_granular;
                                    modified = true;
                                }
                            }
                        }

                        // Crush solid substrate if it still resists the embedded sphere
                        if cell.elevation() > 0 && available_energy >= config.cost_crush_terrain {
                            let current_elevation_height =
                                cell.elevation() as f32 * config.elevation_scale;
                            if current_elevation_height > sphere_bottom {
                                let excess_height = current_elevation_height - sphere_bottom;
                                let needed_crush =
                                    (excess_height / config.elevation_scale).ceil() as u16;
                                let max_affordable =
                                    (available_energy / config.cost_crush_terrain).floor() as u16;
                                let actual_crush =
                                    needed_crush.min(cell.elevation()).min(max_affordable);

                                if actual_crush > 0 {
                                    cell.set_elevation(cell.elevation() - actual_crush);
                                    available_energy -=
                                        actual_crush as f32 * config.cost_crush_terrain;
                                    actual_crushed_terrain += actual_crush as u32;
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
        // Authoritative Grounding Restitution Scan
        // ========================================================================
        // Now that the footprint has successfully yielded to the sphere's mass, we scan the updated
        // terrain heights to ground the sphere perfectly stably at the bottom of the newly carved trench.
        let mut max_required_y = f32::NEG_INFINITY;
        let mut best_contact_offset = Vec3::ZERO;

        for grid_y in min_grid_y..=max_grid_y {
            for grid_x in min_grid_x..=max_grid_x {
                let cell_pos = IVec2::new(grid_x, grid_y);
                let Some(cell_height) = fetch_floor_height(
                    &grid,
                    cell_pos,
                    bounds_min,
                    bounds_max,
                    config.elevation_scale,
                ) else {
                    continue;
                };

                let cell_center_x = grid_x as f32 + HALF_CELL;
                let cell_center_z = -grid_y as f32 - HALF_CELL;
                let delta_x = cell_center_x - current_position.x;
                let delta_z = cell_center_z - current_position.z;
                let distance_squared = delta_x * delta_x + delta_z * delta_z;

                if distance_squared <= sphere_radius * sphere_radius {
                    let vertical_offset = (sphere_radius * sphere_radius - distance_squared).sqrt();
                    let required_y = cell_height + vertical_offset;

                    if required_y > max_required_y {
                        max_required_y = required_y;
                        best_contact_offset = Vec3::new(-delta_x, vertical_offset, -delta_z);
                    }
                }
            }
        }

        // Apply authoritative grounding restitution
        if current_position.y <= max_required_y {
            transform.translation.y = max_required_y;
            current_position.y = max_required_y;

            let contact_normal = if best_contact_offset != Vec3::ZERO {
                best_contact_offset.normalize()
            } else {
                Vec3::Y
            };

            let impact_velocity = sphere.velocity.dot(contact_normal);
            if impact_velocity < 0.0 {
                if impact_velocity.abs() > config.min_bounce_velocity {
                    sphere.velocity -=
                        (1.0 + config.impact_restitution) * impact_velocity * contact_normal;
                } else {
                    // Settle stably at rest without vibrating
                    sphere.velocity -= impact_velocity * contact_normal;
                }
                sphere.velocity *= config.rolling_friction;
            }
        } else {
            sphere.accumulated_compaction =
                (sphere.accumulated_compaction - delta_time * COMPACTION_DECAY_RATE).max(0.0);
        }

        // ========================================================================
        // Immediate Tight Rim Deposition (Adjacent Pile-up)
        // ========================================================================
        // Eliminates the "uncanny detached donut" by depositing harvested material tightly along the immediate
        // perimeter of the carved path (0.9r to 1.2r), piling up naturally against the trench walls.
        if harvested_granular_volume > 0 {
            let deposit_material = dominant_granular_material.unwrap_or(GranularMat::GRANULAR_DIRT);
            let radius_inner = sphere_radius * 0.9;
            let radius_outer = sphere_radius * 1.2;

            let rim_min_grid_x = (current_position.x - radius_outer).floor() as i32;
            let rim_max_grid_x = (current_position.x + radius_outer).floor() as i32;
            let rim_min_grid_y = (-(current_position.z + radius_outer)).floor() as i32;
            let rim_max_grid_y = (-(current_position.z - radius_outer)).floor() as i32;

            // Read-only scan accumulating total valid adjacent rim capacity
            let mut total_rim_capacity = 0u32;

            for grid_y in rim_min_grid_y..=rim_max_grid_y {
                for grid_x in rim_min_grid_x..=rim_max_grid_x {
                    let cell_pos = IVec2::new(grid_x, grid_y);
                    if cell_pos.x < bounds_min.x
                        || cell_pos.x >= bounds_max.x
                        || cell_pos.y < bounds_min.y
                        || cell_pos.y >= bounds_max.y
                    {
                        continue;
                    }

                    let cell_center_x = grid_x as f32 + HALF_CELL;
                    let cell_center_z = -grid_y as f32 - HALF_CELL;
                    let delta_x = cell_center_x - current_position.x;
                    let delta_z = cell_center_z - current_position.z;
                    let distance = (delta_x * delta_x + delta_z * delta_z).sqrt();

                    if distance >= radius_inner && distance <= radius_outer {
                        let cell = grid.get_cell(cell_pos);
                        let current_mat = cell.granular_mat();

                        if current_mat == GranularMat::EMPTY || current_mat == deposit_material {
                            let available_slot =
                                WorldCell::MAX_GRANULAR_VOL.saturating_sub(cell.granular_vol());
                            let capacity = available_slot.min(config.max_rim_deposit_per_cell);
                            total_rim_capacity += capacity as u32;
                        }
                    }
                }
            }

            // Sequential write scan distributing harvested material isotropically via spatial hashing
            if total_rim_capacity > 0 {
                let fill_ratio = (harvested_granular_volume as f32) / (total_rim_capacity as f32);

                for grid_y in rim_min_grid_y..=rim_max_grid_y {
                    for grid_x in rim_min_grid_x..=rim_max_grid_x {
                        let cell_pos = IVec2::new(grid_x, grid_y);
                        if cell_pos.x < bounds_min.x
                            || cell_pos.x >= bounds_max.x
                            || cell_pos.y < bounds_min.y
                            || cell_pos.y >= bounds_max.y
                        {
                            continue;
                        }

                        let cell_center_x = grid_x as f32 + HALF_CELL;
                        let cell_center_z = -grid_y as f32 - HALF_CELL;
                        let delta_x = cell_center_x - current_position.x;
                        let delta_z = cell_center_z - current_position.z;
                        let distance = (delta_x * delta_x + delta_z * delta_z).sqrt();

                        if distance >= radius_inner && distance <= radius_outer {
                            let mut cell = grid.get_cell(cell_pos);
                            let current_mat = cell.granular_mat();

                            if current_mat == GranularMat::EMPTY || current_mat == deposit_material
                            {
                                let available_slot =
                                    WorldCell::MAX_GRANULAR_VOL.saturating_sub(cell.granular_vol());
                                let capacity = available_slot.min(config.max_rim_deposit_per_cell);

                                if capacity > 0 {
                                    let exact_fill = (capacity as f32) * fill_ratio;
                                    let base_deposit = exact_fill.floor() as u16;
                                    let remainder_probability = exact_fill - (base_deposit as f32);

                                    // Micro-sloshing: use spatial hashing to distribute remainder probabilities perfectly isotropically
                                    let random_value =
                                        (spatial_hash(cell_pos, grid.tick) % 1000) as f32 / 1000.0;
                                    let extra = if random_value < remainder_probability {
                                        1
                                    } else {
                                        0
                                    };
                                    let actual_deposit = (base_deposit + extra).min(capacity);

                                    if actual_deposit > 0 {
                                        cell.set_granular_mat(deposit_material);
                                        cell.set_granular_vol(cell.granular_vol() + actual_deposit);
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
