use bevy::{ecs::resource::Resource, math::DVec3};
use bitcode::{Decode, Encode};
use bytemuck::{Pod, Zeroable};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::world::chunk::{ChunkData, ChunkKey, ChunkView};

/// Engine-side storage for lightweight metadata of active chunks.
#[derive(Resource, Default)]
pub struct WorldStore {
    pub active_chunks: FxHashMap<ChunkKey, ChunkData>,
    pub pending_requests: FxHashSet<ChunkKey>,
}

#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct WorldFocus {
    pub position: DVec3,
}

#[derive(Resource)]
pub struct ChunkManager {
    pub current_view: Option<ChunkView>,
    pub view_radius: usize,
}

impl Default for ChunkManager {
    fn default() -> Self {
        Self {
            current_view: None,
            view_radius: 2, // 1x1 chunk grid
        }
    }
}

#[derive(Debug, Clone, Copy, bitcode::Encode, bitcode::Decode)]
pub struct SerializedEntity {
    pub entity_type: EntityTypeId,
    pub position: [f64; 3],
    pub rotation: f32,
    pub scale: f32,
    pub health: f32,
}

impl SerializedEntity {
    /// Helper to easily extract the Bevy math type when loading the chunk into the ECS
    #[inline(always)]
    pub fn get_position(&self) -> DVec3 {
        DVec3::from_array(self.position)
    }

    /// Helper to easily create a SerializedEntity from Bevy transforms
    #[inline(always)]
    pub fn new(
        entity_type: EntityTypeId,
        pos: DVec3,
        rotation: f32,
        scale: f32,
        health: f32,
    ) -> Self {
        Self {
            entity_type,
            position: pos.to_array(),
            rotation,
            scale,
            health,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Encode, Decode)]
pub struct EntityTypeId(pub u32);

impl EntityTypeId {
    pub const HERO: Self = Self(1);
    pub const WIZARD: Self = Self(2);
    pub const MINION_HUMAN: Self = Self(3);
    pub const MINION_GIANT: Self = Self(4);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode, Pod, Zeroable)]
#[repr(transparent)]
pub struct MaterialId(pub u8);

impl MaterialId {
    pub const EMPTY: Self = Self(0);
    pub const DIRT: Self = Self(1);
    pub const ROCK: Self = Self(2);
    pub const WATER: Self = Self(3);
    pub const BLOOD: Self = Self(4);
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
