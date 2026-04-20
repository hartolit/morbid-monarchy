use crate::engine::world::{
    cell::{MaterialId, Pixel, PixelFlags, WorldCell},
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
        let mut cells = Vec::with_capacity(CHUNK_CELL_COUNT);

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

                cells.push(self.generate_cell(&mut rng, world_x, world_y));
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

        // Normalize noise (-1.0 to 1.0) into a 0.0 to 1.0 range
        let normalized = (base_noise + 1.0) * 0.5;

        // ALGEBRAIC PHYSICS RULE: High gas pressure = crushed terrain (valleys). Low gas pressure = mountains.
        // So we invert the noise: high noise (mountains) maps to low gas (0). Low noise maps to high gas (255).
        let gas_pressure = ((1.0 - normalized) * 255.0).clamp(0.0, 255.0) as u8;

        let variant = rng.random_range(0..4);

        // Set Base Terrain
        cell.terrain = Pixel {
            material: MaterialId::SOLID_STONE,
            state: 0,
            variant,
            flags: PixelFlags::IS_SOLID,
        };

        // Set Atmospheric Pressure (The invisible weight crushing the terrain)
        cell.atmosphere = Pixel {
            material: MaterialId::GAS_STEAM, // Doesn't render visibly in the current shader, but holds the data
            state: gas_pressure,
            variant: 0,
            flags: PixelFlags::NONE,
        };

        // Biomes generated based on atmospheric pressure
        if gas_pressure > 160 {
            // DEEP CRATER: Fill it with water
            let fluid_depth = gas_pressure - 160;
            cell.fluid = Pixel {
                material: MaterialId::LIQUID_WATER,
                state: fluid_depth,
                variant: rng.random_range(0..2),
                flags: PixelFlags::NONE,
            };
            cell.terrain.material = MaterialId::LOOSE_SAND;
        } else if gas_pressure > 140 {
            // COASTLINE / SHALLOWS
            cell.terrain.material = MaterialId::LOOSE_SAND;
        } else {
            // HIGHLANDS / MOUNTAINS
            cell.terrain.material = MaterialId::ORGANIC_FOLIAGE;
            cell.terrain.state = rng.random_range(0..10); // Plant aging
        }

        cell
    }
}
