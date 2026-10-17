//! Results the physical world reports back.
//!
//! An observation is a copy of state at a completed tick. It is never written back as
//! authority: a gameplay decision that needs the current physical state belongs inside the
//! simulation, not in a loop that reads observations and issues corrections.

use crate::{body::BodyHandle, cell::WorldCell};

/// One body's state after logical tick `tick`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyObservation {
    pub body: BodyHandle,
    /// The completed tick this state belongs to. Its age is the current tick minus this.
    pub tick: u64,
    /// Centre in render space (y up; cell `y` is render `-z`), world units.
    pub position: [f32; 3],
    /// World units per second, render axes.
    pub velocity: [f32; 3],
    pub grounded: bool,
    /// Share of the volume below a liquid surface, from 0 to 1.
    pub submerged_fraction: f32,
}

impl BodyObservation {
    pub fn speed(&self) -> f32 {
        let [x, y, z] = self.velocity;
        (x * x + y * y + z * z).sqrt()
    }
}

/// Integer totals over a set of cells: what a `MeasureTotals` command reports for the resident
/// world. Every sum is exact; a radius-32 world of full columns holds about 8 × 10¹⁰ units, far
/// beyond 32 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorldTotals {
    /// Fluid volume by the column's fluid material code (`FluidMat` value 0–7). Index 0 counts
    /// volume a column holds without a material, which the rules never produce.
    pub fluid: [u64; 8],
    /// Bedrock elevation units.
    pub elevation: u64,
    /// Loose granular units.
    pub granular: u64,
    /// Cells summed.
    pub cells: u64,
}

impl WorldTotals {
    pub fn add(&mut self, cell: &WorldCell) {
        self.fluid[cell.fluid_mat().0 as usize] += u64::from(cell.fluid_vol());
        self.elevation += u64::from(cell.elevation());
        self.granular += u64::from(cell.granular_vol());
        self.cells += 1;
    }

    pub fn of<'a>(cells: impl IntoIterator<Item = &'a WorldCell>) -> Self {
        let mut totals = Self::default();
        for cell in cells {
            totals.add(cell);
        }
        totals
    }

    /// Every unit of liquid and ice: the conserved fluid quantity.
    pub fn fluid_volume(&self) -> u64 {
        self.fluid.iter().sum()
    }

    /// Bedrock plus loose material: the conserved solid quantity.
    pub fn solid(&self) -> u64 {
        self.elevation + self.granular
    }
}
