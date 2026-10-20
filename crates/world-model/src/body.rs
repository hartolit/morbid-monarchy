//! Sphere bodies: identity, descriptions, presets, control strengths, the solver's tuning and the
//! supported physical envelope.
//!
//! Lengths are world units, speeds world units per second, accelerations world units per
//! second squared. Mass is in the solver's own unit; only ratios of masses and the rut depth's
//! square-root mass scaling use it, so it is not a calibrated kilogram. The wood and metal
//! presets are stylized: their relative densities decide whether they float, not a calibrated
//! material model.
//!
//! [`tuning`] holds every constant of the contact, deformation and displacement rules. The GPU
//! body kernels (`gpu-world`, through generated declarations) read it, as the CPU baseline
//! (`monarch-engine`, removed in 06) did; nothing keeps its own copy.

use std::{fmt, ops::RangeInclusive};

use crate::units::MAX_BODY_SUBSTEP_SECONDS;

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

    pub fn description(self) -> SphereDescription {
        let preset = self.sphere();
        SphereDescription {
            mass: preset.mass,
            radius: preset.radius,
            relative_density: preset.relative_density,
        }
    }
}

/// A sphere inside the supported envelope. Values outside it cannot be constructed, so a
/// spawn the world receives is one it can simulate without tunnelling or overflowing a bin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SphereDescription {
    mass: f32,
    radius: f32,
    relative_density: f32,
}

impl SphereDescription {
    pub fn new(mass: f32, radius: f32, relative_density: f32) -> Result<Self, BodyError> {
        let inside =
            |value: f32, range: &RangeInclusive<f32>| value.is_finite() && range.contains(&value);
        if !inside(mass, &BODY_MASS) {
            return Err(BodyError::Mass(mass));
        }
        if !inside(radius, &BODY_RADIUS) {
            return Err(BodyError::Radius(radius));
        }
        if !inside(relative_density, &BODY_RELATIVE_DENSITY) {
            return Err(BodyError::RelativeDensity(relative_density));
        }
        Ok(Self {
            mass,
            radius,
            relative_density,
        })
    }

    pub fn mass(&self) -> f32 {
        self.mass
    }

    pub fn radius(&self) -> f32 {
        self.radius
    }

    pub fn relative_density(&self) -> f32 {
        self.relative_density
    }

    /// The preset this description equals, if any.
    pub fn preset(&self) -> Option<BodyPreset> {
        [BodyPreset::Metal, BodyPreset::Wood]
            .into_iter()
            .find(|preset| preset.description() == *self)
    }
}

/// Why a body description or control could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BodyError {
    Mass(f32),
    Radius(f32),
    RelativeDensity(f32),
    Acceleration(f32),
    Speed(f32),
}

impl fmt::Display for BodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mass(value) => write!(f, "mass {value} is outside {BODY_MASS:?}"),
            Self::Radius(value) => write!(f, "radius {value} is outside {BODY_RADIUS:?}"),
            Self::RelativeDensity(value) => write!(
                f,
                "relative density {value} is outside {BODY_RELATIVE_DENSITY:?}"
            ),
            Self::Acceleration(value) => write!(
                f,
                "drive acceleration {value} is outside (0, {DRIVE_ACCELERATION}]"
            ),
            Self::Speed(value) => {
                write!(f, "drive speed {value} is outside (0, {DRIVE_MAX_SPEED}]")
            }
        }
    }
}

impl std::error::Error for BodyError {}

/// Exponential velocity decay rate in air, per second: `v *= exp(-rate * dt)`, which gives the
/// same trajectory however time is subdivided. It bounds every unpowered speed.
pub const AIR_DRAG_RATE: f32 = 0.6;

/// Driving accelerates a body toward the drive direction, up to [`DRIVE_MAX_SPEED`] along it;
/// rolling physics, not the controls, decide how it coasts beyond that. A drive may ask for
/// less, never more.
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
/// Below this squared distance a body has no direction to its attract or lift target.
pub const TARGET_EPSILON_SQUARED: f32 = 0.0001;

/// Supported radius. The tools spawn radius 4. Below 2 a body spans so few cell vertices that
/// contact sampling becomes coarse; above 8 the deformation and splash footprint (up to 2.75
/// radii, 22 cells) outgrows what the GPU body owner sizes its claim boxes for.
pub const BODY_RADIUS: RangeInclusive<f32> = 2.0..=8.0;

