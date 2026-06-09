use bevy::prelude::*;
use bevy_egui::EguiContexts;
use monarch_engine::{
    engine::entities::{
        DeformationProfile, GlobalPhysicsConfig, KinematicProfile, spherical::DynamicRigidSphere,
    },
    prelude::{ActiveWorldGrid, FluidMat, GranularMat, SurfaceMat, WorldCell},
};

use crate::runtime::{
    dev_tools::{BrushSettings, GridBrush},
    render::WorldMaterial,
};

const RAYMARCH_MAX_DIST: f32 = 1000.0;
const SPHERE_SPAWN_OFFSET_Y: f32 = 10.0;
const SPHERE_SPAWN_RADIUS: f32 = 5.0;
const SPHERE_SPAWN_MASS: f32 = 5.0;
const ATTRACT_FORCE_MAGNITUDE: f32 = 250.0;
const LIFT_ACCELERATION: f32 = 250.0;
const LIFT_CENTERING_FORCE: f32 = 40.0;
const LIFT_INFLUENCE_RADIUS_SQ: f32 = 22500.0;
const EPSILON_DIST_SQ: f32 = 0.0001;

/// Validates absolute mathematical grid inclusion to prevent memory panics.
#[inline(always)]
fn is_within_grid(pos: IVec2, grid: &ActiveWorldGrid) -> bool {
    let bounds_min = grid.spatial.window_origin;
    let bounds_max = bounds_min + IVec2::new(grid.spatial.width, grid.spatial.height);
    pos.x >= bounds_min.x && pos.x < bounds_max.x && pos.y >= bounds_min.y && pos.y < bounds_max.y
}

/// A unified extraction membrane for harvesting deterministic cursor raycasts,
/// bypassing ECS queries when obscured by UI boundaries.
fn extract_pointer_hit(
    windows: &Query<&Window>,
    camera_q: &Query<(&Camera, &GlobalTransform)>,
    grid: &ActiveWorldGrid,
    elevation_scale: f32,
    egui_contexts: &mut EguiContexts,
) -> Option<Vec3> {
    let ctx = egui_contexts.ctx_mut().ok()?;
    if ctx.wants_pointer_input() {
        return None;
    }

    let window = windows.single().ok()?;
    let (camera, camera_transform) = camera_q.single().ok()?;
    let cursor_pos = window.cursor_position()?;
    let ray = camera
        .viewport_to_world(camera_transform, cursor_pos)
        .ok()?;

    raymarch_grid(&ray, grid, elevation_scale)
}

/// Volumetric raymarcher executing a pure 2D Digital Differential Analyzer (DDA)
/// across the XZ plane to mathematically guarantee intersection with the dynamic thermodynamic floor.
#[inline(always)]
pub fn raymarch_grid(ray: &Ray3d, grid: &ActiveWorldGrid, elevation_scale: f32) -> Option<Vec3> {
    let start_pos = ray.origin;

    // Isolate mathematical origin coordinates
    let mut cx = start_pos.x.floor() as i32;
    let mut cy = (-start_pos.z).floor() as i32;

    // Vector step directions mapped to the grid's topology (cy acts against -Z)
    let step_x = if ray.direction.x > 0.0 { 1 } else { -1 };
    let step_y = if ray.direction.z < 0.0 { 1 } else { -1 };

    // Delta T: distance the ray must travel to cross exactly one full cell width/height
    let t_delta_x = if ray.direction.x != 0.0 {
        (1.0 / ray.direction.x).abs()
    } else {
        f32::MAX
    };
    let t_delta_y = if ray.direction.z != 0.0 {
        (1.0 / ray.direction.z).abs()
    } else {
        f32::MAX
    };

    // Max T: distance to the very first cellular boundary crossing
    let mut t_max_x = if ray.direction.x > 0.0 {
        (cx as f32 + 1.0 - start_pos.x) * t_delta_x
    } else {
        (start_pos.x - cx as f32) * t_delta_x
    };

    let mut t_max_y = if ray.direction.z < 0.0 {
        (cy as f32 + 1.0 - (-start_pos.z)) * t_delta_y
    } else {
        ((-start_pos.z) - cy as f32) * t_delta_y
    };

    let bounds_min = grid.spatial.window_origin;
    let bounds_max = bounds_min + IVec2::new(grid.spatial.width, grid.spatial.height);

    let mut t = 0.0;

    while t < RAYMARCH_MAX_DIST {
        if cx >= bounds_min.x && cx < bounds_max.x && cy >= bounds_min.y && cy < bounds_max.y {
            let cell = grid.get_cell(IVec2::new(cx, cy));
            let h = (cell.elevation() as f32 + cell.granular_vol() as f32) * elevation_scale;

            let entry_t = t;
            let exit_t = t_max_x.min(t_max_y);

            let y_entry = start_pos.y + ray.direction.y * entry_t;
            let y_exit = start_pos.y + ray.direction.y * exit_t;

            // Check if the ray's vertical bounds intersect the physical cellular pillar
            if y_entry.min(y_exit) <= h {
                if ray.direction.y < 0.0 {
                    if y_entry >= h {
                        let hit_t = (h - start_pos.y) / ray.direction.y;
                        return Some(start_pos + *ray.direction * hit_t);
                    } else {
                        return Some(start_pos + *ray.direction * entry_t);
                    }
                } else if y_entry <= h {
                    return Some(start_pos + *ray.direction * entry_t);
                }
            }
        } else if start_pos.y + ray.direction.y * t < 0.0 {
            // Ray has fallen off the mathematical map into the void
            return None;
        }

        // Advance the DDA to the next cellular boundary
        if t_max_x < t_max_y {
            t = t_max_x;
            t_max_x += t_delta_x;
            cx += step_x;
        } else {
            t = t_max_y;
            t_max_y += t_delta_y;
            cy += step_y;
        }
    }
    None
}

