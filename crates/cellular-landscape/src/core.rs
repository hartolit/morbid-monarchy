use bevy::{
    app::{App, Plugin, Update},
    ecs::schedule::IntoScheduleConfigs,
};

use crate::core::{
    events::{ChunkLoadRequest, ChunkLoadedEvent, ChunkUnloadEvent, ResizeSimulationEvent},
    physics::{components::GlobalPhysicsConfig, systems::simulate_grid_kinematics},
    simulation::{SimulationConfig, SimulationEventQueue, simulate_world},
    world::{
        WorldFocus, WorldManager, WorldStore, grid::ActiveWorldGrid, handle_chunk_loaded,
        handle_simulation_resize, manage_chunk_window,
    },
};

pub mod events;
pub mod physics;
pub mod simulation;
pub mod utils;
pub mod world;

pub struct LandscapePlugin;

impl Plugin for LandscapePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorldFocus>()
            .init_resource::<WorldManager>()
            .init_resource::<WorldStore>()
            .init_resource::<SimulationEventQueue>()
            .init_resource::<SimulationConfig>()
            .init_resource::<GlobalPhysicsConfig>()
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
            .add_systems(Update, (simulate_world, simulate_grid_kinematics));
    }
}