/// Supported relative density. 0.01 is the solver's floor for buoyancy ratios; 25 exceeds the
/// densest metals, so no realistic material is refused.
pub const BODY_RELATIVE_DENSITY: RangeInclusive<f32> = 0.01..=25.0;

/// Supported mass, in the solver's unit.
pub const BODY_MASS: RangeInclusive<f32> = 0.01..=1000.0;

/// Highest speed the body solver handles without tunnelling. Attract has no speed cap:
/// against air drag of 0.6 per second its terminal speed is 350 / 0.6 ≈ 583 world units per
/// second, and attract and lift together could exceed this. The GPU solver caps every body's
/// speed here and counts each capped body substep, so the travel bound below always holds.
pub const MAX_BODY_SPEED: f32 = 600.0;

/// The farthest a body moves in one substep: 5 world units, more than the smallest radius.
/// The GPU solver therefore sweeps fast bodies: against terrain in steps of at most
/// [`TERRAIN_SWEEP_STEP`], against other bodies along their swept segments.
pub const MAX_SUBSTEP_TRAVEL: f32 = MAX_BODY_SPEED * MAX_BODY_SUBSTEP_SECONDS;

/// Longest step of a terrain sweep: half the smallest radius, so a sphere cannot skip a cell
/// vertex or a one-cell wall between two steps.
pub const TERRAIN_SWEEP_STEP: f32 = 1.0;

/// Most bodies the world supports at once (the capacity profile).
pub const MAX_BODIES: u32 = 4096;

/// Most substeps one tick runs: 12 at the slowest supported tick rate of 10 Hz.
pub const MAX_SUBSTEPS_PER_TICK: u32 = 12;

/// The fewest equal substeps no longer than [`MAX_BODY_SUBSTEP_SECONDS`] that make up one tick
/// of `tick_rate_hz`: 2 at 60 Hz, 12 at 10 Hz, 1 at 144 and 240 Hz.
pub fn substeps_per_tick(tick_rate_hz: f64) -> u32 {
    let ratio = 1.0 / (tick_rate_hz * f64::from(MAX_BODY_SUBSTEP_SECONDS));
    // A tick that is a whole number of substeps must not round up to one more.
    ((ratio - 1e-9).ceil() as u32).clamp(1, MAX_SUBSTEPS_PER_TICK)
}

/// Horizontal reach, in cells from the centre, of everything a body of `radius` reads or
/// changes in one substep: its splash ring (the widest footprint), the cell rounding of its
/// footprint box, and `travel`. A body whose reach includes a cell that is not resident is
/// suspended (the residency-edge policy in the GPU contract).
pub fn footprint_reach(radius: f32, travel: f32) -> f32 {
    radius * tuning::SPLASH_RING_OUTER_RATIO + 2.0 + travel
}

/// A unit of material a body moves, split across slots in proportion to their capacities.
///
/// Slot `i` receives `floor(total * C_i / T) - floor(total * C_{i-1} / T)`, where `C_i` is the
/// capacity of slots `0..=i` and `T` the capacity of all slots. The shares telescope to exactly
/// `total` and none exceeds its slot's capacity (`total <= T`), and each share depends only on
/// two prefix sums, so a GPU workgroup computes them in parallel with one scan. This replaces
/// the baseline's largest-remainder rounding, which needs a global ranking of remainders; both
/// are exact and capacity-respecting. Callers keep `total` at or below the combined capacity.
pub fn apportion(total: u32, capacities: &[u32]) -> Vec<u32> {
    let capacity: u32 = capacities.iter().sum();
    if total == 0 || capacity == 0 {
        return vec![0; capacities.len()];
    }
    let total = total.min(capacity);
    let mut before = 0u32;
    let mut cumulative = 0u32;
    capacities
        .iter()
        .map(|slot| {
            cumulative += slot;
            let through = mul_div_floor(total, cumulative, capacity);
            let share = through - before;
            before = through;
            share
        })
        .collect()
}

/// `floor(a * b / c)` for `a, b <= c < 2^21`, computed with 32-bit products only, as the GPU
/// kernels compute it: `a = high * 2^10 + low` keeps every intermediate below `2^32`.
pub fn mul_div_floor(a: u32, b: u32, c: u32) -> u32 {
    debug_assert!(a <= c && b <= c && c < 1 << 21);
    let high = a >> 10;
    let low = a & 0x3FF;
    let x = high * b;
    let (q1, r1) = (x / c, x % c);
    q1 * 1024 + (r1 * 1024 + low * b) / c
}

