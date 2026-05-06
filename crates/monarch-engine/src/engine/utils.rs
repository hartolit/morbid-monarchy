use crate::engine::world::cell::CompassFlags;
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
        flags: u8,
        rng: &mut R,
    ) -> Self {
        let (mut dirs, count) = Self::get_base_pattern(pattern);

        dirs[0..count].shuffle(rng);

        let mut forward = IVec2::ZERO;
        if (flags & CompassFlags::FACING_N) != 0 {
            forward.y += 1;
        }
        if (flags & CompassFlags::FACING_S) != 0 {
            forward.y -= 1;
        }
        if (flags & CompassFlags::FACING_E) != 0 {
            forward.x += 1;
        }
        if (flags & CompassFlags::FACING_W) != 0 {
            forward.x -= 1;
        }

        if forward != IVec2::ZERO {
            if let Some(idx) = dirs[0..count].iter().position(|&d| d == forward) {
                dirs.swap(0, idx);
            }
        }

        Self { dirs, count }
    }

    #[inline(always)]
    pub fn new_deterministic(
        pattern: FlowPattern,
        pos: IVec2,
        tick: u32,
        material: u8,
        cadence: u32,
    ) -> Self {
        let mut h = spatial_hash_extended(pos, tick, material, cadence);
        let (mut dirs, count) = Self::get_base_pattern(pattern);

        for i in (1..count).rev() {
            let j = (h % (i as u32 + 1)) as usize;
            dirs.swap(i, j);
            h = h.wrapping_mul(0x9E37_79B1).wrapping_add(1);
        }

        Self { dirs, count }
    }

    #[inline(always)]
    pub fn new_deterministic_with_momentum(
        pattern: FlowPattern,
        pos: IVec2,
        tick: u32,
        material: u8,
        cadence: u32,
        flags: u8,
    ) -> Self {
        let mut h = spatial_hash_extended(pos, tick, material, cadence);
        let (mut dirs, count) = Self::get_base_pattern(pattern);

        for i in (1..count).rev() {
            let j = (h % (i as u32 + 1)) as usize;
            dirs.swap(i, j);
            h = h.wrapping_mul(0x9E37_79B1).wrapping_add(1);
        }

        let mut forward = IVec2::ZERO;
        if (flags & CompassFlags::FACING_N) != 0 {
            forward.y += 1;
        }
        if (flags & CompassFlags::FACING_S) != 0 {
            forward.y -= 1;
        }
        if (flags & CompassFlags::FACING_E) != 0 {
            forward.x += 1;
        }
        if (flags & CompassFlags::FACING_W) != 0 {
            forward.x -= 1;
        }

        if forward != IVec2::ZERO {
            if let Some(idx) = dirs[0..count].iter().position(|&d| d == forward) {
                dirs.swap(0, idx);
            }
        }

        Self { dirs, count }
    }

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
pub fn spatial_hash_extended(pos: IVec2, tick: u32, material: u8, cadence: u32) -> u32 {
    let mut h = spatial_hash(pos, tick)
        .wrapping_add((material as u32).wrapping_mul(0x27D4_EB2F))
        .wrapping_add(cadence.wrapping_mul(0x1656_67B1));
    h ^= h >> 13;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 16;
    h
}
