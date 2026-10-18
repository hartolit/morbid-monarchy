//! The cellular automaton around the column rules: which rules run and how often, their tuning
//! constants, the activity (wake) levels, and the keyed hashes behind every pseudo-random choice.
//!
//! How one column changes is [`crate::cell`]'s; material properties are [`crate::material`]'s.
//! This module holds what a tick of the automaton needs besides them. The CPU baseline engine,
//! the GPU kernels (through declarations generated from these items) and the evidence reference
//! all read these definitions, so a constant or a hash cannot differ between them.
//!
//! Every pseudo-random choice is a pure function of the world's rule seed, a cell's absolute
//! position, the logical tick and a salt. [`RuleTick`] carries the tick's low word and a key
//! mixing the seed with the tick's high word; the key of seed 0 below tick 2³² is 0, for which
//! [`keyed_spatial_hash`] and [`ShuffledDirs`] are exactly the baseline's position-and-tick hash
//! and shuffles.

use glam::IVec2;

use crate::material::CompassFlags;

/// Which rule families run. A rule that is off leaves its quantities untouched; turning one on
/// wakes the whole resident world once, so sleeping regions resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SimulationRules {
    pub liquid: bool,
    pub granular: bool,
    pub thermal: bool,
    pub fire: bool,
    pub biology: bool,
}

impl SimulationRules {
    pub const ALL: Self = Self {
        liquid: true,
        granular: true,
        thermal: true,
        fire: true,
        biology: true,
    };

    pub const NONE: Self = Self {
        liquid: false,
        granular: false,
        thermal: false,
        fire: false,
        biology: false,
    };

    /// Liquid, granular and thermal rules only: every rule that is a pure function of the state.
    pub const DETERMINISTIC: Self = Self {
        fire: false,
        biology: false,
        ..Self::ALL
    };

    pub const LIQUID_BIT: u32 = 1 << 0;
    pub const GRANULAR_BIT: u32 = 1 << 1;
    pub const THERMAL_BIT: u32 = 1 << 2;
    pub const FIRE_BIT: u32 = 1 << 3;
    pub const BIOLOGY_BIT: u32 = 1 << 4;

    /// The rules as bits (`*_BIT`), the form the GPU tick parameters carry.
    pub const fn bits(self) -> u32 {
        let mut bits = 0;
        if self.liquid {
            bits |= Self::LIQUID_BIT;
        }
        if self.granular {
            bits |= Self::GRANULAR_BIT;
        }
        if self.thermal {
            bits |= Self::THERMAL_BIT;
        }
        if self.fire {
            bits |= Self::FIRE_BIT;
        }
        if self.biology {
            bits |= Self::BIOLOGY_BIT;
        }
        bits
    }

    /// The rules `bits` names, or `None` when it sets a bit no rule has (a saved value from
    /// another revision, or corrupt).
    pub const fn from_bits(bits: u32) -> Option<Self> {
        let known = Self::LIQUID_BIT
            | Self::GRANULAR_BIT
            | Self::THERMAL_BIT
            | Self::FIRE_BIT
            | Self::BIOLOGY_BIT;
        if bits & !known != 0 {
            return None;
        }
        Some(Self {
            liquid: bits & Self::LIQUID_BIT != 0,
            granular: bits & Self::GRANULAR_BIT != 0,
            thermal: bits & Self::THERMAL_BIT != 0,
            fire: bits & Self::FIRE_BIT != 0,
            biology: bits & Self::BIOLOGY_BIT != 0,
        })
    }
}

impl Default for SimulationRules {
    fn default() -> Self {
        Self::ALL
    }
}

// --- Cadence and sampling ---

/// Granular avalanches run on every second tick (the odd ones), so slopes slide slower than
/// liquids flow.
pub const GRANULAR_TICK_INTERVAL: u64 = 2;