/// Mutates the GPU uniform buffer directly, eliminating ECS entity allocation overhead.
pub fn update_brush_cursor(
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform)>,
    grid: Res<ActiveWorldGrid>,
    global_config: Res<GlobalPhysicsConfig>,
    brush: Res<GridBrush>,
    settings: Res<BrushSettings>,
    mut materials: ResMut<Assets<WorldMaterial>>,
    mut egui_contexts: EguiContexts,
) {
    let is_active = *brush != GridBrush::None;

    // Evaluate raycast extraction once, minimizing global lock contention
    let hit_pos = if is_active {
        extract_pointer_hit(
            &windows,
            &camera_q,
            &grid,
            global_config.elevation_scale,
            &mut egui_contexts,
        )
    } else {
        None
    };

    for (_, mat) in materials.iter_mut() {
        if let Some(pos) = hit_pos {
            let cursor_radius = if *brush == GridBrush::SpawnSphere {
                0.0
            } else {
                settings.radius as f32
            };
            mat.window.head_cursor.z = pos.x.floor();
            mat.window.head_cursor.w = (-pos.z).floor();
            mat.window.config.y = cursor_radius;
        } else {
            mat.window.config.y = -1.0;
        }
    }
}

pub fn handle_brush_input(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform)>,
    mut grid: ResMut<ActiveWorldGrid>,
    brush: Res<GridBrush>,
    settings: Res<BrushSettings>,
    global_config: Res<GlobalPhysicsConfig>,
    mut egui_contexts: EguiContexts,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if *brush == GridBrush::None {
        return;
    }

    let is_spawning_entity = *brush == GridBrush::SpawnSphere;
    if is_spawning_entity {
        if !mouse.just_pressed(MouseButton::Left) {
            return;
        }
    } else if !mouse.pressed(MouseButton::Left) {
        return;
    }

    let Some(hit_position) = extract_pointer_hit(
        &windows,
        &camera_q,
        &grid,
        global_config.elevation_scale,
        &mut egui_contexts,
    ) else {
        return;
    };

    let center_x = hit_position.x.floor() as i32;
    let center_y = (-hit_position.z).floor() as i32;
    let cell_pos = IVec2::new(center_x, center_y);

    if is_spawning_entity {
        let mut spawn_y = SPHERE_SPAWN_OFFSET_Y;

        if is_within_grid(cell_pos, &grid) {
            let cell = grid.get_cell(cell_pos);
            let floor_h = (cell.elevation() as f32 + cell.granular_vol() as f32)
                * global_config.elevation_scale;
            spawn_y = floor_h + SPHERE_SPAWN_OFFSET_Y;
        }

        commands.spawn((
            Mesh3d(meshes.add(Sphere::new(SPHERE_SPAWN_RADIUS))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.6, 0.6, 0.65),
                metallic: 0.8,
                perceptual_roughness: 0.2,
                ..default()
            })),
            Transform::from_translation(Vec3::new(hit_position.x, spawn_y, hit_position.z)),
            DynamicRigidSphere::new(SPHERE_SPAWN_MASS, SPHERE_SPAWN_RADIUS),
            KinematicProfile::default(),
            DeformationProfile::default(),
        ));
        return;
    }

    let radius = settings.radius;
    let radius_sq = radius * radius;

    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx * dx + dy * dy > radius_sq {
                continue;
            }

            let world_position = IVec2::new(center_x + dx, center_y + dy);

            if is_within_grid(world_position, &grid) {
                let mut cell = grid.get_cell(world_position);
                let mut cell_was_mutated = false;

                match *brush {
                    GridBrush::Water => {
                        let old_state = if cell.fluid_mat() == FluidMat::FLUID_WATER {
                            cell.fluid_vol()
                        } else {
                            0
                        };
                        let new_state =
                            old_state.saturating_add(settings.strength as u16).min(1023);

                        if cell.fluid_mat() != FluidMat::FLUID_WATER
                            || cell.fluid_vol() != new_state
                        {
                            cell.set_fluid_mat(FluidMat::FLUID_WATER);
                            cell.set_fluid_vol(new_state);
                            cell_was_mutated = true;
                        }
                    }
                    GridBrush::Fire => {
                        let fluid = cell.fluid_mat();
                        if cell.surface_mat() != SurfaceMat::SURFACE_FIRE
                            && (fluid == FluidMat::EMPTY || fluid == FluidMat::FLUID_OIL)
                        {
                            cell.set_surface_mat(SurfaceMat::SURFACE_FIRE);
                            cell.set_surface_state(0);
                            cell_was_mutated = true;
                        }
                    }
                    GridBrush::Sand => {
                        let old_vol = if cell.granular_mat() == GranularMat::GRANULAR_SAND {
                            cell.granular_vol()
                        } else {
                            0
                        };
                        let new_vol = old_vol
                            .saturating_add(settings.strength as u16)
                            .min(WorldCell::MAX_GRANULAR_VOL);

                        if cell.granular_mat() != GranularMat::GRANULAR_SAND
                            || cell.granular_vol() != new_vol
                        {
                            cell.set_granular_mat(GranularMat::GRANULAR_SAND);
                            cell.set_granular_vol(new_vol);
                            cell_was_mutated = true;
                        }
                    }
                    GridBrush::RaiseTerrain => {
                        let new_elevation =
                            cell.elevation().saturating_add(settings.strength as u16);
                        if new_elevation != cell.elevation() {
                            cell.set_elevation(new_elevation);
                            cell_was_mutated = true;
                        }
                    }
                    GridBrush::LowerTerrain => {
                        let new_elevation =
                            cell.elevation().saturating_sub(settings.strength as u16);
                        if new_elevation != cell.elevation() {
                            cell.set_elevation(new_elevation);
                            cell_was_mutated = true;
                        }
                    }
                    _ => {}
                }

                if cell_was_mutated {
                    grid.set_cell(world_position, cell);
                    grid.wake_cell(world_position);
                }
            }
        }
    }
}

