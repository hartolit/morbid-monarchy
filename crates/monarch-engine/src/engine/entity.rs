use bevy::math::DVec3;
use bitcode::{Decode, Encode};
use bytemuck::{Pod, Zeroable};

// TODO: This should be a generic type that can be used for any entity type
#[derive(Debug, Clone, Copy, bitcode::Encode, bitcode::Decode)]
pub struct SerializedEntity {
    pub entity_type: EntityTypeId,
    pub position: [f64; 3],
    pub rotation: f32,
    pub scale: f32,
    pub state: u8,
    pub variant: u8,
    pub flags: EntityFlags,
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
        state: u8,
        variant: u8,
        flags: EntityFlags,
    ) -> Self {
        Self {
            entity_type,
            position: pos.to_array(),
            rotation,
            scale,
            state,
            variant,
            flags,
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
pub struct EntityFlags(pub u16);

impl EntityFlags {
    pub const NONE: Self = Self(0);
    pub const IS_ACTIVE: Self = Self(1 << 0);
    pub const IS_VISIBLE: Self = Self(1 << 1);
    pub const IS_COLLIDABLE: Self = Self(1 << 2);
    pub const IS_INTERACTABLE: Self = Self(1 << 3);
    pub const IS_DESTRUCTIBLE: Self = Self(1 << 4);
    pub const IS_TRANSPARENT: Self = Self(1 << 5);
    pub const IS_ANIMATED: Self = Self(1 << 6);
    pub const IS_HOSTILE: Self = Self(1 << 8);
}
