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
    entities::EntityPhysicsConfig,
    physics::grid_api::GridPhysicsApi,
    utils::spatial_hash,
    world::{
        cell::{GranularMat, WorldCell},
        grid::ActiveWorldGrid,
    },
};

#[derive(Component, Debug, Clone)]
pub struct DynamicRigidSphere {
    pub velocity: Vec3,
    pub mass: f32,
    pub radius: f32,
    pub accumulated_compaction: f32,
    pub is_granular_inactive: bool,
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

fn integrate_ballistic_motion(
    spheres: &mut Query<(&mut Transform, &mut DynamicRigidSphere)>,
    config: &EntityPhysicsConfig,
    delta_time: f32,
) {
    for (mut transform, mut sphere) in spheres.iter_mut() {
        sphere.velocity += config.gravity * delta_time;
        sphere.velocity.x *= config.air_resistance;
        sphere.velocity.z *= config.air_resistance;
        transform.translation += sphere.velocity * delta_time;
    }
}

fn resolve_sphere_sphere_collisions(
    spheres: &mut Query<(&mut Transform, &mut DynamicRigidSphere)>,
    config: &EntityPhysicsConfig,
) {
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
            let collision_normal = if distance > config.epsilon_distance {
                position_delta / distance
            } else {
                Vec3::X
            };
            let positional_overlap = minimum_distance - distance;
            let inverse_mass_a = 1.0 / sphere_a.mass;
            let inverse_mass_b = 1.0 / sphere_b.mass;
            let total_inverse_mass = inverse_mass_a + inverse_mass_b;

            let positional_correction = positional_overlap / total_inverse_mass;
            transform_a.translation += collision_normal * (positional_correction * inverse_mass_a);
            transform_b.translation -= collision_normal * (positional_correction * inverse_mass_b);

            let relative_velocity = sphere_a.velocity - sphere_b.velocity;
            let velocity_along_normal = relative_velocity.dot(collision_normal);

            if velocity_along_normal < 0.0 {
                let impulse =
                    -(1.0 + config.impact_restitution) * velocity_along_normal / total_inverse_mass;
                sphere_a.velocity += collision_normal * (impulse * inverse_mass_a);
                sphere_b.velocity -= collision_normal * (impulse * inverse_mass_b);
                sphere_a.velocity *= config.cluster_damping;
                sphere_b.velocity *= config.cluster_damping;
            }
        }
    }
}

fn evaluate_submersion_and_inactivity(
    sphere: &mut DynamicRigidSphere,
    physics: &GridPhysicsApi,
    current_position: Vec3,
    delta_time: f32,
    active_velocity_squared: f32,
) -> bool {
    if active_velocity_squared > physics.config.wake_velocity_squared {
        sphere.is_granular_inactive = false;
    } else if active_velocity_squared < physics.config.sleep_velocity_squared {
        sphere.is_granular_inactive = true;
    }

    let sphere_radius = sphere.radius;
    let minimum_grid_x = (current_position.x - sphere_radius).floor() as i32;
    let maximum_grid_x = (current_position.x + sphere_radius).floor() as i32;
    let minimum_grid_y = (-(current_position.z + sphere_radius)).floor() as i32;
    let maximum_grid_y = (-(current_position.z - sphere_radius)).floor() as i32;

    let mut max_surrounding_granular_height = f32::NEG_INFINITY;
    for grid_y in minimum_grid_y..=maximum_grid_y {
        for grid_x in minimum_grid_x..=maximum_grid_x {
            if let Some(height) = physics.get_floor_height(IVec2::new(grid_x, grid_y)) {
                max_surrounding_granular_height = max_surrounding_granular_height.max(height);
            }
        }
    }

    let is_submerged = max_surrounding_granular_height > current_position.y;
    if is_submerged {
        let horizontal_speed = (sphere.velocity.x.powi(2) + sphere.velocity.z.powi(2)).sqrt();
        if horizontal_speed > 0.1 {
            sphere.velocity.y +=
                physics.config.submerged_buoyancy_lift * delta_time * horizontal_speed;
        }
    }
    is_submerged
}

