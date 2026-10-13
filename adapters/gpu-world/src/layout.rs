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
    material::{FluidMat, GranularMat, SurfaceMat, TerrainMat},
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

/// A record shared with kernels.
pub trait GpuRecord: Sized + Copy {
    const LAYOUT: RecordLayout;
    const WORDS: usize;
    const BYTES: u64 = Self::WORDS as u64 * 4;

    fn write_words(&self, out: &mut [u32]);
    fn read_words(words: &[u32]) -> Self;

    /// Appends the record's little-endian bytes.
    fn write_bytes(&self, out: &mut [u8]) {
        let mut words = [0u32; 32];
        self.write_words(&mut words[..Self::WORDS]);
        let (chunks, _) = out.as_chunks_mut::<4>();
        for (chunk, word) in chunks.iter_mut().zip(&words[..Self::WORDS]) {
            *chunk = word.to_le_bytes();
        }
    }

    fn read_bytes(bytes: &[u8]) -> Self {
        let mut words = [0u32; 32];
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
    /// Per-tick parameters (uniform). Sixteen words, so the uniform struct is a multiple of 16
    /// bytes. Kernels see window-local coordinates only: cell `(0, 0)` is the first cell of the
    /// window's lowest chunk, so no absolute coordinate reaches a kernel.
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
        reserved0: u32,
        reserved1: u32,
        reserved2: u32,
        reserved3: u32,
        reserved4: u32,
        reserved5: u32,
        reserved6: u32,
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
pub const RECORDS: [RecordLayout; 8] = [
    TickParams::LAYOUT,
    DispatchParams::LAYOUT,
    CommandRecord::LAYOUT,
    ResultRecord::LAYOUT,
    EventHeader::LAYOUT,
    EventRecord::LAYOUT,
    ChunkSlotRecord::LAYOUT,
    BodyRecord::LAYOUT,
];

/// Codes shared with the kernels. Values are part of the ABI: change one only together with
/// every kernel and test that reads it.
pub mod codes {
    pub const COMMAND_EDIT: u32 = 1;
    pub const COMMAND_QUERY_COLUMN: u32 = 2;

    pub const RESULT_PENDING: u32 = 0;
    pub const RESULT_EDITED: u32 = 1;
    pub const RESULT_COLUMN: u32 = 2;
    pub const RESULT_COLUMN_NOT_RESIDENT: u32 = 3;

    /// A window entry or result slot with no chunk.
    pub const SLOT_NONE: u32 = u32::MAX;
    pub const SLOT_FREE: u32 = 0;
    pub const SLOT_RESIDENT: u32 = 1;

    /// Events appended by the pressure-injection kernel (fault injection only).
    pub const EVENT_INJECTED: u32 = 1;
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
        ("SLOT_NONE", codes::SLOT_NONE),
        ("SLOT_FREE", codes::SLOT_FREE),
        ("SLOT_RESIDENT", codes::SLOT_RESIDENT),
        ("EVENT_INJECTED", codes::EVENT_INJECTED),
    ];
    for (name, value) in constants {
        let _ = writeln!(text, "const {name}: u32 = {value:#x}u;");
    }
    text.push('\n');
    for record in RECORDS {
        text.push_str(&record.wgsl());
    }
    Ok(text)
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
                BodyRecord::BYTES,
            ],
            [64, 16, 48, 48, 16, 16, 32, 64]
        );
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
    }

    #[test]
    fn a_field_across_the_word_boundary_is_refused() {
        let mut layout = WorldCell::LAYOUT;
        layout[5].shift = 24; // elevation, 12 bits from bit 24, would cross bit 32
        assert!(wgsl_declarations_for(&layout).is_err());
    }
}
