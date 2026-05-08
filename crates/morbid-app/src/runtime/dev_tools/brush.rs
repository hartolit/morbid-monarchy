use bevy::prelude::*;
use bevy_egui::EguiContexts;
use monarch_engine::prelude::{ActiveWorldGrid, FluidMat, GranularMat, SurfaceMat, WorldCell};

use crate::runtime::dev_tools::{BrushSettings, GridBrush};

pub fn handle_brush_input(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform)>,
    mut grid: ResMut<ActiveWorldGrid>,
    brush: Res<GridBrush>,
    settings: Res<BrushSettings>,
    mut egui_contexts: EguiContexts,
) {
    if *brush == GridBrush::None || !mouse.pressed(MouseButton::Left) {
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

        let t = -ray.origin.y / ray.direction.y;
        if t < 0.0 {
            return;
        }

        let hit_pos = ray.origin + ray.direction * t;

        let center_x = hit_pos.x.floor() as i32;
        let center_y = (-hit_pos.z).floor() as i32;

        let radius = settings.radius;
        let bounds_min = grid.window_origin;
        let bounds_max = grid.window_origin + IVec2::new(grid.width, grid.height);

        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dy * dy > radius * radius {
                    continue;
                }

                let world_pos = IVec2::new(center_x + dx, center_y + dy);

                if world_pos.x >= bounds_min.x
                    && world_pos.x < bounds_max.x
                    && world_pos.y >= bounds_min.y
                    && world_pos.y < bounds_max.y
                {
                    let mut cell = grid.get_cell(world_pos);
                    let mut mutated = false;

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
                                mutated = true;
                            }
                        }
                        GridBrush::Fire => {
                            // Only allow spawning fire if the cell is dry OR covered in oil
                            let fluid = cell.fluid_mat();
                            if cell.surface_mat() != SurfaceMat::SURFACE_FIRE
                                && (fluid == FluidMat::EMPTY || fluid == FluidMat::FLUID_OIL)
                            {
                                cell.set_surface_mat(SurfaceMat::SURFACE_FIRE);
                                cell.set_surface_state(0);
                                mutated = true;
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
                                mutated = true;
                            }
                        }
                        GridBrush::RaiseTerrain => {
                            let new_elev =
                                cell.elevation().saturating_add(settings.strength as u16);
                            if new_elev != cell.elevation() {
                                cell.set_elevation(new_elev);
                                mutated = true;
                            }
                        }
                        GridBrush::LowerTerrain => {
                            let new_elev =
                                cell.elevation().saturating_sub(settings.strength as u16);
                            if new_elev != cell.elevation() {
                                cell.set_elevation(new_elev);
                                mutated = true;
                            }
                        }
                        GridBrush::None => {}
                    }

                    if mutated {
                        grid.set_cell(world_pos, cell);
                        // Safely trigger external wakes to ensure physics reacts next frame
                        grid.wake_cell(world_pos);
                    }
                }
            }
        }
    }
}