/// Whether granular movement runs in logical tick `tick`.
#[inline(always)]
pub const fn granular_runs(tick: u64) -> bool {
    !tick.is_multiple_of(GRANULAR_TICK_INTERVAL)
}

/// Biology is sampled: an active cell runs the growth rules in one tick out of this many.
pub const BIOLOGY_ODDS_DENOMINATOR: u32 = 10;
/// Fire is sampled: an active cell runs the combustion rules in one tick out of this many.
pub const FIRE_ODDS_DENOMINATOR: u32 = 2;

// --- Granular flow ---

/// Upper bound on granular volume moved across one edge per tick; keeps avalanches gradual.
pub const MAX_GRANULAR_TRANSFER: u32 = 2;
/// Smallest head difference that moves a whole unit (`diff / 2 >= 1`). Below it a slope is
/// left at rest rather than dithered, so piles reach a fixed point.
pub const MIN_TRANSFER_HEAD: u32 = 2;
/// Range of the per-tick stiction added to the angle of repose.
pub const STICTION_VARIANTS: u32 = 3;

// --- Liquid flow ---

/// Bed erosion triggers when the masked hash falls below the erosive flux, so the odds per tick
/// are `flux / (EROSION_MASK + 1)`.
pub const EROSION_MASK: u32 = 511;

// --- Thermal ---

/// Temperature at which water and ice trade places.
pub const FREEZING_POINT_C: f32 = 0.0;
/// Coolant volume boiled off per tick for every magma column sharing an edge.
pub const STEAM_LOSS_PER_CONTACT: u16 = 2;
/// Odds per tick, per degree below freezing, that a nucleated water column freezes.
pub const FREEZE_CHANCE_PER_DEGREE: f32 = 0.02;
/// Odds per tick, per degree above freezing, that an exposed ice column melts.
pub const MELT_CHANCE_PER_DEGREE: f32 = 0.002;
/// Upper bound on either phase-change chance, so even extreme temperatures advance a front
/// over several ticks instead of flashing a whole pool at once.
pub const MAX_PHASE_CHANCE: f32 = 0.5;
/// A phase roll compares a hash modulo this resolution with a threshold.
pub const CHANCE_RESOLUTION: u32 = 1 << 16;
/// Salts the per-cell hash so phase rolls are uncorrelated with the other rules' hashes.
pub const PHASE_HASH_CADENCE: u32 = 7;

/// The threshold a phase roll's hash is compared with, for a chance per tick.
#[inline(always)]
pub fn phase_threshold(chance: f32) -> u32 {
    (chance.clamp(0.0, MAX_PHASE_CHANCE) * CHANCE_RESOLUTION as f32) as u32
}

/// Freezing and melting at one ambient temperature: each is `None` when the climate does not
/// allow it, otherwise the threshold of its roll. Both depend only on the temperature, so they
/// are computed once per tick, never per cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhaseThresholds {
    pub freeze: Option<u32>,
    pub melt: Option<u32>,
}

impl PhaseThresholds {
    pub fn at(ambient_temperature_c: f32) -> Self {
        let warmth = ambient_temperature_c - FREEZING_POINT_C;
        let chill = FREEZING_POINT_C - ambient_temperature_c;
        Self {
            freeze: (chill > 0.0).then(|| phase_threshold(chill * FREEZE_CHANCE_PER_DEGREE)),
            melt: (warmth > 0.0).then(|| phase_threshold(warmth * MELT_CHANCE_PER_DEGREE)),
        }
    }
}

// --- Fire ---

/// Burn ticks a flame survives once it has no liquid fuel left.
pub const FLAME_LIFETIME_TICKS: u16 = 20;
/// Oil volume consumed per burning tick. The flame is held alive until the pool is gone.
pub const OIL_BURN_RATE: u16 = 1;
/// Each adjacent heat source adds one chance in this many to ignite per evaluated tick.
pub const IGNITION_ODDS_DENOMINATOR: u32 = 15;
/// A burnt-out flame leaves ash with odds of one in this many.
pub const ASH_RESIDUE_ODDS: u32 = 2;