fn apply_dynamic_carving_and_deformation(
    sphere: &mut DynamicRigidSphere,
    physics: &mut GridPhysicsApi,
    current_position: Vec3,
    initial_kinetic_energy: f32,
    is_actively_moving: bool,
    is_submerged: bool,
) -> (u32, Option<GranularMat>) {
    let sphere_radius = sphere.radius;
    let minimum_grid_x = (current_position.x - sphere_radius).floor() as i32;
    let maximum_grid_x = (current_position.x + sphere_radius).floor() as i32;
    let minimum_grid_y = (-(current_position.z + sphere_radius)).floor() as i32;
    let maximum_grid_y = (-(current_position.z - sphere_radius)).floor() as i32;

    let center_position = IVec2::new(
        current_position.x.floor() as i32,
        (-current_position.z).floor() as i32,
    );
    let baseline_height = physics
        .get_floor_height(center_position)
        .unwrap_or(current_position.y - sphere_radius);

    let local_resistance = physics.compute_outward_resistance(center_position, baseline_height);
    let effective_crush_cost = physics.config.cost_crush_terrain * local_resistance;

    let deformation_extension = if initial_kinetic_energy > 0.0 {
        (initial_kinetic_energy * 0.005)
            .sqrt()
            .min(sphere_radius * 0.4)
    } else {
        0.0
    };

    let mut remaining_kinetic_energy = initial_kinetic_energy;
    let mut harvested_granular_volume = 0u32;
    let mut dominant_granular_material = None;
    let mut actual_crushed_terrain = 0u32;

    for grid_y in minimum_grid_y..=maximum_grid_y {
        for grid_x in minimum_grid_x..=maximum_grid_x {
            let cell_position = IVec2::new(grid_x, grid_y);
            if !physics.is_in_bounds(cell_position) {
                continue;
            }

            let distance_squared = (grid_x as f32 + physics.config.half_cell_offset
                - current_position.x)
                .powi(2)
                + (-grid_y as f32 - physics.config.half_cell_offset - current_position.z).powi(2);

            if distance_squared <= sphere_radius.powi(2) {
                let vertical_offset = (sphere_radius.powi(2) - distance_squared).sqrt();
                let carving_bottom_height =
                    current_position.y - vertical_offset - deformation_extension;

                physics.clear_surface_organics(cell_position, carving_bottom_height);

                let (crushed_volume, energy_spent) = physics.crush_bedrock(
                    cell_position,
                    carving_bottom_height,
                    remaining_kinetic_energy,
                    effective_crush_cost,
                );
                if crushed_volume > 0 {
                    actual_crushed_terrain += crushed_volume;
                    remaining_kinetic_energy = (remaining_kinetic_energy - energy_spent).max(0.0);
                }

                if is_actively_moving && !sphere.is_granular_inactive && !is_submerged {
                    let (excavated_volume, excavated_material) =
                        physics.excavate_granular(cell_position, carving_bottom_height);
                    if excavated_volume > 0 {
                        harvested_granular_volume += excavated_volume;
                        if dominant_granular_material.is_none() {
                            dominant_granular_material = excavated_material;
                        }
                        if remaining_kinetic_energy > 0.0 {
                            remaining_kinetic_energy = (remaining_kinetic_energy
                                - (excavated_volume as f32
                                    * physics.config.cost_displace_granular))
                                .max(0.0);
                        }
                    }
                }
            }
        }
    }

    if actual_crushed_terrain > 0 {
        sphere.accumulated_compaction +=
            actual_crushed_terrain as f32 * physics.config.force_to_volume_factor;
        if initial_kinetic_energy > 0.0 && remaining_kinetic_energy < initial_kinetic_energy {
            sphere.velocity *= (remaining_kinetic_energy / initial_kinetic_energy).sqrt();
        }
    }

    (harvested_granular_volume, dominant_granular_material)
}

