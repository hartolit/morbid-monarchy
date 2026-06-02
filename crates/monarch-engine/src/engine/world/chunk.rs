use bitcode::{Decode, Encode};
use spatial_lib::chunk::Chunk;

use crate::engine::world::cell::WorldCell;

pub const CHUNK_SIZE: usize = 64;
pub const CHUNK_CELL_COUNT: usize = CHUNK_SIZE * CHUNK_SIZE;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode, Default)]
pub struct ChunkTheme(pub u8);

impl ChunkTheme {
    pub const GRASS_PLAINS: Self = Self(0);
    pub const OCEAN: Self = Self(1);
    pub const DESERT: Self = Self(2);
    pub const CAVE: Self = Self(3);
    pub const FOREST: Self = Self(4);
    pub const SWAMP: Self = Self(5);
    pub const TUNDRA: Self = Self(6);
    pub const MOUNTAIN: Self = Self(7);
}

#[derive(Clone, Encode, Decode, Default, Debug, PartialEq)]
pub struct ChunkMetadata {
    pub last_simulated: f64,
    pub theme: ChunkTheme,
}

pub type CellChunk = Chunk<WorldCell, ChunkMetadata>;
