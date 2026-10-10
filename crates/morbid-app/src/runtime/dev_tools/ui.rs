use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::{diagnostic::DiagnosticsStore, ecs::message::MessageWriter, prelude::*};
use bevy_egui::{EguiContexts, egui};
use monarch_engine::prelude::{
    ActiveWorldGrid, FluidMat, GlobalPhysicsConfig, ResizeSimulationEvent, SimulationConfig,
    WorldManager,
};

use crate::runtime::{
    dev_tools::{
        BrushSettings, DevOverlay, GridBrush,
        compass::{COMPASS_ANCHOR_OFFSET, COMPASS_FOOTPRINT_HEIGHT},
    },
    render::WorldLighting,
};

const BRUSH_STRENGTH_RANGE: std::ops::RangeInclusive<u8> = 1..=255;
const BRUSH_RADIUS_RANGE: std::ops::RangeInclusive<i32> = 0..=64;

const UI_FONT_SIZE_HEADING: f32 = 15.0;
const UI_SPACING_DEFAULT: f32 = 16.0;

const UI_ELEVATION_MIN: f32 = 0.01;
const UI_ELEVATION_MAX: f32 = 5.0;
const UI_ELEVATION_SPEED: f32 = 0.05;

const UI_RADIUS_MIN: u32 = 1;
const UI_RADIUS_MAX: u32 = 32;

const UI_STATS_OFFSET_X: f32 = -10.0;
const UI_STATS_OFFSET_Y: f32 = 40.0;
const UI_STACK_GAP: f32 = 8.0;

const UI_FPS_THRESHOLD_GOOD: f64 = 32.0;

const UI_TICK_RATE_RANGE: std::ops::RangeInclusive<f64> = 10.0..=240.0;
const UI_CATCH_UP_RANGE: std::ops::RangeInclusive<u32> = 1..=16;
const UI_TEMPERATURE_RANGE: std::ops::RangeInclusive<f32> = -30.0..=40.0;
const UI_PANEL_OFFSET_X: f32 = 10.0;

const UI_SUN_AZIMUTH_RANGE: std::ops::RangeInclusive<f32> = 0.0..=360.0;
const UI_SUN_ELEVATION_RANGE: std::ops::RangeInclusive<f32> = 2.0..=90.0;
const UI_AMBIENT_RANGE: std::ops::RangeInclusive<f32> = 0.0..=3000.0;
const UI_UNIT_RANGE: std::ops::RangeInclusive<f32> = 0.0..=1.0;

const BRUSH_OPTIONS: [(GridBrush, &str); 11] = [
    (GridBrush::None, "None"),
    (GridBrush::Water, "Spawn Water"),
    (GridBrush::Lava, "Spawn Lava"),
    (GridBrush::Oil, "Spawn Oil"),
    (GridBrush::Ice, "Spawn Ice"),
    (GridBrush::Fire, "Spawn Fire"),
    (GridBrush::Sand, "Spawn Sand"),
    (GridBrush::RaiseTerrain, "Raise Terrain"),
    (GridBrush::LowerTerrain, "Lower Terrain"),
    (GridBrush::SpawnMetalSphere, "Spawn Metal Sphere"),
    (GridBrush::SpawnWoodSphere, "Spawn Wooden Sphere"),
];

