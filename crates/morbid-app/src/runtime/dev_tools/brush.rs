use bevy::prelude::*;
use monarch_engine::prelude::{spherical::DynamicRigidSphere, *};
use world_model::{
    body::{
        self, ATTRACT_ACCELERATION, LIFT_ACCELERATION, LIFT_CENTERING_ACCELERATION,
        LIFT_INFLUENCE_RADIUS,
    },
    cell::WorldCell,
    material::{FluidMat, GranularMat, SurfaceMat},
    spatial::quad_cell,
};

use crate::runtime::{
    dev_tools::{BrushSettings, GridBrush},
    render::{
        FluidMaterial, GranularMaterial, SurfaceMaterial, TerrainMaterial, WorldWindowUniform,
    },
};

const RAYMARCH_MAX_DIST: f32 = 1000.0;
const SPHERE_SPAWN_OFFSET_Y: f32 = 10.0;

/// A spawnable sphere: its physics from `world_model::body`, and how it looks.
struct SphereLook {
    preset: body::SpherePreset,
    color: Color,
    metallic: f32,
    roughness: f32,
}

const METAL_SPHERE: SphereLook = SphereLook {
    preset: body::METAL_SPHERE,
    color: Color::srgb(0.6, 0.6, 0.65),
    metallic: 0.8,
    roughness: 0.2,
};

const WOOD_SPHERE: SphereLook = SphereLook {
    preset: body::WOOD_SPHERE,
    color: Color::srgb(0.55, 0.38, 0.22),
    metallic: 0.0,
    roughness: 0.8,
};

