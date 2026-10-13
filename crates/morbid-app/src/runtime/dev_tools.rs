use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::prelude::*;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use world_model::{
    command::EditTool,
    config::{DEFAULT_BRUSH_RADIUS_CELLS, DEFAULT_BRUSH_STRENGTH},
    material::FluidMat,
};

pub mod brush;
pub mod compass;
pub mod ui;

#[derive(Resource, Default, PartialEq, Eq, Clone, Copy)]
pub enum GridBrush {
    #[default]
    None,
    Water,
    Lava,
    Oil,
    Ice,
    Fire,
    Sand,
    RaiseTerrain,
    LowerTerrain,
    SpawnMetalSphere,
    SpawnWoodSphere,
}

impl GridBrush {
    /// The material edit this brush issues; `None` for selection and the spawning brushes.
    pub fn edit_tool(self) -> Option<EditTool> {
        Some(match self {
            Self::Water => EditTool::Water,
            Self::Lava => EditTool::Lava,
            Self::Oil => EditTool::Oil,
            Self::Ice => EditTool::Ice,
            Self::Fire => EditTool::Fire,
            Self::Sand => EditTool::Sand,
            Self::RaiseTerrain => EditTool::Raise,
            Self::LowerTerrain => EditTool::Lower,
            Self::None | Self::SpawnMetalSphere | Self::SpawnWoodSphere => return None,
        })
    }

    /// Liquid painted by fluid brushes ([`EditTool::fluid`] owns the mapping).
    pub fn fluid(self) -> Option<FluidMat> {
        self.edit_tool().and_then(EditTool::fluid)
    }

    pub fn spawns_entity(self) -> bool {
        matches!(self, Self::SpawnMetalSphere | Self::SpawnWoodSphere)
    }
}

/// Whether the developer overlay (tool bar, panels and compass) is drawn.
#[derive(Resource)]
pub struct DevOverlay {
    pub visible: bool,
}

impl Default for DevOverlay {
    fn default() -> Self {
        Self { visible: true }
    }
}

#[derive(Resource)]
pub struct BrushSettings {
    pub radius: i32,
    pub strength: u8,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self {
            radius: DEFAULT_BRUSH_RADIUS_CELLS,
            strength: DEFAULT_BRUSH_STRENGTH,
        }
    }
}

pub struct DevToolsPlugin {
    /// Whether mouse buttons drive the brush, attract and lift tools. A scripted caller applies
    /// the same tool functions itself.
    pub pointer_tools: bool,
}

impl Plugin for DevToolsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GridBrush>()
            .init_resource::<BrushSettings>()
            .init_resource::<DevOverlay>()
            .add_plugins(EguiPlugin::default())
            .add_plugins(FrameTimeDiagnosticsPlugin::default())
            .add_systems(
                EguiPrimaryContextPass,
                (ui::dev_tuning_ui, compass::navigation_compass).chain(),
            )
            .add_systems(Update, brush::update_brush_cursor);
        if self.pointer_tools {
            app.add_systems(
                Update,
                (
                    brush::handle_brush_input,
                    brush::attract_spheres_input,
                    brush::lift_spheres_input,
                ),
            );
        }
    }
}