// --- Biology ---

/// Growth state at which foliage is fully grown; the cell stays awake until it is reached.
pub const FOLIAGE_MATURE_STATE: u16 = 10;
/// The largest height difference across which roots wick water from a neighbour.
pub const MAX_CAPILLARY_LIFT: u32 = 2;
/// Water a growing plant draws from its own column per growth step.
pub const FOLIAGE_WATER_USE: u16 = 2;
/// A column holding more liquid than this drowns plants.
pub const DROWNING_FLUID_VOLUME: u16 = 15;
/// Foliage at least this grown seeds its neighbours.
pub const SEEDING_FOLIAGE_STATE: u16 = 2;
/// Seeds take root only across a height step of at most this.
pub const SEEDING_MAX_STEP: u32 = 1;
/// Odds (one in this many) that a cell beside seeding foliage takes root, on bare ground and on
/// ash; ash fertilizes.
pub const SEEDING_ODDS: u32 = 3;
pub const SEEDING_ODDS_ON_ASH: u32 = 1;
/// Odds (one in this many) of spontaneous growth on bare ground and on ash.
pub const SPONTANEOUS_GROWTH_ODDS: u32 = 800;
pub const SPONTANEOUS_GROWTH_ODDS_ON_ASH: u32 = 150;
/// Odds (one in this many) that hydrated immature foliage grows one state.
pub const FOLIAGE_GROWTH_ODDS: u32 = 3;
/// Odds (one in this many) that foliage without standing water dries its mud to dirt.
pub const MUD_DRYING_ODDS: u32 = 10;

// --- Activity ---
//
// Mass moves between cells by an exchange that is only conservative when the donor and the
// receiver both run in the same tick. Cells are therefore split into donors (active) and
// receive-only halo cells, and every active cell has its whole 3 × 3 neighbourhood at halo level
// or higher.

/// Asleep: the cell is not visited.
pub const WAKE_ASLEEP: u8 = 0;
/// Halo: the cell may receive mass from an active neighbour but never donates or reacts.
pub const WAKE_HALO: u8 = 1;
/// Lowest level at which a cell runs every rule and may donate mass.
pub const WAKE_ACTIVE_MIN: u8 = 2;
/// Level granted to a changed cell. It stays active for `WAKE_ACTIVE_FRESH - WAKE_ACTIVE_MIN + 1`
/// ticks without further change, which survives the interleaved granular cadence.
pub const WAKE_ACTIVE_FRESH: u8 = 3;
/// Chebyshev radius around a changed cell that becomes active. Liquid trickles depend on the
/// receiver's neighbours, so a change can alter decisions two cells away. One further ring
/// becomes halo.
pub const WAKE_CHANGE_RADIUS: i32 = 2;

// --- Keyed hashes ---

/// The murmur3 finalizer: a bijection on `u32` with full avalanche; `fmix32(0) == 0`.
#[inline(always)]
pub const fn fmix32(mut h: u32) -> u32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h
}

/// The tick as the rules' hashes see it: the low word of the logical tick and a key mixing the
/// world's rule seed with the tick's high word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct RuleTick {
    pub tick: u32,
    pub key: u32,
}

impl RuleTick {
    pub const fn new(seed: u64, tick: u64) -> Self {
        Self {
            tick: tick as u32,
            key: rule_key(seed, (tick >> 32) as u32),
        }
    }

    /// The baseline engine's 32-bit tick, unkeyed.
    pub const fn baseline(tick: u32) -> Self {
        Self { tick, key: 0 }
    }
}

/// The key of a rule seed and a tick's high word. Seed 0 below tick 2³² gives key 0.
#[inline(always)]
pub const fn rule_key(seed: u64, tick_hi: u32) -> u32 {
    fmix32(
        (seed as u32) ^ ((seed >> 32) as u32).rotate_left(16) ^ tick_hi.wrapping_mul(0x9E37_79B9),
    )
}

