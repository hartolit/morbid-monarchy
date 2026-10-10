use bitcode::{Decode, Encode};
use bytemuck::{Pod, Zeroable};

// Material Definitions

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode, Pod, Zeroable)]
#[repr(transparent)]
pub struct TerrainMat(pub u8);

impl TerrainMat {
    // 4 Bits (Max 15)
    pub const EMPTY: Self = Self(0);
    pub const TERRAIN_STONE: Self = Self(1);
    pub const TERRAIN_DIRT: Self = Self(2);
    pub const TERRAIN_SANDSTONE: Self = Self(3);
    pub const TERRAIN_DENSE_SNOW: Self = Self(4);
    pub const TERRAIN_METAL: Self = Self(5);
    pub const TERRAIN_CORRUPTION: Self = Self(6);

    /// Maps solid bedrock to the loose material it breaks down into when eroded.
    /// Inverse of [`GranularMat::to_terrain`], so erosion followed by compression round-trips.
    /// Returns `None` for materials that resist mechanical erosion.
    pub fn to_granular(self) -> Option<GranularMat> {
        match self {
            Self::TERRAIN_DIRT => Some(GranularMat::GRANULAR_DIRT),
            Self::TERRAIN_SANDSTONE => Some(GranularMat::GRANULAR_SAND),
            Self::TERRAIN_STONE => Some(GranularMat::GRANULAR_GRAVEL),
            Self::TERRAIN_DENSE_SNOW => Some(GranularMat::GRANULAR_SNOW),
            Self::TERRAIN_CORRUPTION => Some(GranularMat::GRANULAR_CORRUPTION),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode, Pod, Zeroable)]
#[repr(transparent)]
pub struct SurfaceMat(pub u8);

impl SurfaceMat {
    // 4 Bits (Max 15)
    pub const EMPTY: Self = Self(0);
    pub const SURFACE_FIRE: Self = Self(1);
    pub const SURFACE_FOLIAGE: Self = Self(2);
    pub const SURFACE_WOOD: Self = Self(3);
    pub const SURFACE_FLESH: Self = Self(4);
    pub const SURFACE_BONE: Self = Self(5);
    pub const SURFACE_ROT: Self = Self(6);
    pub const SURFACE_ASH: Self = Self(7);
    pub const SURFACE_SNOW: Self = Self(8);
    pub const SURFACE_CLAY: Self = Self(9);
    // 10 is unassigned: ice is a solid phase of the liquid layer (`FluidMat::FLUID_ICE`).
    pub const SURFACE_METAL: Self = Self(11);
    pub const SURFACE_GLASS: Self = Self(12);
    pub const SURFACE_CORRUPTION: Self = Self(13);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode, Pod, Zeroable)]
#[repr(transparent)]
pub struct GranularMat(pub u8);

impl GranularMat {
    // 4 Bits (Max 15)
    pub const EMPTY: Self = Self(0);
    pub const GRANULAR_DIRT: Self = Self(1);
    pub const GRANULAR_SAND: Self = Self(2);
    pub const GRANULAR_MUD: Self = Self(3);
    pub const GRANULAR_GRAVEL: Self = Self(4);
    pub const GRANULAR_SNOW: Self = Self(5);
    pub const GRANULAR_LIQUID_METAL: Self = Self(6);
    pub const GRANULAR_CORRUPTION: Self = Self(7);

    /// Geologically maps a loose granular material to its solid bedrock equivalent under high compression.
    pub fn to_terrain(self) -> TerrainMat {
        match self {
            Self::GRANULAR_SAND => TerrainMat::TERRAIN_SANDSTONE,
            Self::GRANULAR_SNOW => TerrainMat::TERRAIN_DENSE_SNOW,
            Self::GRANULAR_DIRT | Self::GRANULAR_MUD => TerrainMat::TERRAIN_DIRT,
            Self::GRANULAR_GRAVEL | Self::GRANULAR_LIQUID_METAL => TerrainMat::TERRAIN_STONE,
            Self::GRANULAR_CORRUPTION => TerrainMat::TERRAIN_CORRUPTION,
            _ => TerrainMat::TERRAIN_STONE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode, Pod, Zeroable)]
#[repr(transparent)]
pub struct FluidMat(pub u8);

impl FluidMat {
    // 3 Bits (Max 7)
    pub const EMPTY: Self = Self(0);
    pub const FLUID_WATER: Self = Self(1);
    pub const FLUID_MAGMA: Self = Self(2);
    pub const FLUID_BLOOD: Self = Self(3);
    pub const FLUID_ACID: Self = Self(4);
    pub const FLUID_OIL: Self = Self(5);
    pub const FLUID_CORRUPTION: Self = Self(6);
    pub const FLUID_ICE: Self = Self(7);

