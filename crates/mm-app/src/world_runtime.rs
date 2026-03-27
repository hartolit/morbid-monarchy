use std::collections::{BTreeMap, BTreeSet};

use bevy::asset::RenderAssetUsages;
use bevy::log::warn;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use mm_core::{
    active_chunk_keys, chunk_pixel_from_world_position, chunk_world_units_per_pixel,
    generate_chunk, ChunkKey, ChunkLocalPixel, ChunkState, ChunkTheme, CollisionKind,
    InteractionKind, Player, SurfaceTraversal, WorldConfig, WorldObjectId, WorldPixel,
    WorldStore, CHUNK_PIXEL_SIZE,
};

use crate::world_persistence::ChunkPersistence;

const WORLD_BASE_Z: f32 = -10.0;
const WORLD_ASSET_Z: f32 = 2.0;
const PLAYER_INTERACTION_RANGE: f32 = 36.0;
const ENEMY_INTERACTION_RANGE: f32 = 48.0;

#[derive(Component, Debug, Default, Clone, Copy)]
pub struct PreviousTranslation(pub Vec3);

#[derive(Component, Debug)]
pub struct PrimaryWorldCamera;

#[derive(Component, Debug)]
pub struct Enemy;

#[derive(Component, Debug, Clone)]
pub struct ChunkRender {
    image: Handle<Image>,
}

#[derive(Debug, Clone, Copy)]
struct WorldSplatter {
    world_position: Vec3,
    pixel: WorldPixel,
}

#[derive(Resource, Default)]
pub struct RuntimeWorldState {
    pub store: WorldStore,
}

#[derive(Resource, Default)]
pub struct ChunkRenderState {
    entities: BTreeMap<ChunkKey, Entity>,
}

#[derive(Resource, Default)]
pub struct PendingWorldSplatters {
    items: Vec<WorldSplatter>,
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

