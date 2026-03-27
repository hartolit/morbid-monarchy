use bevy_ecs::prelude::Resource;
use bitcode::{Decode, Encode};
use rand_chacha::ChaCha8Rng;
use rand_core::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};

use crate::world::{
    ChunkData, ChunkKey, ChunkLocalPixel, ChunkLocalPoint, ChunkTheme, ProcAsset, ProcAssetKind,
    WorldObjectId, WorldPixel, CHUNK_PIXEL_COUNT, CHUNK_PIXEL_SIZE, DEFAULT_CHUNK_WORLD_SIZE,
};

pub const DEFAULT_WORLD_SEED: u64 = 7;
pub const DEFAULT_ACTIVE_CHUNK_RADIUS: i32 = 1;
pub const DEFAULT_MAX_PROC_ASSETS_PER_CHUNK: u8 = 6;

#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct WorldConfig {
    pub world_seed: u64,
    pub chunk_world_size: f32,
    pub active_chunk_radius: i32,
    pub max_proc_assets_per_chunk: u8,
}

fn grass_plane_pixel(cell_seed: u64, coarse_seed: u64, edge_bias: u16) -> WorldPixel {
    if edge_bias <= 1 && coarse_seed % 9 == 0 {
        return WorldPixel::Dirt;
    }

    if cell_seed % 37 == 0 {
        WorldPixel::Dirt
    } else {
        WorldPixel::Grass
    }
}

fn dark_pixel(cell_seed: u64, coarse_seed: u64) -> WorldPixel {
    if coarse_seed % 7 == 0 {
        WorldPixel::Rock
    } else if cell_seed % 5 == 0 {
        WorldPixel::Grass
    } else {
        WorldPixel::Dirt
    }
}

fn cave_pixel(cell_seed: u64, coarse_seed: u64) -> WorldPixel {
    if coarse_seed % 5 <= 1 {
        WorldPixel::Rock
    } else if cell_seed % 11 == 0 {
        WorldPixel::Grass
    } else {
        WorldPixel::Dirt
    }
}

fn ocean_pixel(cell_seed: u64, coarse_seed: u64, edge_bias: u16) -> WorldPixel {
    if edge_bias <= 1 && coarse_seed % 5 == 0 {
        return WorldPixel::Dirt;
    }

    if coarse_seed % 11 == 0 {
        WorldPixel::Rock
    } else if cell_seed % 19 == 0 {
        WorldPixel::Dirt
    } else {
        WorldPixel::Water
    }
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            world_seed: DEFAULT_WORLD_SEED,
            chunk_world_size: DEFAULT_CHUNK_WORLD_SIZE,
            active_chunk_radius: DEFAULT_ACTIVE_CHUNK_RADIUS,
            max_proc_assets_per_chunk: DEFAULT_MAX_PROC_ASSETS_PER_CHUNK,
        }
    }
}

pub fn generate_chunk(config: &WorldConfig, key: ChunkKey) -> ChunkData {
    let theme = select_theme(config, key);
    let mut rng = seeded_rng(config, key);
    let materials = bake_materials(config, key, theme);
    let mut assets = build_assets(config, key, theme, &mut rng);
    assets.sort_by_key(|asset| asset.id);

    ChunkData::new(key, theme, materials, assets)
}

fn bake_materials(config: &WorldConfig, key: ChunkKey, theme: ChunkTheme) -> Vec<WorldPixel> {
    let mut materials = vec![theme.base_pixel(); CHUNK_PIXEL_COUNT];
    let chunk_seed = mix_seed(config.world_seed ^ 0xD1B5_4A32_9C77_EF10, key);

    for y in 0..CHUNK_PIXEL_SIZE {
        for x in 0..CHUNK_PIXEL_SIZE {
            let local = ChunkLocalPixel { x, y };
            let world_pixel_x = key.x * i32::from(CHUNK_PIXEL_SIZE) + i32::from(x);
            let world_pixel_y = key.y * i32::from(CHUNK_PIXEL_SIZE) + i32::from(y);
            let cell_seed = mix_world_pixel_seed(chunk_seed, world_pixel_x, world_pixel_y);
            let coarse_seed = mix_world_pixel_seed(chunk_seed ^ 0x9E37_79B9_7F4A_7C15, world_pixel_x / 8, world_pixel_y / 8);
            let edge_bias = y.min(CHUNK_PIXEL_SIZE - 1 - y).min(x.min(CHUNK_PIXEL_SIZE - 1 - x));
            let pixel = match theme {
                ChunkTheme::GrassPlane => grass_plane_pixel(cell_seed, coarse_seed, edge_bias),
                ChunkTheme::Dark => dark_pixel(cell_seed, coarse_seed),
                ChunkTheme::Cave => cave_pixel(cell_seed, coarse_seed),
                ChunkTheme::Ocean => ocean_pixel(cell_seed, coarse_seed, edge_bias),
            };
            materials[usize::from(local.as_index())] = pixel;
        }
    }

    materials
}

fn select_theme(config: &WorldConfig, key: ChunkKey) -> ChunkTheme {
    if key == ChunkKey::ORIGIN {
        return ChunkTheme::GrassPlane;
    }

    match mix_seed(config.world_seed, key) % 4 {
        0 => ChunkTheme::GrassPlane,
        1 => ChunkTheme::Dark,
        2 => ChunkTheme::Cave,
        _ => ChunkTheme::Ocean,
    }
}

