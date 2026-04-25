use bevy::prelude::*;
use bevy_egui::EguiContexts;
use monarch_engine::prelude::{ActiveWorldGrid, MaterialId, PixelFlags};

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

    // Do not paint if the user is clicking on an Egui window/dropdown
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

        // Intersect the ray with the XZ plane (where Y = 0)
        if ray.direction.y.abs() < 0.001 {
            return;
        }

        let t = -ray.origin.y / ray.direction.y;
        if t < 0.0 {
            return;
        }

        let hit_pos = ray.origin + ray.direction * t;

        // Map 3D XZ coordinates back to 2D Grid XY coordinates
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

                // Prevent painting out of bounds
                if world_pos.x >= bounds_min.x
                    && world_pos.x < bounds_max.x
                    && world_pos.y >= bounds_min.y
                    && world_pos.y < bounds_max.y
                {
                    let mut cell = grid.get_cell(world_pos);
                    let mut mutated = false;

                    match *brush {
                        GridBrush::Water => {
                            // If it's already water, we add to it. If it's empty/magma/blood, we start from 0.
                            let old_state = if cell.fluid.material == MaterialId::LIQUID_WATER {
                                cell.fluid.state
                            } else {
                                0
                            };

                            let new_state = old_state.saturating_add(settings.strength);

                            if cell.fluid.material != MaterialId::LIQUID_WATER
                                || cell.fluid.state != new_state
                            {
                                let amount_added = new_state.saturating_sub(old_state);

                                cell.fluid.material = MaterialId::LIQUID_WATER;
                                cell.fluid.state = new_state;

                                // Displace the atmosphere 1:1
                                cell.atmosphere.state =
                                    cell.atmosphere.state.saturating_sub(amount_added);

                                // Wake the cell so the engine processes the new fluid
                                cell.fluid.flags.insert(PixelFlags::WAKES_AWAKE);
                                cell.terrain.flags.insert(PixelFlags::WAKES_AWAKE);
                                mutated = true;
                            }
                        }
                        GridBrush::Sand => {
                            if cell.terrain.material != MaterialId::LOOSE_SAND {
                                cell.terrain.material = MaterialId::LOOSE_SAND;
                                cell.terrain.state = 0;
                                cell.terrain.flags.insert(PixelFlags::WAKES_AWAKE);
                                mutated = true;
                            }
                        }
                        GridBrush::IncreasePressure => {
                            let new_state = cell.atmosphere.state.saturating_add(settings.strength);
                            if new_state != cell.atmosphere.state {
                                cell.atmosphere.state = new_state;
                                cell.atmosphere.flags.insert(PixelFlags::WAKES_AWAKE);
                                mutated = true;
                            }
                        }
                        GridBrush::DecreasePressure => {
                            let new_state = cell.atmosphere.state.saturating_sub(settings.strength);
                            if new_state != cell.atmosphere.state {
                                cell.atmosphere.state = new_state;
                                cell.atmosphere.flags.insert(PixelFlags::WAKES_AWAKE);
                                mutated = true;
                            }
                        }
                        GridBrush::None => {}
                    }

                    if mutated {
                        grid.set_cell(world_pos, cell);
                    }
                }
            }
        }
    }
}