    /// Frozen liquids are solid: they never flow, nothing flows into them, and bodies and
    /// loose material rest on them. Freezing and melting are plain material swaps, so a column
    /// keeps its volume through every phase change.
    #[inline(always)]
    pub const fn is_solid(self) -> bool {
        self.0 == Self::FLUID_ICE.0
    }

    /// Water-like liquids (and ice) that smother flames and quench magma into rock.
    #[inline(always)]
    pub const fn is_coolant(self) -> bool {
        matches!(
            self,
            Self::FLUID_WATER
                | Self::FLUID_BLOOD
                | Self::FLUID_ACID
                | Self::FLUID_CORRUPTION
                | Self::FLUID_ICE
        )
    }

    /// The solid this liquid freezes into, if it freezes.
    #[inline(always)]
    pub const fn frozen(self) -> Option<Self> {
        match self {
            Self::FLUID_WATER => Some(Self::FLUID_ICE),
            _ => None,
        }
    }

    /// The liquid this solid melts into, if it melts.
    #[inline(always)]
    pub const fn melted(self) -> Option<Self> {
        match self {
            Self::FLUID_ICE => Some(Self::FLUID_WATER),
            _ => None,
        }
    }
}

pub struct CompassFlags;
impl CompassFlags {
    pub const FACING_N: u8 = 1 << 0;
    pub const FACING_S: u8 = 1 << 1;
    pub const FACING_E: u8 = 1 << 2;
    pub const FACING_W: u8 = 1 << 3;
}

// WorldCell Core Data Structure

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Encode, Decode, Pod, Zeroable)]
#[repr(transparent)]
pub struct WorldCell(pub u64);

impl WorldCell {
    // WORD 0: Geometry & Visuals (Lower 32 bits)

    // Terrain Material: 4 bits (Bits 0-3)
    const MAT_TERRAIN_SHIFT: u64 = 0;
    const MAT_TERRAIN_MASK: u64 = 0xF;

    // Surface Material: 4 bits (Bits 4-7)
    const MAT_SURFACE_SHIFT: u64 = 4;
    const MAT_SURFACE_MASK: u64 = 0xF;

    // Granular Material: 4 bits (Bits 8-11)
    const MAT_GRANULAR_SHIFT: u64 = 8;
    const MAT_GRANULAR_MASK: u64 = 0xF;

    // Fluid Material: 3 bits (Bits 12-14)
    const MAT_FLUID_SHIFT: u64 = 12;
    const MAT_FLUID_MASK: u64 = 0x7;

    // Variants: 5 bits (Bits 15-19)
    const VARIANTS_SHIFT: u64 = 15;
    const VARIANTS_MASK: u64 = 0x1F;

    // Elevation: 12 bits (Bits 20-31)
    const ELEVATION_SHIFT: u64 = 20;
    const ELEVATION_MASK: u64 = 0xFFF;

    // WORD 1: Physics & State (Upper 32 bits)

    // Fluid Volume: 9 bits (Bits 32-40)
    const FLUID_VOL_SHIFT: u64 = 32;
    const FLUID_VOL_MASK: u64 = 0x1FF;

    // Granular Volume: 4 bits (Bits 41-44)
    const GRANULAR_VOL_SHIFT: u64 = 41;
    const GRANULAR_VOL_MASK: u64 = 0xF;

    // Surface State: 9 bits (Bits 45-53)
    const SURFACE_STATE_SHIFT: u64 = 45;
    const SURFACE_STATE_MASK: u64 = 0x1FF;

    // Terrain State: 6 bits (Bits 54-59)
    const TERRAIN_STATE_SHIFT: u64 = 54;
    const TERRAIN_STATE_MASK: u64 = 0x3F;

