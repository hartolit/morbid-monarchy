use bevy::math::IVec2;
use rand::{Rng, seq::SliceRandom};

#[derive(Clone, Copy)]
pub enum FlowPattern {
    Omni,
    Cardinal,
}

pub struct ShuffledDirs {
    dirs: [IVec2; 8],
    count: usize,
}

impl ShuffledDirs {
    #[inline(always)]
    pub fn new<R: Rng + ?Sized>(pattern: FlowPattern, rng: &mut R) -> Self {
        let (mut dirs, count) = match pattern {
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
        };

        dirs[0..count].shuffle(rng);

        Self { dirs, count }
    }

    #[inline(always)]
    pub fn get(&self) -> &[IVec2] {
        &self.dirs[0..self.count]
    }
}
