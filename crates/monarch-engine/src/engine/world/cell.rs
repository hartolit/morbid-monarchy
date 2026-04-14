use bitcode::{Decode, Encode};
use bytemuck::{Pod, Zeroable};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode, Pod, Zeroable)]
#[repr(transparent)]
pub struct MaterialId(pub u8);

impl MaterialId {
    // ==========================================
    // SYSTEM (0, 255)
    // ==========================================
    pub const EMPTY: Self = Self(0);
    pub const VOID: Self = Self(255);

    // ==========================================
    // LIQUIDS (1 - 31)
    // ==========================================
    pub const LIQUID_WATER: Self = Self(1);
    pub const LIQUID_MAGMA: Self = Self(2);
    pub const LIQUID_BLOOD: Self = Self(3);
    pub const LIQUID_ACID:  Self = Self(4);
    pub const LIQUID_OIL:   Self = Self(5);

    // ==========================================
    // GASES & PLASMAS (32 - 63)
    // ==========================================
    pub const GAS_STEAM:  Self = Self(32);
    pub const GAS_SMOKE:  Self = Self(33);
    pub const GAS_POISON: Self = Self(34);
    pub const FIRE:       Self = Self(35);

    // ==========================================
    // ORGANICS (64 - 127)
    // ==========================================
    pub const ORGANIC_WOOD:    Self = Self(64);
    pub const ORGANIC_FOLIAGE: Self = Self(65); // Variants: 0=Grass, 1=Flower, 2=Vine
    pub const ORGANIC_FLESH:   Self = Self(66); // Variants: 0=Human, 1=Goblin, 2=Animal
    pub const ORGANIC_BONE:    Self = Self(67); // High acid resistance
    pub const ORGANIC_ROT:     Self = Self(68); // Decayed flesh/plants

    // ==========================================
    // POWDERS & LOOSE SOLIDS (128 - 191)
    // ==========================================
    pub const LOOSE_SAND: Self = Self(128);
    pub const LOOSE_DIRT: Self = Self(129);
    pub const LOOSE_ASH:  Self = Self(130);
    pub const LOOSE_SNOW: Self = Self(131);

    // ==========================================
    // SOLIDS (192 - 254)
    // ==========================================
    pub const SOLID_STONE: Self = Self(192);
    pub const SOLID_CLAY:  Self = Self(193);
    pub const SOLID_ICE:   Self = Self(194);
    pub const SOLID_METAL: Self = Self(195);
    pub const SOLID_GLASS: Self = Self(196);
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
