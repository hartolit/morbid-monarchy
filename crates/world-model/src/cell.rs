//! The packed column cell and the rules that change it.
//!
//! A `WorldCell` is one horizontal cell of the 2.5D world: bedrock, loose material, a liquid or
//! frozen layer and a surface, all in one `u64`. [`WorldCell::LAYOUT`] is the single owner of
//! the bit layout; the GPU word layout, saved bytes and every accessor derive from it. The column
//! rules below are the only code that moves liquid in or out of a cell, freezes it, or turns it
//! into rock, so conservation and phase behavior are decided in one place.
//!
//! Accessors carry `#[inline(always)]` because the CPU solvers call them per cell from another
//! crate, where small functions are not inlined without it.

use bytemuck::{Pod, Zeroable};

use crate::material::{FluidMat, GranularMat, SurfaceMat, TerrainMat};

/// One bit field of the packed cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field {
    pub name: &'static str,
    /// Position of the field's least significant bit; bit 0 is the low bit of word 0.
    pub shift: u32,
    pub bits: u32,
}

impl Field {
    /// The largest value the field can hold.
    #[inline(always)]
    pub const fn max(self) -> u64 {
        (1 << self.bits) - 1
    }

    #[inline(always)]
    const fn get(self, word: u64) -> u64 {
        (word >> self.shift) & self.max()
    }

    #[inline(always)]
    const fn set(self, word: u64, value: u64) -> u64 {
        (word & !(self.max() << self.shift)) | ((value & self.max()) << self.shift)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Pod, Zeroable)]
#[repr(transparent)]
pub struct WorldCell(pub u64);

impl WorldCell {
    // Word 0 (bits 0-31): geometry and appearance.
    pub const TERRAIN_MATERIAL: Field = Field {
        name: "terrain_material",
        shift: 0,
        bits: 4,
    };
    pub const SURFACE_MATERIAL: Field = Field {
        name: "surface_material",
        shift: 4,
        bits: 4,
    };
    pub const GRANULAR_MATERIAL: Field = Field {
        name: "granular_material",
        shift: 8,
        bits: 4,
    };
    pub const FLUID_MATERIAL: Field = Field {
        name: "fluid_material",
        shift: 12,
        bits: 3,
    };
    pub const VARIANTS: Field = Field {
        name: "variants",
        shift: 15,
        bits: 5,
    };
    /// Bedrock height in elevation units.
    pub const ELEVATION: Field = Field {
        name: "elevation",
        shift: 20,
        bits: 12,
    };
    // Word 1 (bits 32-63): quantities and state.
    /// Liquid or frozen volume, in elevation units of column height.
    pub const FLUID_VOLUME: Field = Field {
        name: "fluid_volume",
        shift: 32,
        bits: 9,
    };
    /// Loose material height above bedrock, in elevation units.
    pub const GRANULAR_VOLUME: Field = Field {
        name: "granular_volume",
        shift: 41,
        bits: 4,
    };
    pub const SURFACE_STATE: Field = Field {
        name: "surface_state",
        shift: 45,
        bits: 9,
    };
    pub const TERRAIN_STATE: Field = Field {
        name: "terrain_state",
        shift: 54,
        bits: 6,
    };
    /// Liquid momentum flags (`CompassFlags`).
    pub const COMPASS: Field = Field {
        name: "compass",
        shift: 60,
        bits: 4,
    };

    /// Every field in bit order. The fields tile all 64 bits without overlap.
    pub const LAYOUT: [Field; 11] = [
        Self::TERRAIN_MATERIAL,
        Self::SURFACE_MATERIAL,
        Self::GRANULAR_MATERIAL,
        Self::FLUID_MATERIAL,
        Self::VARIANTS,
        Self::ELEVATION,
        Self::FLUID_VOLUME,
        Self::GRANULAR_VOLUME,
        Self::SURFACE_STATE,
        Self::TERRAIN_STATE,
        Self::COMPASS,
    ];

    pub const MAX_ELEVATION: u32 = Self::ELEVATION.max() as u32; // 4,095
    pub const MAX_FLUID_VOL: u16 = Self::FLUID_VOLUME.max() as u16; // 511
    pub const MAX_GRANULAR_VOL: u16 = Self::GRANULAR_VOLUME.max() as u16; // 15
    pub const MAX_SURFACE_STATE: u16 = Self::SURFACE_STATE.max() as u16; // 511
    pub const MAX_TERRAIN_STATE: u8 = Self::TERRAIN_STATE.max() as u8; // 63
    pub const MAX_VARIANTS: u8 = Self::VARIANTS.max() as u8; // 31

