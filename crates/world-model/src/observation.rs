//! Results the physical world reports back.
//!
//! An observation is a copy of state at a completed tick. It is never written back as
//! authority: a gameplay decision that needs the current physical state belongs inside the
//! simulation, not in a loop that reads observations and issues corrections.

use glam::{DVec3, IVec2, Vec3};

use crate::{body::BodyHandle, cell::WorldCell, material::FluidMat};

/// Interaction counters a body accumulates for its whole life. They only grow (each wraps at
/// `u32::MAX`), so a consumer that remembers the last values it saw derives exactly what
/// happened since, however late or seldom it observes, and applies nothing twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BodyCounters {
    /// Substeps that ended grounded after one that did not.
    pub landings: u32,
    /// Substeps in which the body's submerged fraction rose from zero.
    pub liquid_entries: u32,
    /// Substeps that began touching another body after touching none.
    pub body_contacts: u32,
    /// Hard landings that cratered the ground.
    pub craters: u32,
    /// Plunges that threw a splash.
    pub splashes: u32,
    /// Kinetic energy absorbed by craters and splashes, in sixteenths of a solver energy unit.
    pub absorbed_energy: u32,
}

impl BodyCounters {
    /// What happened between `earlier` and `self`, with wrapping arithmetic.
    pub fn since(&self, earlier: &Self) -> Self {
        Self {
            landings: self.landings.wrapping_sub(earlier.landings),
            liquid_entries: self.liquid_entries.wrapping_sub(earlier.liquid_entries),
            body_contacts: self.body_contacts.wrapping_sub(earlier.body_contacts),
            craters: self.craters.wrapping_sub(earlier.craters),
            splashes: self.splashes.wrapping_sub(earlier.splashes),
            absorbed_energy: self.absorbed_energy.wrapping_sub(earlier.absorbed_energy),
        }
    }

    pub fn is_zero(&self) -> bool {
        *self == Self::default()
    }
}

/// One body's state after logical tick `tick`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodySample {
    pub body: BodyHandle,
    /// The completed tick this state belongs to. Its age is the current tick minus this.
    pub tick: u64,
    /// Centre in absolute render space (y up; cell `y` is render `-z`), world units.
    pub position: DVec3,
    /// World units per second, render axes.
    pub velocity: Vec3,
    /// Averaged ground contact normal while grounded; world up otherwise.
    pub ground_normal: Vec3,
    pub grounded: bool,
    /// Frozen at the edge of the resident world (see the GPU contract's residency policy).
    pub suspended: bool,
    /// Share of the volume below a liquid surface, from 0 to 1.
    pub submerged_fraction: f32,
    /// The liquid in the body's centre column, if any.
    pub liquid: Option<FluidMat>,
    /// Other bodies it touched in the tick's last substep.
    pub touching: u32,
    pub counters: BodyCounters,
}

impl BodySample {
    pub fn speed(&self) -> f32 {
        self.velocity.length()
    }
}

/// Where a ray met the physical surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SurfaceHit {
    /// The first surface triangle the ray crossed: the quad's lower-corner cell and the point.
    Hit {
        cell: IVec2,
        position: DVec3,
        distance: f32,
    },
    /// The ray ran its full length over resident columns without meeting the surface.
    Miss,
    /// The ray reached a column that is not resident below the highest possible surface, at
    /// `distance`: what lies there is unknown, so nothing is claimed about a hit.
    NotResident { distance: f32 },
}

impl SurfaceHit {
    pub fn cell(&self) -> Option<IVec2> {
        match self {
            Self::Hit { cell, .. } => Some(*cell),
            _ => None,
        }
    }
}

/// The nearest body a ray crossed, before its surface hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyHit {
    pub body: BodyHandle,
    pub position: DVec3,
    pub distance: f32,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_differences_survive_wrapping() {
        let earlier = BodyCounters {
            landings: u32::MAX,
            absorbed_energy: u32::MAX - 4,
            ..BodyCounters::default()
        };
        let later = BodyCounters {
            landings: 1,
            absorbed_energy: 3,
            craters: 2,
            ..BodyCounters::default()
        };
        let delta = later.since(&earlier);
        assert_eq!(delta.landings, 2);
        assert_eq!(delta.absorbed_energy, 8);
        assert_eq!(delta.craters, 2);
        assert!(later.since(&later).is_zero());
    }
}
