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
const EPSILON_DISTANCE: f32 = 0.000001;

/// Damping multiplier applied to relative velocities during elastic cluster impulse resolution.
const CLUSTER_DAMPING: f32 = 0.99;

/// Half of a single grid cell's width/height, used to locate the analytical center of a cell.
const HALF_CELL_OFFSET: f32 = 0.5;

/// Decay rate per second of accumulated terrain compaction when an entity is ungrounded or airborne.
const COMPACTION_DECAY_RATE: f32 = 2.0;

/// Minimum surface thickness enforced for rendering layers.
const MIN_SURFACE_THICKNESS: f32 = 1.0;

/// Velocity squared threshold below which the sphere is considered stable/resting.
const SLEEP_VELOCITY_SQUARED: f32 = 0.001;

/// Velocity squared threshold above which the sphere transitions from inactive to actively moving.
const WAKE_VELOCITY_SQUARED: f32 = 0.04;

/// Fraction of the sphere radius defining the vertical blade ceiling for rim material displacement.
const BLADE_CEILING_RATIO: f32 = 0.25;

/// Fraction of the sphere radius defining the core load-bearing underside for vertical grounding.
const LOAD_BEARING_RADIUS_RATIO: f32 = 0.7;

/// Fraction of the sphere radius defining the maximum allowable horizontal wall push correction per frame.
const MAX_WALL_PUSH_RATIO: f32 = 0.5;

/// Multiplier for the inner radius boundary of the rim deposition ring.
const RIM_INNER_RADIUS_RATIO: f32 = 0.9;

/// Small safety buffer added to the drift velocity threshold to ensure stable vertical grounding.
const GROUNDING_DRIFT_BUFFER: f32 = 0.01;

/// Scale factor for converting spatial hash output into a fractional remainder probability.
const PROBABILITY_HASH_SCALE: f32 = 1000.0;

/// Upward velocity deflection factor applied when moving horizontally through granular material while submerged.
const SUBMERGED_BUOYANCY_LIFT: f32 = 2.0;

// ============================================================================
// Component Definitions
// ============================================================================

#[derive(Component, Debug, Clone)]
pub struct DynamicRigidSphere {
    pub velocity: Vec3,
    pub mass: f32,
    pub radius: f32,
    pub accumulated_compaction: f32,
    /// Tracks whether the sphere has settled and stopped actively displacing granular material.
    pub is_granular_inactive: bool,
}

impl Default for DynamicRigidSphere {
    fn default() -> Self {
        Self {
            velocity: Vec3::ZERO,
            mass: 50.0,
            radius: 5.0,
            accumulated_compaction: 0.0,
            is_granular_inactive: false,
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
            is_granular_inactive: false,
        }
    }
}

// ============================================================================
// Kinematics Simulation System
// ============================================================================