    /// The two 32-bit words in GPU order: word 0 holds bits 0-31, word 1 bits 32-63. Shaders do
    /// not need 64-bit integers to read a cell.
    #[inline(always)]
    pub const fn to_words(self) -> [u32; 2] {
        [self.0 as u32, (self.0 >> 32) as u32]
    }

    #[inline(always)]
    pub const fn from_words(words: [u32; 2]) -> Self {
        Self(words[0] as u64 | ((words[1] as u64) << 32))
    }

    #[inline(always)]
    pub fn terrain_mat(&self) -> TerrainMat {
        TerrainMat(Self::TERRAIN_MATERIAL.get(self.0) as u8)
    }
    #[inline(always)]
    pub fn set_terrain_mat(&mut self, mat: TerrainMat) {
        self.0 = Self::TERRAIN_MATERIAL.set(self.0, mat.0 as u64);
    }

    #[inline(always)]
    pub fn surface_mat(&self) -> SurfaceMat {
        SurfaceMat(Self::SURFACE_MATERIAL.get(self.0) as u8)
    }
    #[inline(always)]
    pub fn set_surface_mat(&mut self, mat: SurfaceMat) {
        self.0 = Self::SURFACE_MATERIAL.set(self.0, mat.0 as u64);
    }

    #[inline(always)]
    pub fn granular_mat(&self) -> GranularMat {
        GranularMat(Self::GRANULAR_MATERIAL.get(self.0) as u8)
    }
    #[inline(always)]
    pub fn set_granular_mat(&mut self, mat: GranularMat) {
        self.0 = Self::GRANULAR_MATERIAL.set(self.0, mat.0 as u64);
    }

    #[inline(always)]
    pub fn fluid_mat(&self) -> FluidMat {
        FluidMat(Self::FLUID_MATERIAL.get(self.0) as u8)
    }
    #[inline(always)]
    pub fn set_fluid_mat(&mut self, mat: FluidMat) {
        self.0 = Self::FLUID_MATERIAL.set(self.0, mat.0 as u64);
    }

    #[inline(always)]
    pub fn variants(&self) -> u8 {
        Self::VARIANTS.get(self.0) as u8
    }
    #[inline(always)]
    pub fn set_variants(&mut self, val: u8) {
        self.0 = Self::VARIANTS.set(self.0, val as u64);
    }

    #[inline(always)]
    pub fn elevation(&self) -> u16 {
        Self::ELEVATION.get(self.0) as u16
    }
    #[inline(always)]
    pub fn set_elevation(&mut self, val: u16) {
        self.0 = Self::ELEVATION.set(self.0, val as u64);
    }

    #[inline(always)]
    pub fn fluid_vol(&self) -> u16 {
        Self::FLUID_VOLUME.get(self.0) as u16
    }
    #[inline(always)]
    pub fn set_fluid_vol(&mut self, val: u16) {
        self.0 = Self::FLUID_VOLUME.set(self.0, val as u64);
    }

    #[inline(always)]
    pub fn granular_vol(&self) -> u16 {
        Self::GRANULAR_VOLUME.get(self.0) as u16
    }

    /// Sets the loose material height. Volume beyond the field's capacity is compressed into
    /// bedrock of the matching solid material, raising the elevation (up to its ceiling).
    #[inline(always)]
    pub fn set_granular_vol(&mut self, val: u16) {
        if val > Self::MAX_GRANULAR_VOL {
            let excess = val - Self::MAX_GRANULAR_VOL;
            let current_elev = self.elevation();
            self.set_terrain_mat(self.granular_mat().to_terrain());
            self.set_elevation(
                current_elev
                    .saturating_add(excess)
                    .min(Self::MAX_ELEVATION as u16),
            );
            self.0 = Self::GRANULAR_VOLUME.set(self.0, Self::MAX_GRANULAR_VOL as u64);
        } else {
            self.0 = Self::GRANULAR_VOLUME.set(self.0, val as u64);
        }
    }

    #[inline(always)]
    pub fn surface_state(&self) -> u16 {
        Self::SURFACE_STATE.get(self.0) as u16
    }
    #[inline(always)]
    pub fn set_surface_state(&mut self, val: u16) {
        self.0 = Self::SURFACE_STATE.set(self.0, val as u64);
    }

    #[inline(always)]
    pub fn terrain_state(&self) -> u8 {
        Self::TERRAIN_STATE.get(self.0) as u8
    }
    #[inline(always)]
    pub fn set_terrain_state(&mut self, val: u8) {
        self.0 = Self::TERRAIN_STATE.set(self.0, val as u64);
    }

