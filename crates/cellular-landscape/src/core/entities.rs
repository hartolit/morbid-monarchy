pub mod observer;
pub mod spherical;

use bevy::{
    ecs::{component::Component, resource::Resource},
    math::{DVec3, Vec3},
};
use bitcode::{Decode, Encode};
use bytemuck::{Pod, Zeroable};

/// Universal thermodynamic constants binding the simulation space.
/// Modifies the absolute physical laws of the environment, irrespective of localized entities.
#[derive(Resource, Debug, Clone, Copy)]
pub struct GlobalPhysicsConfig {
    pub gravity: Vec3,
    pub elevation_scale: f32,
    pub epsilon_distance: f32,
    pub half_cell_offset: f32,
}

impl Default for GlobalPhysicsConfig {
    fn default() -> Self {
        Self {
            gravity: Vec3::new(0.0, -32.0, 0.0),
            elevation_scale: 0.50,
            epsilon_distance: 0.000001,
            half_cell_offset: 0.5,
        }
    }
}

/// Isolated kinematic properties defining how an entity resolves momentum and collision.
#[derive(Component, Debug, Clone, Copy)]
pub struct KinematicProfile {
    pub mass: f32,
    pub air_resistance: f32,
    pub rolling_friction: f32,
    pub impact_restitution: f32,
}

impl Default for KinematicProfile {
    fn default() -> Self {
        Self {
            mass: 1.0,
            air_resistance: 0.99,
            rolling_friction: 0.90,
            impact_restitution: 0.20,
        }
    }
}

/// Structural thresholds defining an entity's capacity to induce localized grid entropy (cratering).
#[derive(Component, Debug, Clone, Copy)]
pub struct DeformationProfile {
    pub min_deformation_energy: f32,
    pub energy_to_deformation_scale: f32,
    pub cost_displace_granular: f32,
    pub cost_crush_terrain: f32,
}

impl Default for DeformationProfile {
    fn default() -> Self {
        Self {
            min_deformation_energy: 250.0,
            energy_to_deformation_scale: 0.001,
            cost_displace_granular: 2.0,
            cost_crush_terrain: 3.0,
        }
    }
}

/// A packed structural representation of an entity, optimized for direct I/O zero-copy streaming.
#[derive(Debug, Clone, Copy, Encode, Decode)]
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
    #[inline(always)]
    pub fn get_position(&self) -> DVec3 {
        DVec3::from_array(self.position)
    }

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
    pub const RIGID_SPHERE: Self = Self(5);
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
