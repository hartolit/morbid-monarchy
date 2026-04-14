use crate::world::{
    cell::{MaterialId, Pixel, PixelFlags, WorldCell},
    chunk::{CHUNK_CELL_COUNT, CHUNK_SIZE, ChunkData, ChunkKey, ChunkTheme},
};
use noise::{NoiseFn, OpenSimplex};
use rand::{RngExt, SeedableRng, rngs::StdRng};

pub struct WorldGenerator {
    pub seed: u32,
    elevation_noise: OpenSimplex,
    moisture_noise: OpenSimplex,
}

impl WorldGenerator {
    pub fn new(seed: u32) -> Self {
        Self {
            seed,
            elevation_noise: OpenSimplex::new(seed),
            moisture_noise: OpenSimplex::new(seed.wrapping_add(1337)),
        }
    }

    pub fn generate_chunk(&self, key: ChunkKey) -> ChunkData {
        let mut cells = Vec::with_capacity(CHUNK_CELL_COUNT);

        // Cast to u32 first to prevent sign-extension bit bleeding
        let mut rng = StdRng::seed_from_u64(
            (self.seed as u64)
                ^ (((key.key.x as u32) as u64) << 32)
                ^ ((key.key.y as u32) as u64)
                ^ (((key.key.z as u32) as u64) << 16),
        );

        if key.key.z < 0 {
            for _ in 0..CHUNK_CELL_COUNT {
                let variant = rng.random_range(0..4);
                let mut cell = WorldCell::default();
                cell.terrain = Self::solid_pixel(MaterialId::SAND, variant);
                cells.push(cell);
            }
            return ChunkData {
                last_simulated: 0.0,
                theme: ChunkTheme::CAVE,
                cells,
                serialized_entities: Vec::new(),
            };
        }

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
        let variant = rng.random_range(0..4);
        let mut cell = WorldCell::default();

        let global_scale = 0.002;
        let base_elevation = self
            .elevation_noise
            .get([world_x as f64 * global_scale, world_y as f64 * global_scale]);
        let moisture = self
            .moisture_noise
            .get([world_x as f64 * global_scale, world_y as f64 * global_scale]);

        let detail_scale = 0.02;
        let detail_noise = self
            .elevation_noise
            .get([world_x as f64 * detail_scale, world_y as f64 * detail_scale]);

        let final_elevation = base_elevation + (detail_noise * 0.05);

        // --- ORGANIC WATER & COASTLINES ---
        if final_elevation < -0.2 {
            cell.terrain = Self::solid_pixel(MaterialId::SAND, variant);

            let depth_normalized = ((-0.2 - final_elevation) * 5.0).clamp(0.0, 1.0);
            let state = if depth_normalized > 0.6 {
                255
            } else if depth_normalized > 0.3 {
                128
            } else {
                64
            };

            let mut fluid_pixel = Self::liquid_pixel(MaterialId::WATER, 0);
            fluid_pixel.state = state;
            cell.fluid = fluid_pixel;

            return cell;
        }

        // --- NATURAL BEACHES ---
        if final_elevation < -0.15 {
            cell.terrain = Self::solid_pixel(MaterialId::SAND, variant);
            return cell;
        }

        // --- PER-CELL LAND BIOMES ---
        // Utilize the moisture noise for macro-biome separation.
        // Detail noise adds slight jitter to the border so it isn't completely smooth.
        let final_moisture = moisture + (detail_noise * 0.1);

        let material = if final_moisture > -0.15 {
            MaterialId::GRASS
        } else {
            MaterialId::SAND
        };

        cell.terrain = Self::solid_pixel(material, variant);

        // Both Sand and Grass spawn with initial cellular strength
        if material == MaterialId::GRASS || material == MaterialId::SAND {
            cell.terrain.state = rng.random_range(5..=10);
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