    #[inline(always)]
    pub fn compass(&self) -> u8 {
        Self::COMPASS.get(self.0) as u8
    }
    #[inline(always)]
    pub fn set_compass(&mut self, flags: u8) {
        self.0 = Self::COMPASS.set(self.0, flags as u64);
    }
}

// Column rules. Every system that moves liquid, freezes it or turns it into rock goes through
// these, so the conservation and phase rules are enforced in one place.
impl WorldCell {
    /// Height of the column's solid top in elevation units: bedrock, loose material and any
    /// frozen liquid. Bodies rest on it and loose material slides over it.
    #[inline(always)]
    pub fn solid_top(&self) -> u32 {
        let frozen = if self.fluid_mat().is_solid() {
            self.fluid_vol() as u32
        } else {
            0
        };
        self.elevation() as u32 + self.granular_vol() as u32 + frozen
    }

    /// Height of the column including all of its liquid, in elevation units.
    #[inline(always)]
    pub fn liquid_head(&self) -> u32 {
        self.elevation() as u32 + self.granular_vol() as u32 + self.fluid_vol() as u32
    }

    /// Removes up to `amount` of the resident liquid and clears the liquid state once the
    /// column runs dry. Returns the volume removed.
    #[inline(always)]
    pub fn drain_fluid(&mut self, amount: u16) -> u16 {
        let removed = amount.min(self.fluid_vol());
        let remaining = self.fluid_vol() - removed;
        self.set_fluid_vol(remaining);
        if remaining == 0 {
            self.set_fluid_mat(FluidMat::EMPTY);
            self.set_compass(0);
        }
        removed
    }

    /// Adds liquid that arrived by flow or displacement and returns the volume accepted; the
    /// caller keeps anything beyond the column's capacity.
    ///
    /// When the column already holds a different liquid:
    /// - magma meeting a coolant is quenched, turning the magma's share into stone bedrock one
    ///   unit per unit, so the column keeps its height;
    /// - otherwise the side bringing more volume keeps the column and absorbs the other. The
    ///   incoming amount is compared, not the donor's whole column: a flow only carries a
    ///   fraction of the head difference, so one liquid cannot sweep across another pool cell
    ///   by cell, and no volume is ever discarded.
    ///
    /// Frozen columns are solid and accept nothing.
    #[inline(always)]
    pub fn receive_fluid(&mut self, material: FluidMat, amount: u16) -> u16 {
        let resident = self.fluid_mat();
        if amount == 0 || material == FluidMat::EMPTY || resident.is_solid() {
            return 0;
        }

        if resident == material || resident == FluidMat::EMPTY {
            let accepted = amount.min(Self::MAX_FLUID_VOL - self.fluid_vol());
            self.set_fluid_mat(material);
            self.set_fluid_vol(self.fluid_vol() + accepted);
            return accepted;
        }

        if material == FluidMat::FLUID_MAGMA && resident.is_coolant() {
            return self.raise_bedrock(TerrainMat::TERRAIN_STONE, amount);
        }

        if resident == FluidMat::FLUID_MAGMA && material.is_coolant() {
            self.quench();
            return self.receive_fluid(material, amount);
        }

        let accepted = amount.min(Self::MAX_FLUID_VOL - self.fluid_vol());
        if accepted > self.fluid_vol() {
            self.set_fluid_mat(material);
        }
        self.set_fluid_vol(self.fluid_vol() + accepted);
        accepted
    }

    /// Places liquid deliberately (tools, world edits): `material` takes the column over and
    /// keeps the resident volume, so painting never deletes liquid. Returns the volume added.
    #[inline(always)]
    pub fn inject_fluid(&mut self, material: FluidMat, amount: u16) -> u16 {
        if material == FluidMat::EMPTY {
            return 0;
        }
        let added = amount.min(Self::MAX_FLUID_VOL - self.fluid_vol());
        if self.fluid_mat() != material {
            self.set_fluid_mat(material);
            self.set_compass(0);
        }
        self.set_fluid_vol(self.fluid_vol() + added);
        added
    }

    /// Turns resident magma into stone bedrock of the same height. Returns whether it did.
    ///
    /// Known defect, kept here unchanged so the extraction preserves behavior: the magma is
    /// drained before a capacity-limited bedrock raise, so magma beyond the elevation ceiling
    /// disappears. Prompt 03 replaces this with conservative ceiling accounting.
    #[inline(always)]
    pub fn quench(&mut self) -> bool {
        if self.fluid_mat() != FluidMat::FLUID_MAGMA {
            return false;
        }
        let volume = self.fluid_vol();
        self.drain_fluid(volume);
        self.raise_bedrock(TerrainMat::TERRAIN_STONE, volume);
        true
    }

