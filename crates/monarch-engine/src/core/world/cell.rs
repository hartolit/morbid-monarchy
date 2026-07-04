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
    pub const TERRAIN_ICE: Self = Self(4);
    pub const TERRAIN_METAL: Self = Self(5);
    pub const TERRAIN_CORRUPTION: Self = Self(6);
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
    pub const SURFACE_ICE: Self = Self(10);
    pub const SURFACE_METAL: Self = Self(11);
    pub const SURFACE_GLASS: Self = Self(12);
    pub const SURFACE_CORRUPTION: Self = Self(13);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode, Pod, Zeroable)]
#[repr(transparent)]
pub struct GranularMat(pub u8);

impl GranularMat {
    // 3 Bits (Max 7)
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
            Self::GRANULAR_SNOW => TerrainMat::TERRAIN_ICE,
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
    // 4 Bits (Max 15)
    pub const EMPTY: Self = Self(0);
    pub const FLUID_WATER: Self = Self(1);
    pub const FLUID_MAGMA: Self = Self(2);
    pub const FLUID_BLOOD: Self = Self(3);
    pub const FLUID_ACID: Self = Self(4);
    pub const FLUID_OIL: Self = Self(5);
    pub const FLUID_CORRUPTION: Self = Self(6);
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

    // Granular Material: 3 bits (Bits 8-10)
    const MAT_GRANULAR_SHIFT: u64 = 8;
    const MAT_GRANULAR_MASK: u64 = 0x7;

    // Fluid Material: 4 bits (Bits 11-14)
    const MAT_FLUID_SHIFT: u64 = 11;
    const MAT_FLUID_MASK: u64 = 0xF;

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
