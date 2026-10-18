//! The packed layouts shared by the CPU and the kernels, and the WGSL declarations generated
//! from them.
//!
//! Each GPU record is declared once, below, with `gpu_record!`: the declaration produces the
//! Rust struct, its encoder and decoder, and the WGSL struct the kernels use. Every field is
//! one 32-bit little-endian word (`u32`, `i32`, `f32` or an atomic `u32`), so the WGSL struct
//! has no padding and field `n` sits at byte `4n` in both languages; there are no host booleans,
//! vectors or 64-bit fields. Wide values (identities, ticks, revisions) are split into a low
//! and a high word.
//!
//! The cell's field constants come from [`WorldCell::LAYOUT`], which `world-model` owns, and the
//! material and command codes from their Rust owners. [`wgsl_declarations`] writes all of them
//! into one generated WGSL module that every assembled shader starts with.
//!
//! Golden tests below compare the encoders with words written out by hand, and the `transfers`
//! GPU suite executes the generated declarations on a device and compares what the kernels read
//! with what the CPU wrote.

use std::fmt::Write as _;

use world_model::{
    body::{self, tuning},
    cell::{Field, WorldCell},
    command::EditTool,
    config,
    material::{
        CompassFlags, FluidMat, GranularMat, SurfaceMat, TerrainMat, fluid_properties,
        granular_repose, ground_softness, is_combustible, terrain_crush_hardness,
    },
    rules::{self, SimulationRules, salt},
    spatial::{CHUNK_CELL_COUNT, CHUNK_SIZE},
    units,
};

/// The type of one 32-bit word of a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordType {
    U32,
    I32,
    F32,
    /// A `u32` that kernels update with atomic operations.
    AtomicU32,
}

impl WordType {
    pub fn wgsl(self) -> &'static str {
        match self {
            Self::U32 => "u32",
            Self::I32 => "i32",
            Self::F32 => "f32",
            Self::AtomicU32 => "atomic<u32>",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldDecl {
    pub name: &'static str,
    pub ty: WordType,
}

/// A record's name and fields in word order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordLayout {
    pub name: &'static str,
    pub fields: &'static [FieldDecl],
}

impl RecordLayout {
    pub fn offset_of(&self, field: &str) -> Option<u64> {
        self.fields
            .iter()
            .position(|declared| declared.name == field)
            .map(|index| index as u64 * 4)
    }

    /// The WGSL struct declaration.
    pub fn wgsl(&self) -> String {
        let mut text = format!("struct {} {{\n", self.name);
        for field in self.fields {
            let _ = writeln!(text, "    {}: {},", field.name, field.ty.wgsl());
        }
        text.push_str("}\n");
        text
    }
}

/// One 32-bit word of a record, as stored.
pub trait Word: Copy {
    fn to_word(self) -> u32;
    fn from_word(word: u32) -> Self;
}

impl Word for u32 {
    fn to_word(self) -> u32 {
        self
    }
    fn from_word(word: u32) -> Self {
        word
    }
}

impl Word for i32 {
    fn to_word(self) -> u32 {
        self as u32
    }
    fn from_word(word: u32) -> Self {
        word as i32
    }
}

impl Word for f32 {
    fn to_word(self) -> u32 {
        self.to_bits()
    }
    fn from_word(word: u32) -> Self {
        f32::from_bits(word)
    }
}

/// The largest record, in words.
pub const MAX_RECORD_WORDS: usize = 64;

/// A record shared with kernels.
pub trait GpuRecord: Sized + Copy {
    const LAYOUT: RecordLayout;
    const WORDS: usize;
    const BYTES: u64 = Self::WORDS as u64 * 4;

    fn write_words(&self, out: &mut [u32]);
    fn read_words(words: &[u32]) -> Self;

    /// Appends the record's little-endian bytes.
    fn write_bytes(&self, out: &mut [u8]) {
        let mut words = [0u32; MAX_RECORD_WORDS];
        self.write_words(&mut words[..Self::WORDS]);
        let (chunks, _) = out.as_chunks_mut::<4>();
        for (chunk, word) in chunks.iter_mut().zip(&words[..Self::WORDS]) {
            *chunk = word.to_le_bytes();
        }
    }

