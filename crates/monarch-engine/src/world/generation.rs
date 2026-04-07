use crate::world::{
    chunk::{CHUNK_CELL_COUNT, CHUNK_SIZE, ChunkData, ChunkKey, ChunkTheme},
    types::{MaterialId, Pixel, PixelFlags, WorldCell},
};
use noise::{NoiseFn, OpenSimplex};
use rand::{RngExt, SeedableRng, rngs::StdRng};

/// The authoritative source for procedural world generation.
pub struct WorldGenerator {
    pub seed: u32,
    elevation_noise: OpenSimplex,
    moisture_noise: OpenSimplex,
}

impl WorldGenerator {
    pub fn new(seed: u32) -> Self {
        Self {
            seed,
            // Offset the moisture seed so it doesn't mirror elevation
            elevation_noise: OpenSimplex::new(seed),
            moisture_noise: OpenSimplex::new(seed.wrapping_add(1337)),
        }
    }

    pub fn generate_chunk(&self, key: ChunkKey) -> ChunkData {
        let mut cells = Vec::with_capacity(CHUNK_CELL_COUNT);

        // Sample biome theme at the center of the chunk
        let center = key.center();
        let theme = self.determine_theme(center.x, center.y);

        // Deterministic RNG for small details (like variants) to avoid costly noise lookups
        let mut rng = StdRng::seed_from_u64(
            (self.seed as u64) ^ ((key.key.x as u64) << 32) ^ (key.key.y as u64),
        );

        for local_y in 0..CHUNK_SIZE as i32 {
            for local_x in 0..CHUNK_SIZE as i32 {
                let world_x = key.key.x * CHUNK_SIZE as i32 + local_x;
                let world_y = key.key.y * CHUNK_SIZE as i32 + local_y;

                cells.push(self.generate_cell(&mut rng, theme, world_x, world_y));
            }
        }

        ChunkData {
            last_simulated: 0.0,
            theme,
            cells,
            serialized_entities: Vec::new(),
        }
    }

    /// Calculates a biome theme based on large-scale contiguous noise
    fn determine_theme(&self, world_x: f64, world_y: f64) -> ChunkTheme {
        let scale = 0.002; // Very low frequency for massive biomes
        let elevation = self.elevation_noise.get([world_x * scale, world_y * scale]);
        let moisture = self.moisture_noise.get([world_x * scale, world_y * scale]);

        if elevation < -0.2 {
            ChunkTheme::OCEAN
        } else if moisture < -0.3 {
            ChunkTheme::DESERT
        } else if moisture > 0.4 && elevation > 0.2 {
            ChunkTheme::CAVE // High elevation + high moisture = Dense rock/caves
        } else {
            ChunkTheme::GRASS_PLAINS
        }
    }

    /// Generates a specific cell, using higher-frequency noise for terrain patching
    fn generate_cell(
        &self,
        rng: &mut StdRng,
        theme: ChunkTheme,
        world_x: i32,
        world_y: i32,
    ) -> WorldCell {
        let variant = rng.random_range(0..4);
        let mut cell = WorldCell::default();

        // Higher frequency noise for patches of dirt/rock within a biome
        let patch_scale = 0.05;
        let patch_noise = self
            .elevation_noise
            .get([world_x as f64 * patch_scale, world_y as f64 * patch_scale]);

        match theme {
            ChunkTheme::GRASS_PLAINS => {
                let material = if patch_noise > 0.4 {
                    MaterialId::ROCK
                } else {
                    MaterialId::DIRT
                };
                cell.terrain = Self::solid_pixel(material, variant);
            }
            ChunkTheme::OCEAN => {
                // Ocean floor is rock, fluid layer is water
                cell.terrain = Self::solid_pixel(MaterialId::ROCK, variant);
                cell.fluid = Self::liquid_pixel(MaterialId::WATER, variant);
            }
            ChunkTheme::DESERT => {
                let material = if patch_noise > 0.6 {
                    MaterialId::ROCK
                } else {
                    MaterialId::DIRT
                }; // mostly sand/dirt
                cell.terrain = Self::solid_pixel(material, variant);
            }
            ChunkTheme::CAVE => {
                let material = if patch_noise < -0.4 {
                    MaterialId::DIRT
                } else {
                    MaterialId::ROCK
                }; // mostly rock
                cell.terrain = Self::solid_pixel(material, variant);
            }
            _ => {
                cell.terrain = Self::solid_pixel(MaterialId::DIRT, variant);
            }
        }

        cell
    }

    #[inline(always)]
    fn solid_pixel(material: MaterialId, variant: u8) -> Pixel {
        Pixel {
            material,
            state: 0,
            variant,
            flags: PixelFlags::IS_SOLID,
        }
    }

    #[inline(always)]
    fn liquid_pixel(material: MaterialId, variant: u8) -> Pixel {
        Pixel {
            material,
            state: 0,
            variant,
            flags: PixelFlags::NONE,
        }
    }
}
