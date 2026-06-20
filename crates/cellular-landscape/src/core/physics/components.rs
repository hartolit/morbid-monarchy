use bevy::{
    ecs::{component::Component, resource::Resource},
    math::Vec3,
};

/// Universal thermodynamic constants binding the simulation space.
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
