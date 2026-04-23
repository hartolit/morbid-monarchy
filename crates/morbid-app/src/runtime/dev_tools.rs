use bevy::diagnostic::DiagnosticsStore;
use bevy::prelude::*;
use bevy::{diagnostic::FrameTimeDiagnosticsPlugin, ecs::message::MessageWriter};
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};
use monarch_engine::prelude::{
    ActiveWorldGrid, ChunkManager, ResizeSimulationEvent, SimulationConfig,
};

use crate::runtime::render::WorldTuningConfig;

pub struct DevToolsPlugin;

impl Plugin for DevToolsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .add_plugins(FrameTimeDiagnosticsPlugin::default())
            .add_systems(EguiPrimaryContextPass, dev_tuning_ui);
    }
}

fn dev_tuning_ui(
    mut contexts: EguiContexts,
    mut world_config: ResMut<WorldTuningConfig>,
    mut sim_config: ResMut<SimulationConfig>,
    mut resize_writer: MessageWriter<ResizeSimulationEvent>,
    mut pending_resize: Local<Option<[u32; 2]>>,
    mut show_menu: Local<Option<bool>>,
    mut show_stats: Local<bool>,
    manager: Res<ChunkManager>,
    grid: Res<ActiveWorldGrid>,
    keys: Res<ButtonInput<KeyCode>>,
    diagnostics: Res<DiagnosticsStore>,
) {
    // Toggle
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
            // --- Left Aligned Tools ---
            ui.label(egui::RichText::new("Tools").strong().size(15.0));
            ui.add_space(16.0);

            // Toggle statistics display
            ui.checkbox(&mut *show_stats, "Statistics");

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(16.0);

            // --- SIMULATION TOGGLES ---
            ui.checkbox(&mut sim_config.run_liquid, "Run Liquid");
            ui.checkbox(&mut sim_config.run_biology, "Run Biology");

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(16.0);

            // --- Physics Tuning ---
            ui.label(egui::RichText::new("Terrain H-Max:").color(egui::Color32::LIGHT_GRAY));
            ui.add(
                egui::DragValue::new(&mut world_config.h_max)
                    .range(50.0..=500.0)
                    .speed(8.0),
            );

            ui.add_space(16.0);

            ui.label(egui::RichText::new("Elevation Scale:").color(egui::Color32::LIGHT_GRAY));
            ui.add(
                egui::DragValue::new(&mut world_config.elevation_scale)
                    .range(0.1..=5.0)
                    .speed(0.1),
            );

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(16.0);

            // --- Grid Resizing ---
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

            // --- Right Aligned FPS Counter ---
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

    // --- Floating Statistics Panel ---
    if *show_stats {
        egui::Window::new("World Statistics")
            .anchor(egui::Align2::RIGHT_TOP, [-10.0, 40.0])
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                let total_mass: u64 = grid
                    .cells
                    .iter()
                    .filter(|c| c.fluid.material.0 >= 1 && c.fluid.material.0 <= 31) // Is liquid
                    .map(|c| c.fluid.state as u64)
                    .sum();

                let total_atmos: u64 = grid.cells.iter().map(|c| c.atmosphere.state as u64).sum();

                egui::Grid::new("stats_grid").striped(true).show(ui, |ui| {
                    ui.label(egui::RichText::new("Total Liquid Vol:").strong());
                    ui.label(
                        egui::RichText::new(total_mass.to_string()).color(egui::Color32::CYAN),
                    );
                    ui.end_row();

                    ui.label(egui::RichText::new("Total Atmos Pressure:").strong());
                    ui.label(
                        egui::RichText::new(total_atmos.to_string())
                            .color(egui::Color32::LIGHT_GRAY),
                    );
                    ui.end_row();
                });
            });
    }
}
