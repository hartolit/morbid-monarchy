use bevy_ecs::bundle::Bundle;
use bevy_ecs::prelude::{Component, Resource};
use bevy_math::Vec3;
use bevy_transform::components::{GlobalTransform, Transform};

pub const DEFAULT_PLAYER_SPEED: f32 = 240.0;

#[derive(Component, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Player;

#[derive(Component, Debug, Default, Clone, Copy, PartialEq)]
pub struct MovementIntent(pub Vec3);

#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct MovementConfig {
    pub units_per_second: f32,
}

impl Default for MovementConfig {
    fn default() -> Self {
        Self {
            units_per_second: DEFAULT_PLAYER_SPEED,
        }
    }
}

#[derive(Bundle, Debug, Default)]
pub struct PlayerBundle {
    pub player: Player,
    pub movement_intent: MovementIntent,
    pub transform: Transform,
    pub global_transform: GlobalTransform,
}

#[derive(Resource, Debug, Default, Clone, Copy, PartialEq)]
pub struct SimulationStep {
    pub delta_seconds: f32,
}