const LIFT_INFLUENCE_RADIUS_SQ: f32 = LIFT_INFLUENCE_RADIUS * LIFT_INFLUENCE_RADIUS;
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
    egui_wants_input: &bevy_egui::input::EguiWantsInput,
) -> Option<Vec3> {
    if egui_wants_input.wants_pointer_input() {
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

/// Volumetric raymarcher with 2D Digital Differential Analyzer (DDA) across the XZ plane.
#[inline(always)]
pub fn raymarch_grid(ray: &Ray3d, grid: &ActiveWorldGrid, elevation_scale: f32) -> Option<Vec3> {
    let start_pos = ray.origin;

    // Isolate mathematical origin coordinates
    let IVec2 {
        x: mut cx,
        y: mut cy,
    } = quad_cell(start_pos.x, start_pos.z);

    // Vector step directions mapped to the grid's topology (cy acts against -Z)
    let step_x = if ray.direction.x > 0.0 { 1 } else { -1 };
    let step_y = if ray.direction.z < 0.0 { 1 } else { -1 };

    // Distance the ray must travel to cross exactly one full cell width/height
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

    // Distance to the very first cellular boundary crossing
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
    mut terrain_mats: ResMut<Assets<TerrainMaterial>>,
    mut granular_mats: ResMut<Assets<GranularMaterial>>,
    mut fluid_mats: ResMut<Assets<FluidMaterial>>,
    mut surface_mats: ResMut<Assets<SurfaceMaterial>>,
    egui_wants_input: Res<bevy_egui::input::EguiWantsInput>,
) {
    let is_active = *brush != GridBrush::None;

    let hit_pos = if is_active {
        extract_pointer_hit(
            &windows,
            &camera_q,
            &grid,
            global_config.elevation_scale,
            &egui_wants_input,
        )
    } else {
        None
    };

    let cursor_radius = if brush.spawns_entity() {
        0.0
    } else {
        settings.radius as f32
    };

    let update_mat = |mat: &mut WorldWindowUniform| {
        if let Some(pos) = hit_pos {
            let cell = quad_cell(pos.x, pos.z);
            mat.head_cursor.z = cell.x as f32;
            mat.head_cursor.w = cell.y as f32;
            mat.config.y = cursor_radius;
        } else {
            mat.config.y = -1.0;
        }
    };

    for (_, mat) in terrain_mats.iter_mut() {
        update_mat(&mut mat.window);
    }
    for (_, mat) in granular_mats.iter_mut() {
        update_mat(&mut mat.window);
    }
    for (_, mat) in fluid_mats.iter_mut() {
        update_mat(&mut mat.window);
    }
    for (_, mat) in surface_mats.iter_mut() {
        update_mat(&mut mat.window);
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
    egui_wants_input: Res<bevy_egui::input::EguiWantsInput>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if *brush == GridBrush::None {
        return;
    }

    let is_spawning_entity = brush.spawns_entity();
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
        &egui_wants_input,
    ) else {
        return;
    };

    if is_spawning_entity {
        spawn_sphere(
            &mut commands,
            &mut meshes,
            &mut materials,
            &grid,
            global_config.elevation_scale,
            SphereSpawn {
                wood: *brush == GridBrush::SpawnWoodSphere,
                hit: hit_position,
                drop_height: SPHERE_SPAWN_OFFSET_Y,
            },
        );
        return;
    }

    let center = quad_cell(hit_position.x, hit_position.z);
    apply_brush(&mut grid, *brush, &settings, center);
}

/// Which sphere to spawn, and where.
#[derive(Debug, Clone, Copy)]
pub struct SphereSpawn {
    /// Wood floats in water; metal sinks.
    pub wood: bool,
    /// World position whose column the sphere drops onto.
    pub hit: Vec3,
    /// World units above the column's bedrock and loose material (the tool uses 10).
    pub drop_height: f32,
}

/// Spawns a sphere above the column under `request.hit`.
///
/// The interactive tool and the scripted baseline driver both spawn through this function.
pub fn spawn_sphere(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    grid: &ActiveWorldGrid,
    elevation_scale: f32,
    request: SphereSpawn,
) -> Entity {
    let SphereSpawn {
        wood,
        hit,
        drop_height,
    } = request;
    let cell_pos = quad_cell(hit.x, hit.z);
    let mut spawn_y = drop_height;

    if is_within_grid(cell_pos, grid) {
        let cell = grid.get_cell(cell_pos);
        let floor_h = (cell.elevation() as f32 + cell.granular_vol() as f32) * elevation_scale;
        spawn_y = floor_h + drop_height;
    }

    let look = if wood { &WOOD_SPHERE } else { &METAL_SPHERE };
    let preset = look.preset;

    commands
        .spawn((
            Mesh3d(meshes.add(Sphere::new(preset.radius))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: look.color,
                metallic: look.metallic,
                perceptual_roughness: look.roughness,
                ..default()
            })),
            Transform::from_translation(Vec3::new(hit.x, spawn_y, hit.z)),
            DynamicRigidSphere::new(preset.mass, preset.radius, preset.relative_density),
            KinematicProfile::default(),
            DeformationProfile::default(),
        ))
        .id()
}

