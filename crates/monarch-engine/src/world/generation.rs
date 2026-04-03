use bevy::math::IVec2;

use crate::world::chunk::{MaterialId, PersistedChunk, Pixel, ThemeId, CHUNK_PIXELS, CHUNK_SIDE};

pub fn generate_chunk(world_chunk: IVec2) -> PersistedChunk {
    let theme = chunk_theme(world_chunk);
    let world_origin = world_chunk * CHUNK_SIDE as i32;
    let pixels = Box::new(std::array::from_fn(|index| {
        let local_x = (index % CHUNK_SIDE) as i32;
        let local_y = (index / CHUNK_SIDE) as i32;
        let world_pixel = world_origin + IVec2::new(local_x, local_y);
        generate_pixel(world_pixel, theme)
    }));

    PersistedChunk {
        theme,
        pixels,
        entities: Vec::new(),
    }
}

pub fn chunk_theme(world_chunk: IVec2) -> ThemeId {
    let world_center = world_chunk * CHUNK_SIDE as i32 + IVec2::splat((CHUNK_SIDE / 2) as i32);
    let moisture = octave_noise(world_center, 320.0, 4, 0.55, 0x91e1_0da5);
    let heat = octave_noise(world_center + IVec2::new(913, -271), 420.0, 3, 0.52, 0x7f4a_7c15);
    let water_bias = octave_noise(world_center + IVec2::new(-433, 811), 540.0, 4, 0.58, 0xd1b5_4a35);
    let ruggedness = octave_noise(world_center + IVec2::new(223, 337), 210.0, 4, 0.61, 0x51c3_2f91);

    if water_bias > 0.69 && ruggedness < 0.56 {
        ThemeId::OCEAN
    } else if heat > 0.66 && moisture < 0.42 {
        ThemeId::DESERT
    } else if ruggedness > 0.72 {
        ThemeId::CAVE
    } else {
        ThemeId::GRASS_PLAINS
    }
}

pub fn empty_chunk(theme: ThemeId) -> PersistedChunk {
    PersistedChunk {
        theme,
        pixels: Box::new([Pixel::EMPTY; CHUNK_PIXELS]),
        entities: Vec::new(),
    }
}

fn generate_pixel(world_pixel: IVec2, theme: ThemeId) -> Pixel {
    let moisture = octave_noise(world_pixel + IVec2::new(41, -103), 110.0, 4, 0.55, 0x2d4b_5a1f);
    let elevation = octave_noise(world_pixel + IVec2::new(-501, 617), 180.0, 4, 0.58, 0x6c8e_9cf5);
    let stone = octave_noise(world_pixel + IVec2::new(271, 199), 54.0, 3, 0.5, 0xa531_d497);
    let lakes = octave_noise(world_pixel + IVec2::new(-877, -121), 82.0, 4, 0.53, 0x44c2_a3ed);
    let detail = octave_noise(world_pixel + IVec2::new(1207, 733), 24.0, 2, 0.45, 0x11a7_0f2d);

    match theme {
        ThemeId::OCEAN => {
            if elevation > 0.64 && detail > 0.42 {
                Pixel::DIRT
            } else {
                Pixel::WATER
            }
        }
        ThemeId::DESERT => {
            if lakes > 0.83 && moisture > 0.47 {
                Pixel::WATER
            } else if stone > 0.72 || elevation > 0.78 {
                Pixel::ROCK
            } else {
                Pixel::DIRT
            }
        }
        ThemeId::CAVE => {
            if lakes > 0.8 && moisture > 0.51 {
                Pixel::WATER
            } else if stone > 0.49 || elevation > 0.6 {
                Pixel::new(MaterialId::ROCK, crate::world::chunk::PixelFlags::IS_SOLID)
            } else {
                Pixel::DIRT
            }
        }
        _ => {
            if lakes > 0.82 && moisture > 0.58 && elevation < 0.62 {
                Pixel::WATER
            } else if stone > 0.71 || elevation > 0.76 {
                Pixel::ROCK
            } else {
                Pixel::DIRT
            }
        }
    }
}

fn octave_noise(world_pixel: IVec2, base_scale: f32, octaves: usize, persistence: f32, seed: u32) -> f32 {
    let mut total = 0.0;
    let mut amplitude = 1.0;
    let mut amplitude_sum = 0.0;
    let mut scale = base_scale;

    for octave in 0..octaves {
        let octave_seed = seed.wrapping_add((octave as u32).wrapping_mul(0x9e37_79b9));
        total += value_noise(world_pixel, scale, octave_seed) * amplitude;
        amplitude_sum += amplitude;
        amplitude *= persistence;
        scale *= 0.5;
    }

    total / amplitude_sum
}

fn value_noise(world_pixel: IVec2, scale: f32, seed: u32) -> f32 {
    let scaled_x = world_pixel.x as f32 / scale;
    let scaled_y = world_pixel.y as f32 / scale;
    let base_x = scaled_x.floor() as i32;
    let base_y = scaled_y.floor() as i32;
    let frac_x = smoothstep(scaled_x - base_x as f32);
    let frac_y = smoothstep(scaled_y - base_y as f32);

    let v00 = hash_to_unit(base_x, base_y, seed);
    let v10 = hash_to_unit(base_x + 1, base_y, seed);
    let v01 = hash_to_unit(base_x, base_y + 1, seed);
    let v11 = hash_to_unit(base_x + 1, base_y + 1, seed);

    let top = lerp(v00, v10, frac_x);
    let bottom = lerp(v01, v11, frac_x);
    lerp(top, bottom, frac_y)
}

fn hash_to_unit(x: i32, y: i32, seed: u32) -> f32 {
    hash_2d(x, y, seed) as f32 / u32::MAX as f32
}

fn hash_2d(x: i32, y: i32, seed: u32) -> u32 {
    let mut value = (x as u32)
        .wrapping_mul(0x9e37_79b9)
        .wrapping_add((y as u32).wrapping_mul(0x85eb_ca6b))
        ^ seed;
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - (2.0 * t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_chunk_is_stable_for_origin() {
        let chunk = generate_chunk(IVec2::ZERO);
        assert_eq!(chunk.pixels.len(), CHUNK_PIXELS);
    }

    #[test]
    fn generate_chunk_handles_large_coordinates_without_panicking() {
        let world_chunk = IVec2::new(1_000_000, -1_000_000);
        let chunk = generate_chunk(world_chunk);
        assert_eq!(chunk.pixels.len(), CHUNK_PIXELS);
    }
}
