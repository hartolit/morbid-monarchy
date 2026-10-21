//! What a saved world's values mean: the identities a store records and checks.
//!
//! A saved world is meaningful only under the cell layout it was packed with, the physics it was
//! simulated with and the generator its untouched chunks come from. A store records these in its
//! header and refuses a world whose identity differs from this build's, since no conversion
//! exists yet: a different layout reads other fields out of the same bits, and different rules
//! or body tuning make it a different physical world, not the same one restyled.
//!
//! Each identity is a revision, named in the docs when it changes, and a fingerprint computed
//! from the values themselves. The fingerprints' tests pin their current value, so changing a
//! listed constant without raising its revision fails a test instead of silently reinterpreting
//! saved worlds. The fingerprints cover the constants listed here; a change to a rule's code
//! still needs its revision raised by hand.

use crate::{body, cell::WorldCell, rules, units};

/// Revision of the packed cell format (`cell::WorldCell::LAYOUT`).
pub const CELL_FORMAT_REVISION: u32 = 1;

/// Revision of the material rules and body physics: `rules`, `body` and `units`.
pub const PHYSICS_REVISION: u32 = 2;

/// Revision of `generation::WorldGenerator`'s terrain: chunks that were never saved are
/// regenerated from the seed, so a world can only reopen with the generator it was created with.
pub const GENERATOR_REVISION: u32 = 1;

/// FNV-1a, 64 bits: stable, dependency-free and adequate for detecting a changed table.
struct Fingerprint(u64);

impl Fingerprint {
    const fn new() -> Self {
        Self(0xCBF2_9CE4_8422_2325)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }

    fn name(&mut self, name: &str) {
        self.bytes(name.as_bytes());
        self.bytes(&[0]);
    }

    fn word(&mut self, name: &str, value: u64) {
        self.name(name);
        self.bytes(&value.to_le_bytes());
    }

    fn real(&mut self, name: &str, value: f32) {
        self.word(name, u64::from(value.to_bits()));
    }
}

/// The cell layout's fingerprint: every field's name, shift and width.
pub fn cell_layout_fingerprint() -> u64 {
    let mut print = Fingerprint::new();
    print.word("cell.format", u64::from(CELL_FORMAT_REVISION));
    for field in WorldCell::LAYOUT {
        print.name(field.name);
        print.word("shift", u64::from(field.shift));
        print.word("bits", u64::from(field.bits));
    }
    print.0
}

