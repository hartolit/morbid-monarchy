use bevy::prelude::*;

use crate::{
    engine::{
        entities::{EntityPhysicsConfig, spherical::simulate_rigid_sphere_kinematics},
        events::{ChunkLoadRequest, ChunkLoadedEvent, ChunkUnloadEvent, ResizeSimulationEvent},
        simulation::{SimulationEventQueue, simulate_world},
        world::{
            ChunkManager, WorldFocus, WorldStore, grid::ActiveWorldGrid, handle_chunk_loaded,
            handle_simulation_resize, manage_chunk_window,
        },
    },
    prelude::SimulationConfig,
};

pub mod engine;
pub mod prelude;

pub struct MonarchEnginePlugin;

impl Plugin for MonarchEnginePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorldFocus>()
            .init_resource::<ChunkManager>()
            .init_resource::<WorldStore>()
            .init_resource::<SimulationEventQueue>()
            .init_resource::<SimulationConfig>()
            .init_resource::<EntityPhysicsConfig>()
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
            .add_systems(Update, (simulate_world, simulate_rigid_sphere_kinematics));
    }
}