/// The baseline's per-cell, per-tick hash with the rule key folded in before the avalanche.
#[inline(always)]
pub const fn keyed_spatial_hash(pos: IVec2, tick: RuleTick) -> u32 {
    fmix32(
        (pos.x as u32).wrapping_mul(0x736A_153D)
            ^ (pos.y as u32).wrapping_mul(0x9E37_79B1)
            ^ tick.tick.wrapping_mul(0x11C6_4E6D)
            ^ tick.key,
    )
}

/// [`keyed_spatial_hash`] salted by a material and a cadence, as the baseline's shuffles and
/// phase rolls use it.
#[inline(always)]
pub const fn keyed_spatial_hash_extended(
    pos: IVec2,
    tick: RuleTick,
    material: u8,
    cadence: u32,
) -> u32 {
    let mut h = keyed_spatial_hash(pos, tick)
        .wrapping_add((material as u32).wrapping_mul(0x27D4_EB2F))
        .wrapping_add(cadence.wrapping_mul(0x1656_67B1));
    h ^= h >> 13;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 16;
    h
}

/// Salts of the keyed draws, one per decision. They are cadences of
/// [`keyed_spatial_hash_extended`] with material 0, distinct from every (material, cadence) pair
/// the shuffles and phase rolls use (checked below).
pub mod salt {
    pub const BIOLOGY_SAMPLE: u32 = 101;
    pub const BIOLOGY_SEEDING: u32 = 102;
    pub const BIOLOGY_SPONTANEOUS: u32 = 103;
    pub const BIOLOGY_GROWTH: u32 = 104;
    pub const BIOLOGY_MUD_DRYING: u32 = 105;
    pub const FIRE_SAMPLE: u32 = 201;
    pub const FIRE_ASH: u32 = 202;
    pub const FIRE_IGNITION: u32 = 203;

    pub const ALL: [u32; 8] = [
        BIOLOGY_SAMPLE,
        BIOLOGY_SEEDING,
        BIOLOGY_SPONTANEOUS,
        BIOLOGY_GROWTH,
        BIOLOGY_MUD_DRYING,
        FIRE_SAMPLE,
        FIRE_ASH,
        FIRE_IGNITION,
    ];
}

/// A keyed draw for one decision of one cell in one tick.
#[inline(always)]
pub const fn draw(pos: IVec2, tick: RuleTick, salt: u32) -> u32 {
    keyed_spatial_hash_extended(pos, tick, 0, salt)
}

/// `numerator` chances in `denominator`, from a draw. The modulo bias is below 2⁻²² for every
/// denominator the rules use.
#[inline(always)]
pub const fn ratio(draw: u32, numerator: u32, denominator: u32) -> bool {
    draw % denominator < numerator
}

// --- Directions ---

/// The eight neighbour steps in the order every shuffle starts from.
pub const NEIGHBOR_DIRS: [IVec2; 8] = [
    IVec2::new(0, 1),
    IVec2::new(1, 1),
    IVec2::new(1, 0),
    IVec2::new(1, -1),
    IVec2::new(0, -1),
    IVec2::new(-1, -1),
    IVec2::new(-1, 0),
    IVec2::new(-1, 1),
];

