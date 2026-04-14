use bevy::prelude::*;

use crate::world::{
    ChunkManager, WorldFocus, WorldStore,
    events::{ChunkLoadRequest, ChunkLoadedEvent, ChunkUnloadEvent, ResizeSimulationEvent},
    grid::ActiveWorldGrid,
    handle_chunk_loaded, handle_simulation_resize, manage_chunk_window,
    simulation::simulate_biology,
};

pub mod world;

pub struct MonarchEnginePlugin;

impl Plugin for MonarchEnginePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorldFocus>()
            .init_resource::<ChunkManager>()
            .init_resource::<WorldStore>()
            .insert_resource(ActiveWorldGrid::default())
            .add_message::<ChunkLoadRequest>()
            .add_message::<ChunkLoadedEvent>()
            .add_message::<ChunkUnloadEvent>()
            .add_message::<ResizeSimulationEvent>()
            .add_systems(
                Update,
                (
                    handle_simulation_resize,
                    manage_chunk_window,
                    handle_chunk_loaded,
                )
                    .chain(),
            )
            .add_systems(Update, (simulate_biology,));
    }
}
