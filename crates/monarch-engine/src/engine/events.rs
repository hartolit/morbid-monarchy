use crate::engine::world::chunk::CellChunk;
use bevy::ecs::message::Message;
use spatial_lib::prelude::math::ChunkKey;

/// Emitted by monarch-engine when the player moves and a chunk falls out of the Active Window.
/// morbid-app listens to this and writes the Box to disk.
#[derive(Message)]
pub struct ChunkUnloadEvent {
    pub key: ChunkKey,
    pub data: CellChunk,
}

/// Emitted by monarch-engine to tell morbid-app: "I need this chunk to fill the grid!"
#[derive(Message)]
pub struct ChunkLoadRequest {
    pub key: ChunkKey,
}

/// Emitted by morbid-app when it finishes reading the chunk from disk (or generating it).
/// monarch-engine listens to this and injects it into the ActiveWorldGrid.
#[derive(Message)]
pub struct ChunkLoadedEvent {
    pub key: ChunkKey,
    pub data: CellChunk,
}

/// Emitted to resize the active simulation domain and its preloading buffer.
#[derive(Message)]
pub struct ResizeSimulationEvent {
    pub new_active_radius_x: u32,
    pub new_active_radius_y: u32,
}
