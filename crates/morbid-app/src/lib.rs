use bevy::prelude::*;
use monarch_engine::MonarchEnginePlugin;

pub fn run() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(MonarchEnginePlugin)
        .run();
}
