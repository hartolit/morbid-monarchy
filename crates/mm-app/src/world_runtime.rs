use std::collections::{BTreeMap, BTreeSet};

use bevy::log::warn;
use bevy::prelude::*;
use mm_core::{
    active_chunk_keys, generate_chunk, ChunkKey, ChunkState, CollisionKind, InteractionKind,
    Player, SurfaceTraversal, WorldConfig, WorldObjectId, WorldStore,
};

use crate::world_persistence::ChunkPersistence;

const WORLD_ASSET_Z: f32 = 2.0;
const PLAYER_INTERACTION_RANGE: f32 = 36.0;

#[derive(Component, Debug, Default, Clone, Copy)]
pub struct PreviousTranslation(pub Vec3);

#[derive(Resource, Default)]
pub struct RuntimeWorldState {
    pub store: WorldStore,
}

#[derive(Resource, Default)]
pub struct EntitySpatialIndex {
    entity_to_chunk: BTreeMap<Entity, ChunkKey>,
    chunk_to_entities: BTreeMap<ChunkKey, Vec<Entity>>,
}

impl EntitySpatialIndex {
    pub fn upsert(&mut self, entity: Entity, next_chunk: ChunkKey) {
        if let Some(current) = self.entity_to_chunk.get(&entity).copied() {
            if current == next_chunk {
                return;
            }
            self.remove_from_chunk(entity, current);
        }

        self.entity_to_chunk.insert(entity, next_chunk);
        self.chunk_to_entities
            .entry(next_chunk)
            .or_default()
            .push(entity);
    }

    fn remove_from_chunk(&mut self, entity: Entity, key: ChunkKey) {
        let Some(entities) = self.chunk_to_entities.get_mut(&key) else {
            return;
        };

        entities.retain(|stored| *stored != entity);
        if entities.is_empty() {
            self.chunk_to_entities.remove(&key);
        }
    }
}

pub fn capture_previous_player_positions(
    mut players: Query<(&Transform, &mut PreviousTranslation), With<Player>>,
) {
    for (transform, mut previous) in &mut players {
        previous.0 = transform.translation;
    }
}

pub fn update_entity_spatial_index(
    config: Res<WorldConfig>,
    mut index: ResMut<EntitySpatialIndex>,
    players: Query<(Entity, &Transform), With<Player>>,
) {
    for (entity, transform) in &players {
        let chunk = ChunkKey::from_world_position(transform.translation, config.chunk_world_size);
        index.upsert(entity, chunk);
    }
}

pub fn stream_world_around_player(
    config: Res<WorldConfig>,
    persistence: Res<ChunkPersistence>,
    mut runtime: ResMut<RuntimeWorldState>,
    players: Query<&Transform, With<Player>>,
) {
    let Some(transform) = players.iter().next() else {
        return;
    };

    let center = ChunkKey::from_world_position(transform.translation, config.chunk_world_size);
    let active_keys = active_chunk_keys(center, config.active_chunk_radius);
    let active_set: BTreeSet<_> = active_keys.iter().copied().collect();

    for key in active_keys {
        if runtime.store.get(key).is_some() {
            continue;
        }

        match persistence.load_chunk(key) {
            Ok(Some(state)) => runtime.store.insert(state),
            Ok(None) => {
                let generated = ChunkState::new(generate_chunk(&config, key));
                if let Err(error) = persistence.save_chunk(&generated) {
                    warn!("failed to persist generated chunk {:?}: {}", key, error);
                }
                runtime.store.insert(generated);
            }
            Err(error) => {
                warn!("failed to load chunk {:?}: {}", key, error);
                runtime.store.insert(ChunkState::new(generate_chunk(&config, key)));
            }
        }
    }

    let stale_keys: Vec<_> = runtime
        .store
        .keys()
        .filter(|key| !active_set.contains(key))
        .collect();
    for key in stale_keys {
        runtime.store.remove(key);
    }
}

pub fn resolve_player_world_collision(
    config: Res<WorldConfig>,
    runtime: Res<RuntimeWorldState>,
    mut players: Query<(&mut Transform, &PreviousTranslation), With<Player>>,
) {
    for (mut transform, previous) in &mut players {
        if collides_with_blocker(transform.translation, &runtime.store, &config) {
            transform.translation = previous.0;
        }
    }
}