    /// Freezes the resident liquid in place. Returns whether the column changed phase.
    #[inline(always)]
    pub fn freeze(&mut self) -> bool {
        match self.fluid_mat().frozen() {
            Some(solid) => {
                self.set_fluid_mat(solid);
                self.set_compass(0);
                true
            }
            None => false,
        }
    }

    /// Melts a frozen column in place. Returns whether the column changed phase.
    #[inline(always)]
    pub fn thaw(&mut self) -> bool {
        match self.fluid_mat().melted() {
            Some(liquid) => {
                self.set_fluid_mat(liquid);
                true
            }
            None => false,
        }
    }

    /// Raises bedrock by `units` of `material`, returning the units that fit under the
    /// elevation ceiling.
    #[inline(always)]
    fn raise_bedrock(&mut self, material: TerrainMat, units: u16) -> u16 {
        let room = (Self::MAX_ELEVATION as u16).saturating_sub(self.elevation());
        let raised = units.min(room);
        self.set_elevation(self.elevation() + raised);
        self.set_terrain_mat(material);
        raised
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::CompassFlags;

    fn column(material: FluidMat, volume: u16) -> WorldCell {
        let mut cell = WorldCell::default();
        cell.set_elevation(100);
        cell.inject_fluid(material, volume);
        cell
    }

    #[test]
    fn small_inflow_is_absorbed_by_a_deeper_pool() {
        let mut lake = column(FluidMat::FLUID_WATER, 40);
        assert_eq!(lake.receive_fluid(FluidMat::FLUID_OIL, 6), 6);
        assert_eq!(lake.fluid_mat(), FluidMat::FLUID_WATER);
        assert_eq!(lake.fluid_vol(), 46);
    }

    #[test]
    fn larger_inflow_takes_the_column_over_without_losing_volume() {
        let mut film = column(FluidMat::FLUID_WATER, 3);
        assert_eq!(film.receive_fluid(FluidMat::FLUID_OIL, 10), 10);
        assert_eq!(film.fluid_mat(), FluidMat::FLUID_OIL);
        assert_eq!(film.fluid_vol(), 13);
    }

    #[test]
    fn magma_and_coolant_quench_into_rock_either_way_round() {
        let mut lake = column(FluidMat::FLUID_WATER, 20);
        assert_eq!(lake.receive_fluid(FluidMat::FLUID_MAGMA, 8), 8);
        assert_eq!((lake.elevation(), lake.fluid_vol()), (108, 20));
        assert_eq!(lake.terrain_mat(), TerrainMat::TERRAIN_STONE);

        let mut lava = column(FluidMat::FLUID_MAGMA, 15);
        assert_eq!(lava.receive_fluid(FluidMat::FLUID_WATER, 4), 4);
        assert_eq!((lava.elevation(), lava.fluid_vol()), (115, 4));
        assert_eq!(lava.fluid_mat(), FluidMat::FLUID_WATER);
    }

    #[test]
    fn ice_is_solid_and_phase_changes_keep_volume() {
        let mut pond = column(FluidMat::FLUID_WATER, 30);
        let floor = pond.solid_top();
        assert!(pond.freeze());
        assert_eq!(pond.solid_top(), floor + 30);
        assert_eq!(pond.receive_fluid(FluidMat::FLUID_WATER, 5), 0);
        assert!(pond.thaw());
        assert_eq!(
            (pond.fluid_mat(), pond.fluid_vol()),
            (FluidMat::FLUID_WATER, 30)
        );
    }

    #[test]
    fn draining_dry_clears_the_liquid_state() {
        let mut puddle = column(FluidMat::FLUID_OIL, 4);
        puddle.set_compass(CompassFlags::FACING_N);
        assert_eq!(puddle.drain_fluid(10), 4);
        assert_eq!((puddle.fluid_mat(), puddle.compass()), (FluidMat::EMPTY, 0));
    }
}

/// Golden checks of the layout against values written out by hand, independent of
/// [`WorldCell::LAYOUT`], so an error in the layout owner cannot also hide in its test oracle.
#[cfg(test)]
mod layout_tests {
    use super::*;

    /// The documented layout (`docs/reference/world-format.md`): name, first bit, width.
    const DOCUMENTED: [(&str, u32, u32); 11] = [
        ("terrain_material", 0, 4),
        ("surface_material", 4, 4),
        ("granular_material", 8, 4),
        ("fluid_material", 12, 3),
        ("variants", 15, 5),
        ("elevation", 20, 12),
        ("fluid_volume", 32, 9),
        ("granular_volume", 41, 4),
        ("surface_state", 45, 9),
        ("terrain_state", 54, 6),
        ("compass", 60, 4),
    ];

