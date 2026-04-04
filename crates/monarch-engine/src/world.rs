use bevy::ecs::resource::Resource;
use rustc_hash::FxHashMap;

use crate::world::chunk::{ChunkData, ChunkKey};

mod chunk;
mod grid;
mod types;

#[derive(Resource)]
pub struct WorldStore {
    pub active_chunks: FxHashMap<ChunkKey, ChunkData>,
}
