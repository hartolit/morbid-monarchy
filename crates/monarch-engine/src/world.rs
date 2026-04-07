use bevy::{ecs::resource::Resource, math::DVec3};
use rustc_hash::FxHashMap;

use crate::world::chunk::{ChunkData, ChunkKey, ChunkView};

mod chunk;
mod events;
mod grid;
mod types;

#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct WorldFocus {
    pub position: DVec3,
}

#[derive(Resource)]
pub struct ChunkManager {
    pub current_view: ChunkView,
    pub view_radius: usize,
}
