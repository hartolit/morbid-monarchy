use bitcode::{Decode, Encode};
use bytemuck::{Pod, Zeroable};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode, Pod, Zeroable)]
#[repr(transparent)]
pub struct MaterialId(pub u8);

impl MaterialId {
    pub const EMPTY: Self = Self(0);
    pub const WATER: Self = Self(1);
    pub const BLOOD: Self = Self(2);
    pub const GRASS: Self = Self(3);
    pub const SAND: Self = Self(4);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode, Pod, Zeroable)]
#[repr(transparent)]
pub struct PixelFlags(pub u8);

impl PixelFlags {
    pub const NONE: Self = Self(0);
    pub const IS_SOLID: Self = Self(1 << 0);
    pub const WAKES_AWAKE: Self = Self(1 << 1);

    #[inline(always)]
    pub fn contains(&self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    #[inline(always)]
    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    #[inline(always)]
    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode, Pod, Zeroable)]
#[repr(C)]
pub struct Pixel {
    pub material: MaterialId,
    pub state: u8,   // Tracks dynamic state (e.g. health, velocity, temperature)
    pub variant: u8, // Used for visual representation or static sub-properties
    pub flags: PixelFlags,
}

impl Default for Pixel {
    fn default() -> Self {
        Self {
            material: MaterialId::EMPTY,
            state: 0,
            variant: 0,
            flags: PixelFlags::NONE,
        }
    }
}

/// A single X/Y coordinate in the top-down world, containing multiple Z-layers.
/// Note: Perfectly aligned for 64-byte CPU cache lines (4 cells / 16 bytes per line).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Encode, Decode, Pod, Zeroable)]
#[repr(C)]
pub struct WorldCell {
    /// Layer 0: The base ground (Rock, Dirt, Pit).
    pub terrain: Pixel,
    /// Layer 1: The fluid layer (Water, Blood, Loose Sand).
    pub fluid: Pixel,
    /// Layer 2: The atmosphere layer (Air, Gas, Mist).
    pub atmosphere: Pixel,
    /// Layer 3: The surface layer floats on top of terrain and fluid layers.
    /// Note: Can represent things like a boat, bridge, fire, blood splatters, pots, flowers.
    pub surface: Pixel,
}
