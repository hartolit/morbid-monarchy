use crate::engine::world::{
    cell::{FluidMat, GranularMat, SurfaceMat, TerrainMat, WorldCell},
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

        // Base structural elevation
        let elevation = (normalized * 255.0).clamp(0.0, 255.0) as u16;
        cell.set_elevation(elevation);
        cell.set_variants(rng.random_range(0..WorldCell::MAX_VARIANTS));

        if elevation < 100 {
            // Deep Ocean: Stone base, sand floor, heavy water column
            cell.set_terrain_mat(TerrainMat::TERRAIN_STONE);
            cell.set_granular_mat(GranularMat::GRANULAR_SAND);
            cell.set_granular_vol(2);
            cell.set_fluid_mat(FluidMat::FLUID_WATER);
            cell.set_fluid_vol(100 - elevation);
        } else if elevation < 120 {
            // Coastlines / Shoals: Sandstone base, heavy granular sand
            cell.set_terrain_mat(TerrainMat::TERRAIN_SANDSTONE);
            cell.set_granular_mat(GranularMat::GRANULAR_SAND);
            cell.set_granular_vol(5);
        } else {
            // Highlands: Dirt base, organic surface cover
            cell.set_terrain_mat(TerrainMat::TERRAIN_DIRT);
            cell.set_surface_mat(SurfaceMat::SURFACE_FOLIAGE);

            // Randomize starting lifecycle to prevent massive simultaneous die-offs
            cell.set_surface_state(rng.random_range(0..10));
        }

        cell
    }
}
