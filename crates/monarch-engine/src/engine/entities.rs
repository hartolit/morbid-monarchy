pub mod observer;
pub mod spherical;
pub mod utils;

use bevy::{
    ecs::resource::Resource,
    math::{DVec3, Vec3},
};
use bitcode::{Decode, Encode};
use bytemuck::{Pod, Zeroable};

/// Centralized compile-time configuration parameters for entity physics.
/// Tuning these parameters directly alters the "game feel" and physical
/// weight of rigid bodies in the voxel simulation.
#[derive(Resource, Debug, Clone, Copy)]
pub struct EntityPhysicsConfig {
    // --- KINEMATICS & MOTION ---
    /// The base gravity vector. Larger negative Y values make objects fall faster and feel heavier.
    pub gravity: Vec3,
    /// Drag applied while airborne. 1.0 means no drag; lower values make objects floaty like balloons.
    pub air_resistance: f32,
    /// Friction while sliding/rolling on surfaces. 1.0 is frictionless ice; lower values make objects stop abruptly like mud.
    pub rolling_friction: f32,
    /// Bounciness. 0.0 means the object hits with a dead thud; 1.0 means it retains all momentum.
    pub impact_restitution: f32,
    /// The threshold to stop micro-bounces. If an impact is slower than this, it just stops, preventing endless jittering.
    pub min_bounce_velocity: f32,
    /// The world-space height of a single grid elevation unit. Determines the visual and physical scale of the voxel stairs.
    pub elevation_scale: f32,

    // --- TERRAIN CARVING & CRATERING ---
    /// How far out the engine looks to find the "average" plateau level when an object lands. More rings = wider structural awareness.
    pub outward_sample_rings: usize,
    /// The gap between concentric resistance samples. Larger steps skip local bumps to analyze broader terrain features.
    pub outward_stride_step: i32,
    /// Prevents sheer cliffs from confusing the terrain resistance check. Drops larger than this are ignored as "not part of the plateau".
    pub volatile_cliff_threshold: f32,
    /// Makes digging deep holes exponentially harder than skimming the surface. Higher values mean objects quickly lose energy as they sink.
    pub resistance_multiplier: f32,
    /// Converts crushed terrain volume into "squish" on the ball. Higher values make the ball compress heavily when hitting solid rock.
    pub force_to_volume_factor: f32,
    /// The minimum kinetic energy required to scratch the terrain. Below this, the object simply rolls over the ground harmlessly.
    pub min_deformation_energy: f32,
    /// The scalar that converts kinetic energy into physical excavation depth.
    pub energy_to_deformation_scale: f32,
    /// The maximum allowable crater depth expressed as a ratio of the entity's size/radius.
    pub max_deformation_size_ratio: f32,
    /// Energy cost to plow through loose materials (sand, snow). Lower values allow objects to easily clear massive paths through granulars.
    pub cost_displace_granular: f32,
    /// Energy cost to shatter solid bedrock. Higher values mean only massive, high-speed impacts will leave a dent.
    pub cost_crush_terrain: f32,
    /// How far the "splash" of excavated dirt flies outwards. 1.0 deposits right at the edge; 2.0 flings it far away.
    pub rim_expansion_factor: f32,
    /// Caps how much dirt can pile up on a single voxel per tick, preventing impossibly steep sand towers during heavy impacts.
    pub max_rim_deposit_per_cell: u16,

    // --- SPHERICAL ENTITY SPECIFICS ---
    /// A microscopic buffer to prevent floating-point division-by-zero crashes during exact overlap.
    pub epsilon_distance: f32,
    /// Damps kinetic energy when multiple objects grind against each other, stopping tight clusters from jittering explosively.
    pub cluster_damping: f32,
    /// Centers the mathematical calculation to the middle of a grid cell (usually 0.5).
    pub half_cell_offset: f32,
    /// How quickly an object regains its original shape after being compressed by a heavy impact.
    pub compaction_decay_rate: f32,
    /// The speed below which the physics engine puts the object to sleep, disabling costly terrain deformation checks.
    pub sleep_velocity_squared: f32,
    /// The speed required to jolt a sleeping object back into active simulation.
    pub wake_velocity_squared: f32,
    /// Defines what counts as a "stair" versus a "wall". e.g., 0.5 means it smoothly rolls over blocks half its height.
    pub step_threshold_ratio: f32,
    /// Limits how high a "snowplow" pile of dirt can get in front of a rolling object before the dirt spills over it.
    pub blade_ceiling_ratio: f32,
    /// Defines the "footprint" of the sphere on the floor. 1.0 means the entire sphere width checks for ground support.
    pub load_bearing_radius_ratio: f32,
    /// Limits the speed at which the engine forcibly pushes clipping objects out of solid walls.
    pub max_wall_push_ratio: f32,
    /// Defines the size of the "empty bowl" left by an impact before the raised rim begins.
    pub rim_inner_radius_ratio: f32,
    /// Stops resting objects from endlessly micro-sliding down almost imperceptible slopes.
    pub grounding_drift_buffer: f32,
    /// Controls the precision of the spatial hashing algorithm used for organic sand sloshing.
    pub probability_hash_scale: f32,
    /// How violently an object launches upward when it is buried under sand or submerged in fluid.
    pub submerged_buoyancy_lift: f32,
    /// The minimum horizontal speed an entity must maintain to trigger submerged buoyancy lift.
    pub buoyancy_horizontal_speed_threshold: f32,
}

impl Default for EntityPhysicsConfig {
    fn default() -> Self {
        Self {
            // Kinematics & Motion
            gravity: Vec3::new(0.0, -24.0, 0.0),
            air_resistance: 0.99,
            rolling_friction: 0.90,
            impact_restitution: 0.20,
            min_bounce_velocity: 0.05,
            elevation_scale: 0.50,

            // Terrain Carving
            outward_sample_rings: 2,
            outward_stride_step: 10,
            volatile_cliff_threshold: 16.0,
            resistance_multiplier: 1.0,
            force_to_volume_factor: 0.8,
            min_deformation_energy: 250.0,
            energy_to_deformation_scale: 0.001,
            max_deformation_size_ratio: 1.0,
            cost_displace_granular: 2.0,
            cost_crush_terrain: 4.0,
            rim_expansion_factor: 1.5,
            max_rim_deposit_per_cell: 3,

            // Spherical Specifics
            epsilon_distance: 0.000001,
            cluster_damping: 0.99,
            half_cell_offset: 0.5,
            compaction_decay_rate: 8.0,
            sleep_velocity_squared: 0.1,
            wake_velocity_squared: 0.5,
            step_threshold_ratio: 0.5,
            blade_ceiling_ratio: 0.10,
            load_bearing_radius_ratio: 0.85,
            max_wall_push_ratio: 0.5,
            rim_inner_radius_ratio: 0.9,
            grounding_drift_buffer: 0.01,
            probability_hash_scale: 1000.0,
            submerged_buoyancy_lift: 2.5,
            buoyancy_horizontal_speed_threshold: 0.1,
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