/// The constants of the sphere solver: contact, ground yield, deformation and displacement.
///
/// The two profile structs hold the per-body tuning the baseline spawns every sphere with;
/// the presets differ only in mass, radius and density.
pub mod tuning {
    /// Position passes per substep. Each pass pushes the sphere out of its deepest overlap; a
    /// few passes settle creases where several triangles overlap it at once.
    pub const CONTACT_ITERATIONS: usize = 4;
    /// Sweeps over the contact normals when removing velocity that points into the ground.
    pub const VELOCITY_ITERATIONS: usize = 2;
    /// Gap beyond the radius that still counts as touching, so resting contact survives the sag
    /// gravity adds between substeps.
    pub const CONTACT_SLOP: f32 = 0.05;
    /// Contacts whose normal rises at least this steeply (60 degrees from vertical) are ground;
    /// anything steeper is a wall that blocks but neither supports nor gets cratered.
    pub const GROUND_NORMAL_MIN_Y: f32 = 0.5;
    /// Distinct normals kept per contact manifold; near-coplanar triangles share an entry.
    pub const MAX_MANIFOLD_CONTACTS: usize = 16;
    pub const DUPLICATE_NORMAL_COSINE: f32 = 0.999;
    /// A contact whose direction lies within about ten degrees of its triangle's face normal
    /// takes the face normal. Neighbouring triangles of a smooth surface otherwise report
    /// contacts on their shared edges with slightly tilted normals ("internal edge" ghosts),
    /// and at speed each of those turns horizontal velocity into a hop.
    pub const INTERNAL_EDGE_COSINE: f32 = 0.985;
    /// Contacts within this distance of touching constrain velocity; looser ones only count
    /// towards being grounded.
    pub const CONSTRAINT_TOLERANCE: f32 = 0.002;
    /// Contacts this close to the deepest one share in the ground normal, so a crease between
    /// two faces averages them while distant near-misses are ignored.
    pub const GROUND_NORMAL_BAND: f32 = 0.01;
    /// Below this separation the closest-point direction is unreliable; use the face normal.
    pub const DEGENERATE_DISTANCE: f32 = 1e-5;
    /// Below this centre distance two spheres have no separating direction; the lower body
    /// index is pushed along +x.
    pub const COINCIDENT_DISTANCE: f32 = 0.000001;

    /// Sampling pattern for the confinement that makes deeper craters progressively harder.
    pub const RESISTANCE_SAMPLE_RINGS: usize = 2;
    pub const RESISTANCE_SAMPLE_STRIDE: i32 = 10;
    pub const RESISTANCE_CLIFF_THRESHOLD: f32 = 16.0;
    pub const RESISTANCE_MULTIPLIER: f32 = 1.0;

    /// Slowest downward speed at water entry that throws a splash.
    pub const SPLASH_MIN_SPEED: f32 = 4.0;
    /// Liquid units thrown per unit of entry energy, and the energy each displaced unit costs.
    pub const SPLASH_UNITS_PER_ENERGY: f32 = 0.3;
    pub const SPLASH_ENERGY_PER_UNIT: f32 = 1.0;
    /// The cavity is emptied out to this many radii; the crown lands between the ring radii.
    pub const SPLASH_CAVITY_RATIO: f32 = 1.25;
    pub const SPLASH_RING_OUTER_RATIO: f32 = 2.75;
    pub const SPLASH_MAX_PER_CELL: u16 = 12;
    /// Slowest horizontal speed at which a sphere breaking the surface pushes a bow wave.
    pub const WAKE_MIN_SPEED: f32 = 4.0;
    /// Depth of the surface layer (elevation units) a waterline sweeps aside per world unit
    /// travelled; the waterline's chord and slope scale it.
    pub const WAKE_LAYER_UNITS: f32 = 1.0;
    pub const WAKE_MAX_PER_CELL: u16 = 1;
    /// Width of the ring beside the waterline that receives the bow wave, in cells.
    pub const WAKE_RING_WIDTH: f32 = 3.0;
    pub const WAKE_MAX_RING_PER_CELL: u16 = 2;
    /// Fractional wake volume carried between substeps is capped so a blocked wake cannot bank
    /// a surge for later.
    pub const WAKE_MAX_PENDING: f32 = 4.0;

