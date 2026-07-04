use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::prelude::*;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};

pub mod brush;
pub mod ui;

#[derive(Resource, Default, PartialEq, Eq, Clone, Copy)]
pub enum GridBrush {
    #[default]
    None,
    Water,
    Fire,
    Sand,
    RaiseTerrain,
    LowerTerrain,
    SpawnSphere,
}

#[derive(Resource)]
pub struct BrushSettings {
    pub radius: i32,
    pub strength: u8,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self {
            radius: 5,
            strength: 1,
        }
    }
}

pub struct DevToolsPlugin;

impl Plugin for DevToolsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GridBrush>()
            .init_resource::<BrushSettings>()
            .add_plugins(EguiPlugin::default())
            .add_plugins(FrameTimeDiagnosticsPlugin::default())
            .add_systems(EguiPrimaryContextPass, ui::dev_tuning_ui)
            .add_systems(
                Update,
                (
                    brush::update_brush_cursor,
                    brush::handle_brush_input,
                    brush::attract_spheres_input,
                    brush::lift_spheres_input,
                ),
            );
    }
}
