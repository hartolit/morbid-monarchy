pub mod player;
pub mod systems;
pub mod world;

use bevy_app::{App, Plugin, Update};
use bevy_ecs::prelude::SystemSet;
use bevy_ecs::schedule::IntoScheduleConfigs;

pub use player::{
    DEFAULT_PLAYER_SPEED, MovementConfig, MovementIntent, Player, PlayerBundle, SimulationStep,
};
pub use systems::apply_movement_intent;
pub use world::{
    active_chunk_keys, generate_chunk, BaseLayer, BaseMaterial, ChunkBounds, ChunkKey,
    ChunkLocalPoint, ChunkMutation, ChunkSnapshot, ChunkState, ChunkTheme, ChunkView,
    CollisionKind, InteractionKind, ProcAsset, ProcAssetKind, SurfaceTraversal, WorldConfig,
    WorldObjectId, WorldStore,
};

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MorbidMonarchyCoreSystems {
    Movement,
}

pub struct MorbidMonarchyCorePlugin;

impl Plugin for MorbidMonarchyCorePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MovementConfig>()
            .init_resource::<SimulationStep>()
            .init_resource::<WorldConfig>()
            .configure_sets(Update, MorbidMonarchyCoreSystems::Movement)
            .add_systems(
                Update,
                apply_movement_intent.in_set(MorbidMonarchyCoreSystems::Movement),
            );
    }
}
