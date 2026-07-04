use bevy::{
    ecs::{
        component::Component,
        system::{Query, Res, ResMut},
    },
    math::{IVec2, Vec3},
    time::Time,
    transform::components::Transform,
};

use crate::core::{
    entities::{DeformationProfile, GlobalPhysicsConfig, KinematicProfile},
    physics::grid_physics::GridPhysicsApi,
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

    // Geometry constraints
    pub sleep_velocity_squared: f32,
    pub wake_velocity_squared: f32,
    pub submerged_buoyancy_lift: f32,
    pub buoyancy_horizontal_speed_threshold: f32,
    pub step_threshold_ratio: f32,
    pub load_bearing_radius_ratio: f32,
    pub max_wall_push_ratio: f32,
    pub rim_inner_radius_ratio: f32,
    pub rim_expansion_factor: f32,
    pub blade_ceiling_ratio: f32,
    pub max_rim_deposit_per_cell: u16,
    pub force_to_volume_factor: f32,
    pub compaction_decay_rate: f32,
    pub max_deformation_size_ratio: f32,
    pub grounding_drift_buffer: f32,
}

impl DynamicRigidSphere {
    pub fn new(mass: f32, radius: f32) -> Self {
        Self {
            velocity: Vec3::ZERO,
            mass,
            radius,
            accumulated_compaction: 0.0,
            is_granular_inactive: false,
            sleep_velocity_squared: 0.1,
            wake_velocity_squared: 0.5,
            submerged_buoyancy_lift: 2.5,
            buoyancy_horizontal_speed_threshold: 0.1,
            step_threshold_ratio: 0.5,
            load_bearing_radius_ratio: 0.85,
            max_wall_push_ratio: 0.5,
            rim_inner_radius_ratio: 0.9,
            rim_expansion_factor: 1.5,
            blade_ceiling_ratio: 0.10,
            max_rim_deposit_per_cell: 3,
            force_to_volume_factor: 0.8,
            compaction_decay_rate: 8.0,
            max_deformation_size_ratio: 1.0,
            grounding_drift_buffer: 0.01,
        }
    }
}