    /// Trail depth is measured below a plane fitted to the ground on a ring this many radii
    /// out, which lies on undisturbed ground around the sphere and follows slopes.
    pub const RUT_REFERENCE_RING_RATIO: f32 = 1.75;
    pub const RUT_REFERENCE_SAMPLES: usize = 12;
    /// Fewest ring samples a plane may be fitted to.
    pub const RUT_MIN_REFERENCE_SAMPLES: usize = 6;
    /// Ring samples further than this (world units) from the first fit lie in a trail or on a
    /// berm, not on undisturbed ground, and are dropped before refitting.
    pub const RUT_TRIM_DEPTH: f32 = 0.5;
    /// Time constant over which the reference follows newly fitted ground. Filtering keeps the
    /// yield floor from twitching as the sample ring sweeps over small features.
    pub const RUT_REFERENCE_SMOOTHING_SECONDS: f32 = 0.1;
    /// Sinkage never exceeds this share of the radius, keeping the centre above the true floor.
    pub const MAX_SINKAGE_RATIO: f32 = 0.5;
    pub const PLANE_SINGULARITY: f32 = 1e-6;
    /// Berms of shoved loose material form in a band just outside the sphere's footprint (in
    /// radii), up to a height above the undisturbed ground (in radii), and only beside the
    /// trail while the sphere is moving. Inside the footprint the sphere would ride up its own
    /// berm.
    pub const RUT_BERM_INNER_RATIO: f32 = 1.0;
    pub const RUT_BERM_OUTER_RATIO: f32 = 1.5;
    pub const RUT_BERM_HEIGHT_RATIO: f32 = 0.3;
    pub const RUT_BERM_SIDE_COSINE: f32 = 0.5;
    /// Below this speed a sphere has no heading and its berm forms all around it.
    pub const RUT_HEADING_MIN_SPEED: f32 = 1.0;

    /// Fraction of an elevation unit by which a height comparison may miss its target, so
    /// float noise neither strands a unit just above a limit nor removes one just below it.
    pub const QUANTIZATION_TOLERANCE: f32 = 0.1;

    /// How a body moves: drag, rolling and bouncing.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct KinematicProfile {
        /// Exponential velocity decay rate (1/s) in air: `v *= exp(-rate * dt)`, which gives the
        /// same trajectory however a step is subdivided.
        pub air_drag_rate: f32,
        /// Coulomb rolling resistance: while grounded the sphere loses speed at a constant
        /// `coefficient * g * n.y`, so it coasts realistically and rolls down slopes steeper
        /// than the coefficient.
        pub rolling_resistance_coefficient: f32,
        /// Additional exponential decay rate (1/s) of tangential velocity while grounded,
        /// standing in for drag from grass and loose soil.
        pub ground_damping_rate: f32,
        /// Steepest slope (as rise over run) on which a slow sphere is held in place.
        pub static_friction_coefficient: f32,
        /// Tangential speed below which static friction may lock a grounded sphere.
        pub static_friction_speed: f32,
        pub impact_restitution: f32,
    }

    pub const KINEMATIC: KinematicProfile = KinematicProfile {
        air_drag_rate: super::AIR_DRAG_RATE,
        rolling_resistance_coefficient: 0.12,
        ground_damping_rate: 0.4,
        static_friction_coefficient: 0.2,
        static_friction_speed: 0.5,
        impact_restitution: 0.20,
    };

    /// How a body deforms the ground: craters on hard landings, trails while rolling.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct DeformationProfile {
        pub min_deformation_energy: f32,
        pub energy_to_deformation_scale: f32,
        pub cost_displace_granular: f32,
        pub cost_crush_terrain: f32,
        /// Trail depth (world units) pressed into ground of softness 1 by a body of
        /// `rut_reference_mass`. Heavier bodies sink deeper with the square root of their mass.
        pub rut_depth_scale: f32,
        pub rut_reference_mass: f32,
        /// Fraction of the radius that bears on the ground and forms the trail.
        pub rut_width_ratio: f32,
        /// Most a single substep presses one column, in elevation units, so sinking is gradual.
        pub rut_press_per_substep: u16,
        /// Kinetic energy spent per elevation unit of ground pressed or shoved aside while
        /// rolling.
        pub rut_cost_per_unit: f32,
    }

    pub const DEFORMATION: DeformationProfile = DeformationProfile {
        min_deformation_energy: 250.0,
        energy_to_deformation_scale: 0.001,
        cost_displace_granular: 2.0,
        cost_crush_terrain: 3.0,
        rut_depth_scale: 1.0,
        rut_reference_mass: 4.0,
        rut_width_ratio: 0.7,
        rut_press_per_substep: 1,
        rut_cost_per_unit: 1.0,
    };

    /// Crater geometry and contact response of every sphere.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct CraterProfile {
        pub rim_inner_radius_ratio: f32,
        pub rim_expansion_factor: f32,
        pub blade_ceiling_ratio: f32,
        pub max_rim_deposit_per_cell: u16,
        pub max_deformation_size_ratio: f32,
        pub grounding_drift_buffer: f32,
        /// Impacts slower than this are absorbed without rebound, so rolling over uneven
        /// ground does not turn every facet into a hop.
        pub minimum_bounce_speed: f32,
    }

    pub const CRATER: CraterProfile = CraterProfile {
        rim_inner_radius_ratio: 0.9,
        rim_expansion_factor: 1.5,
        blade_ceiling_ratio: 0.10,
        max_rim_deposit_per_cell: 3,
        max_deformation_size_ratio: 1.0,
        grounding_drift_buffer: 0.01,
        minimum_bounce_speed: 6.0,
    };
}

