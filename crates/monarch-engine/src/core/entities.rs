pub mod observer;
pub mod spherical;

use bevy::{
    ecs::{component::Component, resource::Resource},
    math::Vec3,
};
use world_model::{
    body::AIR_DRAG_RATE,
    config::DEFAULT_ELEVATION_SCALE,
    units::{GRAVITY_Y, MAX_BODY_SUBSTEP_SECONDS},
};

#[derive(Resource, Debug, Clone, Copy)]
pub struct GlobalPhysicsConfig {
    pub gravity: Vec3,
    pub elevation_scale: f32,
    pub epsilon_distance: f32,
    pub half_cell_offset: f32,
    /// Longest rigid-body integration step. Frames longer than this are split into equal
    /// substeps so contact response and damping do not depend on the render frame rate.
    pub max_substep_seconds: f32,
    /// Upper bound on substeps per frame; bounds solver cost during frame-time spikes.
    pub max_substeps: u32,
}

impl Default for GlobalPhysicsConfig {
    fn default() -> Self {
        Self {
            gravity: Vec3::new(0.0, GRAVITY_Y, 0.0),
            elevation_scale: DEFAULT_ELEVATION_SCALE,
            epsilon_distance: 0.000001,
            half_cell_offset: 0.5,
            max_substep_seconds: MAX_BODY_SUBSTEP_SECONDS,
            max_substeps: 8,
        }
    }
}

#[derive(Component, Debug, Clone, Copy)]
pub struct KinematicProfile {
    pub mass: f32,
    /// Exponential velocity decay rate (1/s) in air: `v *= exp(-rate * dt)`, which gives the
    /// same trajectory however a frame is subdivided.
    pub air_drag_rate: f32,
    /// Coulomb rolling resistance: while grounded the sphere loses speed at a constant
    /// `coefficient * g * n.y`, so it coasts realistically and rolls down slopes steeper than
    /// the coefficient.
    pub rolling_resistance_coefficient: f32,
    /// Additional exponential decay rate (1/s) of tangential velocity while grounded, standing
    /// in for drag from grass and loose soil.
    pub ground_damping_rate: f32,
    /// Steepest slope (as rise over run) on which a slow sphere is held in place.
    pub static_friction_coefficient: f32,
    /// Tangential speed below which static friction may lock a grounded sphere.
    pub static_friction_speed: f32,
    pub impact_restitution: f32,
}

impl Default for KinematicProfile {
    fn default() -> Self {
        Self {
            mass: 1.0,
            air_drag_rate: AIR_DRAG_RATE,
            rolling_resistance_coefficient: 0.12,
            ground_damping_rate: 0.4,
            static_friction_coefficient: 0.2,
            static_friction_speed: 0.5,
            impact_restitution: 0.20,
        }
    }
}

#[derive(Component, Debug, Clone, Copy)]
pub struct DeformationProfile {
    pub min_deformation_energy: f32,
    pub energy_to_deformation_scale: f32,
    pub cost_displace_granular: f32,
    pub cost_crush_terrain: f32,
    /// Trail depth (world units) pressed into ground of softness 1 by a body of
    /// `rut_reference_mass`. Heavier bodies sink deeper with the square root of their mass.
    pub rut_depth_scale: f32,
    pub rut_reference_mass: f32,
    /// Fraction of the radius that bears on the ground and forms the trail.
    pub rut_width_ratio: f32,
    /// Most a single substep presses one column, in elevation units, so sinking is gradual.
    pub rut_press_per_substep: u16,
    /// Kinetic energy spent per elevation unit of ground pressed or shoved aside while rolling.
    pub rut_cost_per_unit: f32,
}

impl Default for DeformationProfile {
    fn default() -> Self {
        Self {
            min_deformation_energy: 250.0,
            energy_to_deformation_scale: 0.001,
            cost_displace_granular: 2.0,
            cost_crush_terrain: 3.0,
            rut_depth_scale: 1.0,
            rut_reference_mass: 4.0,
            rut_width_ratio: 0.7,
            rut_press_per_substep: 1,
            rut_cost_per_unit: 1.0,
        }
    }
}
