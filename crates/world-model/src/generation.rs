//! Deterministic terrain generation.
//!
//! A chunk depends only on the seed and its absolute key, so any chunk can be generated in any
//! order, on any thread, and comes out the same. Only a chunk that is genuinely absent from a
//! store may be generated; an unreadable stored chunk must never be replaced this way.

use noise::{NoiseFn, OpenSimplex};
use rand::{RngExt, SeedableRng, rngs::StdRng};

use crate::{
    cell::WorldCell,
    material::{FluidMat, GranularMat, SurfaceMat, TerrainMat},
    spatial::{CHUNK_CELL_COUNT, CHUNK_SIZE, ChunkKey},
};

/// The look a chunk was generated for. Only `GRASS_PLAINS` is produced today; the other values
/// keep their identities because saved chunks record a theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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

/// The cells of one generated chunk, row-major, and its theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedChunk {
    pub cells: Box<[WorldCell]>,
    pub theme: ChunkTheme,
}

pub struct WorldGenerator {
    pub seed: u32,
    elevation_noise: OpenSimplex,
}

impl WorldGenerator {
    pub fn new(seed: u32) -> Self {
        Self {
            seed,
            elevation_noise: OpenSimplex::new(seed),
        }
    }

    pub fn generate_chunk(&self, key: ChunkKey) -> GeneratedChunk {
        let mut cells = vec![WorldCell::default(); CHUNK_CELL_COUNT];

        let mut rng = StdRng::seed_from_u64(
            (self.seed as u64)
                ^ (((key.key.x as u32) as u64) << 32)
                ^ ((key.key.y as u32) as u64)
                ^ (((key.key.z as u32) as u64) << 16),
        );

        for local_y in 0..CHUNK_SIZE as i32 {
            for local_x in 0..CHUNK_SIZE as i32 {
                let world_x = key.key.x * CHUNK_SIZE as i32 + local_x;
                let world_y = key.key.y * CHUNK_SIZE as i32 + local_y;

                cells[(local_y * CHUNK_SIZE as i32 + local_x) as usize] =
                    self.generate_cell(&mut rng, world_x, world_y);
            }
        }

        GeneratedChunk {
            cells: cells.into_boxed_slice(),
            theme: ChunkTheme::GRASS_PLAINS,
        }
    }

    fn generate_cell(&self, rng: &mut StdRng, world_x: i32, world_y: i32) -> WorldCell {
        let mut cell = WorldCell::default();

        let global_scale = 0.005;
        let base_noise = self
            .elevation_noise
            .get([world_x as f64 * global_scale, world_y as f64 * global_scale]);
        let normalized = (base_noise + 1.0) * 0.5;

        let elevation = (normalized * 255.0).clamp(0.0, 255.0) as u16;
        cell.set_elevation(elevation);
        cell.set_variants(rng.random_range(0..WorldCell::MAX_VARIANTS));

        if elevation < 100 {
            cell.set_terrain_mat(TerrainMat::TERRAIN_STONE);
            cell.set_granular_mat(GranularMat::GRANULAR_SAND);
            cell.set_granular_vol(2);
            cell.set_fluid_mat(FluidMat::FLUID_WATER);
            cell.set_fluid_vol(100 - elevation);
        } else if elevation < 120 {
            cell.set_terrain_mat(TerrainMat::TERRAIN_SANDSTONE);
            cell.set_granular_mat(GranularMat::GRANULAR_SAND);
            cell.set_granular_vol(5);
        } else {
            cell.set_terrain_mat(TerrainMat::TERRAIN_DIRT);
            cell.set_surface_mat(SurfaceMat::SURFACE_FOLIAGE);
            cell.set_surface_state(rng.random_range(0..10));
        }

        cell
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Words computed by the baseline generator before extraction, recorded in the legacy
    /// fixture manifest (`crates/morbid-app/tests/fixtures/legacy-v0/manifest.toml`).
    #[test]
    fn origin_chunk_matches_the_baseline_generator() {
        let chunk = WorldGenerator::new(69).generate_chunk(ChunkKey::new(0, 0, 0));
        let first: Vec<u64> = chunk.cells[..4].iter().map(|cell| cell.0).collect();
        assert_eq!(
            first,
            [
                0x0000000007F40022,
                0x00012000080C0022,
                0x0000A00008018022,
                0x00004000081B0022
            ]
        );
        let elevation: u64 = chunk.cells.iter().map(|cell| cell.elevation() as u64).sum();
        let granular: u64 = chunk
            .cells
            .iter()
            .map(|cell| cell.granular_vol() as u64)
            .sum();
        assert_eq!((elevation, granular), (528_638, 4_695));
        assert_eq!(chunk.theme, ChunkTheme::GRASS_PLAINS);
    }

    #[test]
    fn generation_depends_only_on_seed_and_key() {
        let key = ChunkKey::new(-1, -2, 0);
        let a = WorldGenerator::new(69).generate_chunk(key);
        let b = WorldGenerator::new(69).generate_chunk(key);
        assert_eq!(a, b);
        assert_ne!(a, WorldGenerator::new(70).generate_chunk(key));
    }
}
