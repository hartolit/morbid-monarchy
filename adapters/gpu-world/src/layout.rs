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
    cell::{Field, WorldCell},
    command::EditTool,
    material::{
        CompassFlags, FluidMat, GranularMat, SurfaceMat, TerrainMat, fluid_properties,
        granular_repose, is_combustible,
    },
    rules::{self, SimulationRules, salt},
    spatial::{CHUNK_CELL_COUNT, CHUNK_SIZE},
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
    /// - [`codes::COMMAND_EDIT`]: `tool`, `center_x`, `center_y`, `radius`, `strength`;
    /// - [`codes::COMMAND_QUERY_COLUMN`]: `cell_x`, `cell_y`.
    ///
    /// Coordinates are window-local cells. `sequence` is the command's position in the tick and
    /// the index of its result record.
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
        reserved0: u32,
    }
}

gpu_record! {
    /// One command's result. The `begin_tick` kernel writes the identity fields of every
    /// command position in use each tick, so no result survives from an earlier tick; the
    /// command's own kernel then fills the rest.
    pub struct ResultRecord {
        id_lo: u32,
        id_hi: u32,
        sequence: u32,
        status: u32,
        tick_lo: u32,
        tick_hi: u32,
        /// Edit: cells that took a new value.
        changed: atomic_u32,
        /// Edit: cells outside the resident window or in a missing chunk.
        not_resident: atomic_u32,
        /// Query: the column's two words.
        word0: u32,
        word1: u32,
        /// Query: where the column was read.
        slot_index: u32,
        slot_generation: u32,
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
    /// One sphere body's physical state, as prompt 04's body kernels will hold it. Positions
    /// are window-local world units (render axes, y up), velocities world units per second.
    /// Declared here so its layout is executed with the others before a solver depends on it.
    pub struct BodyRecord {
        handle_index: u32,
        handle_generation: u32,
        flags: u32,
        reserved0: u32,
        position_x: f32,
        position_y: f32,
        position_z: f32,
        radius: f32,
        velocity_x: f32,
        velocity_y: f32,
        velocity_z: f32,
        mass: f32,
        relative_density: f32,
        submerged_fraction: f32,
        reserved1: u32,
        reserved2: u32,
    }
}

/// Every record, in the order the generated module declares them.
pub const RECORDS: [RecordLayout; 10] = [
    TickParams::LAYOUT,
    DispatchParams::LAYOUT,
    CommandRecord::LAYOUT,
    ResultRecord::LAYOUT,
    EventHeader::LAYOUT,
    EventRecord::LAYOUT,
    ChunkSlotRecord::LAYOUT,
    DiagnosticsRecord::LAYOUT,
    TotalsRecord::LAYOUT,
    BodyRecord::LAYOUT,
];

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

    pub const RESULT_PENDING: u32 = 0;
    pub const RESULT_EDITED: u32 = 1;
    pub const RESULT_COLUMN: u32 = 2;
    pub const RESULT_COLUMN_NOT_RESIDENT: u32 = 3;
    pub const RESULT_CONFIGURED: u32 = 4;
    pub const RESULT_TOTALS: u32 = 5;

    /// A window entry or result slot with no chunk.
    pub const SLOT_NONE: u32 = u32::MAX;
    pub const SLOT_FREE: u32 = 0;
    pub const SLOT_RESIDENT: u32 = 1;

    /// Events appended by the pressure-injection kernel (fault injection only).
    pub const EVENT_INJECTED: u32 = 1;
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
    text.push('\n');
    write_material_functions(&mut text);
    write_rule_constants(&mut text);
    write_offsets(&mut text, "DIAG", &DiagnosticsRecord::LAYOUT);
    write_offsets(&mut text, "TOTALS", &TotalsRecord::LAYOUT);
    text.push('\n');
    for record in RECORDS {
        text.push_str(&record.wgsl());
    }
    Ok(text)
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
            cell_x: 0,
            cell_y: 0,
            reserved0: 0,
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
                0
            ]
        );
        assert_eq!(CommandRecord::read_words(&words), command);

        let body = BodyRecord {
            handle_index: 9,
            handle_generation: 2,
            position_y: -1.5,
            mass: 4.0,
            ..BodyRecord::default()
        };
        let mut bytes = [0u8; 64];
        body.write_bytes(&mut bytes);
        assert_eq!(&bytes[0..8], &[9, 0, 0, 0, 2, 0, 0, 0]);
        // -1.5f32 is 0xBFC00000, little-endian, at word 5.
        assert_eq!(&bytes[20..24], &[0x00, 0x00, 0xC0, 0xBF]);
        assert_eq!(&bytes[44..48], &4.0f32.to_le_bytes());
        assert_eq!(BodyRecord::read_bytes(&bytes), body);
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
                BodyRecord::BYTES,
            ],
            [128, 16, 48, 48, 16, 16, 32, 144, 96, 64]
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
        assert_eq!(ChunkSlotRecord::LAYOUT.offset_of("generation"), Some(12));
        assert_eq!(TickParams::LAYOUT.offset_of("window_width"), Some(16));
        assert_eq!(TickParams::LAYOUT.offset_of("injected_events"), Some(32));
        assert_eq!(BodyRecord::LAYOUT.offset_of("velocity_x"), Some(32));
        // Uniform structs must be multiples of 16 bytes.
        assert_eq!(TickParams::BYTES % 16, 0);
        assert_eq!(DispatchParams::BYTES % 16, 0);
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
