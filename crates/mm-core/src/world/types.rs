use std::collections::BTreeMap;

use bevy_math::{Vec2, Vec3};
use bitcode::{Decode, Encode};
use serde::{Deserialize, Serialize};

pub const DEFAULT_CHUNK_WORLD_SIZE: f32 = 256.0;
pub const CHUNK_PIXEL_SIZE: u16 = 64;
pub const CHUNK_PIXEL_COUNT: usize = CHUNK_PIXEL_SIZE as usize * CHUNK_PIXEL_SIZE as usize;

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
        chunk_pixel_from_world_position(position, chunk_world_size).key
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
pub struct ChunkLocalPixel {
    pub x: u16,
    pub y: u16,
}

impl ChunkLocalPixel {
    pub fn new(x: u16, y: u16) -> Option<Self> {
        if x >= CHUNK_PIXEL_SIZE || y >= CHUNK_PIXEL_SIZE {
            return None;
        }

        Some(Self { x, y })
    }

    pub fn from_index(index: u16) -> Option<Self> {
        if usize::from(index) >= CHUNK_PIXEL_COUNT {
            return None;
        }

        let width = CHUNK_PIXEL_SIZE;
        Some(Self {
            x: index % width,
            y: index / width,
        })
    }

    pub fn as_index(self) -> u16 {
        self.y * CHUNK_PIXEL_SIZE + self.x
    }