    // Compass / Momentum: 4 bits (Bits 60-63)
    const COMPASS_SHIFT: u64 = 60;
    const COMPASS_MASK: u64 = 0xF;

    // EXPOSED MAX VALUES

    pub const MAX_ELEVATION: u32 = Self::ELEVATION_MASK as u32; // 4,095
    pub const MAX_FLUID_VOL: u16 = Self::FLUID_VOL_MASK as u16; // 511
    pub const MAX_GRANULAR_VOL: u16 = Self::GRANULAR_VOL_MASK as u16; // 15
    pub const MAX_SURFACE_STATE: u16 = Self::SURFACE_STATE_MASK as u16; // 511
    pub const MAX_TERRAIN_STATE: u8 = Self::TERRAIN_STATE_MASK as u8; // 63
    pub const MAX_VARIANTS: u8 = Self::VARIANTS_MASK as u8; // 31

    // GETTERS & SETTERS

    // Terrain Mat
    #[inline(always)]
    pub fn terrain_mat(&self) -> TerrainMat {
        TerrainMat(((self.0 >> Self::MAT_TERRAIN_SHIFT) & Self::MAT_TERRAIN_MASK) as u8)
    }
    #[inline(always)]
    pub fn set_terrain_mat(&mut self, mat: TerrainMat) {
        self.0 = (self.0 & !(Self::MAT_TERRAIN_MASK << Self::MAT_TERRAIN_SHIFT))
            | (((mat.0 as u64) & Self::MAT_TERRAIN_MASK) << Self::MAT_TERRAIN_SHIFT);
    }

    // Surface Mat
    #[inline(always)]
    pub fn surface_mat(&self) -> SurfaceMat {
        SurfaceMat(((self.0 >> Self::MAT_SURFACE_SHIFT) & Self::MAT_SURFACE_MASK) as u8)
    }
    #[inline(always)]
    pub fn set_surface_mat(&mut self, mat: SurfaceMat) {
        self.0 = (self.0 & !(Self::MAT_SURFACE_MASK << Self::MAT_SURFACE_SHIFT))
            | (((mat.0 as u64) & Self::MAT_SURFACE_MASK) << Self::MAT_SURFACE_SHIFT);
    }

    // Granular Mat
    #[inline(always)]
    pub fn granular_mat(&self) -> GranularMat {
        GranularMat(((self.0 >> Self::MAT_GRANULAR_SHIFT) & Self::MAT_GRANULAR_MASK) as u8)
    }
    #[inline(always)]
    pub fn set_granular_mat(&mut self, mat: GranularMat) {
        self.0 = (self.0 & !(Self::MAT_GRANULAR_MASK << Self::MAT_GRANULAR_SHIFT))
            | (((mat.0 as u64) & Self::MAT_GRANULAR_MASK) << Self::MAT_GRANULAR_SHIFT);
    }

    // Fluid Mat
    #[inline(always)]
    pub fn fluid_mat(&self) -> FluidMat {
        FluidMat(((self.0 >> Self::MAT_FLUID_SHIFT) & Self::MAT_FLUID_MASK) as u8)
    }
    #[inline(always)]
    pub fn set_fluid_mat(&mut self, mat: FluidMat) {
        self.0 = (self.0 & !(Self::MAT_FLUID_MASK << Self::MAT_FLUID_SHIFT))
            | (((mat.0 as u64) & Self::MAT_FLUID_MASK) << Self::MAT_FLUID_SHIFT);
    }

    // Variants
    #[inline(always)]
    pub fn variants(&self) -> u8 {
        ((self.0 >> Self::VARIANTS_SHIFT) & Self::VARIANTS_MASK) as u8
    }
    #[inline(always)]
    pub fn set_variants(&mut self, val: u8) {
        self.0 = (self.0 & !(Self::VARIANTS_MASK << Self::VARIANTS_SHIFT))
            | (((val as u64) & Self::VARIANTS_MASK) << Self::VARIANTS_SHIFT);
    }

