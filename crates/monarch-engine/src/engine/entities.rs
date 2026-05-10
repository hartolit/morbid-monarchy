pub mod spherical;
pub mod utils;

use bevy::{
    ecs::resource::Resource,
    math::{DVec3, Vec3},
};
use bitcode::{Decode, Encode};
use bytemuck::{Pod, Zeroable};

/// Centralized compile-time configuration parameters for entity physics.
#[derive(Resource, Debug, Clone, Copy)]
pub struct EntityPhysicsConfig {
    pub gravity: Vec3,
    pub air_resistance: f32,
    pub rolling_friction: f32,
    pub impact_restitution: f32,
    pub min_bounce_velocity: f32,
    pub elevation_scale: f32,
    pub outward_sample_rings: usize,
    pub outward_stride_step: i32,
    pub volatile_cliff_threshold: f32,
    pub resistance_multiplier: f32,
    pub force_to_volume_factor: f32,
    pub min_deformation_energy: f32,
    pub cost_displace_granular: f32,
    pub cost_crush_terrain: f32,
    pub rim_expansion_factor: f32,
    pub max_rim_deposit_per_cell: u16,
}

impl Default for EntityPhysicsConfig {
    fn default() -> Self {
        Self {
            gravity: Vec3::new(0.0, -35.0, 0.0),
            air_resistance: 0.995,
            rolling_friction: 0.96,
            impact_restitution: 0.45,
            min_bounce_velocity: 0.05,
            elevation_scale: 0.50,
            outward_sample_rings: 3,
            outward_stride_step: 8, // 8 cells maps cleanly across 64-byte L1 cache-line strides
            volatile_cliff_threshold: 12.0,
            resistance_multiplier: 1.5, // Increased to provide strong confining pressure at depth
            force_to_volume_factor: 1.0, // Pure physical mapping without arbitrary scaling
            min_deformation_energy: 0.5,
            cost_displace_granular: 0.1,
            cost_crush_terrain: 25.0, // Solid rock requires substantial energy to fracture
            rim_expansion_factor: 1.5,
            max_rim_deposit_per_cell: 3,
        }
    }
}

/// Represents a serialized entity that can be deserialized into the ECS.
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

/// Represents a unique type ID for an entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Encode, Decode)]
pub struct EntityTypeId(pub u32);

impl EntityTypeId {
    pub const HERO: Self = Self(1);
    pub const WIZARD: Self = Self(2);
    pub const MINION_HUMAN: Self = Self(3);
    pub const MINION_GIANT: Self = Self(4);
    pub const RIGID_SPHERE: Self = Self(5);
}

/// Represents the flags for an entity.
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