/// The four edge-sharing steps, in [`NEIGHBOR_DIRS`] order.
pub const CARDINAL_DIRS: [IVec2; 4] = [
    IVec2::new(0, 1),
    IVec2::new(1, 0),
    IVec2::new(0, -1),
    IVec2::new(-1, 0),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowPattern {
    Omni,
    Cardinal,
}

/// Neighbour steps in a deterministic per-cell, per-tick order: a Fisher–Yates shuffle driven by
/// [`keyed_spatial_hash_extended`], optionally with the cell's momentum direction moved first.
#[derive(Debug, Clone, Copy)]
pub struct ShuffledDirs {
    dirs: [IVec2; 8],
    count: usize,
}

impl ShuffledDirs {
    #[inline(always)]
    pub fn new(
        pattern: FlowPattern,
        pos: IVec2,
        tick: RuleTick,
        material: u8,
        cadence: u32,
    ) -> Self {
        let mut h = keyed_spatial_hash_extended(pos, tick, material, cadence);
        let (mut dirs, count) = match pattern {
            FlowPattern::Omni => (NEIGHBOR_DIRS, 8),
            FlowPattern::Cardinal => {
                let mut dirs = [IVec2::ZERO; 8];
                dirs[..4].copy_from_slice(&CARDINAL_DIRS);
                (dirs, 4)
            }
        };
        for i in (1..count).rev() {
            let j = (h % (i as u32 + 1)) as usize;
            dirs.swap(i, j);
            h = h.wrapping_mul(0x9E37_79B1).wrapping_add(1);
        }
        Self { dirs, count }
    }

    /// [`new`](Self::new), then the step the compass `flags` point along moved to the front.
    #[inline(always)]
    pub fn with_momentum(
        pattern: FlowPattern,
        pos: IVec2,
        tick: RuleTick,
        material: u8,
        cadence: u32,
        flags: u8,
    ) -> Self {
        let mut shuffled = Self::new(pattern, pos, tick, material, cadence);
        let forward = compass_forward(flags);
        if forward != IVec2::ZERO
            && let Some(index) = shuffled.get().iter().position(|&dir| dir == forward)
        {
            shuffled.dirs.swap(0, index);
        }
        shuffled
    }

    #[inline(always)]
    pub fn get(&self) -> &[IVec2] {
        &self.dirs[..self.count]
    }
}

/// Grid step encoded by a cell's compass (momentum) flags; zero when the cell has none.
#[inline(always)]
pub const fn compass_forward(flags: u8) -> IVec2 {
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
    forward
}

/// Compass flags a column takes when liquid flows into it along `flow` (receiver minus source).
#[inline(always)]
pub const fn compass_for_flow(flow: IVec2) -> u8 {
    let mut flags = 0;
    if flow.y > 0 {
        flags |= CompassFlags::FACING_N;
    }
    if flow.y < 0 {
        flags |= CompassFlags::FACING_S;
    }
    if flow.x > 0 {
        flags |= CompassFlags::FACING_E;
    }
    if flow.x < 0 {
        flags |= CompassFlags::FACING_W;
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The baseline engine's hash before 03 (`monarch-engine` `utils::spatial_hash`), written out
    /// again here: the keyed hash with key 0 must equal it.
    fn baseline_spatial_hash(pos: IVec2, tick: u32) -> u32 {
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

    #[test]
    fn seed_zero_below_tick_two_to_the_32_is_the_baseline_hash() {
        assert_eq!(rule_key(0, 0), 0);
        for (x, y, tick) in [
            (0, 0, 0),
            (-7, 13, 1),
            (i32::MIN, i32::MAX, u32::MAX),
            (64, -65, 600),
        ] {
            let pos = IVec2::new(x, y);
            assert_eq!(
                keyed_spatial_hash(pos, RuleTick::new(0, u64::from(tick))),
                baseline_spatial_hash(pos, tick)
            );
        }
        // Hand-computed value of the formula at the origin and tick 1: fmix32(0x11C64E6D).
        assert_eq!(baseline_spatial_hash(IVec2::ZERO, 1), fmix32(0x11C6_4E6D));
    }

    #[test]
    fn the_seed_and_the_high_tick_word_change_the_stream() {
        let pos = IVec2::new(5, -9);
        let base = keyed_spatial_hash(pos, RuleTick::new(0, 77));
        assert_ne!(keyed_spatial_hash(pos, RuleTick::new(1, 77)), base);
        assert_ne!(
            keyed_spatial_hash(pos, RuleTick::new(0, 77 + (1 << 32))),
            base
        );
        assert_eq!(RuleTick::new(0, 77 + (1 << 32)).tick, 77);
        assert_eq!(RuleTick::new(9, 3), RuleTick::new(9, 3), "a pure function");
    }

    #[test]
    fn draw_salts_never_alias_a_shuffle_or_phase_roll() {
        // The extended hash adds material * M + cadence * C before its second avalanche; a salt
        // must not produce the same sum as any (material, cadence) another rule uses.
        let sum = |material: u32, cadence: u32| {
            material
                .wrapping_mul(0x27D4_EB2F)
                .wrapping_add(cadence.wrapping_mul(0x1656_67B1))
        };
        let mut used: Vec<u32> = (0..16).map(|material| sum(material, 0)).collect();
        used.push(sum(0, PHASE_HASH_CADENCE));
        for salt in salt::ALL {
            assert!(!used.contains(&sum(0, salt)), "salt {salt}");
            used.push(sum(0, salt));
        }
    }

    #[test]
    fn shuffles_are_permutations_and_momentum_leads() {
        let tick = RuleTick::new(3, 41);
        for pattern in [FlowPattern::Omni, FlowPattern::Cardinal] {
            let dirs = ShuffledDirs::new(pattern, IVec2::new(-3, 8), tick, 1, 0);
            let mut sorted: Vec<(i32, i32)> = dirs.get().iter().map(|d| (d.x, d.y)).collect();
            sorted.sort();
            let mut expected: Vec<(i32, i32)> = match pattern {
                FlowPattern::Omni => NEIGHBOR_DIRS.iter().map(|d| (d.x, d.y)).collect(),
                FlowPattern::Cardinal => CARDINAL_DIRS.iter().map(|d| (d.x, d.y)).collect(),
            };
            expected.sort();
            assert_eq!(sorted, expected);
        }
        let east = ShuffledDirs::with_momentum(
            FlowPattern::Omni,
            IVec2::new(2, 2),
            tick,
            1,
            0,
            CompassFlags::FACING_E,
        );
        assert_eq!(east.get()[0], IVec2::new(1, 0));
        assert_eq!(
            compass_for_flow(IVec2::new(-1, 1)),
            CompassFlags::FACING_W | CompassFlags::FACING_N
        );
        assert_eq!(
            compass_forward(compass_for_flow(IVec2::new(1, -1))),
            IVec2::new(1, -1)
        );
    }

    #[test]
    fn phase_thresholds_follow_the_climate() {
        let mild = PhaseThresholds::at(12.0);
        assert_eq!(mild.freeze, None);
        // 12 degrees above freezing: 0.024 per tick of 65,536.
        assert_eq!(mild.melt, Some((0.024f32 * 65_536.0) as u32));
        let cold = PhaseThresholds::at(-30.0);
        assert_eq!(cold.melt, None);
        // 0.6 clamps to the 0.5 ceiling.
        assert_eq!(cold.freeze, Some(32_768));
        assert_eq!(
            PhaseThresholds::at(0.0),
            PhaseThresholds {
                freeze: None,
                melt: None
            }
        );
    }

    #[test]
    fn cadence_and_rule_bits() {
        assert!(!granular_runs(0) && granular_runs(1) && granular_runs(u64::MAX));
        assert!(
            !granular_runs(1 << 32),
            "parity continues across the low word's rollover"
        );
        assert_eq!(SimulationRules::ALL.bits(), 0b11111);
        assert_eq!(SimulationRules::NONE.bits(), 0);
        assert_eq!(SimulationRules::DETERMINISTIC.bits(), 0b00111);
        for rules in [
            SimulationRules::ALL,
            SimulationRules::NONE,
            SimulationRules::DETERMINISTIC,
        ] {
            assert_eq!(SimulationRules::from_bits(rules.bits()), Some(rules));
        }
        assert_eq!(SimulationRules::from_bits(0b100000), None);
        assert!(ratio(10, 1, 10) && !ratio(9, 1, 10));
    }
}
