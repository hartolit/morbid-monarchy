use bevy::prelude::*;
use bevy_egui::EguiContexts;
use monarch_engine::{
    engine::entities::{EntityPhysicsConfig, spherical::DynamicRigidSphere},
    prelude::{ActiveWorldGrid, FluidMat, GranularMat, SurfaceMat, WorldCell},
};

use crate::runtime::{
    dev_tools::{BrushSettings, GridBrush},
    render::WorldMaterial,
};

/// Volumetric raymarcher executing a pure 2D Digital Differential Analyzer (DDA)
/// across the XZ plane to mathematically guarantee intersection with the dynamic thermodynamic floor.
#[inline(always)]
pub fn raymarch_grid(ray: &Ray3d, grid: &ActiveWorldGrid, elevation_scale: f32) -> Option<Vec3> {
    let max_dist = 1000.0;
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

    while t < max_dist {
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
                        // Impacted from above (hit the roof of the voxel)
                        let hit_t = (h - start_pos.y) / ray.direction.y;
                        return Some(start_pos + *ray.direction * hit_t);
                    } else {
                        // Impacted from the side (hit the wall of the voxel)
                        return Some(start_pos + *ray.direction * entry_t);
                    }
                } else if y_entry <= h {
                    // Ascending from below/side
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
    config: Res<EntityPhysicsConfig>,
    brush: Res<GridBrush>,
    settings: Res<BrushSettings>,
    mut materials: ResMut<Assets<WorldMaterial>>,
    mut egui_contexts: EguiContexts,
) {
    let is_active = *brush != GridBrush::None;
    let Ok(ctx) = egui_contexts.ctx_mut() else {
        return;
    };
    let pointer_over_ui = ctx.wants_pointer_input();

    for (_, mat) in materials.iter_mut() {
        if !is_active || pointer_over_ui {
            mat.window.config.y = -1.0; // Negative radius disables shader evaluation
            continue;
        }

        let Ok(window) = windows.single() else {
            continue;
        };
        let Ok((camera, camera_transform)) = camera_q.single() else {
            continue;
        };

        let Some(cursor_pos) = window.cursor_position() else {
            mat.window.config.y = -1.0;
            continue;
        };
        let Ok(ray) = camera.viewport_to_world(camera_transform, cursor_pos) else {
            mat.window.config.y = -1.0;
            continue;
        };

        if let Some(hit_pos) = raymarch_grid(&ray, &grid, config.elevation_scale) {
            let cursor_radius = if *brush == GridBrush::SpawnSphere {
                0.0
            } else {
                settings.radius as f32
            };
            // Inject hit coordinates into the Z/W slots of the head_cursor block
            mat.window.head_cursor.z = hit_pos.x.floor();
            mat.window.head_cursor.w = (-hit_pos.z).floor();
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
    config: Res<EntityPhysicsConfig>,
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

    let Ok(ctx) = egui_contexts.ctx_mut() else {
        return;
    };
    if ctx.wants_pointer_input() {
        return;
    }

    let Ok(window) = windows.single() else {
        return;
    };
    let Ok((camera, camera_transform)) = camera_q.single() else {
        return;
    };

    if let Some(cursor_pos) = window.cursor_position() {
        let Ok(ray) = camera.viewport_to_world(camera_transform, cursor_pos) else {
            return;
        };

        let Some(hit_position) = raymarch_grid(&ray, &grid, config.elevation_scale) else {
            return;
        };

        if is_spawning_entity {
            let center_x = hit_position.x.floor() as i32;
            let center_y = (-hit_position.z).floor() as i32;
            let cell_pos = IVec2::new(center_x, center_y);

            let mut spawn_y = 10.0;
            let bounds_minimum = grid.spatial.window_origin;
            let bounds_maximum =
                grid.spatial.window_origin + IVec2::new(grid.spatial.width, grid.spatial.height);

            if cell_pos.x >= bounds_minimum.x
                && cell_pos.x < bounds_maximum.x
                && cell_pos.y >= bounds_minimum.y
                && cell_pos.y < bounds_maximum.y
            {
                let cell = grid.get_cell(cell_pos);
                let floor_h =
                    (cell.elevation() as f32 + cell.granular_vol() as f32) * config.elevation_scale;
                spawn_y = floor_h + 10.0;
            }

            commands.spawn((
                Mesh3d(meshes.add(Sphere::new(5.0))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgb(0.6, 0.6, 0.65),
                    metallic: 0.8,
                    perceptual_roughness: 0.2,
                    ..default()
                })),
                Transform::from_translation(Vec3::new(hit_position.x, spawn_y, hit_position.z)),
                DynamicRigidSphere::new(5.0, 5.0),
            ));
            return;
        }

        let center_x = hit_position.x.floor() as i32;
        let center_y = (-hit_position.z).floor() as i32;

        let radius = settings.radius;
        let bounds_minimum = grid.spatial.window_origin;
        let bounds_maximum =
            grid.spatial.window_origin + IVec2::new(grid.spatial.width, grid.spatial.height);

        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dy * dy > radius * radius {
                    continue;
                }

                let world_position = IVec2::new(center_x + dx, center_y + dy);

                if world_position.x >= bounds_minimum.x
                    && world_position.x < bounds_maximum.x
                    && world_position.y >= bounds_minimum.y
                    && world_position.y < bounds_maximum.y
                {
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
}

pub fn attract_spheres_input(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform)>,
    grid: Res<ActiveWorldGrid>,
    config: Res<EntityPhysicsConfig>,
    mut spheres: Query<(&Transform, &mut DynamicRigidSphere)>,
    time: Res<Time>,
    mut egui_contexts: EguiContexts,
) {
    if !mouse.pressed(MouseButton::Back) && !mouse.pressed(MouseButton::Other(4)) {
        return;
    }
    let Ok(ctx) = egui_contexts.ctx_mut() else {
        return;
    };
    if ctx.wants_pointer_input() {
        return;
    }

    let Ok(window) = windows.single() else {
        return;
    };
    let Ok((camera, camera_transform)) = camera_q.single() else {
        return;
    };

    if let Some(cursor_pos) = window.cursor_position() {
        let Ok(ray) = camera.viewport_to_world(camera_transform, cursor_pos) else {
            return;
        };
        let Some(hit_position) = raymarch_grid(&ray, &grid, config.elevation_scale) else {
            return;
        };

        let pull_strength = 250.0;
        let dt = time.delta_secs();

        for (transform, mut sphere) in spheres.iter_mut() {
            let to_target = hit_position - transform.translation;
            let dist_sq = to_target.length_squared();

            if dist_sq > 0.0001 {
                let direction = to_target.normalize();
                sphere.velocity += direction * pull_strength * dt;
            }
        }
    }
}

pub fn lift_spheres_input(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform)>,
    grid: Res<ActiveWorldGrid>,
    config: Res<EntityPhysicsConfig>,
    mut spheres: Query<(&Transform, &mut DynamicRigidSphere)>,
    time: Res<Time>,
    mut egui_contexts: EguiContexts,
) {
    if !mouse.pressed(MouseButton::Forward) && !mouse.pressed(MouseButton::Other(5)) {
        return;
    }
    let Ok(context) = egui_contexts.ctx_mut() else {
        return;
    };
    if context.wants_pointer_input() {
        return;
    }

    let Ok(window) = windows.single() else {
        return;
    };
    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };

    if let Some(cursor_position) = window.cursor_position() {
        let Ok(ray) = camera.viewport_to_world(camera_transform, cursor_position) else {
            return;
        };
        let Some(hit_position) = raymarch_grid(&ray, &grid, config.elevation_scale) else {
            return;
        };

        let lift_acceleration = 250.0;
        let horizontal_centering_force = 40.0;
        let delta_time = time.delta_secs();
        let influence_radius_squared = 150.0 * 150.0;

        for (transform, mut sphere) in spheres.iter_mut() {
            let vector_to_target = hit_position - transform.translation;
            let horizontal_offset = Vec3::new(vector_to_target.x, 0.0, vector_to_target.z);
            let distance_squared = horizontal_offset.length_squared();

            if distance_squared < influence_radius_squared {
                sphere.velocity.y += lift_acceleration * delta_time;

                if distance_squared > 0.0001 {
                    let horizontal_direction = horizontal_offset.normalize();
                    sphere.velocity +=
                        horizontal_direction * horizontal_centering_force * delta_time;
                }
            }
        }
    }
}