/// Fraction of a sphere's volume below a plane `depth` units above its lowest point.
///
/// Uses the spherical cap volume `pi * h^2 * (3r - h) / 3` divided by `4/3 * pi * r^3`, so the
/// buoyant force grows smoothly from zero at first contact to full displacement.
#[inline(always)]
pub fn submerged_volume_fraction(depth: f32, radius: f32) -> f32 {
    if radius <= 0.0 {
        return 0.0;
    }
    let cap_height = depth.clamp(0.0, 2.0 * radius);
    cap_height * cap_height * (3.0 * radius - cap_height) / (4.0 * radius * radius * radius)
}

/// A drive: accelerate along `direction` (cell axes) by `acceleration` for one tick, up to
/// `max_speed` along it. The controls drive at [`DRIVE_ACCELERATION`] up to
/// [`DRIVE_MAX_SPEED`]; scripted drives may ask for less.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriveStrength {
    acceleration: f32,
    max_speed: f32,
}

impl DriveStrength {
    pub const CONTROLS: Self = Self {
        acceleration: DRIVE_ACCELERATION,
        max_speed: DRIVE_MAX_SPEED,
    };

    pub fn new(acceleration: f32, max_speed: f32) -> Result<Self, BodyError> {
        if !(acceleration.is_finite() && acceleration > 0.0 && acceleration <= DRIVE_ACCELERATION) {
            return Err(BodyError::Acceleration(acceleration));
        }
        if !(max_speed.is_finite() && max_speed > 0.0 && max_speed <= DRIVE_MAX_SPEED) {
            return Err(BodyError::Speed(max_speed));
        }
        Ok(Self {
            acceleration,
            max_speed,
        })
    }

    pub fn acceleration(&self) -> f32 {
        self.acceleration
    }

