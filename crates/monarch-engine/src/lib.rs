use bevy::prelude::*;

use crate::world::{
    events::{ChunkLoadRequest, ChunkLoadedEvent, ChunkUnloadEvent},
    grid::ActiveWorldGrid,
    handle_chunk_loaded, manage_chunk_window,
    types::{ChunkManager, WorldFocus, WorldStore},
};

mod world;

pub struct MonarchEnginePlugin;

impl Plugin for MonarchEnginePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorldFocus>()
            .init_resource::<ChunkManager>()
            .init_resource::<WorldStore>()
            .insert_resource(ActiveWorldGrid::new(1024, 1024, bevy::math::IVec2::ZERO))
            .add_message::<ChunkLoadRequest>()
            .add_message::<ChunkLoadedEvent>()
            .add_message::<ChunkUnloadEvent>()
            .add_systems(Update, (manage_chunk_window, handle_chunk_loaded));
    }
}

pub fn test_system() {}