pub fn handle_world_interaction(
    keyboard_input: Res<ButtonInput<KeyCode>>,
    config: Res<WorldConfig>,
    persistence: Res<ChunkPersistence>,
    mut runtime: ResMut<RuntimeWorldState>,
    players: Query<&Transform, With<Player>>,
) {
    if !keyboard_input.just_pressed(KeyCode::Space) {
        return;
    }

    let Some(player_transform) = players.iter().next() else {
        return;
    };

    let mut candidate: Option<(ChunkKey, WorldObjectId, f32)> = None;
    for key in runtime.store.keys().collect::<Vec<_>>() {
        let Some(state) = runtime.store.get(key) else {
            continue;
        };
        for asset in state.visible_assets() {
            if asset.interaction != InteractionKind::Destructible {
                continue;
            }

            let world_position = asset.position.to_world_position(key, config.chunk_world_size, WORLD_ASSET_Z);
            let distance = world_position.truncate().distance(player_transform.translation.truncate());
            if distance > PLAYER_INTERACTION_RANGE {
                continue;
            }

            match candidate {
                Some((_, _, best_distance)) if best_distance <= distance => {}
                _ => candidate = Some((key, asset.id, distance)),
            }
        }
    }

    if let Some((key, object_id, _)) = candidate {
        if let Some(state) = runtime.store.get_mut(key) {
            state.remove_object(object_id);
            if let Err(error) = persistence.save_chunk(state) {
                warn!("failed to persist chunk mutation {:?}: {}", key, error);
            }
        }
    }
}

pub fn sync_player_surface_state(
    config: Res<WorldConfig>,
    runtime: Res<RuntimeWorldState>,
    mut players: Query<(&Transform, &mut Sprite), With<Player>>,
) {
    for (transform, mut sprite) in &mut players {
        let key = ChunkKey::from_world_position(transform.translation, config.chunk_world_size);
        let traversal = runtime
            .store
            .get(key)
            .map(|state| state.snapshot.base_layer.traversal)
            .unwrap_or(SurfaceTraversal::Walk);

        sprite.color = if traversal == SurfaceTraversal::Swim {
            Color::srgb(0.7, 0.85, 1.0)
        } else {
            Color::WHITE
        };
    }
}

pub fn draw_world_debug(
    config: Res<WorldConfig>,
    runtime: Res<RuntimeWorldState>,
    mut gizmos: Gizmos,
) {
    for key in runtime.store.keys() {
        let Some(state) = runtime.store.get(key) else {
            continue;
        };

        let min = key.min_world_corner(config.chunk_world_size);
        let max = min + Vec2::splat(config.chunk_world_size);
        let color = theme_color(state.snapshot.base_layer.traversal, state.snapshot.theme);

        gizmos.line_2d(min, Vec2::new(max.x, min.y), color);
        gizmos.line_2d(Vec2::new(max.x, min.y), max, color);
        gizmos.line_2d(max, Vec2::new(min.x, max.y), color);
        gizmos.line_2d(Vec2::new(min.x, max.y), min, color);

        for asset in state.visible_assets() {
            let position = asset
                .position
                .to_world_position(key, config.chunk_world_size, WORLD_ASSET_Z)
                .truncate();
            gizmos.circle_2d(position, asset.kind.radius() * 0.5, asset_color(asset.collision, asset.interaction));
        }
    }
}

fn collides_with_blocker(position: Vec3, store: &WorldStore, config: &WorldConfig) -> bool {
    let player_point = position.truncate();

    for key in store.keys() {
        let Some(state) = store.get(key) else {
            continue;
        };
        for asset in state.visible_assets() {
            if asset.collision != CollisionKind::Blocking {
                continue;
            }

            let min_corner = key.min_world_corner(config.chunk_world_size);
            let world_min = Vec2::new(min_corner.x + asset.bounds.min_x, min_corner.y + asset.bounds.min_y);
            let world_max = Vec2::new(min_corner.x + asset.bounds.max_x, min_corner.y + asset.bounds.max_y);
            if player_point.x >= world_min.x
                && player_point.x <= world_max.x
                && player_point.y >= world_min.y
                && player_point.y <= world_max.y
            {
                return true;
            }
        }
    }

    false
}

