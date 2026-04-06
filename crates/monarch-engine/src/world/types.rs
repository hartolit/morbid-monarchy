use bevy::math::DVec3;
use bitflags::bitflags;

#[derive(Debug, Clone, Copy)]
pub struct SerializedEntity {
    pub entity_type: EntityTypeId,
    pub position: DVec3,
    pub rotation: f32,
    pub scale: f32,
    pub health: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntityTypeId(pub u32);

impl EntityTypeId {
    pub const HERO: Self = Self(1);
    pub const WIZARD: Self = Self(2);
    pub const MINION_HUMAN: Self = Self(3);
    pub const MINION_GIANT: Self = Self(4);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaterialId(pub u8);

impl MaterialId {
    pub const EMPTY: Self = Self(0);
    pub const DIRT: Self = Self(1);
    pub const ROCK: Self = Self(2);
    pub const WATER: Self = Self(3);
    pub const BLOOD: Self = Self(4);
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PixelFlags: u8 {
        const NONE = 0;
        const IS_SOLID = 1 << 0;
        const WAKES_AWAKE = 1 << 1;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Pixel {
    pub material: MaterialId,
    pub state: u8,
    pub variant: u8,
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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