    /// Differences between `layout` and the documented table, and gaps or overlaps in it.
    fn layout_violations(layout: &[Field]) -> Vec<String> {
        let mut problems = Vec::new();
        if layout.len() != DOCUMENTED.len() {
            problems.push(format!(
                "{} fields, documented {}",
                layout.len(),
                DOCUMENTED.len()
            ));
        }
        for (field, (name, shift, bits)) in layout.iter().zip(DOCUMENTED) {
            if (field.name, field.shift, field.bits) != (name, shift, bits) {
                problems.push(format!(
                    "{field:?} differs from documented {name} {shift}+{bits}"
                ));
            }
        }
        let mut covered = 0u64;
        for field in layout {
            let mask = field.max() << field.shift;
            if covered & mask != 0 {
                problems.push(format!("{} overlaps another field", field.name));
            }
            covered |= mask;
        }
        if covered != u64::MAX {
            problems.push(format!("fields leave bits {:#018x} unused", !covered));
        }
        problems
    }

    #[test]
    fn layout_matches_the_documented_table_and_tiles_64_bits() {
        assert_eq!(layout_violations(&WorldCell::LAYOUT), Vec::<String>::new());
    }

    #[test]
    fn a_shifted_field_is_detected() {
        // Deliberate failure specimen: elevation one bit too high must not pass.
        let mut wrong = WorldCell::LAYOUT;
        wrong[5].shift = 21;
        assert!(!layout_violations(&wrong).is_empty());
    }

    /// Reads every field through the public accessors.
    fn fields(cell: WorldCell) -> [u64; 11] {
        [
            cell.terrain_mat().0 as u64,
            cell.surface_mat().0 as u64,
            cell.granular_mat().0 as u64,
            cell.fluid_mat().0 as u64,
            cell.variants() as u64,
            cell.elevation() as u64,
            cell.fluid_vol() as u64,
            cell.granular_vol() as u64,
            cell.surface_state() as u64,
            cell.terrain_state() as u64,
            cell.compass() as u64,
        ]
    }

    #[test]
    fn each_hand_written_field_maximum_reads_back_alone() {
        // The same words as cells 1-11 of the legacy boundary fixture row (7, -3, 0).
        let words: [(u64, u64); 11] = [
            (0x0000_0000_0000_000F, 15),
            (0x0000_0000_0000_00F0, 15),
            (0x0000_0000_0000_0F00, 15),
            (0x0000_0000_0000_7000, 7),
            (0x0000_0000_000F_8000, 31),
            (0x0000_0000_FFF0_0000, 4095),
            (0x0000_01FF_0000_0000, 511),
            (0x0000_1E00_0000_0000, 15),
            (0x003F_E000_0000_0000, 511),
            (0x0FC0_0000_0000_0000, 63),
            (0xF000_0000_0000_0000, 15),
        ];
        for (index, (word, maximum)) in words.into_iter().enumerate() {
            let mut expected = [0u64; 11];
            expected[index] = maximum;
            assert_eq!(fields(WorldCell(word)), expected, "word {word:#018x}");
        }
        assert_eq!(
            fields(WorldCell(u64::MAX)),
            words.map(|(_, maximum)| maximum)
        );
    }

    #[test]
    fn setters_change_only_their_field() {
        let mut cell = WorldCell(u64::MAX);
        cell.set_elevation(0);
        assert_eq!(cell.0, !0x0000_0000_FFF0_0000);
        let mut cell = WorldCell(0);
        cell.set_fluid_vol(WorldCell::MAX_FLUID_VOL);
        cell.set_compass(0xF);
        assert_eq!(cell.0, 0xF000_01FF_0000_0000);
        // Values wider than the field are truncated to it, never spilling into a neighbour.
        let mut cell = WorldCell(0);
        cell.set_variants(0xFF);
        assert_eq!(cell.0, 0x0000_0000_000F_8000);
    }

    #[test]
    fn words_are_low_then_high_independent_of_host_order() {
        let cell = WorldCell(0x0123_4567_89AB_CDEF);
        assert_eq!(cell.to_words(), [0x89AB_CDEF, 0x0123_4567]);
        assert_eq!(WorldCell::from_words([0x89AB_CDEF, 0x0123_4567]), cell);
        // Little-endian bytes put word 0 first, which is what a GPU buffer of u32 pairs reads.
        let bytes = cell.0.to_le_bytes();
        assert_eq!(
            u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            cell.to_words()[0]
        );
        assert_eq!(
            u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            cell.to_words()[1]
        );
    }
}