/// Decoupled 3D integration pass applying ballistic integration, inter-entity collision,
/// dynamic bedrock/granular deformation, submerged physics adaptation, and authoritative support.
pub fn simulate_rigid_sphere_kinematics(
    mut spheres: Query<(&mut Transform, &mut DynamicRigidSphere)>,
    mut grid: ResMut<ActiveWorldGrid>,
    time: Res<Time>,
    config: Res<EntityPhysicsConfig>,
) {
    let delta_time = time.delta_secs();
    let bounds_minimum = grid.window_origin;
    let bounds_maximum = grid.window_origin + IVec2::new(grid.width, grid.height);

    // 1. Unconstrained Ballistic Integration
    for (mut transform, mut sphere) in spheres.iter_mut() {
        sphere.velocity += config.gravity * delta_time;
        sphere.velocity.x *= config.air_resistance;
        sphere.velocity.z *= config.air_resistance;

        transform.translation += sphere.velocity * delta_time;
    }

    // 2. Inter-Entity Overlap Resolution (Sphere-to-Sphere)
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
            let collision_normal = if distance > EPSILON_DISTANCE {
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

    // 3. State Evaluation, Carving, Submerged Physics & Authoritative Grounding
    for (mut transform, mut sphere) in spheres.iter_mut() {
        let mut current_position = transform.translation;
        let sphere_radius = sphere.radius;

        let minimum_grid_x = (current_position.x - sphere_radius).floor() as i32;
        let maximum_grid_x = (current_position.x + sphere_radius).floor() as i32;
        let minimum_grid_y = (-(current_position.z + sphere_radius)).floor() as i32;
        let maximum_grid_y = (-(current_position.z - sphere_radius)).floor() as i32;

        let center_grid_position = IVec2::new(
            current_position.x.floor() as i32,
            (-current_position.z).floor() as i32,
        );
        let baseline_contact_height = fetch_floor_height(
            &grid,
            center_grid_position,
            bounds_minimum,
            bounds_maximum,
            config.elevation_scale,
        )
        .unwrap_or(current_position.y - sphere_radius);

        let local_outward_resistance = compute_outward_resistance(
            &grid,
            center_grid_position,
            baseline_contact_height,
            bounds_minimum,
            bounds_maximum,
            &config,
        );

        let effective_crush_cost = config.cost_crush_terrain * local_outward_resistance;

        // Determine the sphere's active kinetic energy excluding pure gravitational frame drift
        let gravity_drift = config.gravity * delta_time;
        let mut active_velocity = sphere.velocity - gravity_drift;

        if active_velocity.length_squared() < SLEEP_VELOCITY_SQUARED {
            active_velocity = Vec3::ZERO;
        }

        let mut kinetic_energy = 0.5 * sphere.mass * active_velocity.length_squared();

        // Update granular inactivity state
        if active_velocity.length_squared() > WAKE_VELOCITY_SQUARED {
            sphere.is_granular_inactive = false;
        } else if active_velocity.length_squared() < SLEEP_VELOCITY_SQUARED {
            sphere.is_granular_inactive = true;
        }

        // Determine surrounding granular submersion level
        let mut maximum_surrounding_granular_height = f32::NEG_INFINITY;
        for grid_y in minimum_grid_y..=maximum_grid_y {
            for grid_x in minimum_grid_x..=maximum_grid_x {
                let cell_position = IVec2::new(grid_x, grid_y);
                if cell_position.x >= bounds_minimum.x
                    && cell_position.x < bounds_maximum.x
                    && cell_position.y >= bounds_minimum.y
                    && cell_position.y < bounds_maximum.y
                {
                    let cell = grid.get_cell(cell_position);
                    if cell.granular_vol() > 0 {
                        let total_height = (cell.elevation() as f32 + cell.granular_vol() as f32)
                            * config.elevation_scale;
                        if total_height > maximum_surrounding_granular_height {
                            maximum_surrounding_granular_height = total_height;
                        }
                    }
                }
            }
        }

        // If surrounding granular material is higher than the sphere's center, it is submerged/covered.
        let is_submerged_under_granular = maximum_surrounding_granular_height > current_position.y;

        let mut harvested_granular_volume = 0u32;
        let mut dominant_granular_material = None;
        let mut actual_crushed_terrain = 0u32;

        // ========================================================================
        // Pass A: Bedrock Crushing & Selective Granular Carving
        // ========================================================================
        // Running carving BEFORE authoritative grounding ensures that an impacting sphere penetrates
        // bedrock normally and expends kinetic energy to crush/fracture it, solving the "hard as a rock" issue.
        for grid_y in minimum_grid_y..=maximum_grid_y {
            for grid_x in minimum_grid_x..=maximum_grid_x {
                let cell_position = IVec2::new(grid_x, grid_y);
                if cell_position.x < bounds_minimum.x
                    || cell_position.x >= bounds_maximum.x
                    || cell_position.y < bounds_minimum.y
                    || cell_position.y >= bounds_maximum.y
                {
                    continue;
                }

                let cell_center_x = grid_x as f32 + HALF_CELL_OFFSET;
                let cell_center_z = -grid_y as f32 - HALF_CELL_OFFSET;
                let delta_x = cell_center_x - current_position.x;
                let delta_z = cell_center_z - current_position.z;
                let distance_squared = delta_x * delta_x + delta_z * delta_z;

                if distance_squared <= sphere_radius * sphere_radius {
                    let vertical_offset = (sphere_radius * sphere_radius - distance_squared).sqrt();
                    let sphere_bottom = current_position.y - vertical_offset;

                    let mut cell = grid.get_cell(cell_position);
                    let mut cell_modified = false;

                    // Obliterate biologic growth (foliage)
                    if cell.surface_mat() == SurfaceMat::SURFACE_FOLIAGE {
                        let floor_base_height = (cell.elevation() as f32
                            + cell.granular_vol() as f32
                            + cell.fluid_vol() as f32)
                            * config.elevation_scale;
                        let surface_top_height = floor_base_height
                            + (MIN_SURFACE_THICKNESS.max(cell.surface_state() as f32))
                                * config.elevation_scale;

                        if sphere_bottom < surface_top_height {
                            cell.set_surface_mat(SurfaceMat::EMPTY);
                            cell.set_surface_state(0);
                            cell_modified = true;
                        }
                    }

                    let solid_height = cell.elevation() as f32 * config.elevation_scale;
                    let total_height = (cell.elevation() as f32 + cell.granular_vol() as f32)
                        * config.elevation_scale;

                    // 1. Solid Bedrock Crushing (Always evaluated on impact)
                    if cell.elevation() > 0
                        && solid_height > sphere_bottom
                        && kinetic_energy >= config.min_deformation_energy
                        && kinetic_energy >= effective_crush_cost
                    {
                        let excess_height = solid_height - sphere_bottom;
                        let needed_crush = (excess_height / config.elevation_scale).ceil() as u16;
                        let maximum_affordable =
                            (kinetic_energy / effective_crush_cost).floor() as u16;
                        let actual_crush =
                            needed_crush.min(cell.elevation()).min(maximum_affordable);

                        if actual_crush > 0 {
                            cell.set_elevation(cell.elevation() - actual_crush);

                            let energy_spent = actual_crush as f32 * effective_crush_cost;
                            let previous_kinetic = kinetic_energy;
                            kinetic_energy = (kinetic_energy - energy_spent).max(0.0);

                            if previous_kinetic > 0.0 {
                                let speed_scale = (kinetic_energy / previous_kinetic).sqrt();
                                sphere.velocity *= speed_scale;
                            }

                            actual_crushed_terrain += actual_crush as u32;
                            cell_modified = true;
                        }
                    }

                    // 2. Selective Granular Carving / Displacement
                    // If the sphere is active and NOT submerged, it displaces overlapping sand out to the rim.
                    // If the sphere is inactive OR submerged, it gracefully allows incoming sand to flow in/overlap
                    // without pushing it away, enabling elegant sand-covering realism and avoiding black holes/cavitation.
                    let granular_volume = cell.granular_vol();
                    if !sphere.is_granular_inactive
                        && !is_submerged_under_granular
                        && granular_volume > 0
                        && total_height > sphere_bottom
                    {
                        let target_granular_volume = if sphere_bottom <= solid_height {
                            0
                        } else {
                            ((sphere_bottom - solid_height) / config.elevation_scale)
                                .floor()
                                .max(0.0) as u16
                        };

                        if granular_volume > target_granular_volume {
                            let actual_removal = granular_volume - target_granular_volume;
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

                                // Draining kinetic energy during active carving conserves momentum
                                if kinetic_energy > 0.0 {
                                    let energy_spent =
                                        actual_removal as f32 * config.cost_displace_granular;
                                    let previous_kinetic = kinetic_energy;
                                    kinetic_energy = (kinetic_energy - energy_spent).max(0.0);

                                    if previous_kinetic > 0.0 {
                                        let speed_scale =
                                            (kinetic_energy / previous_kinetic).sqrt();
                                        sphere.velocity *= speed_scale;
                                    }
                                }

                                cell_modified = true;
                            }
                        }
                    }

                    if cell_modified {
                        grid.set_cell(cell_position, cell);
                        grid.wake_cell(cell_position);
                    }
                }
            }
        }

        if actual_crushed_terrain > 0 {
            sphere.accumulated_compaction +=
                actual_crushed_terrain as f32 * config.force_to_volume_factor;
        }

        // ========================================================================
        // Pass B: Submerged Movement Physics Adaptation
        // ========================================================================
        // When moving horizontally while submerged/covered in granular material, the surrounding sand
        // applies a natural buoyant normal force deflecting the trajectory upwards, allowing the sphere
        // to smoothly emerge/climb out of dunes rather than getting permanently trapped.
        if is_submerged_under_granular {
            let horizontal_speed = (sphere.velocity.x * sphere.velocity.x
                + sphere.velocity.z * sphere.velocity.z)
                .sqrt();
            if horizontal_speed > 0.1 {
                sphere.velocity.y += SUBMERGED_BUOYANCY_LIFT * delta_time * horizontal_speed;
            }
        }

        // ========================================================================
        // Pass C: Authoritative Grounding & Horizontal Wall Resolution Scan
        // ========================================================================
        let mut maximum_required_y = f32::NEG_INFINITY;
        let mut accumulated_push_direction = Vec3::ZERO;
        let mut wall_contact_count = 0;

        for grid_y in minimum_grid_y..=maximum_grid_y {
            for grid_x in minimum_grid_x..=maximum_grid_x {
                let cell_position = IVec2::new(grid_x, grid_y);
                if cell_position.x < bounds_minimum.x
                    || cell_position.x >= bounds_maximum.x
                    || cell_position.y < bounds_minimum.y
                    || cell_position.y >= bounds_maximum.y
                {
                    continue;
                }

                let cell = grid.get_cell(cell_position);
                let solid_height = cell.elevation() as f32 * config.elevation_scale;
                let total_height =
                    (cell.elevation() as f32 + cell.granular_vol() as f32) * config.elevation_scale;

                let cell_center_x = grid_x as f32 + HALF_CELL_OFFSET;
                let cell_center_z = -grid_y as f32 - HALF_CELL_OFFSET;
                let delta_x = cell_center_x - current_position.x;
                let delta_z = cell_center_z - current_position.z;
                let distance_squared = delta_x * delta_x + delta_z * delta_z;

                if distance_squared <= sphere_radius * sphere_radius {
                    let distance_horizontal = (delta_x * delta_x + delta_z * delta_z).sqrt();
                    let vertical_offset = (sphere_radius * sphere_radius - distance_squared).sqrt();
                    let sphere_bottom_at_cell = current_position.y - vertical_offset;

                    // 1. Horizontal Wall Pushing: Strictly constrained to solid bedrock. Loose sand shifts
                    // and flows, lacking the structural strength to block or shove a rigid metal sphere horizontally.
                    if solid_height > sphere_bottom_at_cell {
                        let step_threshold =
                            current_position.y + sphere_radius * BLADE_CEILING_RATIO;
                        if solid_height <= step_threshold {
                            let required_y = solid_height + vertical_offset;
                            if required_y > maximum_required_y {
                                maximum_required_y = required_y;
                            }
                        } else if distance_horizontal > EPSILON_DISTANCE {
                            let penetration_horizontal = sphere_radius - distance_horizontal;
                            let push_direction =
                                Vec3::new(-delta_x, 0.0, -delta_z) / distance_horizontal;
                            accumulated_push_direction += push_direction * penetration_horizontal;
                            wall_contact_count += 1;
                        }
                    }

                    // 2. Vertical Grounding Support: Solid bedrock always supports. Granular material supports
                    // ONLY if the sphere is not submerged OR if the cell is directly beneath the core load-bearing
                    // underside. This prevents side-sand overlapping a covered sphere from falsely ejecting it upwards.
                    let core_load_bearing_radius = sphere_radius * LOAD_BEARING_RADIUS_RATIO;
                    let supports_vertically = solid_height > sphere_bottom_at_cell
                        || (!is_submerged_under_granular && total_height > sphere_bottom_at_cell)
                        || (total_height > sphere_bottom_at_cell
                            && distance_horizontal <= core_load_bearing_radius);

                    if supports_vertically {
                        let effective_floor_height = if solid_height > sphere_bottom_at_cell {
                            solid_height
                        } else {
                            total_height
                        };
                        let required_y = effective_floor_height + vertical_offset;
                        if required_y > maximum_required_y {
                            maximum_required_y = required_y;
                        }
                    }
                }
            }
        }

        // Resolve horizontal wall pushes
        if wall_contact_count > 0 {
            let mut mean_push_direction = accumulated_push_direction / (wall_contact_count as f32);
            let maximum_safe_push = sphere_radius * MAX_WALL_PUSH_RATIO;
            if mean_push_direction.length() > maximum_safe_push {
                mean_push_direction = mean_push_direction.normalize() * maximum_safe_push;
            }

            current_position += mean_push_direction;
            transform.translation.x = current_position.x;
            transform.translation.z = current_position.z;

            let wall_normal = if mean_push_direction.length_squared() > EPSILON_DISTANCE {
                mean_push_direction.normalize()
            } else {
                Vec3::ZERO
            };

            if wall_normal != Vec3::ZERO {
                let impact_velocity = sphere.velocity.dot(wall_normal);
                if impact_velocity < 0.0 {
                    sphere.velocity -=
                        (1.0 + config.impact_restitution) * impact_velocity * wall_normal;
                }
            }
        }

        // Enforce authoritative vertical support
        if current_position.y <= maximum_required_y {
            transform.translation.y = maximum_required_y;
            current_position.y = maximum_required_y;

            let contact_normal = Vec3::Y;
            let impact_velocity = sphere.velocity.dot(contact_normal);
            if impact_velocity < 0.0 {
                let gravity_drift_velocity =
                    (config.gravity * delta_time).dot(contact_normal).abs();
                let effective_bounce_threshold = config
                    .min_bounce_velocity
                    .max(gravity_drift_velocity + GROUNDING_DRIFT_BUFFER);

                if impact_velocity.abs() > effective_bounce_threshold {
                    sphere.velocity -=
                        (1.0 + config.impact_restitution) * impact_velocity * contact_normal;
                } else {
                    sphere.velocity -= impact_velocity * contact_normal;
                }
                sphere.velocity *= config.rolling_friction;
            }
        } else {
            sphere.accumulated_compaction =
                (sphere.accumulated_compaction - delta_time * COMPACTION_DECAY_RATE).max(0.0);
        }

        // ========================================================================
        // Pass D: Immediate Tight Rim Deposition (Adjacent Pile-up)
        // ========================================================================
        if harvested_granular_volume > 0 {
            let deposit_material = dominant_granular_material.unwrap_or(GranularMat::GRANULAR_DIRT);
            let radius_inner = sphere_radius * RIM_INNER_RADIUS_RATIO;
            let radius_outer = sphere_radius * config.rim_expansion_factor;

            let rim_minimum_grid_x = (current_position.x - radius_outer).floor() as i32;
            let rim_maximum_grid_x = (current_position.x + radius_outer).floor() as i32;
            let rim_minimum_grid_y = (-(current_position.z + radius_outer)).floor() as i32;
            let rim_maximum_grid_y = (-(current_position.z - radius_outer)).floor() as i32;

            // Sweep 1: Read-only scan accumulating total valid adjacent rim capacity
            let mut total_rim_capacity = 0u32;

            for grid_y in rim_minimum_grid_y..=rim_maximum_grid_y {
                for grid_x in rim_minimum_grid_x..=rim_maximum_grid_x {
                    let cell_position = IVec2::new(grid_x, grid_y);
                    if cell_position.x < bounds_minimum.x
                        || cell_position.x >= bounds_maximum.x
                        || cell_position.y < bounds_minimum.y
                        || cell_position.y >= bounds_maximum.y
                    {
                        continue;
                    }

                    let cell_center_x = grid_x as f32 + HALF_CELL_OFFSET;
                    let cell_center_z = -grid_y as f32 - HALF_CELL_OFFSET;
                    let delta_x = cell_center_x - current_position.x;
                    let delta_z = cell_center_z - current_position.z;
                    let distance = (delta_x * delta_x + delta_z * delta_z).sqrt();

                    if distance >= radius_inner && distance <= radius_outer {
                        let cell = grid.get_cell(cell_position);
                        let current_material = cell.granular_mat();

                        if current_material == GranularMat::EMPTY
                            || current_material == deposit_material
                        {
                            let cell_total_height = (cell.elevation() as f32
                                + cell.granular_vol() as f32)
                                * config.elevation_scale;
                            let maximum_allowed_height =
                                current_position.y + sphere_radius * BLADE_CEILING_RATIO;

                            if cell_total_height < maximum_allowed_height {
                                let height_deficiency = maximum_allowed_height - cell_total_height;
                                let volume_allowance =
                                    (height_deficiency / config.elevation_scale).floor() as u16;

                                let available_slot =
                                    WorldCell::MAX_GRANULAR_VOL.saturating_sub(cell.granular_vol());
                                let capacity = available_slot
                                    .min(config.max_rim_deposit_per_cell)
                                    .min(volume_allowance);

                                total_rim_capacity += capacity as u32;
                            }
                        }
                    }
                }
            }

            // Sweep 2: Sequential write scan distributing harvested material isotropically via spatial hashing
            if total_rim_capacity > 0 {
                let fill_ratio = (harvested_granular_volume as f32) / (total_rim_capacity as f32);
                let hash_scale_integer = PROBABILITY_HASH_SCALE as u32;

                for grid_y in rim_minimum_grid_y..=rim_maximum_grid_y {
                    for grid_x in rim_minimum_grid_x..=rim_maximum_grid_x {
                        let cell_position = IVec2::new(grid_x, grid_y);
                        if cell_position.x < bounds_minimum.x
                            || cell_position.x >= bounds_maximum.x
                            || cell_position.y < bounds_minimum.y
                            || cell_position.y >= bounds_maximum.y
                        {
                            continue;
                        }

                        let cell_center_x = grid_x as f32 + HALF_CELL_OFFSET;
                        let cell_center_z = -grid_y as f32 - HALF_CELL_OFFSET;
                        let delta_x = cell_center_x - current_position.x;
                        let delta_z = cell_center_z - current_position.z;
                        let distance = (delta_x * delta_x + delta_z * delta_z).sqrt();

                        if distance >= radius_inner && distance <= radius_outer {
                            let mut cell = grid.get_cell(cell_position);
                            let current_material = cell.granular_mat();

                            if current_material == GranularMat::EMPTY
                                || current_material == deposit_material
                            {
                                let cell_total_height = (cell.elevation() as f32
                                    + cell.granular_vol() as f32)
                                    * config.elevation_scale;
                                let maximum_allowed_height =
                                    current_position.y + sphere_radius * BLADE_CEILING_RATIO;

                                if cell_total_height < maximum_allowed_height {
                                    let height_deficiency =
                                        maximum_allowed_height - cell_total_height;
                                    let volume_allowance =
                                        (height_deficiency / config.elevation_scale).floor() as u16;

                                    let available_slot = WorldCell::MAX_GRANULAR_VOL
                                        .saturating_sub(cell.granular_vol());
                                    let capacity = available_slot
                                        .min(config.max_rim_deposit_per_cell)
                                        .min(volume_allowance);

                                    if capacity > 0 {
                                        let exact_fill = (capacity as f32) * fill_ratio;
                                        let base_deposit = exact_fill.floor() as u16;
                                        let remainder_probability =
                                            exact_fill - (base_deposit as f32);

                                        let random_value = (spatial_hash(cell_position, grid.tick)
                                            % hash_scale_integer)
                                            as f32
                                            / PROBABILITY_HASH_SCALE;
                                        let extra = if random_value < remainder_probability {
                                            1
                                        } else {
                                            0
                                        };
                                        let actual_deposit = (base_deposit + extra).min(capacity);

                                        if actual_deposit > 0 {
                                            cell.set_granular_mat(deposit_material);
                                            cell.set_granular_vol(
                                                cell.granular_vol() + actual_deposit,
                                            );
                                            grid.set_cell(cell_position, cell);
                                            grid.wake_cell(cell_position);
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
}