pub fn attract_spheres_input(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform)>,
    grid: Res<ActiveWorldGrid>,
    global_config: Res<GlobalPhysicsConfig>,
    mut spheres: Query<(&Transform, &mut DynamicRigidSphere)>,
    time: Res<Time>,
    mut egui_contexts: EguiContexts,
) {
    if !mouse.pressed(MouseButton::Back) && !mouse.pressed(MouseButton::Other(4)) {
        return;
    }

    let Some(hit_position) = extract_pointer_hit(
        &windows,
        &camera_q,
        &grid,
        global_config.elevation_scale,
        &mut egui_contexts,
    ) else {
        return;
    };

    let dt = time.delta_secs();

    for (transform, mut sphere) in spheres.iter_mut() {
        let to_target = hit_position - transform.translation;
        let dist_sq = to_target.length_squared();

        if dist_sq > EPSILON_DIST_SQ {
            let direction = to_target.normalize();
            sphere.velocity += direction * ATTRACT_FORCE_MAGNITUDE * dt;
        }
    }
}

pub fn lift_spheres_input(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform)>,
    grid: Res<ActiveWorldGrid>,
    global_config: Res<GlobalPhysicsConfig>,
    mut spheres: Query<(&Transform, &mut DynamicRigidSphere)>,
    time: Res<Time>,
    mut egui_contexts: EguiContexts,
) {
    if !mouse.pressed(MouseButton::Forward) && !mouse.pressed(MouseButton::Other(5)) {
        return;
    }

    let Some(hit_position) = extract_pointer_hit(
        &windows,
        &camera_query,
        &grid,
        global_config.elevation_scale,
        &mut egui_contexts,
    ) else {
        return;
    };

    let delta_time = time.delta_secs();

    for (transform, mut sphere) in spheres.iter_mut() {
        let vector_to_target = hit_position - transform.translation;
        let horizontal_offset = Vec3::new(vector_to_target.x, 0.0, vector_to_target.z);
        let distance_squared = horizontal_offset.length_squared();

        if distance_squared < LIFT_INFLUENCE_RADIUS_SQ {
            sphere.velocity.y += LIFT_ACCELERATION * delta_time;

            if distance_squared > EPSILON_DIST_SQ {
                let horizontal_direction = horizontal_offset.normalize();
                sphere.velocity += horizontal_direction * LIFT_CENTERING_FORCE * delta_time;
            }
        }
    }
}