/// Applies one stroke of a material brush to every in-window cell within `settings.radius` of
/// `center`, waking each changed cell. Spawning brushes do nothing here.
pub fn apply_brush(
    grid: &mut ActiveWorldGrid,
    brush: GridBrush,
    settings: &BrushSettings,
    center: IVec2,
) {
    let (center_x, center_y) = (center.x, center.y);
    let radius = settings.radius;
    let radius_sq = radius * radius;

    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx * dx + dy * dy > radius_sq {
                continue;
            }

            let world_position = IVec2::new(center_x + dx, center_y + dy);

            if is_within_grid(world_position, grid) {
                let mut cell = grid.get_cell(world_position);
                let mut cell_was_mutated = false;

                match (brush, brush.fluid()) {
                    (_, Some(fluid)) => {
                        // Painting takes the column over and keeps the volume already there.
                        let before = cell;
                        cell.inject_fluid(fluid, settings.strength as u16);
                        cell_was_mutated = cell != before;
                    }
                    (GridBrush::Fire, _) => {
                        let fluid = cell.fluid_mat();
                        if cell.surface_mat() != SurfaceMat::SURFACE_FIRE
                            && (fluid == FluidMat::EMPTY || fluid == FluidMat::FLUID_OIL)
                        {
                            cell.set_surface_mat(SurfaceMat::SURFACE_FIRE);
                            cell.set_surface_state(0);
                            cell_was_mutated = true;
                        }
                    }
                    (GridBrush::Sand, _) => {
                        let old_vol = if cell.granular_mat() == GranularMat::GRANULAR_SAND {
                            cell.granular_vol()
                        } else {
                            0
                        };

                        let new_vol = old_vol
                            .saturating_add(settings.strength as u16)
                            .min(WorldCell::MAX_ELEVATION as u16 + WorldCell::MAX_GRANULAR_VOL);

                        if cell.granular_mat() != GranularMat::GRANULAR_SAND || old_vol != new_vol {
                            cell.set_granular_mat(GranularMat::GRANULAR_SAND);
                            cell.set_granular_vol(new_vol);
                            cell_was_mutated = true;
                        }
                    }
                    (GridBrush::RaiseTerrain, _) => {
                        let new_elevation = cell
                            .elevation()
                            .saturating_add(settings.strength as u16)
                            .min(WorldCell::MAX_ELEVATION as u16);
                        if new_elevation != cell.elevation() {
                            cell.set_elevation(new_elevation);
                            cell_was_mutated = true;
                        }
                    }
                    (GridBrush::LowerTerrain, _) => {
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
    egui_wants_input: Res<bevy_egui::input::EguiWantsInput>,
) {
    if !mouse.pressed(MouseButton::Back) && !mouse.pressed(MouseButton::Other(4)) {
        return;
    }

    let Some(hit_position) = extract_pointer_hit(
        &windows,
        &camera_q,
        &grid,
        global_config.elevation_scale,
        &egui_wants_input,
    ) else {
        return;
    };

    let dt = time.delta_secs();

    for (transform, mut sphere) in spheres.iter_mut() {
        attract_sphere(transform, &mut sphere, hit_position, dt);
    }
}

/// Accelerates one sphere toward `target` for `dt` seconds.
pub fn attract_sphere(
    transform: &Transform,
    sphere: &mut DynamicRigidSphere,
    target: Vec3,
    dt: f32,
) {
    let to_target = target - transform.translation;
    let dist_sq = to_target.length_squared();

    if dist_sq > EPSILON_DIST_SQ {
        let direction = to_target.normalize();
        sphere.velocity += direction * ATTRACT_ACCELERATION * dt;
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
    egui_wants_input: Res<bevy_egui::input::EguiWantsInput>,
) {
    if !mouse.pressed(MouseButton::Forward) && !mouse.pressed(MouseButton::Other(5)) {
        return;
    }

    let Some(hit_position) = extract_pointer_hit(
        &windows,
        &camera_query,
        &grid,
        global_config.elevation_scale,
        &egui_wants_input,
    ) else {
        return;
    };

    let delta_time = time.delta_secs();

    for (transform, mut sphere) in spheres.iter_mut() {
        lift_sphere(transform, &mut sphere, hit_position, delta_time);
    }
}

/// Lifts one sphere near `target` and pulls it toward the target column for `delta_time` seconds.
pub fn lift_sphere(
    transform: &Transform,
    sphere: &mut DynamicRigidSphere,
    target: Vec3,
    delta_time: f32,
) {
    let vector_to_target = target - transform.translation;
    let horizontal_offset = Vec3::new(vector_to_target.x, 0.0, vector_to_target.z);
    let distance_squared = horizontal_offset.length_squared();

    if distance_squared < LIFT_INFLUENCE_RADIUS_SQ {
        sphere.velocity.y += LIFT_ACCELERATION * delta_time;

        if distance_squared > EPSILON_DIST_SQ {
            let horizontal_direction = horizontal_offset.normalize();
            sphere.velocity += horizontal_direction * LIFT_CENTERING_ACCELERATION * delta_time;
        }
    }
}