    pub fn center_world_position(self, key: ChunkKey, chunk_world_size: f32, z: f32) -> Vec3 {
        let min = key.min_world_corner(chunk_world_size);
        let units_per_pixel = chunk_world_units_per_pixel(chunk_world_size);
        Vec3::new(
            min.x + (self.x as f32 + 0.5) * units_per_pixel,
            min.y + (self.y as f32 + 0.5) * units_per_pixel,
            z,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkPixelPosition {
    pub key: ChunkKey,
    pub local: ChunkLocalPixel,
}

pub fn chunk_world_units_per_pixel(chunk_world_size: f32) -> f32 {
    chunk_world_size / CHUNK_PIXEL_SIZE as f32
}

pub fn chunk_pixel_from_world_position(
    position: Vec3,
    chunk_world_size: f32,
) -> ChunkPixelPosition {
    let units_per_pixel = chunk_world_units_per_pixel(chunk_world_size);
    let world_pixel_x = (position.x / units_per_pixel).floor() as i32;
    let world_pixel_y = (position.y / units_per_pixel).floor() as i32;
    let chunk_span = i32::from(CHUNK_PIXEL_SIZE);

    ChunkPixelPosition {
        key: ChunkKey {
            x: world_pixel_x.div_euclid(chunk_span),
            y: world_pixel_y.div_euclid(chunk_span),
            z: (position.z / chunk_world_size).floor() as i32,
        },
        local: ChunkLocalPixel {
            x: world_pixel_x.rem_euclid(chunk_span) as u16,
            y: world_pixel_y.rem_euclid(chunk_span) as u16,
        },
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
pub enum SurfaceTraversal {
    Walk,
    Swim,
    Blocked,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Encode, Decode)]
pub enum WorldPixel {
    Empty = 0,
    Grass = 1,
    Dirt = 2,
    Rock = 3,
    Water = 4,
    Blood = 5,
}

impl WorldPixel {
    pub fn traversal(self) -> SurfaceTraversal {
        match self {
            Self::Rock => SurfaceTraversal::Blocked,
            Self::Water => SurfaceTraversal::Swim,
            Self::Empty | Self::Grass | Self::Dirt | Self::Blood => SurfaceTraversal::Walk,
        }
    }

    pub fn blocks_movement(self) -> bool {
        self.traversal() == SurfaceTraversal::Blocked
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
    pub fn base_pixel(self) -> WorldPixel {
        match self {
            Self::Dark => WorldPixel::Dirt,
            Self::GrassPlane => WorldPixel::Grass,
            Self::Cave => WorldPixel::Rock,
            Self::Ocean => WorldPixel::Water,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Encode, Decode)]
pub struct PixelDelta {
    pub local_index: u16,
    pub pixel: WorldPixel,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
pub struct ChunkDelta {
    pub removed_object_ids: Vec<WorldObjectId>,
    pub pixel_overrides: Vec<PixelDelta>,
}

impl ChunkDelta {
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

    pub fn pixel_override(&self, local_index: u16) -> Option<WorldPixel> {
        self.pixel_overrides
            .binary_search_by_key(&local_index, |delta| delta.local_index)
            .ok()
            .map(|index| self.pixel_overrides[index].pixel)
    }

    pub fn set_pixel_override(&mut self, local_index: u16, pixel: WorldPixel) {
        match self
            .pixel_overrides
            .binary_search_by_key(&local_index, |delta| delta.local_index)
        {
            Ok(index) => self.pixel_overrides[index].pixel = pixel,
            Err(index) => self.pixel_overrides.insert(index, PixelDelta { local_index, pixel }),
        }
    }

    pub fn clear_pixel_override(&mut self, local_index: u16) {
        if let Ok(index) = self
            .pixel_overrides
            .binary_search_by_key(&local_index, |delta| delta.local_index)
        {
            self.pixel_overrides.remove(index);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct ChunkData {
    pub key: ChunkKey,
    pub theme: ChunkTheme,
    pub materials: Box<[WorldPixel]>,
    pub assets: Vec<ProcAsset>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct ChunkState {
    pub data: ChunkData,
    pub delta: ChunkDelta,
}

impl ChunkData {
    pub fn new(key: ChunkKey, theme: ChunkTheme, materials: Vec<WorldPixel>, assets: Vec<ProcAsset>) -> Self {
        assert_eq!(materials.len(), CHUNK_PIXEL_COUNT);
        Self {
            key,
            theme,
            materials: materials.into_boxed_slice(),
            assets,
        }
    }

    pub fn filled(key: ChunkKey, theme: ChunkTheme, pixel: WorldPixel, assets: Vec<ProcAsset>) -> Self {
        Self::new(key, theme, vec![pixel; CHUNK_PIXEL_COUNT], assets)
    }

    pub fn base_pixel(&self, local: ChunkLocalPixel) -> WorldPixel {
        self.materials[usize::from(local.as_index())]
    }

    pub fn base_pixel_by_index(&self, local_index: u16) -> WorldPixel {
        self.materials[usize::from(local_index)]
    }
}

impl ChunkState {
    pub fn new(data: ChunkData) -> Self {
        Self {
            data,
            delta: ChunkDelta::default(),
        }
    }

    pub fn visible_assets(&self) -> impl Iterator<Item = &ProcAsset> {
        self.data
            .assets
            .iter()
            .filter(|asset| !self.delta.is_removed(asset.id))
    }

    pub fn pixel(&self, local: ChunkLocalPixel) -> WorldPixel {
        let local_index = local.as_index();
        self.delta
            .pixel_override(local_index)
            .unwrap_or_else(|| self.data.base_pixel_by_index(local_index))
    }

    pub fn pixel_by_index(&self, local_index: u16) -> WorldPixel {
        self.delta
            .pixel_override(local_index)
            .unwrap_or_else(|| self.data.base_pixel_by_index(local_index))
    }

    pub fn set_pixel(&mut self, local: ChunkLocalPixel, pixel: WorldPixel) {
        self.set_pixel_by_index(local.as_index(), pixel);
    }

    pub fn set_pixel_by_index(&mut self, local_index: u16, pixel: WorldPixel) {
        let base_pixel = self.data.base_pixel_by_index(local_index);
        if pixel == base_pixel {
            self.delta.clear_pixel_override(local_index);
        } else {
            self.delta.set_pixel_override(local_index, pixel);
        }
    }

    pub fn pixel_from_world_position(
        &self,
        world_position: Vec3,
        chunk_world_size: f32,
    ) -> Option<WorldPixel> {
        let pixel_position = chunk_pixel_from_world_position(world_position, chunk_world_size);
        if pixel_position.key != self.data.key {
            return None;
        }

        Some(self.pixel(pixel_position.local))
    }

    pub fn remove_object(&mut self, object_id: WorldObjectId) {
        self.delta.remove_object(object_id);
    }
}

#[derive(Debug, Default, Clone)]
pub struct WorldStore {
    chunks: BTreeMap<ChunkKey, ChunkState>,
}

impl WorldStore {
    pub fn insert(&mut self, state: ChunkState) {
        self.chunks.insert(state.data.key, state);
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
