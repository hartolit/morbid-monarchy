//! Seams for the scripted baseline driver in `monarchy-evidence` (feature `legacy-baseline`).
//!
//! The driver measures this application as it is: it composes it with [`crate::build_app`] in
//! [`crate::InputMode::Scripted`], applies tools through the same functions the mouse tools
//! call, and reads the work counters below. This module belongs to the legacy application and
//! is removed with it in prompt 06.

use bevy::{prelude::*, render::storage::ShaderBuffer};
use monarch_engine::prelude::*;

pub use crate::runtime::{
    dev_tools::{
        BrushSettings, DevOverlay, GridBrush,
        brush::{SphereSpawn, apply_brush, attract_sphere, lift_sphere, spawn_sphere},
    },
    input::{drive_sphere, jump_sphere},
    persistence::{
        ChunkLoadTask, ChunkSaveQueue, ChunkSaveTask, PersistenceCounters, PersistenceTaskCounters,
    },
    render::{GlobalWorldMaterials, RenderCounters},
};

/// Cell payload the CPU baseline keeps resident, in bytes of packed cells, plus the counts that
/// explain it. Allocation overhead and metadata are excluded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetainedPayload {
    /// Toroidal front grid (`ActiveWorldGrid::spatial`).
    pub grid_front_bytes: u64,
    /// Double-buffer copy (`ActiveWorldGrid::back_buffer`).
    pub grid_back_bytes: u64,
    /// Both wake arrays, one byte per cell each.
    pub wake_bytes: u64,
    /// Payloads of active chunks, retained alongside the grid that already holds their cells.
    pub active_chunk_bytes: u64,
    /// Payloads in the LRU cache of chunks outside the active window.
    pub cached_chunk_bytes: u64,
    /// Evicted chunks waiting for the next save batch.
    pub save_queue_bytes: u64,
    /// The main-world copy of the grid in the shared storage-buffer asset.
    pub render_asset_bytes: u64,
    pub active_chunks: u64,
    pub cached_chunks: u64,
    pub pending_loads: u64,
    pub save_queue_chunks: u64,
    pub load_tasks: u64,
    pub save_tasks: u64,
}

impl RetainedPayload {
    pub fn total_bytes(&self) -> u64 {
        self.grid_front_bytes
            + self.grid_back_bytes
            + self.wake_bytes
            + self.active_chunk_bytes
            + self.cached_chunk_bytes
            + self.save_queue_bytes
            + self.render_asset_bytes
    }
}

const CELL_BYTES: u64 = std::mem::size_of::<WorldCell>() as u64;

/// Measures [`RetainedPayload`] from the live world.
pub fn retained_payload(world: &mut World) -> RetainedPayload {
    let mut payload = RetainedPayload::default();

    if let Some(grid) = world.get_resource::<ActiveWorldGrid>() {
        payload.grid_front_bytes = grid.spatial.cells.len() as u64 * CELL_BYTES;
        payload.grid_back_bytes = grid.back_buffer.len() as u64 * CELL_BYTES;
        payload.wake_bytes = (grid.wake_buffer.len() + grid.next_wake_buffer.len()) as u64;
    }
    if let Some(store) = world.get_resource::<WorldStore>() {
        let store = &store.inner;
        payload.active_chunks = store.active.len() as u64;
        payload.cached_chunks = store.cached.len() as u64;
        payload.pending_loads = store.pending.len() as u64;
        payload.active_chunk_bytes = store
            .active
            .values()
            .map(|chunk| chunk.cells.len() as u64)
            .sum::<u64>()
            * CELL_BYTES;
        payload.cached_chunk_bytes = store
            .cached
            .iter()
            .map(|(_, chunk)| chunk.cells.len() as u64)
            .sum::<u64>()
            * CELL_BYTES;
    }
    if let Some(queue) = world.get_resource::<ChunkSaveQueue>() {
        payload.save_queue_chunks = queue.chunks.len() as u64;
        payload.save_queue_bytes = queue
            .chunks
            .iter()
            .map(|(_, chunk)| chunk.cells.len() as u64)
            .sum::<u64>()
            * CELL_BYTES;
    }
    let grid_buffer = world
        .get_resource::<GlobalWorldMaterials>()
        .map(|materials| materials.grid_buffer.clone());
    if let (Some(handle), Some(buffers)) =
        (grid_buffer, world.get_resource::<Assets<ShaderBuffer>>())
    {
        payload.render_asset_bytes = buffers
            .get(&handle)
            .and_then(|buffer| buffer.data.as_ref())
            .map_or(0, |data| data.len() as u64);
    }
    payload.load_tasks = world
        .query_filtered::<(), With<ChunkLoadTask>>()
        .iter(world)
        .count() as u64;
    payload.save_tasks = world
        .query_filtered::<(), With<ChunkSaveTask>>()
        .iter(world)
        .count() as u64;
    payload
}
