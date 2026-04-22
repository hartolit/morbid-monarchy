use bevy::diagnostic::DiagnosticsStore;
use bevy::prelude::*;
use bevy::{diagnostic::FrameTimeDiagnosticsPlugin, ecs::message::MessageWriter};
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};
use monarch_engine::prelude::{ChunkManager, ResizeSimulationEvent};

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
    mut tuning: ResMut<WorldTuningConfig>,
    manager: Res<ChunkManager>,
    mut resize_writer: MessageWriter<ResizeSimulationEvent>,
    mut pending_resize: Local<Option<[u32; 2]>>,
    mut show_menu: Local<Option<bool>>,
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
            // --- FPS COUNTER ---
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
                ui.add_space(16.0);
                ui.separator();
                ui.add_space(16.0);
            }

            ui.label(egui::RichText::new("Tools").strong().size(15.0));
            ui.add_space(16.0);
            ui.separator();
            ui.add_space(16.0);

            // --- Physics Tuning ---
            ui.label(egui::RichText::new("Terrain H-Max:").color(egui::Color32::LIGHT_GRAY));
            ui.add(
                egui::DragValue::new(&mut tuning.h_max)
                    .range(50.0..=500.0)
                    .speed(1.0),
            );

            ui.add_space(16.0);

            ui.label(egui::RichText::new("Elevation Scale:").color(egui::Color32::LIGHT_GRAY));
            ui.add(
                egui::DragValue::new(&mut tuning.elevation_scale)
                    .range(0.1..=5.0)
                    .speed(0.05),
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
        });
    });
}