    // Elevation
    #[inline(always)]
    pub fn elevation(&self) -> u16 {
        ((self.0 >> Self::ELEVATION_SHIFT) & Self::ELEVATION_MASK) as u16
    }
    #[inline(always)]
    pub fn set_elevation(&mut self, val: u16) {
        self.0 = (self.0 & !(Self::ELEVATION_MASK << Self::ELEVATION_SHIFT))
            | (((val as u64) & Self::ELEVATION_MASK) << Self::ELEVATION_SHIFT);
    }

    // Fluid Volume
    #[inline(always)]
    pub fn fluid_vol(&self) -> u16 {
        ((self.0 >> Self::FLUID_VOL_SHIFT) & Self::FLUID_VOL_MASK) as u16
    }
    #[inline(always)]
    pub fn set_fluid_vol(&mut self, val: u16) {
        self.0 = (self.0 & !(Self::FLUID_VOL_MASK << Self::FLUID_VOL_SHIFT))
            | (((val as u64) & Self::FLUID_VOL_MASK) << Self::FLUID_VOL_SHIFT);
    }

    // Granular Volume & Compression
    #[inline(always)]
    pub fn granular_vol(&self) -> u16 {
        ((self.0 >> Self::GRANULAR_VOL_SHIFT) & Self::GRANULAR_VOL_MASK) as u16
    }

    #[inline(always)]
    pub fn set_granular_vol(&mut self, val: u16) {
        if val > Self::MAX_GRANULAR_VOL {
            let excess = val - Self::MAX_GRANULAR_VOL;
            let current_elev = self.elevation();

            // Transmute the bedrock if we are compressing granular matter into a void
            // if self.terrain_mat() == TerrainMat::EMPTY {
            //     self.set_terrain_mat(self.granular_mat().to_terrain());
            // }

            self.set_terrain_mat(self.granular_mat().to_terrain());

            // Physically compress the overflow directly into the baseline elevation.
            self.set_elevation(
                current_elev
                    .saturating_add(excess)
                    .min(Self::MAX_ELEVATION as u16),
            );

            self.0 = (self.0 & !(Self::GRANULAR_VOL_MASK << Self::GRANULAR_VOL_SHIFT))
                | (((Self::MAX_GRANULAR_VOL as u64) & Self::GRANULAR_VOL_MASK)
                    << Self::GRANULAR_VOL_SHIFT);
        } else {
            self.0 = (self.0 & !(Self::GRANULAR_VOL_MASK << Self::GRANULAR_VOL_SHIFT))
                | (((val as u64) & Self::GRANULAR_VOL_MASK) << Self::GRANULAR_VOL_SHIFT);
        }
    }

    // Surface State
    #[inline(always)]
    pub fn surface_state(&self) -> u16 {
        ((self.0 >> Self::SURFACE_STATE_SHIFT) & Self::SURFACE_STATE_MASK) as u16
    }
    #[inline(always)]
    pub fn set_surface_state(&mut self, val: u16) {
        self.0 = (self.0 & !(Self::SURFACE_STATE_MASK << Self::SURFACE_STATE_SHIFT))
            | (((val as u64) & Self::SURFACE_STATE_MASK) << Self::SURFACE_STATE_SHIFT);
    }

    // Terrain State
    #[inline(always)]
    pub fn terrain_state(&self) -> u8 {
        ((self.0 >> Self::TERRAIN_STATE_SHIFT) & Self::TERRAIN_STATE_MASK) as u8
    }
    #[inline(always)]
    pub fn set_terrain_state(&mut self, val: u8) {
        self.0 = (self.0 & !(Self::TERRAIN_STATE_MASK << Self::TERRAIN_STATE_SHIFT))
            | (((val as u64) & Self::TERRAIN_STATE_MASK) << Self::TERRAIN_STATE_SHIFT);
    }

    // Compass / Momentum
    #[inline(always)]
    pub fn compass(&self) -> u8 {
        ((self.0 >> Self::COMPASS_SHIFT) & Self::COMPASS_MASK) as u8
    }
    #[inline(always)]
    pub fn set_compass(&mut self, flags: u8) {
        self.0 = (self.0 & !(Self::COMPASS_MASK << Self::COMPASS_SHIFT))
            | (((flags as u64) & Self::COMPASS_MASK) << Self::COMPASS_SHIFT);
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
