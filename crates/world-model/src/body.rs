//! Sphere bodies: identity, presets, control strengths and the supported physical envelope.
//!
//! Lengths are world units, speeds world units per second, accelerations world units per
//! second squared. Mass is in the solver's own unit; only ratios of masses and the rut depth's
//! square-root mass scaling use it, so it is not a calibrated kilogram.

use std::ops::RangeInclusive;

/// A body reference that cannot silently alias a later body: reusing a slot bumps the
/// generation, so a stale handle names no body instead of the wrong one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BodyHandle {
    index: u32,
    generation: u32,
}

impl BodyHandle {
    pub const fn new(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }

    pub const fn index(self) -> u32 {
        self.index
    }

    pub const fn generation(self) -> u32 {
        self.generation
    }
}

/// Physical description of a sphere the tools can spawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpherePreset {
    pub mass: f32,
    pub radius: f32,
    /// Density relative to water; a sphere floats in any liquid denser than this.
    pub relative_density: f32,
}

/// Steel: sinks in every liquid.
pub const METAL_SPHERE: SpherePreset = SpherePreset {
    mass: 4.0,
    radius: 4.0,
    relative_density: 7.8,
};

/// The metal sphere's volume with wood's density, so its mass scales by 0.6 / 7.8. Floats.
pub const WOOD_SPHERE: SpherePreset = SpherePreset {
    mass: 0.31,
    radius: 4.0,
    relative_density: 0.6,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyPreset {
    Metal,
    Wood,
}

impl BodyPreset {
    pub const fn sphere(self) -> SpherePreset {
        match self {
            Self::Metal => METAL_SPHERE,
            Self::Wood => WOOD_SPHERE,
        }
    }
}

/// Exponential velocity decay rate in air, per second: `v *= exp(-rate * dt)`, which gives the
/// same trajectory however time is subdivided. It bounds every unpowered speed.
pub const AIR_DRAG_RATE: f32 = 0.6;

/// Driving accelerates a body toward the drive direction, up to [`DRIVE_MAX_SPEED`] along it;
/// rolling physics, not the controls, decide how it coasts beyond that.
pub const DRIVE_ACCELERATION: f32 = 250.0;
pub const DRIVE_MAX_SPEED: f32 = 40.0;
/// Speed added along the ground normal when a grounded body jumps.
pub const JUMP_IMPULSE: f32 = 35.0;
/// Acceleration of every body toward the attract target.
pub const ATTRACT_ACCELERATION: f32 = 350.0;
/// Upward acceleration of bodies within [`LIFT_INFLUENCE_RADIUS`] of the lift target
/// (horizontally), and their pull toward its column.
pub const LIFT_ACCELERATION: f32 = 250.0;
pub const LIFT_CENTERING_ACCELERATION: f32 = 40.0;
pub const LIFT_INFLUENCE_RADIUS: f32 = 150.0;

/// Supported radius. The tools spawn radius 4. Below 2 a body spans so few cell vertices that
/// contact sampling becomes coarse; above 8 the deformation and splash footprint (up to 2.75
/// radii, 22 cells) outgrows what the GPU body owner sizes its bins for.
pub const BODY_RADIUS: RangeInclusive<f32> = 2.0..=8.0;

/// Supported relative density. 0.01 is the solver's floor for buoyancy ratios; 25 exceeds the
/// densest metals, so no realistic material is refused.
pub const BODY_RELATIVE_DENSITY: RangeInclusive<f32> = 0.01..=25.0;

/// Supported mass, in the solver's unit.
pub const BODY_MASS: RangeInclusive<f32> = 0.01..=1000.0;

/// Highest speed the body solver must handle without tunnelling. Attract has no speed cap:
/// against air drag of 0.6 per second its terminal speed is 350 / 0.6 ≈ 583 world units per
/// second. Gravity alone reaches only 32 / 0.6 ≈ 53. At 1/120 s per substep a body at this
/// speed moves 5 units, more than the smallest radius, so prompt 04 must subdivide or sweep
/// fast bodies rather than rely on the default substep.
pub const MAX_BODY_SPEED: f32 = 600.0;

/// Most bodies the world supports at once (the capacity profile).
pub const MAX_BODIES: u32 = 4096;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_lie_inside_the_supported_envelope() {
        for preset in [METAL_SPHERE, WOOD_SPHERE] {
            assert!(BODY_RADIUS.contains(&preset.radius));
            assert!(BODY_RELATIVE_DENSITY.contains(&preset.relative_density));
            assert!(BODY_MASS.contains(&preset.mass));
        }
        // Wood keeps the metal sphere's volume.
        let ratio = WOOD_SPHERE.mass / METAL_SPHERE.mass;
        let expected = WOOD_SPHERE.relative_density / METAL_SPHERE.relative_density;
        assert!((ratio - expected).abs() < 0.001);
    }

    #[test]
    fn attract_terminal_speed_stays_inside_the_speed_limit() {
        // Evaluated by the compiler: a changed constant that breaks the envelope fails the build.
        const {
            assert!(ATTRACT_ACCELERATION / AIR_DRAG_RATE <= MAX_BODY_SPEED);
            assert!(DRIVE_MAX_SPEED + JUMP_IMPULSE <= MAX_BODY_SPEED);
        }
    }
}
