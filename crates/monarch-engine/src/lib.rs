use bevy::prelude::*;

use crate::world::{
    ChunkManager, WorldFocus, WorldStore,
    chunk::CHUNK_SIZE,
    events::{ChunkLoadRequest, ChunkLoadedEvent, ChunkUnloadEvent, ResizeSimulationEvent},
    grid::ActiveWorldGrid,
    handle_chunk_loaded, handle_simulation_resize, manage_chunk_window,
    simulation::simulate_biology,
};

pub mod world;

pub struct MonarchEnginePlugin;

impl Plugin for MonarchEnginePlugin {
    fn build(&self, app: &mut App) {
        // Calculate exact initial grid size to match ChunkManager's default active_radius of 1
        let default_radius = 1;
        let span_chunks = (default_radius * 2 + 1) as i32;
        let initial_size = span_chunks * (CHUNK_SIZE as i32);

        app.init_resource::<WorldFocus>()
            .init_resource::<ChunkManager>()
            .init_resource::<WorldStore>()
            .insert_resource(ActiveWorldGrid::new(
                initial_size,
                initial_size,
                bevy::math::IVec2::ZERO,
            ))
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