    pub fn max_speed(&self) -> f32 {
        self.max_speed
    }
}

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
        for preset in [BodyPreset::Metal, BodyPreset::Wood] {
            assert_eq!(preset.description().preset(), Some(preset));
        }
    }

    #[test]
    fn attract_terminal_speed_stays_inside_the_speed_limit() {
        // Evaluated by the compiler: a changed constant that breaks the envelope fails the build.
        const {
            assert!(ATTRACT_ACCELERATION / AIR_DRAG_RATE <= MAX_BODY_SPEED);
            assert!(DRIVE_MAX_SPEED + JUMP_IMPULSE <= MAX_BODY_SPEED);
            // A terrain sweep step cannot skip a cell vertex of the smallest sphere.
            assert!(TERRAIN_SWEEP_STEP <= *BODY_RADIUS.start() / 2.0);
        }
    }

    #[test]
    fn descriptions_and_drives_outside_the_envelope_are_refused() {
        assert!(SphereDescription::new(4.0, 4.0, 7.8).is_ok());
        assert_eq!(
            SphereDescription::new(4.0, 1.9, 7.8),
            Err(BodyError::Radius(1.9))
        );
        assert_eq!(
            SphereDescription::new(4.0, 8.5, 7.8),
            Err(BodyError::Radius(8.5))
        );
        assert!(SphereDescription::new(f32::NAN, 4.0, 1.0).is_err());
        assert!(SphereDescription::new(0.001, 4.0, 1.0).is_err());
        assert!(SphereDescription::new(4.0, 4.0, 26.0).is_err());
        assert!(SphereDescription::new(4.0, 4.0, 0.0).is_err());
        assert!(DriveStrength::new(125.0, 8.0).is_ok());
        assert!(DriveStrength::new(0.0, 8.0).is_err());
        assert!(DriveStrength::new(251.0, 8.0).is_err());
        assert!(DriveStrength::new(100.0, 41.0).is_err());
        assert!(DriveStrength::new(100.0, f32::INFINITY).is_err());
    }

    #[test]
    fn a_tick_is_split_into_the_fewest_substeps_of_at_most_a_120th() {
        for (rate, substeps) in [
            (60.0, 2),
            (10.0, 12),
            (240.0, 1),
            (144.0, 1),
            (120.0, 1),
            (50.0, 3),
            (30.0, 4),
        ] {
            assert_eq!(substeps_per_tick(rate), substeps, "{rate} Hz");
            let substep = 1.0 / (rate * f64::from(substeps));
            assert!(substep <= f64::from(MAX_BODY_SUBSTEP_SECONDS) + 1e-12);
        }
        assert!((MAX_SUBSTEP_TRAVEL - 5.0).abs() < 1e-5);
    }

    #[test]
    fn apportioning_is_exact_and_respects_every_capacity() {
        // The baseline's capacities (grid_physics' apportion test) and independent cases.
        let cases: [&[u32]; 4] = [&[3, 1, 7, 0, 2, 5], &[511; 9], &[0, 0, 4], &[1]];
        for capacities in cases {
            let total: u32 = capacities.iter().sum();
            for moved in 0..=total + 3 {
                let shares = apportion(moved, capacities);
                assert_eq!(shares.iter().sum::<u32>(), moved.min(total));
                for (share, capacity) in shares.iter().zip(capacities) {
                    assert!(share <= capacity, "{shares:?} over {capacities:?}");
                }
            }
        }
        // Worked by hand: 9 of 18 units over [3, 1, 7, 0, 2, 5] at cumulative 3, 4, 11, 11, 13, 18
        // gives floor(9 * C / 18) = 1, 2, 5, 5, 6, 9.
        assert_eq!(apportion(9, &[3, 1, 7, 0, 2, 5]), [1, 1, 3, 0, 1, 3]);
    }

    #[test]
    fn the_split_multiply_divide_is_exact_at_its_limits() {
        let limit = (1 << 21) - 1;
        for (a, b, c) in [
            (limit, limit, limit),
            (limit - 1, limit, limit),
            (1_034_775, 517_000, 1_034_775),
            (12_345, 999_999, 1_000_003),
            (0, 7, 9),
            (1023, 1025, 2048),
        ] {
            let exact = (u64::from(a) * u64::from(b) / u64::from(c)) as u32;
            assert_eq!(mul_div_floor(a, b, c), exact, "{a} * {b} / {c}");
        }
    }

    #[test]
    fn submerged_fraction_follows_the_cap_volume() {
        // Independent of the baseline's own test: the cap formula at a quarter and three
        // quarters of the diameter, and symmetry about the equator.
        let radius = 2.5;
        let quarter = submerged_volume_fraction(radius * 0.5, radius);
        assert!((quarter - 0.156_25).abs() < 1e-6, "{quarter}");
        let three_quarters = submerged_volume_fraction(radius * 1.5, radius);
        assert!((quarter + three_quarters - 1.0).abs() < 1e-6);
        assert_eq!(submerged_volume_fraction(-1.0, radius), 0.0);
        assert_eq!(submerged_volume_fraction(1.0, 0.0), 0.0);
    }

    #[test]
    fn the_reach_covers_the_splash_ring_and_the_travel() {
        assert_eq!(footprint_reach(4.0, 0.0), 13.0);
        assert_eq!(footprint_reach(8.0, MAX_SUBSTEP_TRAVEL), 29.0);
        // The largest reach box fits in two chunks per axis.
        assert!(2.0 * footprint_reach(8.0, MAX_SUBSTEP_TRAVEL) < 64.0);
    }
}