/// The physics' fingerprint: the rule constants and the body tuning a resumed world must share.
pub fn physics_fingerprint() -> u64 {
    let mut p = Fingerprint::new();
    p.word("physics", u64::from(PHYSICS_REVISION));
    p.real("GRAVITY_Y", units::GRAVITY_Y);
    p.real("MAX_BODY_SUBSTEP_SECONDS", units::MAX_BODY_SUBSTEP_SECONDS);

    p.word("GRANULAR_TICK_INTERVAL", rules::GRANULAR_TICK_INTERVAL);
    for (name, value) in [
        ("BIOLOGY_ODDS_DENOMINATOR", rules::BIOLOGY_ODDS_DENOMINATOR),
        ("FIRE_ODDS_DENOMINATOR", rules::FIRE_ODDS_DENOMINATOR),
        ("MAX_GRANULAR_TRANSFER", rules::MAX_GRANULAR_TRANSFER),
        ("MIN_TRANSFER_HEAD", rules::MIN_TRANSFER_HEAD),
        ("STICTION_VARIANTS", rules::STICTION_VARIANTS),
        ("EROSION_MASK", rules::EROSION_MASK),
        ("CHANCE_RESOLUTION", rules::CHANCE_RESOLUTION),
        ("PHASE_HASH_CADENCE", rules::PHASE_HASH_CADENCE),
        (
            "IGNITION_ODDS_DENOMINATOR",
            rules::IGNITION_ODDS_DENOMINATOR,
        ),
        ("ASH_RESIDUE_ODDS", rules::ASH_RESIDUE_ODDS),
        ("MAX_CAPILLARY_LIFT", rules::MAX_CAPILLARY_LIFT),
        ("SEEDING_MAX_STEP", rules::SEEDING_MAX_STEP),
        ("SEEDING_ODDS", rules::SEEDING_ODDS),
        ("SEEDING_ODDS_ON_ASH", rules::SEEDING_ODDS_ON_ASH),
        ("SPONTANEOUS_GROWTH_ODDS", rules::SPONTANEOUS_GROWTH_ODDS),
        (
            "SPONTANEOUS_GROWTH_ODDS_ON_ASH",
            rules::SPONTANEOUS_GROWTH_ODDS_ON_ASH,
        ),
        ("FOLIAGE_GROWTH_ODDS", rules::FOLIAGE_GROWTH_ODDS),
        ("MUD_DRYING_ODDS", rules::MUD_DRYING_ODDS),
    ] {
        p.word(name, u64::from(value));
    }
    for (name, value) in [
        ("STEAM_LOSS_PER_CONTACT", rules::STEAM_LOSS_PER_CONTACT),
        ("FLAME_LIFETIME_TICKS", rules::FLAME_LIFETIME_TICKS),
        ("OIL_BURN_RATE", rules::OIL_BURN_RATE),
        ("FOLIAGE_MATURE_STATE", rules::FOLIAGE_MATURE_STATE),
        ("FOLIAGE_WATER_USE", rules::FOLIAGE_WATER_USE),
        ("DROWNING_FLUID_VOLUME", rules::DROWNING_FLUID_VOLUME),
        ("SEEDING_FOLIAGE_STATE", rules::SEEDING_FOLIAGE_STATE),
    ] {
        p.word(name, u64::from(value));
    }
    for (name, value) in [
        ("FREEZING_POINT_C", rules::FREEZING_POINT_C),
        ("FREEZE_CHANCE_PER_DEGREE", rules::FREEZE_CHANCE_PER_DEGREE),
        ("MELT_CHANCE_PER_DEGREE", rules::MELT_CHANCE_PER_DEGREE),
        ("MAX_PHASE_CHANCE", rules::MAX_PHASE_CHANCE),
    ] {
        p.real(name, value);
    }
    for (name, value) in [
        ("WAKE_ASLEEP", rules::WAKE_ASLEEP),
        ("WAKE_HALO", rules::WAKE_HALO),
        ("WAKE_ACTIVE_MIN", rules::WAKE_ACTIVE_MIN),
        ("WAKE_ACTIVE_FRESH", rules::WAKE_ACTIVE_FRESH),
    ] {
        p.word(name, u64::from(value));
    }
    p.word("WAKE_CHANGE_RADIUS", rules::WAKE_CHANGE_RADIUS as u64);
    for salt in rules::salt::ALL {
        p.word("salt", u64::from(salt));
    }
    for direction in rules::NEIGHBOR_DIRS.iter().chain(&rules::CARDINAL_DIRS) {
        p.word("dir.x", direction.x as u64);
        p.word("dir.y", direction.y as u64);
    }

    for (name, value) in [
        ("AIR_DRAG_RATE", body::AIR_DRAG_RATE),
        ("DRIVE_ACCELERATION", body::DRIVE_ACCELERATION),
        ("DRIVE_MAX_SPEED", body::DRIVE_MAX_SPEED),
        ("JUMP_IMPULSE", body::JUMP_IMPULSE),
        ("ATTRACT_ACCELERATION", body::ATTRACT_ACCELERATION),
        ("LIFT_ACCELERATION", body::LIFT_ACCELERATION),
        (
            "LIFT_CENTERING_ACCELERATION",
            body::LIFT_CENTERING_ACCELERATION,
        ),
        ("LIFT_INFLUENCE_RADIUS", body::LIFT_INFLUENCE_RADIUS),
        ("TARGET_EPSILON_SQUARED", body::TARGET_EPSILON_SQUARED),
        ("MAX_BODY_SPEED", body::MAX_BODY_SPEED),
        ("TERRAIN_SWEEP_STEP", body::TERRAIN_SWEEP_STEP),
        ("BODY_RADIUS.min", *body::BODY_RADIUS.start()),
        ("BODY_RADIUS.max", *body::BODY_RADIUS.end()),
        (
            "BODY_RELATIVE_DENSITY.min",
            *body::BODY_RELATIVE_DENSITY.start(),
        ),
        (
            "BODY_RELATIVE_DENSITY.max",
            *body::BODY_RELATIVE_DENSITY.end(),
        ),
        ("BODY_MASS.min", *body::BODY_MASS.start()),
        ("BODY_MASS.max", *body::BODY_MASS.end()),
    ] {
        p.real(name, value);
    }
    p.word("MAX_BODIES", u64::from(body::MAX_BODIES));
    p.word(
        "MAX_SUBSTEPS_PER_TICK",
        u64::from(body::MAX_SUBSTEPS_PER_TICK),
    );

    use body::tuning as t;
    for (name, value) in [
        ("CONTACT_ITERATIONS", t::CONTACT_ITERATIONS),
        ("VELOCITY_ITERATIONS", t::VELOCITY_ITERATIONS),
        ("MAX_MANIFOLD_CONTACTS", t::MAX_MANIFOLD_CONTACTS),
        ("RESISTANCE_SAMPLE_RINGS", t::RESISTANCE_SAMPLE_RINGS),
        ("RUT_REFERENCE_SAMPLES", t::RUT_REFERENCE_SAMPLES),
        ("RUT_MIN_REFERENCE_SAMPLES", t::RUT_MIN_REFERENCE_SAMPLES),
    ] {
        p.word(name, value as u64);
    }
    p.word(
        "RESISTANCE_SAMPLE_STRIDE",
        t::RESISTANCE_SAMPLE_STRIDE as u64,
    );
    for (name, value) in [
        ("SPLASH_MAX_PER_CELL", t::SPLASH_MAX_PER_CELL),
        ("WAKE_MAX_PER_CELL", t::WAKE_MAX_PER_CELL),
        ("WAKE_MAX_RING_PER_CELL", t::WAKE_MAX_RING_PER_CELL),
        (
            "rut_press_per_substep",
            t::DEFORMATION.rut_press_per_substep,
        ),
        (
            "max_rim_deposit_per_cell",
            t::CRATER.max_rim_deposit_per_cell,
        ),
    ] {
        p.word(name, u64::from(value));
    }
    for (name, value) in [
        ("CONTACT_SLOP", t::CONTACT_SLOP),
        ("GROUND_NORMAL_MIN_Y", t::GROUND_NORMAL_MIN_Y),
        ("DUPLICATE_NORMAL_COSINE", t::DUPLICATE_NORMAL_COSINE),
        ("INTERNAL_EDGE_COSINE", t::INTERNAL_EDGE_COSINE),
        ("CONSTRAINT_TOLERANCE", t::CONSTRAINT_TOLERANCE),
        ("GROUND_NORMAL_BAND", t::GROUND_NORMAL_BAND),
        ("DEGENERATE_DISTANCE", t::DEGENERATE_DISTANCE),
        ("COINCIDENT_DISTANCE", t::COINCIDENT_DISTANCE),
        ("RESISTANCE_CLIFF_THRESHOLD", t::RESISTANCE_CLIFF_THRESHOLD),
        ("RESISTANCE_MULTIPLIER", t::RESISTANCE_MULTIPLIER),
        ("SPLASH_MIN_SPEED", t::SPLASH_MIN_SPEED),
        ("SPLASH_UNITS_PER_ENERGY", t::SPLASH_UNITS_PER_ENERGY),
        ("SPLASH_ENERGY_PER_UNIT", t::SPLASH_ENERGY_PER_UNIT),
        ("SPLASH_CAVITY_RATIO", t::SPLASH_CAVITY_RATIO),
        ("SPLASH_RING_OUTER_RATIO", t::SPLASH_RING_OUTER_RATIO),
        ("WAKE_MIN_SPEED", t::WAKE_MIN_SPEED),
        ("WAKE_LAYER_UNITS", t::WAKE_LAYER_UNITS),
        ("WAKE_RING_WIDTH", t::WAKE_RING_WIDTH),
        ("WAKE_MAX_PENDING", t::WAKE_MAX_PENDING),
        ("RUT_REFERENCE_RING_RATIO", t::RUT_REFERENCE_RING_RATIO),
        ("RUT_TRIM_DEPTH", t::RUT_TRIM_DEPTH),
        (
            "RUT_REFERENCE_SMOOTHING_SECONDS",
            t::RUT_REFERENCE_SMOOTHING_SECONDS,
        ),
        ("MAX_SINKAGE_RATIO", t::MAX_SINKAGE_RATIO),
        ("PLANE_SINGULARITY", t::PLANE_SINGULARITY),
        ("RUT_BERM_INNER_RATIO", t::RUT_BERM_INNER_RATIO),
        ("RUT_BERM_OUTER_RATIO", t::RUT_BERM_OUTER_RATIO),
        ("RUT_BERM_HEIGHT_RATIO", t::RUT_BERM_HEIGHT_RATIO),
        ("RUT_BERM_SIDE_COSINE", t::RUT_BERM_SIDE_COSINE),
        ("RUT_HEADING_MIN_SPEED", t::RUT_HEADING_MIN_SPEED),
        ("QUANTIZATION_TOLERANCE", t::QUANTIZATION_TOLERANCE),
        ("air_drag_rate", t::KINEMATIC.air_drag_rate),
        (
            "rolling_resistance_coefficient",
            t::KINEMATIC.rolling_resistance_coefficient,
        ),
        ("ground_damping_rate", t::KINEMATIC.ground_damping_rate),
        (
            "static_friction_coefficient",
            t::KINEMATIC.static_friction_coefficient,
        ),
        ("static_friction_speed", t::KINEMATIC.static_friction_speed),
        ("impact_restitution", t::KINEMATIC.impact_restitution),
        (
            "min_deformation_energy",
            t::DEFORMATION.min_deformation_energy,
        ),
        (
            "energy_to_deformation_scale",
            t::DEFORMATION.energy_to_deformation_scale,
        ),
        (
            "cost_displace_granular",
            t::DEFORMATION.cost_displace_granular,
        ),
        ("cost_crush_terrain", t::DEFORMATION.cost_crush_terrain),
        ("rut_depth_scale", t::DEFORMATION.rut_depth_scale),
        ("rut_reference_mass", t::DEFORMATION.rut_reference_mass),
        ("rut_width_ratio", t::DEFORMATION.rut_width_ratio),
        ("rut_cost_per_unit", t::DEFORMATION.rut_cost_per_unit),
        ("rim_inner_radius_ratio", t::CRATER.rim_inner_radius_ratio),
        ("rim_expansion_factor", t::CRATER.rim_expansion_factor),
        ("blade_ceiling_ratio", t::CRATER.blade_ceiling_ratio),
        (
            "max_deformation_size_ratio",
            t::CRATER.max_deformation_size_ratio,
        ),
        ("grounding_drift_buffer", t::CRATER.grounding_drift_buffer),
        ("minimum_bounce_speed", t::CRATER.minimum_bounce_speed),
    ] {
        p.real(name, value);
    }
    p.0
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pinned values. When one of these fails, a listed constant changed: raise the matching
    // revision above, record the change in docs/reference/world-format.md and pin the new
    // value. Saved worlds of the old revision are then refused, as they must be.
    const CELL_LAYOUT: u64 = 0x0DE2_75B3_5FC5_4DC9;
    const PHYSICS: u64 = 0x45AE_D2EF_11B9_F774;

    #[test]
    fn identities_are_pinned_to_the_current_layout_and_physics() {
        assert_eq!(
            cell_layout_fingerprint(),
            CELL_LAYOUT,
            "cell layout fingerprint {:#018X}",
            cell_layout_fingerprint()
        );
        assert_eq!(
            physics_fingerprint(),
            PHYSICS,
            "physics fingerprint {:#018X}",
            physics_fingerprint()
        );
    }

    #[test]
    fn a_changed_value_changes_the_fingerprint() {
        let mut a = Fingerprint::new();
        a.real("x", 1.0);
        let mut b = Fingerprint::new();
        b.real("x", 1.000_000_1);
        assert_ne!(a.0, b.0);
        let mut c = Fingerprint::new();
        c.real("y", 1.0);
        assert_ne!(a.0, c.0, "the name is part of the fingerprint");
    }
}