fn theme_color(traversal: SurfaceTraversal, theme: mm_core::ChunkTheme) -> Color {
    match (theme, traversal) {
        (_, SurfaceTraversal::Swim) => Color::srgb(0.2, 0.45, 0.85),
        (mm_core::ChunkTheme::GrassPlane, _) => Color::srgb(0.25, 0.7, 0.3),
        (mm_core::ChunkTheme::Dark, _) => Color::srgb(0.25, 0.22, 0.3),
        (mm_core::ChunkTheme::Cave, _) => Color::srgb(0.45, 0.45, 0.5),
        (mm_core::ChunkTheme::Ocean, _) => Color::srgb(0.2, 0.45, 0.85),
    }
}

fn asset_color(collision: CollisionKind, interaction: InteractionKind) -> Color {
    match (collision, interaction) {
        (CollisionKind::Blocking, _) => Color::srgb(0.55, 0.35, 0.2),
        (_, InteractionKind::Destructible) => Color::srgb(0.35, 0.8, 0.25),
        _ => Color::srgb(0.8, 0.75, 0.5),
    }
}

#[cfg(test)]
mod tests {
    use bevy::app::{App, Update};
    use bevy::prelude::Transform;
    use bevy::transform::components::GlobalTransform;
    use mm_core::{ChunkState, MovementIntent, Player, SimulationStep, WorldConfig, generate_chunk};

    use super::{EntitySpatialIndex, RuntimeWorldState, stream_world_around_player, update_entity_spatial_index};
    use crate::world_persistence::ChunkPersistence;

    #[test]
    fn entity_spatial_index_does_not_duplicate_stationary_player() {
        let mut app = App::new();
        app.insert_resource(WorldConfig::default());
        app.insert_resource(EntitySpatialIndex::default());
        app.add_systems(Update, update_entity_spatial_index);

        let entity = app
            .world_mut()
            .spawn((Player, Transform::from_xyz(0.0, 0.0, 0.0), GlobalTransform::default()))
            .id();

        app.update();
        app.update();

        let index = app.world().resource::<EntitySpatialIndex>();
        let key = index.entity_to_chunk.get(&entity).copied().unwrap();
        assert_eq!(index.chunk_to_entities.get(&key).unwrap().len(), 1);
    }

    #[test]
    fn streaming_loads_a_three_by_three_window() {
        let mut app = App::new();
        app.insert_resource(WorldConfig::default());
        app.insert_resource(RuntimeWorldState::default());
        app.insert_resource(ChunkPersistence::new(std::env::temp_dir().join(format!(
            "mm-app-runtime-test-{}-{}",
            std::process::id(),
            "streaming"
        ))));
        app.add_systems(Update, stream_world_around_player);
        app.world_mut().spawn((Player, Transform::from_xyz(0.0, 0.0, 0.0)));

        app.update();

        let runtime = app.world().resource::<RuntimeWorldState>();
        assert_eq!(runtime.store.len(), 9);
    }

    #[test]
    fn persisted_bush_removal_survives_reload() {
        let base_dir = std::env::temp_dir().join(format!(
            "mm-app-runtime-test-{}-{}",
            std::process::id(),
            "reload"
        ));
        let persistence = ChunkPersistence::new(base_dir.clone());
        let config = WorldConfig::default();
        let key = mm_core::ChunkKey::ORIGIN;
        let mut state = ChunkState::new(generate_chunk(&config, key));
        let bush_id = state
            .visible_assets()
            .find(|asset| asset.interaction == mm_core::InteractionKind::Destructible)
            .unwrap()
            .id;
        state.remove_object(bush_id);
        persistence.save_chunk(&state).unwrap();

        let mut app = App::new();
        app.insert_resource(config);
        app.insert_resource(RuntimeWorldState::default());
        app.insert_resource(persistence);
        app.insert_resource(SimulationStep::default());
        app.add_systems(Update, stream_world_around_player);
        app.world_mut().spawn((
            Player,
            MovementIntent::default(),
            Transform::from_xyz(0.0, 0.0, 0.0),
            GlobalTransform::default(),
        ));

        app.update();

        let runtime = app.world().resource::<RuntimeWorldState>();
        let restored = runtime.store.get(key).unwrap();
        assert!(restored.visible_assets().all(|asset| asset.id != bush_id));

        if base_dir.exists() {
            let _ = std::fs::remove_dir_all(base_dir);
        }
    }
}
