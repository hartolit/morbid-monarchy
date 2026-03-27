use std::collections::BTreeMap;

use bevy_math::{Vec2, Vec3};
use bitcode::{Decode, Encode};
use serde::{Deserialize, Serialize};

pub const DEFAULT_CHUNK_WORLD_SIZE: f32 = 256.0;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Encode, Decode,
)]
pub struct ChunkKey {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl ChunkKey {
    pub const ORIGIN: Self = Self { x: 0, y: 0, z: 0 };

    pub fn from_world_position(position: Vec3, chunk_world_size: f32) -> Self {
        Self {
            x: (position.x / chunk_world_size).floor() as i32,
            y: (position.y / chunk_world_size).floor() as i32,
            z: (position.z / chunk_world_size).floor() as i32,
        }
    }

    pub fn min_world_corner(self, chunk_world_size: f32) -> Vec2 {
        Vec2::new(
            self.x as f32 * chunk_world_size,
            self.y as f32 * chunk_world_size,
        )
    }

    pub fn center_world_position(self, chunk_world_size: f32, z: f32) -> Vec3 {
        let min = self.min_world_corner(chunk_world_size);
        Vec3::new(
            min.x + chunk_world_size * 0.5,
            min.y + chunk_world_size * 0.5,
            z,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct ChunkLocalPoint {
    pub x: f32,
    pub y: f32,
}

impl ChunkLocalPoint {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn to_world_position(self, key: ChunkKey, chunk_world_size: f32, z: f32) -> Vec3 {
        let min = key.min_world_corner(chunk_world_size);
        Vec3::new(min.x + self.x, min.y + self.y, z)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct ChunkBounds {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

impl ChunkBounds {
    pub fn from_center(center: ChunkLocalPoint, half_extent: f32) -> Self {
        Self {
            min_x: center.x - half_extent,
            min_y: center.y - half_extent,
            max_x: center.x + half_extent,
            max_y: center.y + half_extent,
        }
    }

    pub fn contains_local_point(self, point: ChunkLocalPoint) -> bool {
        point.x >= self.min_x
            && point.x <= self.max_x
            && point.y >= self.min_y
            && point.y <= self.max_y
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
pub enum BaseMaterial {
    Grass,
    Water,
    Stone,
    DarkSoil,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
pub enum SurfaceTraversal {
    Walk,
    Swim,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
pub struct BaseLayer {
    pub material: BaseMaterial,
    pub traversal: SurfaceTraversal,
}

impl BaseLayer {
    pub fn new(material: BaseMaterial, traversal: SurfaceTraversal) -> Self {
        Self {
            material,
            traversal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Encode, Decode)]
pub enum ChunkTheme {
    Dark,
    GrassPlane,
    Cave,
    Ocean,
}

impl ChunkTheme {
    pub fn base_layer(self) -> BaseLayer {
        match self {
            Self::Dark => BaseLayer::new(BaseMaterial::DarkSoil, SurfaceTraversal::Walk),
            Self::GrassPlane => BaseLayer::new(BaseMaterial::Grass, SurfaceTraversal::Walk),
            Self::Cave => BaseLayer::new(BaseMaterial::Stone, SurfaceTraversal::Walk),
            Self::Ocean => BaseLayer::new(BaseMaterial::Water, SurfaceTraversal::Swim),
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Encode, Decode,
)]
pub struct WorldObjectId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
pub enum ProcAssetKind {
    Tree,
    Bush,
    Grass,
    Rock,
    DirtPatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
pub enum CollisionKind {
    None,
    Blocking,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
pub enum InteractionKind {
    None,
    Destructible,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct ProcAsset {
    pub id: WorldObjectId,
    pub kind: ProcAssetKind,
    pub intensity: u8,
    pub variant: u8,
    pub position: ChunkLocalPoint,
    pub bounds: ChunkBounds,
    pub collision: CollisionKind,
    pub interaction: InteractionKind,
}

impl ProcAssetKind {
    pub fn radius(self) -> f32 {
        match self {
            Self::Tree => 18.0,
            Self::Bush => 14.0,
            Self::Grass => 10.0,
            Self::Rock => 16.0,
            Self::DirtPatch => 20.0,
        }
    }

    pub fn collision(self) -> CollisionKind {
        match self {
            Self::Rock | Self::Tree => CollisionKind::Blocking,
            Self::Bush | Self::Grass | Self::DirtPatch => CollisionKind::None,
        }
    }

    pub fn interaction(self) -> InteractionKind {
        match self {
            Self::Bush => InteractionKind::Destructible,
            Self::Tree | Self::Grass | Self::Rock | Self::DirtPatch => InteractionKind::None,
        }
    }
}

impl ProcAsset {
    pub fn new(
        id: WorldObjectId,
        kind: ProcAssetKind,
        intensity: u8,
        variant: u8,
        position: ChunkLocalPoint,
    ) -> Self {
        let radius = kind.radius();
        Self {
            id,
            kind,
            intensity,
            variant,
            position,
            bounds: ChunkBounds::from_center(position, radius),
            collision: kind.collision(),
            interaction: kind.interaction(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
pub struct ChunkMutation {
    pub removed_object_ids: Vec<WorldObjectId>,
}

impl ChunkMutation {
    pub fn is_removed(&self, object_id: WorldObjectId) -> bool {
        self.removed_object_ids.binary_search(&object_id).is_ok()
    }

    pub fn remove_object(&mut self, object_id: WorldObjectId) {
        if self.is_removed(object_id) {
            return;
        }

        self.removed_object_ids.push(object_id);
        self.removed_object_ids.sort_unstable();
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct ChunkSnapshot {
    pub key: ChunkKey,
    pub theme: ChunkTheme,
    pub base_layer: BaseLayer,
    pub assets: Vec<ProcAsset>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct ChunkState {
    pub snapshot: ChunkSnapshot,
    pub mutation: ChunkMutation,
}

impl ChunkState {
    pub fn new(snapshot: ChunkSnapshot) -> Self {
        Self {
            snapshot,
            mutation: ChunkMutation::default(),
        }
    }

    pub fn visible_assets(&self) -> impl Iterator<Item = &ProcAsset> {
        self.snapshot
            .assets
            .iter()
            .filter(|asset| !self.mutation.is_removed(asset.id))
    }

    pub fn remove_object(&mut self, object_id: WorldObjectId) {
        self.mutation.remove_object(object_id);
    }
}

#[derive(Debug, Default, Clone)]
pub struct WorldStore {
    chunks: BTreeMap<ChunkKey, ChunkState>,
}

impl WorldStore {
    pub fn insert(&mut self, state: ChunkState) {
        self.chunks.insert(state.snapshot.key, state);
    }

    pub fn get(&self, key: ChunkKey) -> Option<&ChunkState> {
        self.chunks.get(&key)
    }

    pub fn get_mut(&mut self, key: ChunkKey) -> Option<&mut ChunkState> {
        self.chunks.get_mut(&key)
    }

    pub fn remove(&mut self, key: ChunkKey) -> Option<ChunkState> {
        self.chunks.remove(&key)
    }

    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    pub fn keys(&self) -> impl Iterator<Item = ChunkKey> + '_ {
        self.chunks.keys().copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkView {
    pub min: ChunkKey,
    pub max: ChunkKey,
}

impl ChunkView {
    pub fn around(center: ChunkKey, radius: i32) -> Self {
        Self {
            min: ChunkKey {
                x: center.x - radius,
                y: center.y - radius,
                z: center.z,
            },
            max: ChunkKey {
                x: center.x + radius,
                y: center.y + radius,
                z: center.z,
            },
        }
    }

    pub fn keys(self) -> Vec<ChunkKey> {
        let mut keys = Vec::new();
        for y in self.min.y..=self.max.y {
            for x in self.min.x..=self.max.x {
                keys.push(ChunkKey { x, y, z: self.min.z });
            }
        }
        keys
    }
}

pub fn active_chunk_keys(center: ChunkKey, radius: i32) -> Vec<ChunkKey> {
    ChunkView::around(center, radius).keys()
}
