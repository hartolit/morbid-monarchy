use bevy::app::{App, Plugin};
use cellular_landscape::CellularLandscapePlugin;

pub mod engine;
pub mod prelude;

pub struct MonarchEnginePlugin;

impl Plugin for MonarchEnginePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(CellularLandscapePlugin);
    }
}
