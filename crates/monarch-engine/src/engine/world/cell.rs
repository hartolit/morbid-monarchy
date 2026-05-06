use bitcode::{Decode, Encode};
use bytemuck::{Pod, Zeroable};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode, Pod, Zeroable)]
#[repr(transparent)]
pub struct TerrainMat(pub u8);

impl TerrainMat {
    pub const EMPTY: Self = Self(0);
    pub const STONE: Self = Self(1);
    pub const DIRT: Self = Self(2);
    pub const SAND: Self = Self(3);
    pub const FOLIAGE: Self = Self(4);
    pub const WOOD: Self = Self(5);
    pub const FLESH: Self = Self(6);
    pub const BONE: Self = Self(7);
    pub const ROT: Self = Self(8);
    pub const ASH: Self = Self(9);
    pub const SNOW: Self = Self(10);
    pub const CLAY: Self = Self(11);
    pub const ICE: Self = Self(12);
    pub const METAL: Self = Self(13);
    pub const GLASS: Self = Self(14);
    pub const GRAVEL: Self = Self(15);
    pub const VOID: Self = Self(31);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode, Pod, Zeroable)]
#[repr(transparent)]
pub struct FluidMat(pub u8);

impl FluidMat {
    pub const EMPTY: Self = Self(0);
    pub const WATER: Self = Self(1);
    pub const MAGMA: Self = Self(2);
    pub const BLOOD: Self = Self(3);
    pub const ACID: Self = Self(4);
    pub const OIL: Self = Self(5);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode, Pod, Zeroable)]
#[repr(transparent)]
pub struct SurfaceMat(pub u8);

impl SurfaceMat {
    pub const EMPTY: Self = Self(0);
    pub const FIRE: Self = Self(1);
    pub const STEAM: Self = Self(2);
    pub const SMOKE: Self = Self(3);
    pub const POISON: Self = Self(4);
}

pub struct CompassFlags;
impl CompassFlags {
    pub const FACING_N: u8 = 1 << 0;
    pub const FACING_S: u8 = 1 << 1;
    pub const FACING_E: u8 = 1 << 2;
    pub const FACING_W: u8 = 1 << 3;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Encode, Decode, Pod, Zeroable)]
#[repr(transparent)]
pub struct WorldCell(pub u64);

impl WorldCell {
    // --- WORD 0 (Geometry: Lower 32 bits) ---

    // Terrain Material: 5 bits (Bits 0-4)
    const MAT_TERRAIN_SHIFT: u64 = 0;
    const MAT_TERRAIN_MASK: u64 = 0x1F;

    // Fluid Material: 5 bits (Bits 5-9)
    const MAT_FLUID_SHIFT: u64 = 5;
    const MAT_FLUID_MASK: u64 = 0x1F;

    // Surface Material: 5 bits (Bits 10-14)
    const MAT_SURFACE_SHIFT: u64 = 10;
    const MAT_SURFACE_MASK: u64 = 0x1F;

    // Elevation: 17 bits (Bits 15-31)
    const ELEVATION_SHIFT: u64 = 15;
    const ELEVATION_MASK: u64 = 0x1FFFF;

    // --- WORD 1 (Physics: Upper 32 bits) ---

    // Fluid Volume: 10 bits (Bits 32-41)
    const FLUID_VOL_SHIFT: u64 = 32;
    const FLUID_VOL_MASK: u64 = 0x3FF;

    // Terrain State: 8 bits (Bits 42-49)
    const TERRAIN_STATE_SHIFT: u64 = 42;
    const TERRAIN_STATE_MASK: u64 = 0xFF;

    // Variants: 6 bits (Bits 50-55)
    const VARIANTS_SHIFT: u64 = 50;
    const VARIANTS_MASK: u64 = 0x3F;

    // Compass / Momentum: 4 bits (Bits 56-59)
    const COMPASS_SHIFT: u64 = 56;
    const COMPASS_MASK: u64 = 0xF;

    // Note: Bits 60-63 (4 bits) are currently unused and left as 0.

    /// The maximum absolute elevation value derived from the 17-bit mask
    pub const MAX_ELEVATION: u32 = Self::ELEVATION_MASK as u32;

