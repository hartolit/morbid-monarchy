use crate::world::{
    chunk::{CHUNK_CELL_COUNT, ChunkKey},
    types::WorldCell,
};
use bevy::prelude::Event;

/// Engine tells App: "The player is moving near this chunk, please fetch it."
#[derive(Event)]
pub struct ChunkLoadRequest(pub ChunkKey);

/// App tells Engine: "Here is the data from disk (or freshly generated)."
#[derive(Event)]
pub struct ChunkLoadEvent {
    pub key: ChunkKey,
    pub cells: Box<[WorldCell; CHUNK_CELL_COUNT]>,
    pub missed_ticks: u32, // How much time passed since it was last saved
}

/// Engine tells App: "This chunk left the view radius. I have extracted it from the grid. Save it."
#[derive(Event)]
pub struct ChunkUnloadEvent {
    pub key: ChunkKey,
    pub cells: Box<[WorldCell; CHUNK_CELL_COUNT]>,
}
