use crate::runtime::render::WorldTuningConfig;
use bevy::prelude::*;
use monarch_engine::prelude::ActiveWorldGrid;

// --- Physical Constants ---
pub const SPHERE_GRAVITY_MULTIPLIER: f32 = 35.0;
pub const SPHERE_ROLLING_FRICTION: f32 = 0.96;
pub const SPHERE_DEFAULT_RADIUS: f32 = 5.0;
pub const OUT_OF_BOUNDS_REPULSION_HEIGHT: f32 = 1000.0;
pub const KINEMATIC_IMPACT_THRESHOLD: f32 = 1.0;
pub const FREE_FALL_LERP_RATE: f32 = 50.0;

// --- Interaction Constants ---
pub const IMPACT_GRANULAR_DEGRADATION_CHANCE: u32 = 20;
pub const IMPACT_TERRAIN_DEGRADATION_CHANCE: u32 = 100;
pub const RANDOM_HASH_MODULO: u32 = 100;

// --- Visual Constants ---
pub const SPAWN_HEIGHT_OFFSET: f32 = 5.0;
pub const SPHERE_BASE_COLOR: Color = Color::srgb(0.6, 0.6, 0.65);
pub const SPHERE_METALLIC_VALUE: f32 = 1.0;
pub const SPHERE_ROUGHNESS_VALUE: f32 = 0.2;

pub struct EntitiesPlugin;

impl Plugin for EntitiesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, update_metal_spheres);
    }
}

/// A dynamic spherical physics entity that interacts with the Cellular Automata grid.
#[derive(Component)]
pub struct MetalSphere {
    pub velocity: Vec3,
    pub radius: f32,
}

impl Default for MetalSphere {
    fn default() -> Self {
        Self {
            velocity: Vec3::ZERO,
            radius: SPHERE_DEFAULT_RADIUS,
        }
    }
}

/// Helper function to safely read grid heights without keeping the grid borrowed.
fn calculate_floor_height(
    grid: &ActiveWorldGrid,
    position: IVec2,
    bounds_minimum: IVec2,
    bounds_maximum: IVec2,
    elevation_scale: f32,
) -> f32 {
    let is_within_bounds = position.x >= bounds_minimum.x
        && position.x < bounds_maximum.x
        && position.y >= bounds_minimum.y
        && position.y < bounds_maximum.y;

    if is_within_bounds {
        let world_cell = grid.get_cell(position);
        (world_cell.elevation() as f32 + world_cell.granular_vol() as f32) * elevation_scale
    } else {
        OUT_OF_BOUNDS_REPULSION_HEIGHT
    }
}

/// Kinematic physics step for updating the position and terrain impact of MetalSpheres.
pub fn update_metal_spheres(
    mut spheres: Query<(&mut Transform, &mut MetalSphere)>,
    mut grid: ResMut<ActiveWorldGrid>,
    time: Res<Time>,
    tuning: Res<WorldTuningConfig>,
) {
    let delta_time = time.delta_secs();
    let elevation_scale = tuning.elevation_scale;
    let bounds_minimum = grid.window_origin;
    let bounds_maximum = grid.window_origin + IVec2::new(grid.width, grid.height);

    for (mut transform, mut sphere) in spheres.iter_mut() {
        let current_position = transform.translation;
        let grid_x_coordinate = current_position.x.floor() as i32;
        let grid_y_coordinate = (-current_position.z).floor() as i32;

        let world_position = IVec2::new(grid_x_coordinate, grid_y_coordinate);

        let is_outside_simulation_grid = world_position.x < bounds_minimum.x
            || world_position.x >= bounds_maximum.x
            || world_position.y < bounds_minimum.y
            || world_position.y >= bounds_maximum.y;

        if is_outside_simulation_grid {
            continue;
        }

        let height_positive_x = calculate_floor_height(
            &grid,
            world_position + IVec2::new(1, 0),
            bounds_minimum,
            bounds_maximum,
            elevation_scale,
        );
        let height_negative_x = calculate_floor_height(
            &grid,
            world_position + IVec2::new(-1, 0),
            bounds_minimum,
            bounds_maximum,
            elevation_scale,
        );
        let height_positive_y = calculate_floor_height(
            &grid,
            world_position + IVec2::new(0, 1),
            bounds_minimum,
            bounds_maximum,
            elevation_scale,
        );
        let height_negative_y = calculate_floor_height(
            &grid,
            world_position + IVec2::new(0, -1),
            bounds_minimum,
            bounds_maximum,
            elevation_scale,
        );

        let slope_x_axis = height_negative_x - height_positive_x;
        let slope_y_axis = height_negative_y - height_positive_y;

        sphere.velocity.x += slope_x_axis * SPHERE_GRAVITY_MULTIPLIER * delta_time;
        sphere.velocity.z -= slope_y_axis * SPHERE_GRAVITY_MULTIPLIER * delta_time;

        sphere.velocity *= SPHERE_ROLLING_FRICTION;

        transform.translation.x += sphere.velocity.x * delta_time;
        transform.translation.z += sphere.velocity.z * delta_time;

        // Apply downward crushing pressure onto the CA grid based on velocity
        if sphere.velocity.length_squared() > KINEMATIC_IMPACT_THRESHOLD {
            let mut target_cell = grid.get_cell(world_position);
            let mut cell_was_mutated = false;

            let spatial_hash = (grid
                .tick
                .wrapping_add(world_position.x as u32)
                .wrapping_add(world_position.y as u32))
                % RANDOM_HASH_MODULO;

            if target_cell.granular_vol() > 0 && spatial_hash < IMPACT_GRANULAR_DEGRADATION_CHANCE {
                target_cell.set_granular_vol(target_cell.granular_vol().saturating_sub(1));
                cell_was_mutated = true;
            } else if target_cell.elevation() > 0
                && spatial_hash < IMPACT_TERRAIN_DEGRADATION_CHANCE
            {
                target_cell.set_elevation(target_cell.elevation().saturating_sub(1));
                cell_was_mutated = true;
            }

            if cell_was_mutated {
                grid.set_cell(world_position, target_cell);
                grid.wake_cell(world_position);
            }
        }

        let new_grid_x_coordinate = transform.translation.x.floor() as i32;
        let new_grid_y_coordinate = (-transform.translation.z).floor() as i32;
        let new_surface_height = calculate_floor_height(
            &grid,
            IVec2::new(new_grid_x_coordinate, new_grid_y_coordinate),
            bounds_minimum,
            bounds_maximum,
            elevation_scale,
        );

        let target_vertical_position = new_surface_height + sphere.radius;
        if transform.translation.y < target_vertical_position {
            transform.translation.y = target_vertical_position;
        } else {
            transform.translation.y = transform
                .translation
                .y
                .lerp(target_vertical_position, FREE_FALL_LERP_RATE * delta_time);
        }
    }
}