    fn read_bytes(bytes: &[u8]) -> Self {
        let mut words = [0u32; MAX_RECORD_WORDS];
        let (chunks, _) = bytes.as_chunks::<4>();
        for (word, chunk) in words[..Self::WORDS].iter_mut().zip(chunks) {
            *word = u32::from_le_bytes(*chunk);
        }
        Self::read_words(&words[..Self::WORDS])
    }
}

/// Declares a record: the Rust struct, its [`GpuRecord`] implementation and its WGSL layout.
/// Field types are `u32`, `i32`, `f32` or `atomic_u32` (stored as `u32` on the CPU).
macro_rules! gpu_record {
    (
        $(#[$meta:meta])*
        pub struct $name:ident {
            $( $(#[$field_meta:meta])* $field:ident : $ty:ident ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Default)]
        pub struct $name {
            $( $(#[$field_meta])* pub $field: gpu_record!(@rust $ty), )+
        }

        impl GpuRecord for $name {
            const LAYOUT: RecordLayout = RecordLayout {
                name: stringify!($name),
                fields: &[$(
                    FieldDecl { name: stringify!($field), ty: gpu_record!(@word $ty) }
                ),+],
            };
            const WORDS: usize = [$( stringify!($field) ),+].len();

            fn write_words(&self, out: &mut [u32]) {
                let mut index = 0;
                $(
                    out[index] = Word::to_word(self.$field);
                    index += 1;
                )+
                let _ = index;
            }

            fn read_words(words: &[u32]) -> Self {
                let mut index = 0;
                $(
                    let $field = Word::from_word(words[index]);
                    index += 1;
                )+
                let _ = index;
                Self { $( $field ),+ }
            }
        }
    };
    (@rust u32) => { u32 };
    (@rust i32) => { i32 };
    (@rust f32) => { f32 };
    (@rust atomic_u32) => { u32 };
    (@word u32) => { WordType::U32 };
    (@word i32) => { WordType::I32 };
    (@word f32) => { WordType::F32 };
    (@word atomic_u32) => { WordType::AtomicU32 };
}

gpu_record! {
    /// Per-tick parameters (uniform). Thirty-two words, so the uniform struct is a multiple of
    /// 16 bytes. Kernels address cells in window-local coordinates: cell `(0, 0)` is the first
    /// cell of the window's lowest chunk. The rules' hashes see absolute positions, which the
    /// kernels form from the window's lowest chunk key with wrapping arithmetic; no absolute
    /// coordinate travels as a float.
    pub struct TickParams {
        tick_lo: u32,
        tick_hi: u32,
        epoch: u32,
        command_count: u32,
        /// Window size in chunks.
        window_width: u32,
        window_height: u32,
        slot_capacity: u32,
        event_capacity: u32,
        /// Events the pressure-injection kernel appends; always zero outside fault injection.
        injected_events: u32,
        /// The window's lowest chunk key.
        origin_key_x: i32,
        origin_key_y: i32,
        /// The rule families that run in this tick (`RULE_*` bits; granular only on odd ticks).
        rules: u32,
        /// `ACTIVITY_WAKE_ALL`, `ACTIVITY_DENSE`.
        activity_flags: u32,
        /// `PHASE_FREEZE` and `PHASE_MELT` when the climate allows them, and their thresholds.
        phase_flags: u32,
        freeze_threshold: u32,
        melt_threshold: u32,
        /// `world_model::rules::RuleTick` of this tick.
        rule_tick: u32,
        rule_key: u32,
        /// The tick's low bit: which of the two bitmap sets of a tile record this tick writes.
        parity: u32,
        /// Tiles of the window (64 per chunk).
        tile_count: u32,
        /// Word offsets of the regions of the activity buffer (`plan::ActivityLayout`).
        requests_base: u32,
        candidates_base: u32,
        process_base: u32,
        records_base: u32,
        reserved0: u32,
        reserved1: u32,
        reserved2: u32,
        reserved3: u32,
        reserved4: u32,
        reserved5: u32,
        reserved6: u32,
        reserved7: u32,
    }
}

gpu_record! {
    /// Which command a dispatch executes (uniform, one entry per command position, bound
    /// through a dynamic offset). Written once when the buffer is created: entry `i` names
    /// command `i`.
    pub struct DispatchParams {
        command_index: u32,
        reserved0: u32,
        reserved1: u32,
        reserved2: u32,
    }
}

gpu_record! {
    /// One command of a tick. `kind` selects which fields are meaningful:
    /// - [`codes::COMMAND_EDIT`]: `tool`, `center_x`, `center_y` (a column target), `radius`,
    ///   `strength`;
    /// - [`codes::COMMAND_QUERY_COLUMN`]: `cell_x`, `cell_y`;
    /// - [`codes::COMMAND_SPAWN`]: `cell_x`, `cell_y` (a column target), `body_*` (the handle
    ///   the CPU allocated), `a` mass, `b` radius, `c` relative density, `d` drop height;
    /// - [`codes::COMMAND_REMOVE`], [`codes::COMMAND_JUMP`]: `body_*`;
    /// - [`codes::COMMAND_DRIVE`]: `body_*`, `a`/`b` the render-space x/z direction, `c`
    ///   acceleration, `d` speed cap;
    /// - [`codes::COMMAND_ATTRACT`], [`codes::COMMAND_LIFT`]: `cell_x`, `cell_y` (a column
    ///   target);
    /// - [`codes::COMMAND_REFUSED`]: `tool` holds the result status the CPU decided.
    ///
    /// `target_kind` is [`codes::TARGET_RAY`] when the command acts where its ray hits: `origin_*`,
    /// `direction_*` and `ray_length` then describe the ray, and the `resolve_target` kernel
    /// writes the hit into the command's result before the command's own kernel runs.
    /// Coordinates are window-local: cells, or render-space world units (x = cell x, z = −cell
    /// y). `sequence` is the command's position in the tick and the index of its result record.
    pub struct CommandRecord {
        kind: u32,
        id_lo: u32,
        id_hi: u32,
        sequence: u32,
        tool: u32,
        center_x: i32,
        center_y: i32,
        radius: u32,
        strength: u32,
        cell_x: i32,
        cell_y: i32,
        target_kind: u32,
        body_index: u32,
        body_generation: u32,
        a: f32,
        b: f32,
        c: f32,
        d: f32,
        origin_x: f32,
        origin_y: f32,
        origin_z: f32,
        direction_x: f32,
        direction_y: f32,
        direction_z: f32,
        ray_length: f32,
        reserved0: u32,
        reserved1: u32,
        reserved2: u32,
    }
}

gpu_record! {
    /// One command's result. The `begin_tick` kernel writes the identity fields of every
    /// command position in use each tick, and zeroes the rest, so no result survives from an
    /// earlier tick; the command's kernels then fill what their status uses:
    ///
    /// | Status | Fields |
    /// | --- | --- |
    /// | `RESULT_EDITED` | `changed`, `not_resident`; `word0`/`word1` the window-local centre |
    /// | `RESULT_COLUMN` | `word0`/`word1` the column, `slot_index`/`slot_generation` |
    /// | `RESULT_RAY`, and any ray-targeted command | `v0` the surface outcome (`SURFACE_*`), `v1`/`v2` the hit's quad cell, `v3` its distance; `v4`/`v5` the nearest body's handle (`v4` = `BODY_NONE` when none), `v6` its distance |
    /// | `RESULT_BODY_SPAWNED` | `word0`/`word1` the anchor chunk, `v7`–`v9` the in-chunk cell x, cell y and height |
    /// | `RESULT_BODY_REMOVED` | `word0` landings, `word1` liquid entries, `slot_index` body contacts, `slot_generation` craters, `v7` splashes, `v8` absorbed energy |
    /// | `RESULT_JUMPED`, `RESULT_ATTRACTED`, `RESULT_LIFTED` | `count`: whether it jumped, or the bodies affected; `v7`–`v9` the window-local target |
    ///
    /// Floats are stored as their bits.
    pub struct ResultRecord {
        id_lo: u32,
        id_hi: u32,
        sequence: u32,
        status: u32,
        tick_lo: u32,
        tick_hi: u32,
        changed: atomic_u32,
        not_resident: atomic_u32,
        word0: u32,
        word1: u32,
        slot_index: u32,
        slot_generation: u32,
        count: atomic_u32,
        v0: u32,
        v1: u32,
        v2: u32,
        v3: u32,
        v4: u32,
        v5: u32,
        v6: u32,
        v7: u32,
        v8: u32,
        v9: u32,
        reserved0: u32,
    }
}

gpu_record! {
    /// Header of the cosmetic event buffer. `count` is incremented for every attempted append,
    /// so `count - capacity` appends were dropped when it exceeds the capacity, and `dropped`
    /// must say the same.
    pub struct EventHeader {
        count: atomic_u32,
        dropped: atomic_u32,
        capacity: u32,
        reserved0: u32,
    }
}

gpu_record! {
    /// A cosmetic event; `kind` is one of the `codes::EVENT_*` values.
    pub struct EventRecord {
        kind: u32,
        a: u32,
        b: u32,
        c: u32,
    }
}

gpu_record! {
    /// What a chunk slot holds. Kernels read it to report where a query was answered.
    pub struct ChunkSlotRecord {
        key_x: i32,
        key_y: i32,
        key_z: i32,
        generation: u32,
        revision_lo: u32,
        revision_hi: u32,
        /// One of the `codes::SLOT_*` states.
        state: u32,
        reserved0: u32,
    }
}

gpu_record! {
    /// Per-tick accounting of the material phases and the tools, written by the kernels with
    /// atomic adds. Quantities are 64-bit counters split into a low and a high word; the low
    /// word's wraps are carried exactly. `*_in`/`*_out` sum every processed cell's increase and
    /// decrease of the fluid volume or the solid column (bedrock plus loose material).
    pub struct DiagnosticsRecord {
        fluid_in_lo: atomic_u32,
        fluid_in_hi: atomic_u32,
        fluid_out_lo: atomic_u32,
        fluid_out_hi: atomic_u32,
        solid_in_lo: atomic_u32,
        solid_in_hi: atomic_u32,
        solid_out_lo: atomic_u32,
        solid_out_hi: atomic_u32,
        /// Magma turned into stone bedrock.
        quench_lo: atomic_u32,
        quench_hi: atomic_u32,
        /// Coolant boiled off beside magma.
        boil_lo: atomic_u32,
        boil_hi: atomic_u32,
        /// Oil consumed by fire.
        burn_lo: atomic_u32,
        burn_hi: atomic_u32,
        /// Water and blood drawn by growing foliage.
        uptake_lo: atomic_u32,
        uptake_hi: atomic_u32,
        /// Bedrock units eroded into loose material (internal to the solid column).
        erosion_lo: atomic_u32,
        erosion_hi: atomic_u32,
        /// What the tick's brush strokes added or removed.
        tool_fluid_in_lo: atomic_u32,
        tool_fluid_in_hi: atomic_u32,
        tool_solid_in_lo: atomic_u32,
        tool_solid_in_hi: atomic_u32,
        tool_solid_out_lo: atomic_u32,
        tool_solid_out_hi: atomic_u32,
        candidate_tiles: atomic_u32,
        process_tiles: atomic_u32,
        active_tiles: atomic_u32,
        awake_cells: atomic_u32,
        active_cells: atomic_u32,
        changed_cells: atomic_u32,
        /// Liquid transfers a receiver accepted less of than proposed (capacity).
        rejected_transfers: atomic_u32,
        /// Proposals or resolutions read with another tick's stamp; zero unless scheduling is
        /// wrong.
        stale_flow: atomic_u32,
        /// Receivers whose column took a different amount than their resolution accepted; zero
        /// unless the rules disagree.
        receive_mismatches: atomic_u32,
        reserved0: u32,
        reserved1: u32,
        reserved2: u32,
    }
}

gpu_record! {
    /// What a `MeasureTotals` command reports: 64-bit sums over every resident cell.
    pub struct TotalsRecord {
        fluid0_lo: atomic_u32,
        fluid0_hi: atomic_u32,
        fluid1_lo: atomic_u32,
        fluid1_hi: atomic_u32,
        fluid2_lo: atomic_u32,
        fluid2_hi: atomic_u32,
        fluid3_lo: atomic_u32,
        fluid3_hi: atomic_u32,
        fluid4_lo: atomic_u32,
        fluid4_hi: atomic_u32,
        fluid5_lo: atomic_u32,
        fluid5_hi: atomic_u32,
        fluid6_lo: atomic_u32,
        fluid6_hi: atomic_u32,
        fluid7_lo: atomic_u32,
        fluid7_hi: atomic_u32,
        elevation_lo: atomic_u32,
        elevation_hi: atomic_u32,
        granular_lo: atomic_u32,
        granular_hi: atomic_u32,
        cells_lo: atomic_u32,
        cells_hi: atomic_u32,
        reserved0: u32,
        reserved1: u32,
    }
}

gpu_record! {
    /// One body's persistent state: what every substep reads and writes. The position is an
    /// absolute anchor chunk plus a position inside it (cell axes, so `local_y` grows north and
    /// render z is its negative), so a body far outside the window keeps its precision and a
    /// resize moves nothing; kernels work in window-local render coordinates derived from it.
    /// Velocities are render-axis world units per second. The counters only grow (wrapping)
    /// and are what the CPU's body monitor derives events from.
    pub struct BodyState {
        anchor_x: i32,
        anchor_y: i32,
        local_x: f32,
        local_y: f32,
        height: f32,
        velocity_x: f32,
        velocity_y: f32,
        velocity_z: f32,
        /// `BODY_ALIVE`, `BODY_GROUNDED`, `BODY_SUSPENDED`, `BODY_PLANE_VALID`,
        /// `BODY_TERRAIN`.
        flags: u32,
        generation: u32,
        normal_x: f32,
        normal_y: f32,
        normal_z: f32,
        submerged: f32,
        previous_submerged: f32,
        pending_wake: f32,
        /// The filtered ground reference plane, around a point given relative to the anchor in
        /// cell axes.
        plane_origin_x: f32,
        plane_origin_y: f32,
        plane_height: f32,
        plane_slope_x: f32,
        plane_slope_y: f32,
        landings: u32,
        liquid_entries: u32,
        body_contacts: u32,
        craters: u32,
        splashes: u32,
        absorbed_energy: u32,
        /// Bodies touched in the last substep, and the liquid material at the centre column.
        touching: u32,
        liquid: u32,
        /// What this tick did, for its coalesced cosmetic event: `TICK_*` bits and the
        /// strongest impact speed.
        tick_events: u32,
        tick_impact: f32,
        /// The averaged terrain contact normal of the last substep (a wall as well as a
        /// ground), octahedral in two 16-bit snorms; meaningful with `BODY_TERRAIN`.
        terrain_normal: u32,
    }
}

gpu_record! {
    /// One body's configuration: written at spawn, read by every pass.
    pub struct BodyConfig {
        mass: f32,
        radius: f32,
        relative_density: f32,
        inverse_mass: f32,
    }
}

gpu_record! {
    /// One selected body's state at the end of a tick, as the CPU receives it.
    pub struct BodySampleRecord {
        /// `SAMPLE_OK`, or `SAMPLE_GONE` when the handle names no live body.
        status: u32,
        index: u32,
        generation: u32,
        flags: u32,
        anchor_x: i32,
        anchor_y: i32,
        local_x: f32,
        local_y: f32,
        height: f32,
        velocity_x: f32,
        velocity_y: f32,
        velocity_z: f32,
        normal_x: f32,
        normal_y: f32,
        normal_z: f32,
        submerged: f32,
        liquid: u32,
        touching: u32,
        landings: u32,
        liquid_entries: u32,
        body_contacts: u32,
        craters: u32,
        splashes: u32,
        absorbed_energy: u32,
    }
}

gpu_record! {
    /// Per-tick parameters of the body kernels (uniform). Region bases are word offsets into
    /// the bodies and bins buffers (`plan::BodyLayout`); they depend on the session's body
    /// capacity, not on the tick.
    pub struct BodyParams {
        /// One past the highest body slot in use: the extent of every per-body dispatch.
        body_count: u32,
        capacity: u32,
        substeps: u32,
        substep_seconds: f32,
        tick_seconds: f32,
        elevation_scale: f32,
        observe_count: u32,
        /// `BROADPHASE_BINS` or `BROADPHASE_ALL_PAIRS`.
        broadphase: u32,
        hash_mask: u32,
        state_base: u32,
        config_base: u32,
        work_base: u32,
        samples_base: u32,
        observe_base: u32,
        counts_base: u32,
        starts_base: u32,
        fill_base: u32,
        entries_base: u32,
        bounds_base: u32,
        /// Word offset of the [`BodyDiagnosticsRecord`] in the diagnostics buffer.
        diagnostics_base: u32,
        /// The highest surface any column can have, in world units: rays above it cannot hit
        /// a column that is not resident.
        max_surface: f32,
        reserved0: u32,
        reserved1: u32,
        reserved2: u32,
        reserved3: u32,
        reserved4: u32,
        reserved5: u32,
        reserved6: u32,
        reserved7: u32,
        reserved8: u32,
        reserved9: u32,
        reserved10: u32,
    }
}

gpu_record! {
    /// Which substep and solver iteration a body dispatch runs (uniform, one entry per
    /// combination at the uniform offset alignment, written once at creation): entry
    /// `substep * 16 + iteration`.
    pub struct BodyDispatch {
        substep: u32,
        iteration: u32,
        /// `iteration & 1`: which half of the ping-pong buffers the iteration reads.
        parity: u32,
        reserved0: u32,
    }
}

gpu_record! {
    /// Per-tick accounting of the body substeps, written with atomics. 64-bit quantities are
    /// a low and a high word. `residual_*` hold the bits of non-negative floats, compared as
    /// integers (`atomicMax`).
    pub struct BodyDiagnosticsRecord {
        candidate_pairs_lo: atomic_u32,
        candidate_pairs_hi: atomic_u32,
        contact_pairs: atomic_u32,
        swept_contacts: atomic_u32,
        terrain_contacts: atomic_u32,
        residual_overlap: atomic_u32,
        residual_approach: atomic_u32,
        speed_limited: atomic_u32,
        swept_bodies: atomic_u32,
        fluid_in_lo: atomic_u32,
        fluid_in_hi: atomic_u32,
        fluid_out_lo: atomic_u32,
        fluid_out_hi: atomic_u32,
        solid_in_lo: atomic_u32,
        solid_in_hi: atomic_u32,
        solid_out_lo: atomic_u32,
        solid_out_hi: atomic_u32,
        compaction_lo: atomic_u32,
        compaction_hi: atomic_u32,
        ice_broken: atomic_u32,
        organics_cleared: atomic_u32,
        craters: atomic_u32,
        ruts: atomic_u32,
        splashes: atomic_u32,
        wakes: atomic_u32,
        claims_lost: atomic_u32,
        live: atomic_u32,
        suspended: atomic_u32,
        /// Candidate slots the pair passes examined, before the generation test: the
        /// broadphase's work.
        pair_visits_lo: atomic_u32,
        pair_visits_hi: atomic_u32,
        reserved0: u32,
        reserved1: u32,
    }
}

/// Every record, in the order the generated module declares them.
pub const RECORDS: [RecordLayout; 15] = [
    TickParams::LAYOUT,
    DispatchParams::LAYOUT,
    CommandRecord::LAYOUT,
    ResultRecord::LAYOUT,
    EventHeader::LAYOUT,
    EventRecord::LAYOUT,
    ChunkSlotRecord::LAYOUT,
    DiagnosticsRecord::LAYOUT,
    TotalsRecord::LAYOUT,
    BodyState::LAYOUT,
    BodyConfig::LAYOUT,
    BodySampleRecord::LAYOUT,
    BodyParams::LAYOUT,
    BodyDispatch::LAYOUT,
    BodyDiagnosticsRecord::LAYOUT,
];

/// Words of a body's per-substep scratch in the bodies buffer, laid out by the body kernels
/// (`shaders/body/bindings.wgsl`): ping-pong positions and velocities, the substep's start, and
/// the terrain and displacement proposals between their prepare and apply passes.
pub const BODY_WORK_WORDS: u32 = 56;

/// Word offsets of a body's per-substep work region in the bodies buffer
/// ([`BODY_WORK_WORDS`] per body), its flags and the decisions its rounds record. Positions are
/// window-local render coordinates, three words each; `POS_A`/`POS_B` and `VEL_A`/`VEL_B` are
/// the solver's ping-pong halves, and every substep starts and ends in `A`. Generated into the
/// kernels' declarations as `WORK_*`, `DECIDE_*` and `DISPLACE_*`.
pub mod body_work {
    pub const WORK_POS_A: u32 = 0;
    pub const WORK_POS_B: u32 = 4;
    pub const WORK_VEL_A: u32 = 8;
    pub const WORK_VEL_B: u32 = 12;
    /// The position at the start of the substep.
    pub const WORK_PREV: u32 = 16;
    /// The contacts the solver scales each pair by.
    pub const WORK_PAIRS: u32 = 19;
    /// `ACTIVE`, `SWEPT`, `WAS_GROUNDED`.
    pub const WORK_FLAGS: u32 = 20;
    pub const WORK_REACH: u32 = 21;
    /// The terrain round's centre (lifted out of the ground, or stopped by a sweep).
    pub const WORK_CENTER: u32 = 22;
    /// The terrain round's decision and its parameters.
    pub const WORK_DECISION: u32 = 25;
    pub const WORK_APPROACH: u32 = 26;
    pub const WORK_NORMAL: u32 = 27;
    pub const WORK_ENERGY: u32 = 30;
    pub const WORK_EXTENSION: u32 = 31;
    pub const WORK_CRUSH_COST: u32 = 32;
    pub const WORK_SINKAGE_PER_SOFTNESS: u32 = 33;
    pub const WORK_MAX_SINKAGE: u32 = 34;
    pub const WORK_HEADING: u32 = 35;
    pub const WORK_BERM_CEILING: u32 = 37;
    pub const WORK_RIM_CEILING: u32 = 38;
    /// The displacement round's decision and its parameters.
    pub const WORK_DISPLACE: u32 = 39;
    pub const WORK_FLUID: u32 = 40;
    pub const WORK_BUDGET: u32 = 41;
    pub const WORK_INNER: u32 = 42;
    pub const WORK_OUTER: u32 = 43;
    pub const WORK_ENTRY_ENERGY: u32 = 44;
    pub const WORK_WAKE_HEADING: u32 = 45;
    /// Bodies touched at the end of the solve.
    pub const WORK_TOUCHING: u32 = 47;
    /// Where the body was binned this substep (the end of its integration): the position its
    /// candidate pairs are generated from.
    pub const WORK_BINNED: u32 = 48;
    /// The velocity the pair solve started from (the end of integration): the approach its
    /// restitution targets are taken from.
    pub const WORK_SOLVE_VELOCITY: u32 = 51;
    /// Integrated this substep (alive and not suspended).
    pub const WORK_ACTIVE: u32 = 1;
    pub const WORK_SWEPT: u32 = 2;
    pub const WORK_WAS_GROUNDED: u32 = 4;
    pub const DECIDE_NONE: u32 = 0;
    pub const DECIDE_CRATER: u32 = 1;
    pub const DECIDE_RUT: u32 = 2;
    pub const DISPLACE_NONE: u32 = 0;
    pub const DISPLACE_SPLASH: u32 = 1;
    pub const DISPLACE_WAKE: u32 = 2;

    // Positions and vectors take three words from their offset.
    const _: () = assert!(WORK_SOLVE_VELOCITY + 3 <= super::BODY_WORK_WORDS);

    /// Every constant, for the generated declarations.
    pub const ALL: [(&str, u32); 39] = [
        ("WORK_POS_A", WORK_POS_A),
        ("WORK_POS_B", WORK_POS_B),
        ("WORK_VEL_A", WORK_VEL_A),
        ("WORK_VEL_B", WORK_VEL_B),
        ("WORK_PREV", WORK_PREV),
        ("WORK_PAIRS", WORK_PAIRS),
        ("WORK_FLAGS", WORK_FLAGS),
        ("WORK_REACH", WORK_REACH),
        ("WORK_CENTER", WORK_CENTER),
        ("WORK_DECISION", WORK_DECISION),
        ("WORK_APPROACH", WORK_APPROACH),
        ("WORK_NORMAL", WORK_NORMAL),
        ("WORK_ENERGY", WORK_ENERGY),
        ("WORK_EXTENSION", WORK_EXTENSION),
        ("WORK_CRUSH_COST", WORK_CRUSH_COST),
        ("WORK_SINKAGE_PER_SOFTNESS", WORK_SINKAGE_PER_SOFTNESS),
        ("WORK_MAX_SINKAGE", WORK_MAX_SINKAGE),
        ("WORK_HEADING", WORK_HEADING),
        ("WORK_BERM_CEILING", WORK_BERM_CEILING),
        ("WORK_RIM_CEILING", WORK_RIM_CEILING),
        ("WORK_DISPLACE", WORK_DISPLACE),
        ("WORK_FLUID", WORK_FLUID),
        ("WORK_BUDGET", WORK_BUDGET),
        ("WORK_INNER", WORK_INNER),
        ("WORK_OUTER", WORK_OUTER),
        ("WORK_ENTRY_ENERGY", WORK_ENTRY_ENERGY),
        ("WORK_WAKE_HEADING", WORK_WAKE_HEADING),
        ("WORK_TOUCHING", WORK_TOUCHING),
        ("WORK_BINNED", WORK_BINNED),
        ("WORK_SOLVE_VELOCITY", WORK_SOLVE_VELOCITY),
        ("WORK_ACTIVE", WORK_ACTIVE),
        ("WORK_SWEPT", WORK_SWEPT),
        ("WORK_WAS_GROUNDED", WORK_WAS_GROUNDED),
        ("DECIDE_NONE", DECIDE_NONE),
        ("DECIDE_CRATER", DECIDE_CRATER),
        ("DECIDE_RUT", DECIDE_RUT),
        ("DISPLACE_NONE", DISPLACE_NONE),
        ("DISPLACE_SPLASH", DISPLACE_SPLASH),
        ("DISPLACE_WAKE", DISPLACE_WAKE),
    ];
}

/// 64-bit sums a [`DiagnosticsRecord`] holds, as `(low word, high word)` pairs.
pub fn wide(lo: u32, hi: u32) -> u64 {
    u64::from(lo) | (u64::from(hi) << 32)
}

/// Codes shared with the kernels. Values are part of the ABI: change one only together with
/// every kernel and test that reads it.
pub mod codes {
    pub const COMMAND_EDIT: u32 = 1;
    pub const COMMAND_QUERY_COLUMN: u32 = 2;
    /// A configuration change the CPU applies to the tick parameters; no kernel runs for it.
    pub const COMMAND_CONFIGURE: u32 = 3;
    pub const COMMAND_TOTALS: u32 = 4;
    pub const COMMAND_SPAWN: u32 = 5;
    pub const COMMAND_REMOVE: u32 = 6;
    pub const COMMAND_DRIVE: u32 = 7;
    pub const COMMAND_JUMP: u32 = 8;
    pub const COMMAND_ATTRACT: u32 = 9;
    pub const COMMAND_LIFT: u32 = 10;
    pub const COMMAND_QUERY_RAY: u32 = 11;
    /// A command the CPU already answered (a stale handle, no free body slot); its `tool`
    /// field holds the status, and no kernel runs for it.
    pub const COMMAND_REFUSED: u32 = 12;

    pub const TARGET_COLUMN: u32 = 0;
    pub const TARGET_RAY: u32 = 1;

    pub const RESULT_PENDING: u32 = 0;
    pub const RESULT_EDITED: u32 = 1;
    pub const RESULT_COLUMN: u32 = 2;
    pub const RESULT_COLUMN_NOT_RESIDENT: u32 = 3;
    pub const RESULT_CONFIGURED: u32 = 4;
    pub const RESULT_TOTALS: u32 = 5;
    pub const RESULT_TARGET_MISSED: u32 = 6;
    pub const RESULT_TARGET_NOT_RESIDENT: u32 = 7;
    pub const RESULT_BODY_SPAWNED: u32 = 8;
    pub const RESULT_BODY_CAPACITY: u32 = 9;
    pub const RESULT_BODY_REMOVED: u32 = 10;
    pub const RESULT_STALE_BODY: u32 = 11;
    pub const RESULT_DRIVEN: u32 = 12;
    pub const RESULT_JUMPED: u32 = 13;
    pub const RESULT_ATTRACTED: u32 = 14;
    pub const RESULT_LIFTED: u32 = 15;
    pub const RESULT_RAY: u32 = 16;

    /// What a ray met (result `v0`).
    pub const SURFACE_MISS: u32 = 0;
    pub const SURFACE_HIT: u32 = 1;
    pub const SURFACE_NOT_RESIDENT: u32 = 2;
    pub const BODY_NONE: u32 = u32::MAX;

    /// Body state flags.
    pub const BODY_ALIVE: u32 = 1;
    pub const BODY_GROUNDED: u32 = 2;
    pub const BODY_SUSPENDED: u32 = 4;
    pub const BODY_PLANE_VALID: u32 = 8;
    /// Touching terrain at the end of the last substep (`BodyState::terrain_normal` is set).
    pub const BODY_TERRAIN: u32 = 16;
    /// What a tick did to a body (`BodyState::tick_events`), for its cosmetic event.
    pub const TICK_CRATER: u32 = 1;
    pub const TICK_SPLASH: u32 = 2;
    pub const TICK_CONTACT: u32 = 4;
    pub const TICK_LANDED: u32 = 8;

    pub const SAMPLE_OK: u32 = 1;
    pub const SAMPLE_GONE: u32 = 2;

    pub const BROADPHASE_BINS: u32 = 0;
    pub const BROADPHASE_ALL_PAIRS: u32 = 1;

    /// A window entry or result slot with no chunk.
    pub const SLOT_NONE: u32 = u32::MAX;
    pub const SLOT_FREE: u32 = 0;
    pub const SLOT_RESIDENT: u32 = 1;

    /// Events appended by the pressure-injection kernel (fault injection only).
    pub const EVENT_INJECTED: u32 = 1;
    /// A body's coalesced notice for a tick: `a` its index, `b` its generation, `c` the
    /// `TICK_*` bits in the low byte and its strongest impact speed (world units per second,
    /// rounded) above them.
    pub const EVENT_BODY: u32 = 2;
}

/// The activity buffer, the flow words and the dispatch arguments, shared with the material
/// kernels. The regions' offsets depend on the window and are in [`crate::plan::ActivityLayout`].
pub mod activity {
    /// Cells along each side of an activity tile; one workgroup processes one tile.
    pub const TILE_EDGE: u32 = 8;
    pub const TILES_PER_CHUNK_EDGE: u32 = world_model::spatial::CHUNK_SIZE as u32 / TILE_EDGE;
    pub const TILES_PER_CHUNK: u32 = TILES_PER_CHUNK_EDGE * TILES_PER_CHUNK_EDGE;
    pub const TILE_NONE: u32 = u32::MAX;

    /// Words of the activity buffer's header: list counts, then reserved words.
    pub const HEADER_WORDS: u32 = 16;
    pub const CANDIDATE_COUNT: u32 = 0;
    pub const PROCESS_COUNT: u32 = 1;

    /// Fields of a tile record, in words.
    pub const TILE_RECORD_WORDS: u32 = 16;
    /// Set by `react` (for the next tick) and by edits; read and cleared by `scan_tiles`.
    pub const TILE_CANDIDATE: u32 = 0;
    /// `TILE_AWAKE`, `TILE_ACTIVE` for this tick; reset by `scan_tiles`, set by `wake_tiles`.
    pub const TILE_STATE: u32 = 1;
    /// Two bits per cell, valid only while the state is awake.
    pub const TILE_WAKE: u32 = 2;
    /// Changed and persisting cells, one two-word bitmap per tick parity.
    pub const TILE_CHANGED: u32 = 6;
    pub const TILE_PERSIST: u32 = 10;
    /// Cells this tick's edits changed.
    pub const TILE_EDITED: u32 = 14;
    pub const TILE_AWAKE: u32 = 1;
    pub const TILE_ACTIVE: u32 = 2;

    /// Words of the dispatch-argument buffer: `x, y, z` for the candidate and process lists.
    pub const ARGS_WORDS: u32 = 6;
    pub const ARGS_CANDIDATES: u32 = 0;
    pub const ARGS_PROCESS: u32 = 3;
    /// Workgroups along x of a list dispatch; a longer list continues along y. WebGPU's
    /// guaranteed `maxComputeWorkgroupsPerDimension`.
    pub const DISPATCH_SPLIT: u32 = 65_535;

    pub const ACTIVITY_WAKE_ALL: u32 = 1;
    pub const ACTIVITY_DENSE: u32 = 2;
    pub const PHASE_FREEZE: u32 = 1;
    pub const PHASE_MELT: u32 = 2;

    /// A flow word: per family a direction (`DIR_NONE` = 15) and an amount, and the tick stamp.
    pub const FLOW_GRANULAR_DIR_SHIFT: u32 = 0;
    pub const FLOW_GRANULAR_AMOUNT_SHIFT: u32 = 4;
    pub const FLOW_LIQUID_DIR_SHIFT: u32 = 6;
    pub const FLOW_LIQUID_AMOUNT_SHIFT: u32 = 10;
    pub const FLOW_STAMP_SHIFT: u32 = 24;
    pub const FLOW_DIR_MASK: u32 = 0xF;
    pub const FLOW_GRANULAR_AMOUNT_MASK: u32 = 0x3;
    pub const FLOW_LIQUID_AMOUNT_MASK: u32 = 0x1FF;
    pub const FLOW_STAMP_MASK: u32 = 0xFF << FLOW_STAMP_SHIFT;

    // The amount fields hold the largest transfer of each family.
    const _: () = assert!(world_model::rules::MAX_GRANULAR_TRANSFER <= FLOW_GRANULAR_AMOUNT_MASK);
    const _: () =
        assert!(world_model::cell::WorldCell::MAX_FLUID_VOL as u32 <= FLOW_LIQUID_AMOUNT_MASK);
    const _: () = assert!(FLOW_LIQUID_AMOUNT_SHIFT + 9 <= FLOW_STAMP_SHIFT);
}

/// The GPU code of an edit tool.
pub const fn tool_code(tool: EditTool) -> u32 {
    match tool {
        EditTool::Water => 0,
        EditTool::Lava => 1,
        EditTool::Oil => 2,
        EditTool::Ice => 3,
        EditTool::Fire => 4,
        EditTool::Sand => 5,
        EditTool::Raise => 6,
        EditTool::Lower => 7,
    }
}

fn screaming(name: &str) -> String {
    name.to_ascii_uppercase()
}

/// Appends the cell field constants derived from `layout` (normally [`WorldCell::LAYOUT`]):
/// for each field, its word, its shift within that word and its mask. A field that straddled
/// the two words could not be read with one shift, so it is reported instead.
pub fn write_cell_declarations(text: &mut String, layout: &[Field]) -> Result<(), String> {
    for field in layout {
        let word = field.shift / 32;
        let shift = field.shift % 32;
        if shift + field.bits > 32 {
            return Err(format!("{} straddles the two cell words", field.name));
        }
        let name = screaming(field.name);
        let mask = (1u64 << field.bits) - 1;
        let _ = writeln!(text, "const CELL_{name}_WORD: u32 = {word}u;");
        let _ = writeln!(text, "const CELL_{name}_SHIFT: u32 = {shift}u;");
        let _ = writeln!(text, "const CELL_{name}_MASK: u32 = {mask:#x}u;");
    }
    Ok(())
}

/// The generated WGSL module: layout constants, codes, materials and record structs.
pub fn wgsl_declarations() -> String {
    wgsl_declarations_for(&WorldCell::LAYOUT)
        .unwrap_or_else(|problem| format!("// invalid cell layout: {problem}\n"))
}

/// The generated module for an arbitrary cell layout; the layout probe uses a deliberately
/// wrong one to prove a wrong declaration is detected.
pub fn wgsl_declarations_for(layout: &[Field]) -> Result<String, String> {
    let mut text = String::from(
        "// Generated by gpu_world::layout::wgsl_declarations from the Rust owners of each\n\
         // layout. Do not edit; change the owner instead.\n\n",
    );
    let _ = writeln!(text, "const CHUNK_SIZE: u32 = {CHUNK_SIZE}u;");
    let _ = writeln!(text, "const CHUNK_CELLS: u32 = {CHUNK_CELL_COUNT}u;");
    write_cell_declarations(&mut text, layout)?;
    let maxima = [
        ("ELEVATION", WorldCell::MAX_ELEVATION),
        ("FLUID_VOLUME", u32::from(WorldCell::MAX_FLUID_VOL)),
        ("GRANULAR_VOLUME", u32::from(WorldCell::MAX_GRANULAR_VOL)),
        ("SURFACE_STATE", u32::from(WorldCell::MAX_SURFACE_STATE)),
    ];
    for (name, value) in maxima {
        let _ = writeln!(text, "const CELL_MAX_{name}: u32 = {value}u;");
    }
    let materials = [
        ("FLUID_EMPTY", FluidMat::EMPTY.0),
        ("FLUID_WATER", FluidMat::FLUID_WATER.0),
        ("FLUID_MAGMA", FluidMat::FLUID_MAGMA.0),
        ("FLUID_OIL", FluidMat::FLUID_OIL.0),
        ("FLUID_ICE", FluidMat::FLUID_ICE.0),
        ("GRANULAR_SAND", GranularMat::GRANULAR_SAND.0),
        (
            "SAND_COMPACTS_TO",
            GranularMat::GRANULAR_SAND.to_terrain().0,
        ),
        ("SURFACE_FIRE", SurfaceMat::SURFACE_FIRE.0),
        ("TERRAIN_EMPTY", TerrainMat::EMPTY.0),
    ];
    for (name, value) in materials {
        let _ = writeln!(text, "const {name}: u32 = {value}u;");
    }
    for tool in EditTool::ALL {
        let _ = writeln!(
            text,
            "const TOOL_{}: u32 = {}u;",
            screaming(&format!("{tool:?}")),
            tool_code(tool)
        );
    }
    text.push_str("// The liquid a tool paints, or FLUID_EMPTY when it paints none.\n");
    text.push_str("fn tool_fluid(tool: u32) -> u32 {\n    switch tool {\n");
    for tool in EditTool::ALL {
        if let Some(fluid) = tool.fluid() {
            let _ = writeln!(
                text,
                "        case {}u: {{ return {}u; }}",
                tool_code(tool),
                fluid.0
            );
        }
    }
    text.push_str("        default: { return 0u; }\n    }\n}\n");
    let constants = [
        ("COMMAND_EDIT", codes::COMMAND_EDIT),
        ("COMMAND_QUERY_COLUMN", codes::COMMAND_QUERY_COLUMN),
        ("RESULT_PENDING", codes::RESULT_PENDING),
        ("RESULT_EDITED", codes::RESULT_EDITED),
        ("RESULT_COLUMN", codes::RESULT_COLUMN),
        (
            "RESULT_COLUMN_NOT_RESIDENT",
            codes::RESULT_COLUMN_NOT_RESIDENT,
        ),
        ("COMMAND_CONFIGURE", codes::COMMAND_CONFIGURE),
        ("COMMAND_TOTALS", codes::COMMAND_TOTALS),
        ("RESULT_CONFIGURED", codes::RESULT_CONFIGURED),
        ("RESULT_TOTALS", codes::RESULT_TOTALS),
        ("SLOT_NONE", codes::SLOT_NONE),
        ("SLOT_FREE", codes::SLOT_FREE),
        ("SLOT_RESIDENT", codes::SLOT_RESIDENT),
        ("EVENT_INJECTED", codes::EVENT_INJECTED),
    ];
    for (name, value) in constants {
        let _ = writeln!(text, "const {name}: u32 = {value:#x}u;");
    }
    write_body_codes(&mut text);
    text.push('\n');
    write_material_functions(&mut text);
    write_rule_constants(&mut text);
    write_body_constants(&mut text);
    write_offsets(&mut text, "DIAG", &DiagnosticsRecord::LAYOUT);
    write_offsets(&mut text, "TOTALS", &TotalsRecord::LAYOUT);
    write_offsets(&mut text, "BDIAG", &BodyDiagnosticsRecord::LAYOUT);
    text.push('\n');
    for record in RECORDS {
        text.push_str(&record.wgsl());
    }
    Ok(text)
}

/// The body command, result, flag and sample codes.
fn write_body_codes(text: &mut String) {
    use codes::*;
    let constants = [
        ("COMMAND_SPAWN", COMMAND_SPAWN),
        ("COMMAND_REMOVE", COMMAND_REMOVE),
        ("COMMAND_DRIVE", COMMAND_DRIVE),
        ("COMMAND_JUMP", COMMAND_JUMP),
        ("COMMAND_ATTRACT", COMMAND_ATTRACT),
        ("COMMAND_LIFT", COMMAND_LIFT),
        ("COMMAND_QUERY_RAY", COMMAND_QUERY_RAY),
        ("COMMAND_REFUSED", COMMAND_REFUSED),
        ("TARGET_COLUMN", TARGET_COLUMN),
        ("TARGET_RAY", TARGET_RAY),
        ("RESULT_TARGET_MISSED", RESULT_TARGET_MISSED),
        ("RESULT_TARGET_NOT_RESIDENT", RESULT_TARGET_NOT_RESIDENT),
        ("RESULT_BODY_SPAWNED", RESULT_BODY_SPAWNED),
        ("RESULT_BODY_CAPACITY", RESULT_BODY_CAPACITY),
        ("RESULT_BODY_REMOVED", RESULT_BODY_REMOVED),
        ("RESULT_STALE_BODY", RESULT_STALE_BODY),
        ("RESULT_DRIVEN", RESULT_DRIVEN),
        ("RESULT_JUMPED", RESULT_JUMPED),
        ("RESULT_ATTRACTED", RESULT_ATTRACTED),
        ("RESULT_LIFTED", RESULT_LIFTED),
        ("RESULT_RAY", RESULT_RAY),
        ("SURFACE_MISS", SURFACE_MISS),
        ("SURFACE_HIT", SURFACE_HIT),
        ("SURFACE_NOT_RESIDENT", SURFACE_NOT_RESIDENT),
        ("BODY_NONE", BODY_NONE),
        ("BODY_ALIVE", BODY_ALIVE),
        ("BODY_GROUNDED", BODY_GROUNDED),
        ("BODY_SUSPENDED", BODY_SUSPENDED),
        ("BODY_PLANE_VALID", BODY_PLANE_VALID),
        ("BODY_TERRAIN", BODY_TERRAIN),
        ("TICK_CRATER", TICK_CRATER),
        ("TICK_SPLASH", TICK_SPLASH),
        ("TICK_CONTACT", TICK_CONTACT),
        ("TICK_LANDED", TICK_LANDED),
        ("SAMPLE_OK", SAMPLE_OK),
        ("SAMPLE_GONE", SAMPLE_GONE),
        ("BROADPHASE_BINS", BROADPHASE_BINS),
        ("BROADPHASE_ALL_PAIRS", BROADPHASE_ALL_PAIRS),
        ("EVENT_BODY", EVENT_BODY),
        ("BODY_STATE_WORDS", BodyState::WORDS as u32),
        ("BODY_CONFIG_WORDS", BodyConfig::WORDS as u32),
        ("BODY_SAMPLE_WORDS", BodySampleRecord::WORDS as u32),
        ("BODY_WORK_WORDS", BODY_WORK_WORDS),
    ];
    for (name, value) in constants {
        let _ = writeln!(text, "const {name}: u32 = {value:#x}u;");
    }
    for (name, value) in body_work::ALL {
        let _ = writeln!(text, "const {name}: u32 = {value}u;");
    }
}

/// The sphere solver's constants, from `world_model::{body, units}`, as WGSL constants: every
/// tuning value the baseline engine reads is the one the kernels read.
fn write_body_constants(text: &mut String) {
    let k = tuning::KINEMATIC;
    let d = tuning::DEFORMATION;
    let c = tuning::CRATER;
    let floats: [(&str, f32); 61] = [
        ("GRAVITY_Y", units::GRAVITY_Y),
        ("MAX_BODY_SPEED", body::MAX_BODY_SPEED),
        ("TERRAIN_SWEEP_STEP", body::TERRAIN_SWEEP_STEP),
        ("DRIVE_ACCELERATION", body::DRIVE_ACCELERATION),
        ("JUMP_IMPULSE", body::JUMP_IMPULSE),
        ("ATTRACT_ACCELERATION", body::ATTRACT_ACCELERATION),
        ("LIFT_ACCELERATION", body::LIFT_ACCELERATION),
        (
            "LIFT_CENTERING_ACCELERATION",
            body::LIFT_CENTERING_ACCELERATION,
        ),
        ("LIFT_INFLUENCE_RADIUS", body::LIFT_INFLUENCE_RADIUS),
        ("TARGET_EPSILON_SQUARED", body::TARGET_EPSILON_SQUARED),
        (
            "MIN_BODY_RELATIVE_DENSITY",
            *body::BODY_RELATIVE_DENSITY.start(),
        ),
        ("RAY_MAX_DISTANCE", config::RAY_MAX_DISTANCE),
        ("CONTACT_SLOP", tuning::CONTACT_SLOP),
        ("GROUND_NORMAL_MIN_Y", tuning::GROUND_NORMAL_MIN_Y),
        ("DUPLICATE_NORMAL_COSINE", tuning::DUPLICATE_NORMAL_COSINE),
        ("INTERNAL_EDGE_COSINE", tuning::INTERNAL_EDGE_COSINE),
        ("CONSTRAINT_TOLERANCE", tuning::CONSTRAINT_TOLERANCE),
        ("GROUND_NORMAL_BAND", tuning::GROUND_NORMAL_BAND),
        ("DEGENERATE_DISTANCE", tuning::DEGENERATE_DISTANCE),
        ("COINCIDENT_DISTANCE", tuning::COINCIDENT_DISTANCE),
        (
            "RESISTANCE_CLIFF_THRESHOLD",
            tuning::RESISTANCE_CLIFF_THRESHOLD,
        ),
        ("RESISTANCE_MULTIPLIER", tuning::RESISTANCE_MULTIPLIER),
        ("SPLASH_MIN_SPEED", tuning::SPLASH_MIN_SPEED),
        ("SPLASH_UNITS_PER_ENERGY", tuning::SPLASH_UNITS_PER_ENERGY),
        ("SPLASH_ENERGY_PER_UNIT", tuning::SPLASH_ENERGY_PER_UNIT),
        ("SPLASH_CAVITY_RATIO", tuning::SPLASH_CAVITY_RATIO),
        ("SPLASH_RING_OUTER_RATIO", tuning::SPLASH_RING_OUTER_RATIO),
        ("WAKE_MIN_SPEED", tuning::WAKE_MIN_SPEED),
        ("WAKE_LAYER_UNITS", tuning::WAKE_LAYER_UNITS),
        ("WAKE_RING_WIDTH", tuning::WAKE_RING_WIDTH),
        ("WAKE_MAX_PENDING", tuning::WAKE_MAX_PENDING),
        ("RUT_REFERENCE_RING_RATIO", tuning::RUT_REFERENCE_RING_RATIO),
        ("RUT_TRIM_DEPTH", tuning::RUT_TRIM_DEPTH),
        (
            "RUT_REFERENCE_SMOOTHING_SECONDS",
            tuning::RUT_REFERENCE_SMOOTHING_SECONDS,
        ),
        ("MAX_SINKAGE_RATIO", tuning::MAX_SINKAGE_RATIO),
        ("PLANE_SINGULARITY", tuning::PLANE_SINGULARITY),
        ("RUT_BERM_INNER_RATIO", tuning::RUT_BERM_INNER_RATIO),
        ("RUT_BERM_OUTER_RATIO", tuning::RUT_BERM_OUTER_RATIO),
        ("RUT_BERM_HEIGHT_RATIO", tuning::RUT_BERM_HEIGHT_RATIO),
        ("RUT_BERM_SIDE_COSINE", tuning::RUT_BERM_SIDE_COSINE),
        ("RUT_HEADING_MIN_SPEED", tuning::RUT_HEADING_MIN_SPEED),
        ("QUANTIZATION_TOLERANCE", tuning::QUANTIZATION_TOLERANCE),
        ("AIR_DRAG_RATE", k.air_drag_rate),
        ("ROLLING_RESISTANCE", k.rolling_resistance_coefficient),
        ("GROUND_DAMPING_RATE", k.ground_damping_rate),
        ("STATIC_FRICTION_COEFFICIENT", k.static_friction_coefficient),
        ("STATIC_FRICTION_SPEED", k.static_friction_speed),
        ("IMPACT_RESTITUTION", k.impact_restitution),
        ("MIN_DEFORMATION_ENERGY", d.min_deformation_energy),
        ("ENERGY_TO_DEFORMATION", d.energy_to_deformation_scale),
        ("COST_DISPLACE_GRANULAR", d.cost_displace_granular),
        ("COST_CRUSH_TERRAIN", d.cost_crush_terrain),
        ("RUT_DEPTH_SCALE", d.rut_depth_scale),
        ("RUT_REFERENCE_MASS", d.rut_reference_mass),
        ("RUT_WIDTH_RATIO", d.rut_width_ratio),
        ("RUT_COST_PER_UNIT", d.rut_cost_per_unit),
        ("RIM_INNER_RATIO", c.rim_inner_radius_ratio),
        ("RIM_EXPANSION", c.rim_expansion_factor),
        ("BLADE_CEILING_RATIO", c.blade_ceiling_ratio),
        ("MAX_DEFORMATION_SIZE_RATIO", c.max_deformation_size_ratio),
        ("GROUNDING_DRIFT_BUFFER", c.grounding_drift_buffer),
    ];
    for (name, value) in floats {
        let _ = writeln!(text, "const {name}: f32 = {value:?};");
    }
    let _ = writeln!(
        text,
        "const MINIMUM_BOUNCE_SPEED: f32 = {:?};",
        c.minimum_bounce_speed
    );
    let words: [(&str, u32); 11] = [
        ("CONTACT_ITERATIONS", tuning::CONTACT_ITERATIONS as u32),
        ("VELOCITY_ITERATIONS", tuning::VELOCITY_ITERATIONS as u32),
        (
            "MAX_MANIFOLD_CONTACTS",
            tuning::MAX_MANIFOLD_CONTACTS as u32,
        ),
        (
            "RESISTANCE_SAMPLE_RINGS",
            tuning::RESISTANCE_SAMPLE_RINGS as u32,
        ),
        (
            "RESISTANCE_SAMPLE_STRIDE",
            tuning::RESISTANCE_SAMPLE_STRIDE as u32,
        ),
        (
            "SPLASH_MAX_PER_CELL",
            u32::from(tuning::SPLASH_MAX_PER_CELL),
        ),
        ("WAKE_MAX_PER_CELL", u32::from(tuning::WAKE_MAX_PER_CELL)),
        (
            "WAKE_MAX_RING_PER_CELL",
            u32::from(tuning::WAKE_MAX_RING_PER_CELL),
        ),
        (
            "RUT_REFERENCE_SAMPLES",
            tuning::RUT_REFERENCE_SAMPLES as u32,
        ),
        (
            "RUT_MIN_REFERENCE_SAMPLES",
            tuning::RUT_MIN_REFERENCE_SAMPLES as u32,
        ),
        ("RUT_PRESS_PER_SUBSTEP", u32::from(d.rut_press_per_substep)),
    ];
    for (name, value) in words {
        let _ = writeln!(text, "const {name}: u32 = {value}u;");
    }
    let _ = writeln!(
        text,
        "const MAX_RIM_DEPOSIT: u32 = {}u;",
        c.max_rim_deposit_per_cell
    );
}

/// WGSL functions that load and store `layout`'s record from a flat `array<u32>` global named
/// `array`, the record starting at word `base`: `load_<prefix>(base)` and
/// `store_<prefix>(base, value)`. Generated, like the structs, so a field cannot be read at
/// one offset and written at another. Atomic fields are not supported here.
pub fn word_access(layout: &RecordLayout, array: &str, prefix: &str) -> String {
    let mut text = format!(
        "fn load_{prefix}(base: u32) -> {} {{\n    var r: {};\n",
        layout.name, layout.name
    );
    for (index, field) in layout.fields.iter().enumerate() {
        let read = format!("{array}[base + {index}u]");
        let value = match field.ty {
            WordType::U32 => read,
            WordType::I32 => format!("bitcast<i32>({read})"),
            WordType::F32 => format!("bitcast<f32>({read})"),
            WordType::AtomicU32 => panic!("{}.{} is atomic", layout.name, field.name),
        };
        let _ = writeln!(text, "    r.{} = {value};", field.name);
    }
    text.push_str("    return r;\n}\n");
    let _ = writeln!(text, "fn store_{prefix}(base: u32, r: {}) {{", layout.name);
    for (index, field) in layout.fields.iter().enumerate() {
        let _ = writeln!(
            text,
            "    {array}[base + {index}u] = bitcast<u32>(r.{});",
            field.name
        );
    }
    text.push_str("}\n");
    text
}

/// Word offsets of a record's fields (`<PREFIX>_<FIELD>`) and its length (`<PREFIX>_WORDS`),
/// for kernels that address it as a flat array of atomics.
fn write_offsets(text: &mut String, prefix: &str, layout: &RecordLayout) {
    for (index, field) in layout.fields.iter().enumerate() {
        let _ = writeln!(
            text,
            "const {prefix}_{}: u32 = {index}u;",
            screaming(field.name)
        );
    }
    let _ = writeln!(
        text,
        "const {prefix}_WORDS: u32 = {}u;",
        layout.fields.len()
    );
}

/// A WGSL function returning `value(m)` for material codes `0..count`, as a switch.
fn material_table(
    text: &mut String,
    name: &str,
    ty: &str,
    count: u8,
    value: impl Fn(u8) -> String,
) {
    let _ = writeln!(text, "fn {name}(m: u32) -> {ty} {{\n    switch m {{");
    for code in 0..count {
        let _ = writeln!(text, "        case {code}u: {{ return {}; }}", value(code));
    }
    let _ = writeln!(
        text,
        "        default: {{ return {}; }}\n    }}\n}}",
        value(count - 1)
    );
}

/// Material properties as WGSL functions, generated from their `world-model` owners so the
/// kernels cannot keep a second copy. `MATERIAL_NONE` stands for "no such material".
fn write_material_functions(text: &mut String) {
    let none = codes_none();
    let _ = writeln!(text, "const MATERIAL_NONE: u32 = {none}u;");
    let _ = writeln!(text, "const REPOSE_NONE: u32 = {}u;", u16::MAX);
    let u = |value: u32| format!("{value}u");
    let b = |value: bool| value.to_string();
    material_table(text, "fluid_flow_divisor", "u32", 8, |m| {
        u(fluid_properties(FluidMat(m)).flow_divisor)
    });
    material_table(text, "fluid_levels_out", "bool", 8, |m| {
        b(fluid_properties(FluidMat(m)).levels_out)
    });
    material_table(text, "fluid_erosion_factor", "u32", 8, |m| {
        u(fluid_properties(FluidMat(m)).erosion_factor)
    });
    material_table(text, "fluid_is_solid", "bool", 8, |m| {
        b(FluidMat(m).is_solid())
    });
    material_table(text, "fluid_is_coolant", "bool", 8, |m| {
        b(FluidMat(m).is_coolant())
    });
    material_table(text, "fluid_frozen", "u32", 8, |m| {
        u(FluidMat(m).frozen().map_or(none, |f| u32::from(f.0)))
    });
    material_table(text, "fluid_melted", "u32", 8, |m| {
        u(FluidMat(m).melted().map_or(none, |f| u32::from(f.0)))
    });
    material_table(text, "fluid_is_fuel", "bool", 8, |m| {
        b(is_combustible(SurfaceMat::EMPTY, FluidMat(m)))
    });
    material_table(text, "surface_is_combustible", "bool", 16, |m| {
        b(is_combustible(SurfaceMat(m), FluidMat::EMPTY))
    });
    material_table(text, "granular_repose", "u32", 16, |m| {
        u(u32::from(granular_repose(GranularMat(m))))
    });
    material_table(text, "granular_to_terrain", "u32", 16, |m| {
        u(u32::from(GranularMat(m).to_terrain().0))
    });
    material_table(text, "terrain_to_granular", "u32", 16, |m| {
        u(TerrainMat(m).to_granular().map_or(none, |g| u32::from(g.0)))
    });
    let f = |value: f32| format!("{value:?}");
    material_table(text, "fluid_relative_density", "f32", 8, |m| {
        f(fluid_properties(FluidMat(m)).relative_density)
    });
    material_table(text, "fluid_drag_rate", "f32", 8, |m| {
        f(fluid_properties(FluidMat(m)).drag_rate)
    });
    material_table(text, "terrain_crush_hardness", "f32", 16, |m| {
        f(terrain_crush_hardness(TerrainMat(m)))
    });
    // `ground_softness` reads the loose layer when there is one, the bedrock otherwise, and
    // nothing under ice; the kernels combine these two tables the same way.
    material_table(text, "loose_softness", "f32", 16, |m| {
        let mut cell = WorldCell::default();
        cell.set_granular_mat(GranularMat(m));
        cell.set_granular_vol(1);
        f(ground_softness(&cell))
    });
    material_table(text, "bedrock_softness", "f32", 16, |m| {
        let mut cell = WorldCell::default();
        cell.set_terrain_mat(TerrainMat(m));
        f(ground_softness(&cell))
    });
}

const fn codes_none() -> u32 {
    0xFF
}

/// The automaton's constants, from `world_model::rules`, and the activity layout.
fn write_rule_constants(text: &mut String) {
    let words: [(&str, u32); 46] = [
        ("RULE_LIQUID", SimulationRules::LIQUID_BIT),
        ("RULE_GRANULAR", SimulationRules::GRANULAR_BIT),
        ("RULE_THERMAL", SimulationRules::THERMAL_BIT),
        ("RULE_FIRE", SimulationRules::FIRE_BIT),
        ("RULE_BIOLOGY", SimulationRules::BIOLOGY_BIT),
        ("BIOLOGY_ODDS_DENOMINATOR", rules::BIOLOGY_ODDS_DENOMINATOR),
        ("FIRE_ODDS_DENOMINATOR", rules::FIRE_ODDS_DENOMINATOR),
        ("MAX_GRANULAR_TRANSFER", rules::MAX_GRANULAR_TRANSFER),
        ("MIN_TRANSFER_HEAD", rules::MIN_TRANSFER_HEAD),
        ("STICTION_VARIANTS", rules::STICTION_VARIANTS),
        ("EROSION_MASK", rules::EROSION_MASK),
        (
            "STEAM_LOSS_PER_CONTACT",
            u32::from(rules::STEAM_LOSS_PER_CONTACT),
        ),
        ("CHANCE_RESOLUTION", rules::CHANCE_RESOLUTION),
        ("PHASE_HASH_CADENCE", rules::PHASE_HASH_CADENCE),
        (
            "FLAME_LIFETIME_TICKS",
            u32::from(rules::FLAME_LIFETIME_TICKS),
        ),
        ("OIL_BURN_RATE", u32::from(rules::OIL_BURN_RATE)),
        (
            "IGNITION_ODDS_DENOMINATOR",
            rules::IGNITION_ODDS_DENOMINATOR,
        ),
        ("ASH_RESIDUE_ODDS", rules::ASH_RESIDUE_ODDS),
        (
            "FOLIAGE_MATURE_STATE",
            u32::from(rules::FOLIAGE_MATURE_STATE),
        ),
        ("MAX_CAPILLARY_LIFT", rules::MAX_CAPILLARY_LIFT),
        ("FOLIAGE_WATER_USE", u32::from(rules::FOLIAGE_WATER_USE)),
        (
            "DROWNING_FLUID_VOLUME",
            u32::from(rules::DROWNING_FLUID_VOLUME),
        ),
        (
            "SEEDING_FOLIAGE_STATE",
            u32::from(rules::SEEDING_FOLIAGE_STATE),
        ),
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
        ("WAKE_HALO", u32::from(rules::WAKE_HALO)),
        ("WAKE_ACTIVE_MIN", u32::from(rules::WAKE_ACTIVE_MIN)),
        ("WAKE_ACTIVE_FRESH", u32::from(rules::WAKE_ACTIVE_FRESH)),
        ("WAKE_CHANGE_RADIUS", rules::WAKE_CHANGE_RADIUS as u32),
        ("SALT_BIOLOGY_SAMPLE", salt::BIOLOGY_SAMPLE),
        ("SALT_BIOLOGY_SEEDING", salt::BIOLOGY_SEEDING),
        ("SALT_BIOLOGY_SPONTANEOUS", salt::BIOLOGY_SPONTANEOUS),
        ("SALT_BIOLOGY_GROWTH", salt::BIOLOGY_GROWTH),
        ("SALT_BIOLOGY_MUD_DRYING", salt::BIOLOGY_MUD_DRYING),
        ("SALT_FIRE_SAMPLE", salt::FIRE_SAMPLE),
        ("SALT_FIRE_ASH", salt::FIRE_ASH),
        ("SALT_FIRE_IGNITION", salt::FIRE_IGNITION),
        ("COMPASS_N", u32::from(CompassFlags::FACING_N)),
        ("COMPASS_S", u32::from(CompassFlags::FACING_S)),
        ("COMPASS_E", u32::from(CompassFlags::FACING_E)),
        ("COMPASS_W", u32::from(CompassFlags::FACING_W)),
    ];
    for (name, value) in words {
        let _ = writeln!(text, "const {name}: u32 = {value}u;");
    }
    use activity::*;
    let layout: [(&str, u32); 37] = [
        ("TILE_EDGE", TILE_EDGE),
        ("TILES_PER_CHUNK_EDGE", TILES_PER_CHUNK_EDGE),
        ("TILES_PER_CHUNK", TILES_PER_CHUNK),
        ("TILE_NONE", TILE_NONE),
        ("ACTIVITY_CANDIDATE_COUNT", CANDIDATE_COUNT),
        ("ACTIVITY_PROCESS_COUNT", PROCESS_COUNT),
        ("TILE_RECORD_WORDS", TILE_RECORD_WORDS),
        ("TILE_CANDIDATE", TILE_CANDIDATE),
        ("TILE_STATE", TILE_STATE),
        ("TILE_WAKE", TILE_WAKE),
        ("TILE_CHANGED", TILE_CHANGED),
        ("TILE_PERSIST", TILE_PERSIST),
        ("TILE_EDITED", TILE_EDITED),
        ("TILE_AWAKE", TILE_AWAKE),
        ("TILE_ACTIVE", TILE_ACTIVE),
        ("ARGS_CANDIDATES", ARGS_CANDIDATES),
        ("ARGS_PROCESS", ARGS_PROCESS),
        ("DISPATCH_SPLIT", DISPATCH_SPLIT),
        ("ACTIVITY_WAKE_ALL", ACTIVITY_WAKE_ALL),
        ("ACTIVITY_DENSE", ACTIVITY_DENSE),
        ("PHASE_FREEZE", PHASE_FREEZE),
        ("PHASE_MELT", PHASE_MELT),
        ("FLOW_GRANULAR_DIR_SHIFT", FLOW_GRANULAR_DIR_SHIFT),
        ("FLOW_GRANULAR_AMOUNT_SHIFT", FLOW_GRANULAR_AMOUNT_SHIFT),
        ("FLOW_LIQUID_DIR_SHIFT", FLOW_LIQUID_DIR_SHIFT),
        ("FLOW_LIQUID_AMOUNT_SHIFT", FLOW_LIQUID_AMOUNT_SHIFT),
        ("FLOW_STAMP_SHIFT", FLOW_STAMP_SHIFT),
        ("FLOW_DIR_MASK", FLOW_DIR_MASK),
        ("FLOW_GRANULAR_AMOUNT_MASK", FLOW_GRANULAR_AMOUNT_MASK),
        ("FLOW_LIQUID_AMOUNT_MASK", FLOW_LIQUID_AMOUNT_MASK),
        ("FLOW_STAMP_MASK", FLOW_STAMP_MASK),
        ("TERRAIN_STONE", u32::from(TerrainMat::TERRAIN_STONE.0)),
        ("TERRAIN_DIRT", u32::from(TerrainMat::TERRAIN_DIRT.0)),
        ("GRANULAR_EMPTY", u32::from(GranularMat::EMPTY.0)),
        ("GRANULAR_DIRT", u32::from(GranularMat::GRANULAR_DIRT.0)),
        ("GRANULAR_MUD", u32::from(GranularMat::GRANULAR_MUD.0)),
        ("FLUID_BLOOD", u32::from(FluidMat::FLUID_BLOOD.0)),
    ];
    for (name, value) in layout {
        let _ = writeln!(text, "const {name}: u32 = {value:#x}u;");
    }
    let surfaces = [
        ("SURFACE_EMPTY", SurfaceMat::EMPTY.0),
        ("SURFACE_FOLIAGE", SurfaceMat::SURFACE_FOLIAGE.0),
        ("SURFACE_ASH", SurfaceMat::SURFACE_ASH.0),
    ];
    for (name, value) in surfaces {
        let _ = writeln!(text, "const {name}: u32 = {value}u;");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_match_words_written_out_by_hand() {
        let command = CommandRecord {
            kind: codes::COMMAND_EDIT,
            id_lo: 0x89AB_CDEF,
            id_hi: 0x0123_4567,
            sequence: 3,
            tool: tool_code(EditTool::Sand),
            center_x: -5,
            center_y: 70,
            radius: 64,
            strength: 255,
            target_kind: codes::TARGET_RAY,
            body_generation: 6,
            d: 0.5,
            origin_z: -2.0,
            ray_length: 1000.0,
            ..CommandRecord::default()
        };
        let mut words = [0u32; CommandRecord::WORDS];
        command.write_words(&mut words);
        assert_eq!(
            words,
            [
                1,
                0x89AB_CDEF,
                0x0123_4567,
                3,
                5,
                0xFFFF_FFFB,
                70,
                64,
                255,
                0,
                0,
                1,
                0,
                6,
                0,
                0,
                0,
                // 0.5f32, -2.0f32 and 1000.0f32 written out as bits.
                0x3F00_0000,
                0,
                0,
                0xC000_0000,
                0,
                0,
                0,
                0x447A_0000,
                0,
                0,
                0
            ]
        );
        assert_eq!(CommandRecord::read_words(&words), command);

        let body = BodyState {
            anchor_x: -3,
            generation: 2,
            local_y: -1.5,
            touching: 7,
            ..BodyState::default()
        };
        let mut bytes = [0u8; 128];
        body.write_bytes(&mut bytes);
        assert_eq!(&bytes[0..4], &(-3i32).to_le_bytes());
        // -1.5f32 is 0xBFC00000, little-endian, at word 3.
        assert_eq!(&bytes[12..16], &[0x00, 0x00, 0xC0, 0xBF]);
        assert_eq!(&bytes[36..40], &[2, 0, 0, 0]);
        assert_eq!(&bytes[108..112], &[7, 0, 0, 0]);
        assert_eq!(BodyState::read_bytes(&bytes), body);
    }

    #[test]
    fn record_sizes_and_offsets_are_fixed() {
        // Strides written out by hand: a changed record is an ABI change.
        assert_eq!(
            [
                TickParams::BYTES,
                DispatchParams::BYTES,
                CommandRecord::BYTES,
                ResultRecord::BYTES,
                EventHeader::BYTES,
                EventRecord::BYTES,
                ChunkSlotRecord::BYTES,
                DiagnosticsRecord::BYTES,
                TotalsRecord::BYTES,
                BodyState::BYTES,
                BodyConfig::BYTES,
                BodySampleRecord::BYTES,
                BodyParams::BYTES,
                BodyDispatch::BYTES,
                BodyDiagnosticsRecord::BYTES,
            ],
            [
                128, 16, 112, 96, 16, 16, 32, 144, 96, 128, 16, 96, 128, 16, 128
            ]
        );
        assert_eq!(TickParams::LAYOUT.offset_of("origin_key_x"), Some(36));
        assert_eq!(TickParams::LAYOUT.offset_of("rule_key"), Some(68));
        assert_eq!(TickParams::LAYOUT.offset_of("records_base"), Some(92));
        assert_eq!(DiagnosticsRecord::LAYOUT.offset_of("quench_lo"), Some(32));
        assert_eq!(
            DiagnosticsRecord::LAYOUT.offset_of("candidate_tiles"),
            Some(96)
        );
        assert_eq!(TotalsRecord::LAYOUT.offset_of("elevation_lo"), Some(64));
        assert_eq!(ResultRecord::LAYOUT.offset_of("changed"), Some(24));
        assert_eq!(ResultRecord::LAYOUT.offset_of("word1"), Some(36));
        assert_eq!(ResultRecord::LAYOUT.offset_of("count"), Some(48));
        assert_eq!(ResultRecord::LAYOUT.offset_of("v9"), Some(88));
        assert_eq!(CommandRecord::LAYOUT.offset_of("body_index"), Some(48));
        assert_eq!(CommandRecord::LAYOUT.offset_of("ray_length"), Some(96));
        assert_eq!(ChunkSlotRecord::LAYOUT.offset_of("generation"), Some(12));
        assert_eq!(TickParams::LAYOUT.offset_of("window_width"), Some(16));
        assert_eq!(TickParams::LAYOUT.offset_of("injected_events"), Some(32));
        assert_eq!(BodyState::LAYOUT.offset_of("velocity_x"), Some(20));
        assert_eq!(BodyState::LAYOUT.offset_of("landings"), Some(84));
        assert_eq!(BodySampleRecord::LAYOUT.offset_of("landings"), Some(72));
        assert_eq!(BodyParams::LAYOUT.offset_of("diagnostics_base"), Some(76));
        assert_eq!(
            BodyDiagnosticsRecord::LAYOUT.offset_of("compaction_lo"),
            Some(68)
        );
        // Uniform structs must be multiples of 16 bytes.
        for bytes in [
            TickParams::BYTES,
            DispatchParams::BYTES,
            BodyParams::BYTES,
            BodyDispatch::BYTES,
        ] {
            assert_eq!(bytes % 16, 0);
        }
    }

    #[test]
    fn generated_wgsl_declares_records_and_cell_fields() {
        let text = wgsl_declarations();
        assert!(text.contains("struct ResultRecord {\n    id_lo: u32,"));
        assert!(text.contains("    changed: atomic<u32>,"));
        assert!(text.contains("const CELL_ELEVATION_WORD: u32 = 0u;"));
        assert!(text.contains("const CELL_ELEVATION_SHIFT: u32 = 20u;"));
        assert!(text.contains("const CELL_ELEVATION_MASK: u32 = 0xfffu;"));
        assert!(text.contains("const CELL_FLUID_VOLUME_WORD: u32 = 1u;"));
        assert!(text.contains("const CELL_FLUID_VOLUME_SHIFT: u32 = 0u;"));
        assert!(
            text.contains("case 1u: { return 2u; }"),
            "lava paints magma"
        );
        assert!(!text.contains("invalid cell layout"));
        // Material tables come from world-model: magma flows with divisor 3, water levels out,
        // ice is solid and melts to water, stone weathers to gravel.
        assert!(text.contains("fn fluid_flow_divisor(m: u32) -> u32 {\n    switch m {\n        case 0u: { return 2u; }\n        case 1u: { return 2u; }\n        case 2u: { return 3u; }"));
        assert!(text.contains("fn fluid_melted(m: u32) -> u32 {"));
        assert!(text.contains("        case 7u: { return 1u; }\n        default"));
        assert!(text.contains("const DIAG_STALE_FLOW: u32 = 31u;"));
        assert!(text.contains("const TOTALS_CELLS_LO: u32 = 20u;"));
        assert!(text.contains("const SALT_FIRE_IGNITION: u32 = 203u;"));
    }

    #[test]
    fn combustibility_splits_into_surface_and_fuel() {
        // The kernels test `surface_is_combustible(s) || fluid_is_fuel(f)`.
        for surface in 0..16 {
            for fluid in 0..8 {
                let (surface, fluid) = (SurfaceMat(surface), FluidMat(fluid));
                assert_eq!(
                    is_combustible(surface, fluid),
                    is_combustible(surface, FluidMat::EMPTY)
                        || is_combustible(SurfaceMat::EMPTY, fluid)
                );
            }
        }
    }

    #[test]
    fn a_field_across_the_word_boundary_is_refused() {
        let mut layout = WorldCell::LAYOUT;
        layout[5].shift = 24; // elevation, 12 bits from bit 24, would cross bit 32
        assert!(wgsl_declarations_for(&layout).is_err());
    }
}
