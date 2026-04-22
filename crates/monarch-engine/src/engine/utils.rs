use bevy::math::IVec2;
use rand::{rngs::ThreadRng, seq::SliceRandom};

#[derive(Clone, Copy)]
pub enum FlowPattern {
    Omni,     // All 8 directions (Liquids, Gases)
    Cardinal, // 4 directions (Slimes, slow oozes)
}

pub struct ShuffledDirs {
    dirs: [IVec2; 8],
    count: usize,
}

impl ShuffledDirs {
    #[inline(always)]
    pub fn new(pattern: FlowPattern, rng: &mut ThreadRng) -> Self {
        match pattern {
            FlowPattern::Omni => {
                let mut dirs = [
                    IVec2::new(0, 1),
                    IVec2::new(1, 1),
                    IVec2::new(1, 0),
                    IVec2::new(1, -1),
                    IVec2::new(0, -1),
                    IVec2::new(-1, -1),
                    IVec2::new(-1, 0),
                    IVec2::new(-1, 1),
                ];
                dirs.shuffle(rng);
                Self { dirs, count: 8 }
            }
            FlowPattern::Cardinal => {
                let mut dirs = [
                    IVec2::new(0, 1),
                    IVec2::new(1, 0),
                    IVec2::new(0, -1),
                    IVec2::new(-1, 0),
                    IVec2::ZERO,
                    IVec2::ZERO,
                    IVec2::ZERO,
                    IVec2::ZERO,
                ];
                dirs[0..4].shuffle(rng);
                Self { dirs, count: 4 }
            }
        }
    }

    #[inline(always)]
    pub fn get(&self) -> &[IVec2] {
        &self.dirs[0..self.count]
    }
}
