use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::{diagnostic::DiagnosticsStore, ecs::message::MessageWriter, prelude::*};
use bevy_egui::{EguiContexts, egui};
use monarch_engine::prelude::{
    ActiveWorldGrid, FluidMat, GlobalPhysicsConfig, ResizeSimulationEvent, SimulationConfig,
    WorldManager,
};

use crate::runtime::{
    dev_tools::{BrushSettings, GridBrush},
    render::WorldTuningConfig,
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

const UI_FPS_THRESHOLD_GOOD: f64 = 32.0;

const BRUSH_OPTIONS: [(GridBrush, &str); 7] = [
    (GridBrush::None, "None"),
    (GridBrush::Water, "Spawn Water"),
    (GridBrush::Fire, "Spawn Fire"),
    (GridBrush::Sand, "Spawn Sand"),
    (GridBrush::RaiseTerrain, "Raise Terrain"),
    (GridBrush::LowerTerrain, "Lower Terrain"),
    (GridBrush::SpawnSphere, "Spawn Metal Sphere"),
];

pub fn dev_tuning_ui(
    mut contexts: EguiContexts,
    mut world_config: ResMut<WorldTuningConfig>,
    mut global_config: ResMut<GlobalPhysicsConfig>,
    mut sim_config: ResMut<SimulationConfig>,
    mut brush: ResMut<GridBrush>,
    mut brush_settings: ResMut<BrushSettings>,
    mut resize_writer: MessageWriter<ResizeSimulationEvent>,
    mut pending_resize: Local<Option<[u32; 2]>>,
    mut show_menu: Local<Option<bool>>,
    mut show_stats: Local<bool>,
    manager: Res<WorldManager>,
    grid: Res<ActiveWorldGrid>,
    keys: Res<ButtonInput<KeyCode>>,
    diagnostics: Res<DiagnosticsStore>,
) {
    let is_visible = show_menu.get_or_insert(true);
    if keys.just_pressed(KeyCode::Backquote) || keys.just_pressed(KeyCode::F12) {
        *is_visible = !*is_visible;
    }

    if !*is_visible {
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

            ui.checkbox(&mut *show_stats, "Statistics");
            add_separator(ui);

            ui.checkbox(&mut sim_config.run_liquid, "Run Liquid");
            ui.checkbox(&mut sim_config.run_biology, "Run Biology");
            ui.checkbox(&mut sim_config.run_granular, "Run Granular");
            add_separator(ui);

            ui.label(egui::RichText::new("Elevation Scale:").color(egui::Color32::LIGHT_GRAY));

            // TODO: Fix
            let mut scale = world_config.elevation_scale;
            if ui
                .add(
                    egui::DragValue::new(&mut scale)
                        .range(UI_ELEVATION_MIN..=UI_ELEVATION_MAX)
                        .speed(UI_ELEVATION_SPEED),
                )
                .changed()
            {
                world_config.elevation_scale = scale;
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

    if *show_stats {
        egui::Window::new("World Statistics")
            .anchor(
                egui::Align2::RIGHT_TOP,
                [UI_STATS_OFFSET_X, UI_STATS_OFFSET_Y],
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
