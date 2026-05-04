use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::{diagnostic::DiagnosticsStore, ecs::message::MessageWriter, prelude::*};
use bevy_egui::{EguiContexts, egui};
use monarch_engine::prelude::{
    ActiveWorldGrid, ChunkManager, FluidMat, ResizeSimulationEvent, SimulationConfig,
};

use crate::runtime::{
    dev_tools::{BrushSettings, GridBrush},
    render::WorldTuningConfig,
};

const BRUSH_STRENGTH_RANGE: std::ops::RangeInclusive<u8> = 1..=255;
const BRUSH_RADIUS_RANGE: std::ops::RangeInclusive<i32> = 0..=64;

const BRUSH_OPTIONS: [(GridBrush, &str); 5] = [
    (GridBrush::None, "None"),
    (GridBrush::Water, "Spawn Water"),
    (GridBrush::Sand, "Spawn Sand"),
    (GridBrush::RaiseTerrain, "Raise Terrain"),
    (GridBrush::LowerTerrain, "Lower Terrain"),
];

pub fn dev_tuning_ui(
    mut contexts: EguiContexts,
    mut world_config: ResMut<WorldTuningConfig>,
    mut sim_config: ResMut<SimulationConfig>,
    mut brush: ResMut<GridBrush>,
    mut brush_settings: ResMut<BrushSettings>,
    mut resize_writer: MessageWriter<ResizeSimulationEvent>,
    mut pending_resize: Local<Option<[u32; 2]>>,
    mut show_menu: Local<Option<bool>>,
    mut show_stats: Local<bool>,
    manager: Res<ChunkManager>,
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

    let current_size =
        pending_resize.get_or_insert_with(|| [manager.active_radius_x, manager.active_radius_y]);

    egui::TopBottomPanel::top("dev_navbar").show(ctx, |ui| {
        ui.horizontal_centered(|ui| {
            let add_separator = |ui: &mut egui::Ui| {
                ui.add_space(16.0);
                ui.separator();
                ui.add_space(16.0);
            };

            ui.label(egui::RichText::new("Tools").strong().size(15.0));
            ui.add_space(16.0);

            ui.checkbox(&mut *show_stats, "Statistics");
            add_separator(ui);

            ui.checkbox(&mut sim_config.run_liquid, "Run Liquid");
            ui.checkbox(&mut sim_config.run_biology, "Run Biology");
            add_separator(ui);

            ui.label(egui::RichText::new("Elevation Scale:").color(egui::Color32::LIGHT_GRAY));
            ui.add(
                egui::DragValue::new(&mut world_config.elevation_scale)
                    .range(0.01..=5.0)
                    .speed(0.05),
            );
            add_separator(ui);

            ui.label(egui::RichText::new("Active Radius (X/Y):").color(egui::Color32::LIGHT_GRAY));
            ui.add(egui::DragValue::new(&mut current_size[0]).range(1..=32));
            ui.label("x");
            ui.add(egui::DragValue::new(&mut current_size[1]).range(1..=32));

            ui.add_space(16.0);

            if ui.button("Apply Resize").clicked() {
                if current_size[0] != manager.active_radius_x
                    || current_size[1] != manager.active_radius_y
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
                            .color(if fps > 55.0 {
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
            .anchor(egui::Align2::RIGHT_TOP, [-10.0, 40.0])
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                let total_mass: u64 = grid
                    .cells
                    .iter()
                    .filter(|c| c.fluid_mat() != FluidMat::EMPTY)
                    .map(|c| c.fluid_vol() as u64)
                    .sum();

                let total_elev: u64 = grid.cells.iter().map(|c| c.elevation() as u64).sum();

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
