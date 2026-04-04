use bevy::{
    ecs::{component::Component, entity::Entity},
    math::{DVec3, IVec2, IVec3},
    time::Time,
};

use crate::world::types::{Pixel, WorldCell};

pub const CHUNK_SIZE: usize = 64;
pub const CHUNK_CELL_COUNT: usize = CHUNK_SIZE * CHUNK_SIZE;

#[derive(Component)]
pub struct ChunkData {
    pub is_loaded: bool,
    pub last_simulated: Time,
    pub theme: ChunkTheme,
    pub cells: Box<[WorldCell; CHUNK_CELL_COUNT]>,
    pub entities: Vec<Entity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkTheme(pub u8);

impl ChunkTheme {
    pub const GRASS_PLAINS: Self = Self(0);
    pub const OCEAN: Self = Self(1);
    pub const DESERT: Self = Self(2);
    pub const CAVE: Self = Self(3);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkKey {
    pub key: IVec3,
}

impl ChunkKey {
    pub fn from_dvec3(pos: DVec3, chunk_size: f64) -> Self {
        Self {
            key: IVec3::new(
                (pos.x / chunk_size).floor() as i32,
                (pos.y / chunk_size).floor() as i32,
                (pos.z / chunk_size).floor() as i32,
            ),
        }
    }

    /// Returns the center of the chunk.
    pub fn center(&self, chunk_size: f64) -> DVec3 {
        DVec3::new(
            (self.key.x as f64 * chunk_size) + (chunk_size / 2.0),
            (self.key.y as f64 * chunk_size) + (chunk_size / 2.0),
            (self.key.z as f64 * chunk_size) + (chunk_size / 2.0),
        )
    }

    pub fn to_ivec2(&self) -> IVec2 {
        IVec2::new(self.key.x, self.key.y)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ChunkView {
    pub min: ChunkKey,
    pub max: ChunkKey,
}

impl ChunkView {
    /// Creates a cubic bounding box centered at `center_pos`.
    /// `radius` controls the size of the bounding box.
    pub fn new_cubic(center_pos: DVec3, radius: f64, chunk_size: f64) -> Self {
        let center = ChunkKey::from_dvec3(center_pos, chunk_size);
        let r_chunks = (radius / chunk_size).ceil() as i32;

        Self {
            min: ChunkKey {
                key: IVec3::new(
                    center.key.x - r_chunks,
                    center.key.y - r_chunks,
                    center.key.z - r_chunks,
                ),
            },
            max: ChunkKey {
                key: IVec3::new(
                    center.key.x + r_chunks,
                    center.key.y + r_chunks,
                    center.key.z + r_chunks,
                ),
            },
        }
    }

    /// Creates a flattened bounding box centered at `center_pos`.
    /// `h_radius` controls the horizontal spread (X and Z axes).
    /// `v_radius` controls the vertical spread (Y axis).
    pub fn new_flat(center_pos: DVec3, h_radius: f64, v_radius: f64, chunk_size: f64) -> Self {
        let center = ChunkKey::from_dvec3(center_pos, chunk_size);
        let r_chunks = (h_radius / chunk_size).ceil() as i32;
        let y_chunks = (v_radius / chunk_size).ceil() as i32;

        Self {
            min: ChunkKey {
                key: IVec3::new(
                    center.key.x - r_chunks,
                    center.key.y - y_chunks,
                    center.key.z - r_chunks,
                ),
            },
            max: ChunkKey {
                key: IVec3::new(
                    center.key.x + r_chunks,
                    center.key.y + y_chunks,
                    center.key.z + r_chunks,
                ),
            },
        }
    }

    pub fn contains(&self, chunk: &ChunkKey) -> bool {
        chunk.key.x >= self.min.key.x
            && chunk.key.x <= self.max.key.x
            && chunk.key.y >= self.min.key.y
            && chunk.key.y <= self.max.key.y
            && chunk.key.z >= self.min.key.z
            && chunk.key.z <= self.max.key.z
    }
}
