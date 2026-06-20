use bevy::{
    ecs::{component::Component, resource::Resource},
    math::Vec3,
};

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

/// A generic capability component. Attach this to ANY entity in the orchestrator
/// to grant it physical interactions, gravity, and CCD collision with the cellular grid.
#[derive(Component, Debug, Clone, Copy)]
pub struct GridKinematicBody {
    pub velocity: Vec3,
    pub mass: f32,
    pub radius: f32,
    pub rolling_friction: f32,
    pub impact_restitution: f32,
}

impl GridKinematicBody {
    pub fn new(mass: f32, radius: f32) -> Self {
        Self {
            velocity: Vec3::ZERO,
            mass,
            radius,
            rolling_friction: 0.90,
            impact_restitution: 0.20,
        }
    }
}

/// A generic capability component. Attach this to a GridKinematicBody to allow it
/// to dynamically crater and excavate the landscape upon high-energy impact.
#[derive(Component, Debug, Clone, Copy)]
pub struct GridDeformer {
    pub min_deformation_energy: f32,
    pub energy_to_deformation_scale: f32,
    pub cost_displace_granular: f32,
    pub cost_crush_terrain: f32,
}

impl Default for GridDeformer {
    fn default() -> Self {
        Self {
            min_deformation_energy: 250.0,
            energy_to_deformation_scale: 0.001,
            cost_displace_granular: 2.0,
            cost_crush_terrain: 3.0,
        }
    }
}
