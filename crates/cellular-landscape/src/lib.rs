use bevy::prelude::*;

use crate::core::LandscapePlugin;

pub mod core;
pub mod prelude;

pub struct CellularLandscapePlugin;

impl Plugin for CellularLandscapePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(LandscapePlugin);
    }
}