pub fn simulate_rigid_sphere_kinematics(
    mut spheres: Query<(
        &mut Transform,
        &mut DynamicRigidSphere,
        &KinematicProfile,
        &DeformationProfile,
    )>,
    mut grid: ResMut<ActiveWorldGrid>,
    time: Res<Time>,
    global_config: Res<GlobalPhysicsConfig>,
) {
    let delta_time = time.delta_secs();

    // Ballistic Integration
    for (mut transform, mut sphere, kinematic, _) in spheres.iter_mut() {
        sphere.velocity += global_config.gravity * delta_time;
        sphere.velocity.x *= kinematic.air_resistance;
        sphere.velocity.z *= kinematic.air_resistance;
        transform.translation += sphere.velocity * delta_time;
    }

    // Rigid Collisions
    let mut combinations = spheres.iter_combinations_mut();
    while let Some([(mut t_a, mut s_a, k_a, _), (mut t_b, mut s_b, k_b, _)]) =
        combinations.fetch_next()
    {
        let pos_delta = t_a.translation - t_b.translation;
        let dist_sq = pos_delta.length_squared();
        let min_dist = s_a.radius + s_b.radius;

        if dist_sq < min_dist * min_dist {
            let dist = dist_sq.sqrt();
            let normal = if dist > global_config.epsilon_distance {
                pos_delta / dist
            } else {
                Vec3::X
            };
            let overlap = min_dist - dist;
            let inv_mass_a = 1.0 / s_a.mass;
            let inv_mass_b = 1.0 / s_b.mass;
            let total_inv_mass = inv_mass_a + inv_mass_b;

            let correction = overlap / total_inv_mass;
            t_a.translation += normal * (correction * inv_mass_a);
            t_b.translation -= normal * (correction * inv_mass_b);

            let rel_vel = s_a.velocity - s_b.velocity;
            let vel_normal = rel_vel.dot(normal);

            if vel_normal < 0.0 {
                let impulse = -(1.0 + k_a.impact_restitution.max(k_b.impact_restitution))
                    * vel_normal
                    / total_inv_mass;
                s_a.velocity += normal * (impulse * inv_mass_a);
                s_b.velocity -= normal * (impulse * inv_mass_b);
            }
        }
    }

    // Grid Physics Application
    let mut physics_api = GridPhysicsApi::new(&mut grid, &global_config);

    for (mut transform, mut sphere, kinematic, deformation) in spheres.iter_mut() {
        let mut active_velocity = sphere.velocity - (global_config.gravity * delta_time);
        if active_velocity.length_squared() < sphere.sleep_velocity_squared {
            active_velocity = Vec3::ZERO;
        }

        let kinetic_energy = 0.5 * sphere.mass * active_velocity.length_squared();
        let is_moving = active_velocity.length_squared() > sphere.wake_velocity_squared;

        // Submersion Profile
        if is_moving {
            sphere.is_granular_inactive = false;
        } else if active_velocity.length_squared() < sphere.sleep_velocity_squared {
            sphere.is_granular_inactive = true;
        }

        let current_pos = transform.translation;
        let min_grid_x = (current_pos.x - sphere.radius).floor() as i32;
        let max_grid_x = (current_pos.x + sphere.radius).floor() as i32;
        let min_grid_y = (-(current_pos.z + sphere.radius)).floor() as i32;
        let max_grid_y = (-(current_pos.z - sphere.radius)).floor() as i32;

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
            let h_speed = (sphere.velocity.x.powi(2) + sphere.velocity.z.powi(2)).sqrt();
            if h_speed > sphere.buoyancy_horizontal_speed_threshold {
                sphere.velocity.y += sphere.submerged_buoyancy_lift * delta_time * h_speed;
            }
        }

        // Carving Profile
        let center_grid = IVec2::new(
            current_pos.x.floor() as i32,
            (-current_pos.z).floor() as i32,
        );
        let baseline_h = physics_api
            .get_floor_height(center_grid)
            .unwrap_or(current_pos.y - sphere.radius);

        let local_resistance =
            physics_api.compute_outward_resistance(center_grid, baseline_h, 2, 10, 16.0, 1.0);
        let effective_crush_cost = deformation.cost_crush_terrain * local_resistance;

        let downward_vel = sphere.velocity.y.min(0.0);
        let downward_ke = 0.5 * sphere.mass * downward_vel.powi(2);

        let def_extension = if downward_ke > deformation.min_deformation_energy {
            (downward_ke * deformation.energy_to_deformation_scale)
                .sqrt()
                .min(sphere.radius * sphere.max_deformation_size_ratio)
        } else {
            0.0
        };

        let mut rem_ke = kinetic_energy;
        let mut harvested_vol = 0u32;
        let mut dominant_mat = None;
        let mut crushed_terrain = 0u32;

        for gy in min_grid_y..=max_grid_y {
            for gx in min_grid_x..=max_grid_x {
                let cell_pos = IVec2::new(gx, gy);
                if !physics_api.is_in_bounds(cell_pos) {
                    continue;
                }

                let dist_sq = (gx as f32 + global_config.half_cell_offset - current_pos.x).powi(2)
                    + (-gy as f32 - global_config.half_cell_offset - current_pos.z).powi(2);

                if dist_sq <= sphere.radius.powi(2) {
                    let v_offset = (sphere.radius.powi(2) - dist_sq).sqrt();
                    let bottom_h = current_pos.y - v_offset - def_extension;

                    physics_api.clear_surface_organics(cell_pos, bottom_h);

                    let (crushed, spent) =
                        physics_api.crush_bedrock(cell_pos, bottom_h, rem_ke, effective_crush_cost);
                    if crushed > 0 {
                        crushed_terrain += crushed;
                        rem_ke = (rem_ke - spent).max(0.0);
                    }

                    if is_moving && !sphere.is_granular_inactive && !is_submerged {
                        let (excavated, exc_mat) =
                            physics_api.excavate_granular(cell_pos, bottom_h);
                        if excavated > 0 {
                            harvested_vol += excavated;
                            if dominant_mat.is_none() {
                                dominant_mat = exc_mat;
                            }
                            rem_ke = (rem_ke
                                - (excavated as f32 * deformation.cost_displace_granular))
                                .max(0.0);
                        }
                    }
                }
            }
        }

        if crushed_terrain > 0 {
            sphere.accumulated_compaction += crushed_terrain as f32 * sphere.force_to_volume_factor;
            if kinetic_energy > 0.0 && rem_ke < kinetic_energy {
                sphere.velocity *= (rem_ke / kinetic_energy).sqrt();
            }
        }

        // Authoritative Collision Support
        let mut max_req_y = f32::NEG_INFINITY;
        let mut acc_push = Vec3::ZERO;
        let mut wall_hits = 0;

        let step_threshold = current_pos.y + sphere.radius * sphere.step_threshold_ratio;
        let load_radius = sphere.radius * sphere.load_bearing_radius_ratio;

        for gy in min_grid_y..=max_grid_y {
            for gx in min_grid_x..=max_grid_x {
                let cell_pos = IVec2::new(gx, gy);
                let solid_h = physics_api.get_bedrock_height(cell_pos).unwrap_or(0.0);
                let total_h = physics_api.get_floor_height(cell_pos).unwrap_or(0.0);

                let d_vec = Vec3::new(
                    gx as f32 + global_config.half_cell_offset - current_pos.x,
                    0.0,
                    -gy as f32 - global_config.half_cell_offset - current_pos.z,
                );
                let d_sq = d_vec.length_squared();

                if d_sq <= sphere.radius.powi(2) {
                    let abs_dist = d_sq.sqrt();
                    let v_offset = (sphere.radius.powi(2) - d_sq).sqrt();
                    let s_bottom = current_pos.y - v_offset;

                    if solid_h > step_threshold && abs_dist > global_config.epsilon_distance {
                        acc_push += (d_vec / abs_dist) * -(sphere.radius - abs_dist);
                        wall_hits += 1;
                    }

                    if abs_dist <= load_radius {
                        if solid_h <= step_threshold && solid_h > s_bottom {
                            max_req_y = max_req_y.max(solid_h + v_offset);
                        }
                        if !is_submerged && total_h <= step_threshold && total_h > s_bottom {
                            max_req_y = max_req_y.max(total_h + v_offset);
                        }
                    }
                }
            }
        }

        if wall_hits > 0 {
            let mean_push = (acc_push / wall_hits as f32)
                .clamp_length_max(sphere.radius * sphere.max_wall_push_ratio);
            transform.translation += mean_push;

            if mean_push.length_squared() > global_config.epsilon_distance {
                let normal = mean_push.normalize();
                let impact_v = sphere.velocity.dot(normal);
                if impact_v < 0.0 {
                    sphere.velocity -= (1.0 + kinematic.impact_restitution) * impact_v * normal;
                }
            }
        }

        if transform.translation.y <= max_req_y {
            transform.translation.y = max_req_y;
            if sphere.velocity.y < 0.0 {
                let drift_v = (global_config.gravity.y * delta_time).abs();
                let bounce_t = 0.05_f32.max(drift_v + sphere.grounding_drift_buffer);

                if sphere.velocity.y.abs() > bounce_t {
                    sphere.velocity.y = -sphere.velocity.y * kinematic.impact_restitution;
                } else {
                    sphere.velocity.y = 0.0;
                }
                sphere.velocity.x *= kinematic.rolling_friction;
                sphere.velocity.z *= kinematic.rolling_friction;
            }
        } else {
            sphere.accumulated_compaction = (sphere.accumulated_compaction
                - delta_time * sphere.compaction_decay_rate)
                .max(0.0);
        }

        // Rim Deposit Generation
        if harvested_vol > 0 {
            let dep_mat = dominant_mat.unwrap_or(GranularMat::GRANULAR_DIRT);
            let r_inner = sphere.radius * sphere.rim_inner_radius_ratio;
            let r_outer = sphere.radius * sphere.rim_expansion_factor;

            let r_min_x = (current_pos.x - r_outer).floor() as i32;
            let r_max_x = (current_pos.x + r_outer).floor() as i32;
            let r_min_y = (-(current_pos.z - r_outer)).floor() as i32;
            let r_max_y = (-(current_pos.z - r_outer)).floor() as i32;

            let mut avail_cells = Vec::new();
            let mut total_cap = 0u32;
            let max_ceiling = current_pos.y + sphere.radius * sphere.blade_ceiling_ratio;

            for gy in r_min_y..=r_max_y {
                for gx in r_min_x..=r_max_x {
                    let cell_pos = IVec2::new(gx, gy);
                    let dist_c = ((gx as f32 + global_config.half_cell_offset - current_pos.x)
                        .powi(2)
                        + (-gy as f32 - global_config.half_cell_offset - current_pos.z).powi(2))
                    .sqrt();

                    if dist_c >= r_inner && dist_c <= r_outer && physics_api.is_in_bounds(cell_pos)
                    {
                        let cell = physics_api.grid.get_cell(cell_pos);
                        if cell.granular_mat() == GranularMat::EMPTY
                            || cell.granular_mat() == dep_mat
                        {
                            let total_h = (cell.elevation() as f32 + cell.granular_vol() as f32)
                                * global_config.elevation_scale;
                            if total_h < max_ceiling {
                                let vol_allowance =
                                    ((max_ceiling - total_h) / global_config.elevation_scale)
                                        .floor() as u16;
                                let cap = WorldCell::MAX_GRANULAR_VOL
                                    .saturating_sub(cell.granular_vol())
                                    .min(sphere.max_rim_deposit_per_cell)
                                    .min(vol_allowance);
                                if cap > 0 {
                                    total_cap += cap as u32;
                                    avail_cells.push((cell_pos, cap));
                                }
                            }
                        }
                    }
                }
            }

            if total_cap > 0 {
                let fill_ratio = harvested_vol as f32 / total_cap as f32;
                for (c_pos, cap) in avail_cells {
                    let exact = cap as f32 * fill_ratio;
                    let base_dep = exact.floor() as u16;
                    let rem = exact - base_dep as f32;

                    let hash_v =
                        (spatial_hash(c_pos, physics_api.grid.tick) % 1000) as f32 / 1000.0;
                    let extra = if hash_v < rem { 1 } else { 0 };

                    let final_dep = (base_dep + extra).min(cap);
                    if final_dep > 0 {
                        physics_api.deposit_granular(
                            c_pos,
                            dep_mat,
                            final_dep,
                            max_ceiling,
                            sphere.max_rim_deposit_per_cell,
                        );
                    }
                }
            }
        }
    }
}