        match persistence.load_chunk(&config, key) {
            Ok(Some(state)) => runtime.store.insert(state),
            Ok(None) => runtime.store.insert(ChunkState::new(generate_chunk(&config, key))),
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

pub fn sync_chunk_renders(
    mut commands: Commands,
    config: Res<WorldConfig>,
    runtime: Res<RuntimeWorldState>,
    mut images: ResMut<Assets<Image>>,
    mut render_state: ResMut<ChunkRenderState>,
    renders: Query<&ChunkRender>,
) {
    let active_keys: BTreeSet<_> = runtime.store.keys().collect();
    let stale: Vec<_> = render_state
        .entities
        .iter()
        .filter(|(key, _)| !active_keys.contains(key))
        .map(|(key, entity)| (*key, *entity))
        .collect();

    for (key, entity) in stale {
        commands.entity(entity).despawn();
        render_state.entities.remove(&key);
    }

    for key in runtime.store.keys() {
        let Some(state) = runtime.store.get(key) else {
            continue;
        };

        if let Some(entity) = render_state.entities.get(&key).copied() {
            if let Ok(render) = renders.get(entity) {
                if let Some(image) = images.get_mut(&render.image) {
                    image.data = Some(chunk_image_bytes(state));
                }
                continue;
            }

            render_state.entities.remove(&key);
        }

        let image = images.add(build_chunk_image(state));
        let entity = commands
            .spawn((
                ChunkRender {
                    image: image.clone(),
                },
                Sprite::from_image(image),
                Transform {
                    translation: key.center_world_position(config.chunk_world_size, WORLD_BASE_Z),
                    scale: Vec3::splat(chunk_world_units_per_pixel(config.chunk_world_size)),
                    ..Default::default()
                },
            ))
            .id();
        render_state.entities.insert(key, entity);
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

pub fn handle_enemy_interaction(
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    players: Query<&Transform, With<Player>>,
    enemies: Query<(Entity, &Transform), With<Enemy>>,
    mut pending_splatters: ResMut<PendingWorldSplatters>,
) {
    if !keyboard_input.just_pressed(KeyCode::KeyF) {
        return;
    }

    let Some(player_transform) = players.iter().next() else {
        return;
    };

    let mut candidate: Option<(Entity, Vec3, f32)> = None;
    for (entity, transform) in &enemies {
        let distance = transform
            .translation
            .truncate()
            .distance(player_transform.translation.truncate());
        if distance > ENEMY_INTERACTION_RANGE {
            continue;
        }

        match candidate {
            Some((_, _, best_distance)) if best_distance <= distance => {}
            _ => candidate = Some((entity, transform.translation, distance)),
        }
    }

    if let Some((entity, world_position, _)) = candidate {
        commands.entity(entity).despawn();
        pending_splatters.items.push(WorldSplatter {
            world_position,
            pixel: WorldPixel::Blood,
        });
    }
}

pub fn apply_pending_world_splatters(
    config: Res<WorldConfig>,
    persistence: Res<ChunkPersistence>,
    mut runtime: ResMut<RuntimeWorldState>,
    mut pending_splatters: ResMut<PendingWorldSplatters>,
) {
    let splatters = std::mem::take(&mut pending_splatters.items);
    for splatter in splatters {
        let pixel_position = chunk_pixel_from_world_position(splatter.world_position, config.chunk_world_size);
        let key = pixel_position.key;

        if runtime.store.get(key).is_none() {
            match persistence.load_chunk(&config, key) {
                Ok(Some(state)) => runtime.store.insert(state),
                Ok(None) => runtime.store.insert(ChunkState::new(generate_chunk(&config, key))),
                Err(error) => {
                    warn!("failed to load chunk for splatter {:?}: {}", key, error);
                    runtime.store.insert(ChunkState::new(generate_chunk(&config, key)));
                }
            }
        }

        if let Some(state) = runtime.store.get_mut(key) {
            state.set_pixel(pixel_position.local, splatter.pixel);
            if let Err(error) = persistence.save_chunk(state) {
                warn!("failed to persist splatter {:?}: {}", key, error);
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
        let pixel_position = chunk_pixel_from_world_position(transform.translation, config.chunk_world_size);
        let traversal = runtime
            .store
            .get(pixel_position.key)
            .map(|state| state.pixel(pixel_position.local).traversal())
            .unwrap_or(SurfaceTraversal::Walk);

        sprite.color = if traversal == SurfaceTraversal::Swim {
            Color::srgb(0.7, 0.85, 1.0)
        } else {
            Color::WHITE
        };
    }
}

pub fn camera_follow_player(
    players: Query<&Transform, With<Player>>,
    mut cameras: Query<&mut Transform, (With<PrimaryWorldCamera>, Without<Player>)>,
) {
    let Some(player_transform) = players.iter().next() else {
        return;
    };

    for mut camera_transform in &mut cameras {
        camera_transform.translation.x = player_transform.translation.x;
        camera_transform.translation.y = player_transform.translation.y;
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
        let color = theme_color(state.data.theme);

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
    let pixel_position = chunk_pixel_from_world_position(position, config.chunk_world_size);

    if let Some(state) = store.get(pixel_position.key) {
        if state.pixel(pixel_position.local).blocks_movement() {
            return true;
        }
    }

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

fn theme_color(theme: ChunkTheme) -> Color {
    match theme {
        ChunkTheme::GrassPlane => Color::srgb(0.25, 0.7, 0.3),
        ChunkTheme::Dark => Color::srgb(0.25, 0.22, 0.3),
        ChunkTheme::Cave => Color::srgb(0.45, 0.45, 0.5),
        ChunkTheme::Ocean => Color::srgb(0.2, 0.45, 0.85),
    }
}

fn asset_color(collision: CollisionKind, interaction: InteractionKind) -> Color {
    match (collision, interaction) {
        (CollisionKind::Blocking, _) => Color::srgb(0.55, 0.35, 0.2),
        (_, InteractionKind::Destructible) => Color::srgb(0.35, 0.8, 0.25),
        _ => Color::srgb(0.8, 0.75, 0.5),
    }
}

fn build_chunk_image(state: &ChunkState) -> Image {
    Image::new_fill(
        Extent3d {
            width: u32::from(CHUNK_PIXEL_SIZE),
            height: u32::from(CHUNK_PIXEL_SIZE),
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &chunk_image_bytes(state),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

fn chunk_image_bytes(state: &ChunkState) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(usize::from(CHUNK_PIXEL_SIZE) * usize::from(CHUNK_PIXEL_SIZE) * 4);
    for y in 0..CHUNK_PIXEL_SIZE {
        for x in 0..CHUNK_PIXEL_SIZE {
            let pixel = state.pixel(ChunkLocalPixel { x, y });
            bytes.extend_from_slice(&pixel_rgba(pixel));
        }
    }
    bytes
}

fn pixel_rgba(pixel: WorldPixel) -> [u8; 4] {
    match pixel {
        WorldPixel::Empty => [0, 0, 0, 0],
        WorldPixel::Grass => [71, 135, 74, 255],
        WorldPixel::Dirt => [108, 79, 52, 255],
        WorldPixel::Rock => [118, 121, 128, 255],
        WorldPixel::Water => [56, 99, 176, 255],
        WorldPixel::Blood => [145, 25, 38, 255],
    }
}

#[cfg(test)]
mod tests {
    use bevy::app::{App, Update};
    use bevy::prelude::{ButtonInput, KeyCode, Sprite, Transform, Vec3};
    use bevy::transform::components::GlobalTransform;
    use mm_core::{ChunkState, MovementIntent, Player, SimulationStep, WorldConfig, WorldPixel, chunk_pixel_from_world_position, generate_chunk};

    use super::{
        ChunkRenderState, Enemy, EntitySpatialIndex, PendingWorldSplatters, PrimaryWorldCamera,
        RuntimeWorldState, WorldSplatter, apply_pending_world_splatters, camera_follow_player,
        handle_enemy_interaction, stream_world_around_player, update_entity_spatial_index,
    };
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

    #[test]
    fn camera_follow_matches_player_xy_and_preserves_camera_z() {
        let mut app = App::new();
        app.add_systems(Update, camera_follow_player);
        app.world_mut().spawn((
            Player,
            Transform::from_xyz(42.0, -18.0, 10.0),
            GlobalTransform::default(),
        ));
        let camera = app
            .world_mut()
            .spawn((
                PrimaryWorldCamera,
                Transform::from_xyz(0.0, 0.0, 999.0),
                GlobalTransform::default(),
            ))
            .id();

        app.update();

        let transform = app.world().entity(camera).get::<Transform>().unwrap();
        assert_eq!(transform.translation, Vec3::new(42.0, -18.0, 999.0));
    }

    #[test]
    fn pending_splatter_paints_and_persists_chunk_pixel() {
        let base_dir = std::env::temp_dir().join(format!(
            "mm-app-runtime-test-{}-{}",
            std::process::id(),
            "splatter"
        ));
        let persistence = ChunkPersistence::new(base_dir.clone());
        let config = WorldConfig::default();
        let mut app = App::new();
        app.insert_resource(config);
        app.insert_resource(RuntimeWorldState::default());
        app.insert_resource(ChunkRenderState::default());
        app.insert_resource(PendingWorldSplatters {
            items: vec![WorldSplatter {
                world_position: Vec3::new(0.0, 0.0, 0.0),
                pixel: WorldPixel::Blood,
            }],
        });
        app.insert_resource(persistence.clone());
        app.add_systems(Update, apply_pending_world_splatters);

        app.update();

        let pixel_position = chunk_pixel_from_world_position(Vec3::new(0.0, 0.0, 0.0), config.chunk_world_size);
        let runtime = app.world().resource::<RuntimeWorldState>();
        let state = runtime.store.get(pixel_position.key).unwrap();
        assert_eq!(state.pixel(pixel_position.local), WorldPixel::Blood);

        let reloaded = persistence
            .load_chunk(&config, pixel_position.key)
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.pixel(pixel_position.local), WorldPixel::Blood);

        if base_dir.exists() {
            let _ = std::fs::remove_dir_all(base_dir);
        }
    }

    #[test]
    fn enemy_interaction_queues_blood_splatter() {
        let mut app = App::new();
        app.insert_resource(ButtonInput::<KeyCode>::default());
        app.insert_resource(PendingWorldSplatters::default());
        app.add_systems(Update, handle_enemy_interaction);
        app.world_mut().spawn((
            Player,
            Transform::from_xyz(0.0, 0.0, 0.0),
            GlobalTransform::default(),
            Sprite::default(),
        ));
        app.world_mut().spawn((
            Enemy,
            Transform::from_xyz(10.0, 0.0, 8.0),
            GlobalTransform::default(),
        ));

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyF);

        app.update();

        let pending = app.world().resource::<PendingWorldSplatters>();
        assert_eq!(pending.items.len(), 1);
        assert_eq!(pending.items[0].pixel, WorldPixel::Blood);
    }
}
