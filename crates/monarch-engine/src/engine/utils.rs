use crate::{engine::world::cell::MaterialId, prelude::PixelFlags};
use bevy::math::IVec2;
use rand::{Rng, seq::SliceRandom};

#[derive(Clone, Copy)]
pub enum FlowPattern {
    Omni,     // All 8 directions (Liquids, Gases)
    Cardinal, // 4 directions (Magma, Slimes)
}

pub struct ShuffledDirs {
    dirs: [IVec2; 8],
    count: usize,
}

impl ShuffledDirs {
    /// Biases the shuffle to prioritize the cell's current facing/momentum flags.
    #[inline(always)]
    pub fn new_with_momentum<R: Rng + ?Sized>(
        pattern: FlowPattern,
        flags: PixelFlags,
        rng: &mut R,
    ) -> Self {
        let (mut dirs, count) = Self::get_base_pattern(pattern);

        // Do a standard shuffle
        dirs[0..count].shuffle(rng);

        // Extract preferred forward vector
        let mut forward = IVec2::ZERO;
        if flags.contains(PixelFlags::FACING_N) {
            forward.y += 1;
        }
        if flags.contains(PixelFlags::FACING_S) {
            forward.y -= 1;
        }
        if flags.contains(PixelFlags::FACING_E) {
            forward.x += 1;
        }
        if flags.contains(PixelFlags::FACING_W) {
            forward.x -= 1;
        }

        if forward != IVec2::ZERO {
            // Find our preferred forward vector in the shuffled list and swap it to index 0
            // This guarantees the cell will check its momentum path first.
            if let Some(idx) = dirs[0..count].iter().position(|&d| d == forward) {
                dirs.swap(0, idx);
            }
        }

        Self { dirs, count }
    }

    /// STRICT CONSENSUS: Generates a perfectly symmetric, stateless chaotic pattern.
    /// Use this when multiple cells need to evaluate the exact same boundary outcome.
    #[inline(always)]
    pub fn new_deterministic(
        pattern: FlowPattern,
        pos: IVec2,
        tick: u32,
        material: MaterialId,
        cadence: u32,
    ) -> Self {
        let mut h = spatial_hash_extended(pos, tick, material, cadence);
        let (mut dirs, count) = Self::get_base_pattern(pattern);

        // Deterministic Fisher-Yates shuffle
        for i in (1..count).rev() {
            let j = (h % (i as u32 + 1)) as usize;
            dirs.swap(i, j);
            // Strong avalanche mutation to guarantee true isotropic shuffling
            h = h.wrapping_mul(0x9E37_79B1).wrapping_add(1);
        }

        Self { dirs, count }
    }

    /// MAXIMUM PERFORMANCE: Uses a fast thread-local RNG.
    /// Use this for biology, falling powders, or gas diffusion where consensus doesn't matter.
    #[inline(always)]
    pub fn new_random<R: Rng + ?Sized>(pattern: FlowPattern, rng: &mut R) -> Self {
        let (mut dirs, count) = Self::get_base_pattern(pattern);
        dirs[0..count].shuffle(rng);
        Self { dirs, count }
    }

    #[inline(always)]
    fn get_base_pattern(pattern: FlowPattern) -> ([IVec2; 8], usize) {
        match pattern {
            FlowPattern::Omni => (
                [
                    IVec2::new(0, 1),
                    IVec2::new(1, 1),
                    IVec2::new(1, 0),
                    IVec2::new(1, -1),
                    IVec2::new(0, -1),
                    IVec2::new(-1, -1),
                    IVec2::new(-1, 0),
                    IVec2::new(-1, 1),
                ],
                8,
            ),
            FlowPattern::Cardinal => (
                [
                    IVec2::new(0, 1),
                    IVec2::new(1, 0),
                    IVec2::new(0, -1),
                    IVec2::new(-1, 0),
                    IVec2::ZERO,
                    IVec2::ZERO,
                    IVec2::ZERO,
                    IVec2::ZERO,
                ],
                4,
            ),
        }
    }

    #[inline(always)]
    pub fn get(&self) -> &[IVec2] {
        &self.dirs[0..self.count]
    }
}

/// Robust PCG-like spatial hash.
#[inline(always)]
pub fn spatial_hash(pos: IVec2, tick: u32) -> u32 {
    let mut h = (pos.x as u32).wrapping_mul(0x736A_153D)
        ^ (pos.y as u32).wrapping_mul(0x9E37_79B1)
        ^ tick.wrapping_mul(0x11C6_4E6D);
    h ^= h >> 16;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h
}

#[inline(always)]
pub fn spatial_hash_extended(pos: IVec2, tick: u32, material: MaterialId, cadence: u32) -> u32 {
    let mut h = spatial_hash(pos, tick)
        .wrapping_add((material.0 as u32).wrapping_mul(0x27D4_EB2F))
        .wrapping_add(cadence.wrapping_mul(0x1656_67B1));
    h ^= h >> 13;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 16;
    h
}
