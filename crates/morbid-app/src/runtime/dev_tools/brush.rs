use bevy::prelude::*;
use bevy_egui::EguiContexts;
use monarch_engine::{
    engine::entities::spherical::DynamicRigidSphere,
    prelude::{ActiveWorldGrid, FluidMat, GranularMat, SurfaceMat, WorldCell},
};

use crate::runtime::dev_tools::{BrushSettings, GridBrush};

pub fn handle_brush_input(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform)>,
    mut grid: ResMut<ActiveWorldGrid>,
    brush: Res<GridBrush>,
    settings: Res<BrushSettings>,
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

        if ray.direction.y.abs() < 0.001 {
            return;
        }

        let distance_to_plane = -ray.origin.y / ray.direction.y;
        if distance_to_plane < 0.0 {
            return;
        }

        let hit_position = ray.origin + ray.direction * distance_to_plane;

        if is_spawning_entity {
            // Spawn the pristine engine component cleanly
            commands.spawn((
                Mesh3d(meshes.add(Sphere::new(10.0))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgb(0.6, 0.6, 0.65),
                    metallic: 0.8,
                    perceptual_roughness: 0.2,
                    ..default()
                })),
                Transform::from_translation(hit_position + Vec3::Y * 10.0),
                DynamicRigidSphere::new(1000.0, 10.0),
            ));
            return;
        }

        let center_x = hit_position.x.floor() as i32;
        let center_y = (-hit_position.z).floor() as i32;

        let radius = settings.radius;
        let bounds_minimum = grid.window_origin;
        let bounds_maximum = grid.window_origin + IVec2::new(grid.width, grid.height);

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

/// Attracts spawned spherical entities towards the raycasted hit position on the ground plane when Mouse Button 4 (Back) is pressed.
pub fn attract_spheres_input(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform)>,
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

        if ray.direction.y.abs() < 0.001 {
            return;
        }

        let distance_to_plane = -ray.origin.y / ray.direction.y;
        if distance_to_plane < 0.0 {
            return;
        }

        let hit_position = ray.origin + ray.direction * distance_to_plane;
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