pub fn dev_tuning_ui(
    mut contexts: EguiContexts,
    mut lighting: ResMut<WorldLighting>,
    mut global_config: ResMut<GlobalPhysicsConfig>,
    mut sim_config: ResMut<SimulationConfig>,
    mut brush: ResMut<GridBrush>,
    mut brush_settings: ResMut<BrushSettings>,
    mut resize_writer: MessageWriter<ResizeSimulationEvent>,
    mut pending_resize: Local<Option<[u32; 2]>>,
    mut overlay: ResMut<DevOverlay>,
    mut show_stats: Local<bool>,
    mut show_panels: Local<[bool; 2]>,
    manager: Res<WorldManager>,
    grid: Res<ActiveWorldGrid>,
    keys: Res<ButtonInput<KeyCode>>,
    diagnostics: Res<DiagnosticsStore>,
) {
    if keys.just_pressed(KeyCode::Backquote) || keys.just_pressed(KeyCode::F12) {
        overlay.visible = !overlay.visible;
    }

    if !overlay.visible {
        return;
    }

    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    let mut root_ui = egui::Ui::new(
        ctx.clone(),
        egui::Id::new("primary_root_ui"),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.content_rect()),
    );

    let current_size = pending_resize
        .get_or_insert_with(|| [manager.inner.active_radius_x, manager.inner.active_radius_y]);

    let _input = egui::RawInput::default();

    egui::Panel::top("dev_navbar").show(&mut root_ui, |ui| {
        ui.horizontal_centered(|ui| {
            let add_separator = |ui: &mut egui::Ui| {
                ui.add_space(UI_SPACING_DEFAULT);
                ui.separator();
                ui.add_space(UI_SPACING_DEFAULT);
            };

            ui.label(
                egui::RichText::new("Tools")
                    .strong()
                    .size(UI_FONT_SIZE_HEADING),
            );
            ui.add_space(UI_SPACING_DEFAULT);

            let [show_simulation, show_lighting] = &mut *show_panels;
            ui.checkbox(&mut *show_stats, "Statistics");
            ui.checkbox(show_simulation, "Simulation");
            ui.checkbox(show_lighting, "Lighting");
            add_separator(ui);

            ui.label(egui::RichText::new("Elevation Scale:").color(egui::Color32::LIGHT_GRAY));

            let mut scale = global_config.elevation_scale;
            if ui
                .add(
                    egui::DragValue::new(&mut scale)
                        .range(UI_ELEVATION_MIN..=UI_ELEVATION_MAX)
                        .speed(UI_ELEVATION_SPEED),
                )
                .changed()
            {
                global_config.elevation_scale = scale;
            }

            add_separator(ui);

            ui.label(egui::RichText::new("Active Radius (X/Y):").color(egui::Color32::LIGHT_GRAY));
            ui.add(egui::DragValue::new(&mut current_size[0]).range(UI_RADIUS_MIN..=UI_RADIUS_MAX));
            ui.label("x");
            ui.add(egui::DragValue::new(&mut current_size[1]).range(UI_RADIUS_MIN..=UI_RADIUS_MAX));

            ui.add_space(UI_SPACING_DEFAULT);

            if ui.button("Apply Resize").clicked() {
                if current_size[0] != manager.inner.active_radius_x
                    || current_size[1] != manager.inner.active_radius_y
                {
                    info!(
                        "Dev UI dispatching Resize: {}x{}",
                        current_size[0], current_size[1]
                    );
                    resize_writer.write(ResizeSimulationEvent {
                        new_active_radius_x: current_size[0],
                        new_active_radius_y: current_size[1],
                    });
                }
            }
            add_separator(ui);

            ui.label(egui::RichText::new("Brush:").strong());

            let selected_text = BRUSH_OPTIONS
                .iter()
                .find(|(b, _)| b == &*brush)
                .map(|(_, label)| *label)
                .unwrap_or("None");

            egui::ComboBox::from_id_salt("brush_selector")
                .selected_text(selected_text)
                .show_ui(ui, |ui| {
                    for (brush_variant, label) in BRUSH_OPTIONS {
                        ui.selectable_value(&mut *brush, brush_variant, label);
                    }
                });

            if *brush != GridBrush::None {
                ui.label("Radius:");
                ui.add(egui::DragValue::new(&mut brush_settings.radius).range(BRUSH_RADIUS_RANGE));

                ui.label("Strength:");
                ui.add(
                    egui::DragValue::new(&mut brush_settings.strength).range(BRUSH_STRENGTH_RANGE),
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(fps) = diagnostics
                    .get(&FrameTimeDiagnosticsPlugin::FPS)
                    .and_then(|fps| fps.smoothed())
                {
                    ui.label(
                        egui::RichText::new(format!("FPS: {:.0}", fps))
                            .strong()
                            .color(if fps > UI_FPS_THRESHOLD_GOOD {
                                egui::Color32::GREEN
                            } else {
                                egui::Color32::RED
                            }),
                    );
                }
            });
        });
    });

    let [show_simulation, show_lighting] = *show_panels;

    if show_simulation {
        egui::Window::new("Simulation")
            .anchor(
                egui::Align2::LEFT_TOP,
                [UI_PANEL_OFFSET_X, UI_STATS_OFFSET_Y],
            )
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.checkbox(&mut sim_config.run_liquid, "Run Liquid");
                ui.checkbox(&mut sim_config.run_granular, "Run Granular");
                ui.checkbox(&mut sim_config.run_thermal, "Run Thermal");
                ui.checkbox(&mut sim_config.run_fire, "Run Fire");
                ui.checkbox(&mut sim_config.run_biology, "Run Biology");
                ui.separator();

                egui::Grid::new("simulation_grid").show(ui, |ui| {
                    ui.label("Tick Rate (Hz):");
                    ui.add(
                        egui::DragValue::new(&mut sim_config.tick_rate_hz)
                            .range(UI_TICK_RATE_RANGE),
                    );
                    ui.end_row();

                    ui.label("Max Ticks / Frame:");
                    ui.add(
                        egui::DragValue::new(&mut sim_config.max_ticks_per_frame)
                            .range(UI_CATCH_UP_RANGE),
                    );
                    ui.end_row();

                    // Edited through a copy: the simulation wakes every pool whenever this
                    // value changes, so it must only change on an actual edit.
                    ui.label("Temperature (°C):");
                    let mut temperature = sim_config.ambient_temperature_c;
                    if ui
                        .add(egui::Slider::new(&mut temperature, UI_TEMPERATURE_RANGE))
                        .changed()
                    {
                        sim_config.ambient_temperature_c = temperature;
                    }
                    ui.end_row();
                });
            });
    }

    if show_lighting {
        egui::Window::new("Lighting")
            .anchor(
                egui::Align2::LEFT_BOTTOM,
                [UI_PANEL_OFFSET_X, -UI_PANEL_OFFSET_X],
            )
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                // Edit through the bypass so the shaders and lights are only re-uploaded when a
                // value actually changes, not every frame the panel is open.
                let mut changed = false;
                {
                    let values = lighting.bypass_change_detection();
                    egui::Grid::new("lighting_grid").show(ui, |ui| {
                        let mut slider =
                            |ui: &mut egui::Ui,
                             label: &str,
                             value: &mut f32,
                             range: std::ops::RangeInclusive<f32>| {
                                ui.label(label);
                                changed |= ui.add(egui::Slider::new(value, range)).changed();
                                ui.end_row();
                            };

                        slider(
                            ui,
                            "Sun Azimuth (deg):",
                            &mut values.sun_azimuth_degrees,
                            UI_SUN_AZIMUTH_RANGE,
                        );
                        slider(
                            ui,
                            "Sun Elevation (deg):",
                            &mut values.sun_elevation_degrees,
                            UI_SUN_ELEVATION_RANGE,
                        );
                        slider(
                            ui,
                            "Sky Brightness:",
                            &mut values.ambient_brightness,
                            UI_AMBIENT_RANGE,
                        );
                        slider(
                            ui,
                            "Terrain Shadows:",
                            &mut values.terrain_shadow_strength,
                            UI_UNIT_RANGE,
                        );
                        slider(
                            ui,
                            "Ambient Occlusion:",
                            &mut values.ambient_occlusion_strength,
                            UI_UNIT_RANGE,
                        );
                    });
                }
                if changed {
                    lighting.set_changed();
                }
            });
    }

    if *show_stats {
        // Stacked under the compass, which owns the top-right corner.
        egui::Window::new("World Statistics")
            .anchor(
                egui::Align2::RIGHT_TOP,
                [
                    UI_STATS_OFFSET_X,
                    COMPASS_ANCHOR_OFFSET[1] + COMPASS_FOOTPRINT_HEIGHT + UI_STACK_GAP,
                ],
            )
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                let total_mass: u64 = grid
                    .spatial
                    .cells
                    .iter()
                    .filter(|c| c.fluid_mat() != FluidMat::EMPTY)
                    .map(|c| c.fluid_vol() as u64)
                    .sum();

                let total_elev: u64 = grid
                    .spatial
                    .cells
                    .iter()
                    .map(|c| c.elevation() as u64)
                    .sum();

                egui::Grid::new("stats_grid").striped(true).show(ui, |ui| {
                    ui.label(egui::RichText::new("Total Liquid Vol:").strong());
                    ui.label(
                        egui::RichText::new(total_mass.to_string()).color(egui::Color32::CYAN),
                    );
                    ui.end_row();

                    ui.label(egui::RichText::new("Total Elevation:").strong());
                    ui.label(
                        egui::RichText::new(total_elev.to_string())
                            .color(egui::Color32::LIGHT_GRAY),
                    );
                    ui.end_row();
                });
            });
    }
}