    /// The maximum fluid depth derived from the 10-bit mask
    pub const MAX_FLUID_VOL: u16 = Self::FLUID_VOL_MASK as u16;

    // ---------------------------------------------------------
    // GETTERS & SETTERS
    // ---------------------------------------------------------

    #[inline(always)]
    pub fn terrain_mat(&self) -> TerrainMat {
        TerrainMat(((self.0 >> Self::MAT_TERRAIN_SHIFT) & Self::MAT_TERRAIN_MASK) as u8)
    }
    #[inline(always)]
    pub fn set_terrain_mat(&mut self, mat: TerrainMat) {
        self.0 = (self.0 & !(Self::MAT_TERRAIN_MASK << Self::MAT_TERRAIN_SHIFT))
            | (((mat.0 as u64) & Self::MAT_TERRAIN_MASK) << Self::MAT_TERRAIN_SHIFT);
    }

    #[inline(always)]
    pub fn fluid_mat(&self) -> FluidMat {
        FluidMat(((self.0 >> Self::MAT_FLUID_SHIFT) & Self::MAT_FLUID_MASK) as u8)
    }
    #[inline(always)]
    pub fn set_fluid_mat(&mut self, mat: FluidMat) {
        self.0 = (self.0 & !(Self::MAT_FLUID_MASK << Self::MAT_FLUID_SHIFT))
            | (((mat.0 as u64) & Self::MAT_FLUID_MASK) << Self::MAT_FLUID_SHIFT);
    }

    #[inline(always)]
    pub fn surface_mat(&self) -> SurfaceMat {
        SurfaceMat(((self.0 >> Self::MAT_SURFACE_SHIFT) & Self::MAT_SURFACE_MASK) as u8)
    }
    #[inline(always)]
    pub fn set_surface_mat(&mut self, mat: SurfaceMat) {
        self.0 = (self.0 & !(Self::MAT_SURFACE_MASK << Self::MAT_SURFACE_SHIFT))
            | (((mat.0 as u64) & Self::MAT_SURFACE_MASK) << Self::MAT_SURFACE_SHIFT);
    }

    #[inline(always)]
    pub fn elevation(&self) -> u16 {
        ((self.0 >> Self::ELEVATION_SHIFT) & Self::ELEVATION_MASK) as u16
    }
    #[inline(always)]
    pub fn set_elevation(&mut self, val: u16) {
        self.0 = (self.0 & !(Self::ELEVATION_MASK << Self::ELEVATION_SHIFT))
            | (((val as u64) & Self::ELEVATION_MASK) << Self::ELEVATION_SHIFT);
    }

    #[inline(always)]
    pub fn fluid_vol(&self) -> u16 {
        ((self.0 >> Self::FLUID_VOL_SHIFT) & Self::FLUID_VOL_MASK) as u16
    }
    #[inline(always)]
    pub fn set_fluid_vol(&mut self, val: u16) {
        self.0 = (self.0 & !(Self::FLUID_VOL_MASK << Self::FLUID_VOL_SHIFT))
            | (((val as u64) & Self::FLUID_VOL_MASK) << Self::FLUID_VOL_SHIFT);
    }

    #[inline(always)]
    pub fn terrain_state(&self) -> u8 {
        ((self.0 >> Self::TERRAIN_STATE_SHIFT) & Self::TERRAIN_STATE_MASK) as u8
    }
    #[inline(always)]
    pub fn set_terrain_state(&mut self, val: u8) {
        self.0 = (self.0 & !(Self::TERRAIN_STATE_MASK << Self::TERRAIN_STATE_SHIFT))
            | (((val as u64) & Self::TERRAIN_STATE_MASK) << Self::TERRAIN_STATE_SHIFT);
    }

    #[inline(always)]
    pub fn variants(&self) -> u8 {
        ((self.0 >> Self::VARIANTS_SHIFT) & Self::VARIANTS_MASK) as u8
    }
    #[inline(always)]
    pub fn set_variants(&mut self, val: u8) {
        self.0 = (self.0 & !(Self::VARIANTS_MASK << Self::VARIANTS_SHIFT))
            | (((val as u64) & Self::VARIANTS_MASK) << Self::VARIANTS_SHIFT);
    }

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
