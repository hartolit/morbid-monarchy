use bevy::prelude::*;

use crate::core::LandscapePlugin;

pub mod core;
pub mod prelude;

pub struct MonarchEnginePlugin;

impl Plugin for MonarchEnginePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(LandscapePlugin);
    }
}
