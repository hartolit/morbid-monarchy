use bevy::prelude::*;

use crate::world::{
    ChunkManager, WorldFocus, WorldStore,
    events::{ChunkLoadRequest, ChunkLoadedEvent, ChunkUnloadEvent},
    grid::ActiveWorldGrid,
    handle_chunk_loaded, manage_chunk_window,
    simulation::simulate_biology,
};

pub mod world;

pub struct MonarchEnginePlugin;

impl Plugin for MonarchEnginePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorldFocus>()
            .init_resource::<ChunkManager>()
            .init_resource::<WorldStore>()
            .insert_resource(ActiveWorldGrid::new(832, 832, bevy::math::IVec2::ZERO))
            .add_message::<ChunkLoadRequest>()
            .add_message::<ChunkLoadedEvent>()
            .add_message::<ChunkUnloadEvent>()
            .add_systems(
                Update,
                (manage_chunk_window, handle_chunk_loaded, simulate_biology),
            );
    }
}