fn apply_authoritative_collision_and_support(
    transform: &mut Transform,
    sphere: &mut DynamicRigidSphere,
    physics: &GridPhysicsApi,
    delta_time: f32,
    is_submerged: bool,
) {
    let mut current_position = transform.translation;
    let mut maximum_required_y = f32::NEG_INFINITY;
    let mut accumulated_push_direction = Vec3::ZERO;
    let mut wall_contact_count = 0;

    let step_threshold_height =
        current_position.y + sphere.radius * physics.config.step_threshold_ratio;
    let core_load_bearing_radius = sphere.radius * physics.config.load_bearing_radius_ratio;

    let minimum_grid_x = (current_position.x - sphere.radius).floor() as i32;
    let maximum_grid_x = (current_position.x + sphere.radius).floor() as i32;
    let minimum_grid_y = (-(current_position.z + sphere.radius)).floor() as i32;
    let maximum_grid_y = (-(current_position.z - sphere.radius)).floor() as i32;

    for grid_y in minimum_grid_y..=maximum_grid_y {
        for grid_x in minimum_grid_x..=maximum_grid_x {
            let cell_position = IVec2::new(grid_x, grid_y);
            let solid_height = physics.get_bedrock_height(cell_position).unwrap_or(0.0);
            let total_height = physics.get_floor_height(cell_position).unwrap_or(0.0);

            let distance_vector = Vec3::new(
                grid_x as f32 + physics.config.half_cell_offset - current_position.x,
                0.0,
                -grid_y as f32 - physics.config.half_cell_offset - current_position.z,
            );
            let distance_squared = distance_vector.length_squared();

            if distance_squared <= sphere.radius.powi(2) {
                let absolute_distance = distance_squared.sqrt();
                let vertical_offset = (sphere.radius.powi(2) - distance_squared).sqrt();
                let sphere_bottom_height = current_position.y - vertical_offset;

                if solid_height > step_threshold_height
                    && absolute_distance > physics.config.epsilon_distance
                {
                    accumulated_push_direction += (distance_vector / absolute_distance)
                        * -(sphere.radius - absolute_distance);
                    wall_contact_count += 1;
                }

                if absolute_distance <= core_load_bearing_radius {
                    if solid_height <= step_threshold_height && solid_height > sphere_bottom_height
                    {
                        maximum_required_y = maximum_required_y.max(solid_height + vertical_offset);
                    }
                    if !is_submerged
                        && total_height <= step_threshold_height
                        && total_height > sphere_bottom_height
                    {
                        maximum_required_y = maximum_required_y.max(total_height + vertical_offset);
                    }
                }
            }
        }
    }

    if wall_contact_count > 0 {
        let mean_push_direction = (accumulated_push_direction / wall_contact_count as f32)
            .clamp_length_max(sphere.radius * physics.config.max_wall_push_ratio);
        current_position += mean_push_direction;
        transform.translation.x = current_position.x;
        transform.translation.z = current_position.z;

        if mean_push_direction.length_squared() > physics.config.epsilon_distance {
            let wall_normal = mean_push_direction.normalize();
            let impact_velocity = sphere.velocity.dot(wall_normal);
            if impact_velocity < 0.0 {
                sphere.velocity -=
                    (1.0 + physics.config.impact_restitution) * impact_velocity * wall_normal;
            }
        }
    }

    if current_position.y <= maximum_required_y {
        transform.translation.y = maximum_required_y;
        if sphere.velocity.y < 0.0 {
            let gravity_drift_velocity = (physics.config.gravity.y * delta_time).abs();
            let bounce_velocity_threshold = physics
                .config
                .min_bounce_velocity
                .max(gravity_drift_velocity + physics.config.grounding_drift_buffer);

            if sphere.velocity.y.abs() > bounce_velocity_threshold {
                sphere.velocity.y = -sphere.velocity.y * physics.config.impact_restitution;
            } else {
                sphere.velocity.y = 0.0;
            }
            sphere.velocity.x *= physics.config.rolling_friction;
            sphere.velocity.z *= physics.config.rolling_friction;
        }
    } else {
        sphere.accumulated_compaction = (sphere.accumulated_compaction
            - delta_time * physics.config.compaction_decay_rate)
            .max(0.0);
    }
}

