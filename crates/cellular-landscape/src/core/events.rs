use crate::core::world::chunk::CellChunk;
use bevy::ecs::message::Message;
use spatial_lib::prelude::math::ChunkKey;

#[derive(Message)]
pub struct ChunkUnloadEvent {
    pub key: ChunkKey,
    pub data: CellChunk,
}

#[derive(Message)]
pub struct ChunkLoadRequest {
    pub key: ChunkKey,
}

#[derive(Message)]
pub struct ChunkLoadedEvent {
    pub key: ChunkKey,
    pub data: CellChunk,
}

#[derive(Message)]
pub struct ResizeSimulationEvent {
    pub new_active_radius_x: u32,
    pub new_active_radius_y: u32,
}
