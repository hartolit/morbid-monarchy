use bevy::ecs::resource::Resource;
use rustc_hash::FxHashMap;

use crate::world::chunk::{ChunkData, ChunkKey};

mod chunk;
mod events;
mod grid;
mod types;

#[derive(Resource)]
pub struct WorldStore {}
