use crate::engine::world::{
    cell::{FluidMat, TerrainMat, WorldCell},
    chunk::{CHUNK_CELL_COUNT, CHUNK_SIZE, ChunkData, ChunkKey, ChunkTheme},
};

use noise::{NoiseFn, OpenSimplex};
use rand::{RngExt, SeedableRng, rngs::StdRng};

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

    pub fn generate_chunk(&self, key: ChunkKey) -> ChunkData {
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

        ChunkData {
            last_simulated: 0.0,
            theme: ChunkTheme::GRASS_PLAINS,
            cells,
            serialized_entities: Vec::new(),
        }
    }

    fn generate_cell(&self, rng: &mut StdRng, world_x: i32, world_y: i32) -> WorldCell {
        let mut cell = WorldCell::default();

        let global_scale = 0.005;
        let base_noise = self
            .elevation_noise
            .get([world_x as f64 * global_scale, world_y as f64 * global_scale]);
        let normalized = (base_noise + 1.0) * 0.5;

        // Map noise directly to elevation (0 - 255 for standard hills)
        let elevation = (normalized * 255.0).clamp(0.0, 255.0) as u16;

        cell.set_elevation(elevation);
        cell.set_variants(rng.random_range(0..4) as u8);

        if elevation < 100 {
            cell.set_fluid_mat(FluidMat::WATER);
            cell.set_fluid_vol(100 - elevation);
            cell.set_terrain_mat(TerrainMat::SAND);
        } else if elevation < 120 {
            cell.set_terrain_mat(TerrainMat::SAND);
        } else {
            cell.set_terrain_mat(TerrainMat::FOLIAGE);
            cell.set_terrain_state(rng.random_range(0..10));
        }

        cell
    }
}