fn build_assets(
    config: &WorldConfig,
    key: ChunkKey,
    theme: ChunkTheme,
    rng: &mut ChaCha8Rng,
) -> Vec<ProcAsset> {
    let mut assets = Vec::new();

    if theme == ChunkTheme::Ocean {
        if mix_seed(config.world_seed ^ 0xA5A5_A5A5_A5A5_A5A5, key) % 2 == 0 {
            assets.push(make_asset(key, ProcAssetKind::Rock, 0, 1, 0, config, rng));
        }
        return assets;
    }

    assets.push(make_asset(key, ProcAssetKind::Rock, 0, 2, 0, config, rng));
    assets.push(make_asset(key, ProcAssetKind::Bush, 1, 3, 1, config, rng));

    let target_assets = usize::from(config.max_proc_assets_per_chunk.max(2));
    for index in 2..target_assets {
        let kind_offset = ((key.x.abs() + key.y.abs()) % 3) as usize;
        let kind = match (index + kind_offset) % 4 {
            0 => ProcAssetKind::Grass,
            1 => ProcAssetKind::Tree,
            2 => ProcAssetKind::DirtPatch,
            _ => ProcAssetKind::Bush,
        };
        assets.push(make_asset(
            key,
            kind,
            index as u8,
            (index % 4) as u8,
            index as u64,
            config,
            rng,
        ));
    }

    assets
}

fn make_asset(
    key: ChunkKey,
    kind: ProcAssetKind,
    intensity: u8,
    variant: u8,
    salt: u64,
    config: &WorldConfig,
    rng: &mut ChaCha8Rng,
) -> ProcAsset {
    let position = ChunkLocalPoint::new(
        random_range_f32(rng, 24.0, config.chunk_world_size - 24.0),
        random_range_f32(rng, 24.0, config.chunk_world_size - 24.0),
    );
    let object_id = WorldObjectId(mix_seed(salt ^ 0x9E37_79B9_7F4A_7C15, key));
    ProcAsset::new(object_id, kind, intensity, variant, position)
}

fn seeded_rng(config: &WorldConfig, key: ChunkKey) -> ChaCha8Rng {
    let seed = mix_seed(config.world_seed, key).to_le_bytes();
    let mut full_seed = [0_u8; 32];
    for (index, chunk) in full_seed.chunks_exact_mut(8).enumerate() {
        let mixed = mix_seed(u64::from_le_bytes(seed) ^ index as u64, key).to_le_bytes();
        chunk.copy_from_slice(&mixed);
    }
    ChaCha8Rng::from_seed(full_seed)
}

fn random_range_f32(rng: &mut ChaCha8Rng, min: f32, max: f32) -> f32 {
    let unit = rng.next_u64() as f64 / u64::MAX as f64;
    min + (max - min) * unit as f32
}

fn mix_seed(seed: u64, key: ChunkKey) -> u64 {
    seed ^ ((key.x as i64 as u64).wrapping_mul(0x9E37_79B1))
        ^ ((key.y as i64 as u64).wrapping_mul(0x85EB_CA77))
        ^ ((key.z as i64 as u64).wrapping_mul(0xC2B2_AE3D))
}

fn mix_world_pixel_seed(seed: u64, x: i32, y: i32) -> u64 {
    seed ^ ((x as i64 as u64).wrapping_mul(0x9E37_79B9))
        ^ ((y as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F))
}

#[cfg(test)]
mod tests {
    use super::{generate_chunk, WorldConfig};
    use crate::world::{ChunkKey, ChunkLocalPixel, ChunkState, WorldPixel};

    #[test]
    fn same_seed_and_chunk_key_produce_same_chunk() {
        let config = WorldConfig::default();
        let key = ChunkKey { x: 2, y: -3, z: 0 };

        let first = generate_chunk(&config, key);
        let second = generate_chunk(&config, key);

        assert_eq!(first, second);
    }

    #[test]
    fn generation_order_does_not_change_chunk_output() {
        let config = WorldConfig::default();
        let a_key = ChunkKey { x: -1, y: 4, z: 0 };
        let b_key = ChunkKey { x: 3, y: -2, z: 0 };

        let a_then_b = (generate_chunk(&config, a_key), generate_chunk(&config, b_key));
        let b_then_a = (generate_chunk(&config, b_key), generate_chunk(&config, a_key));

        assert_eq!(a_then_b.0, b_then_a.1);
        assert_eq!(a_then_b.1, b_then_a.0);
    }

    #[test]
    fn chunk_mutation_filters_removed_assets() {
        let config = WorldConfig::default();
        let key = ChunkKey::ORIGIN;
        let data = generate_chunk(&config, key);
        let removed = data.assets[0].id;
        let original_len = data.assets.len();

        let mut state = ChunkState::new(data);
        state.remove_object(removed);

        assert_eq!(state.visible_assets().count(), original_len - 1);
        assert!(state.visible_assets().all(|asset| asset.id != removed));
    }

    #[test]
    fn chunk_pixel_generation_is_stable_and_mutation_is_delta_backed() {
        let config = WorldConfig::default();
        let key = ChunkKey::ORIGIN;
        let data = generate_chunk(&config, key);
        let local = ChunkLocalPixel::new(3, 7).unwrap();

        let mut state = ChunkState::new(data.clone());
        let original = state.pixel(local);
        state.set_pixel(local, WorldPixel::Blood);

        assert_eq!(state.pixel(local), WorldPixel::Blood);
        assert_eq!(state.delta.pixel_overrides.len(), 1);

        state.set_pixel(local, original);

        assert_eq!(state.pixel(local), original);
        assert!(state.delta.pixel_overrides.is_empty());
        assert_eq!(generate_chunk(&config, key), data);
    }
}