fn deposit_harvested_rim_material(
    physics: &mut GridPhysicsApi,
    current_position: Vec3,
    sphere_radius: f32,
    harvested_volume: u32,
    dominant_material: Option<GranularMat>,
) {
    let deposit_material = dominant_material.unwrap_or(GranularMat::GRANULAR_DIRT);
    let radius_inner = sphere_radius * physics.config.rim_inner_radius_ratio;
    let radius_outer = sphere_radius * physics.config.rim_expansion_factor;

    let minimum_grid_x = (current_position.x - radius_outer).floor() as i32;
    let maximum_grid_x = (current_position.x + radius_outer).floor() as i32;
    let minimum_grid_y = (-(current_position.z - radius_outer)).floor() as i32;
    let maximum_grid_y = (-(current_position.z - radius_outer)).floor() as i32;

    let mut available_cell_capacities = Vec::new();
    let mut total_rim_capacity = 0u32;
    let maximum_allowed_height =
        current_position.y + sphere_radius * physics.config.blade_ceiling_ratio;

    for grid_y in minimum_grid_y..=maximum_grid_y {
        for grid_x in minimum_grid_x..=maximum_grid_x {
            let cell_position = IVec2::new(grid_x, grid_y);
            let distance_from_center = ((grid_x as f32 + physics.config.half_cell_offset
                - current_position.x)
                .powi(2)
                + (-grid_y as f32 - physics.config.half_cell_offset - current_position.z).powi(2))
            .sqrt();

            if distance_from_center >= radius_inner
                && distance_from_center <= radius_outer
                && physics.is_in_bounds(cell_position)
            {
                let cell = physics.grid.get_cell(cell_position);
                if cell.granular_mat() == GranularMat::EMPTY
                    || cell.granular_mat() == deposit_material
                {
                    let total_cell_height = (cell.elevation() as f32 + cell.granular_vol() as f32)
                        * physics.config.elevation_scale;
                    if total_cell_height < maximum_allowed_height {
                        let volume_allowance = ((maximum_allowed_height - total_cell_height)
                            / physics.config.elevation_scale)
                            .floor() as u16;
                        let cell_capacity = WorldCell::MAX_GRANULAR_VOL
                            .saturating_sub(cell.granular_vol())
                            .min(physics.config.max_rim_deposit_per_cell)
                            .min(volume_allowance);
                        if cell_capacity > 0 {
                            total_rim_capacity += cell_capacity as u32;
                            available_cell_capacities.push((cell_position, cell_capacity));
                        }
                    }
                }
            }
        }
    }

    if total_rim_capacity > 0 {
        let fill_ratio = harvested_volume as f32 / total_rim_capacity as f32;
        for (cell_position, cell_capacity) in available_cell_capacities {
            let exact_fill_volume = cell_capacity as f32 * fill_ratio;
            let base_deposit_volume = exact_fill_volume.floor() as u16;

            let remainder_probability = exact_fill_volume - base_deposit_volume as f32;
            let spatial_hash_value = (spatial_hash(cell_position, physics.grid.tick)
                % physics.config.probability_hash_scale as u32)
                as f32
                / physics.config.probability_hash_scale;
            let extra_deposit = if spatial_hash_value < remainder_probability {
                1
            } else {
                0
            };

            let final_deposit_volume = (base_deposit_volume + extra_deposit).min(cell_capacity);
            if final_deposit_volume > 0 {
                physics.deposit_granular(
                    cell_position,
                    deposit_material,
                    final_deposit_volume,
                    maximum_allowed_height,
                );
            }
        }
    }
}

pub fn simulate_rigid_sphere_kinematics(
    mut spheres: Query<(&mut Transform, &mut DynamicRigidSphere)>,
    mut grid: ResMut<ActiveWorldGrid>,
    time: Res<Time>,
    config: Res<EntityPhysicsConfig>,
) {
    let delta_time = time.delta_secs();

    integrate_ballistic_motion(&mut spheres, &config, delta_time);
    resolve_sphere_sphere_collisions(&mut spheres, &config);

    let mut physics_api = GridPhysicsApi::new(&mut grid, &config);

    for (mut transform, mut sphere) in spheres.iter_mut() {
        let mut active_velocity = sphere.velocity - (config.gravity * delta_time);
        if active_velocity.length_squared() < config.sleep_velocity_squared {
            active_velocity = Vec3::ZERO;
        }

        let kinetic_energy = 0.5 * sphere.mass * active_velocity.length_squared();
        let is_actively_moving = active_velocity.length_squared() > config.wake_velocity_squared;

        let is_submerged = evaluate_submersion_and_inactivity(
            &mut sphere,
            &physics_api,
            transform.translation,
            delta_time,
            active_velocity.length_squared(),
        );

        let (harvested_volume, dominant_material) = apply_dynamic_carving_and_deformation(
            &mut sphere,
            &mut physics_api,
            transform.translation,
            kinetic_energy,
            is_actively_moving,
            is_submerged,
        );

        apply_authoritative_collision_and_support(
            &mut transform,
            &mut sphere,
            &physics_api,
            delta_time,
            is_submerged,
        );

        if harvested_volume > 0 {
            deposit_harvested_rim_material(
                &mut physics_api,
                transform.translation,
                sphere.radius,
                harvested_volume,
                dominant_material,
            );
        }
    }
}
