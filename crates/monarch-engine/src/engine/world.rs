use std::num::NonZeroUsize;

use bevy::{
    ecs::{
        message::{MessageReader, MessageWriter},
        resource::Resource,
        system::{Res, ResMut},
    },
    math::DVec3,
    time::Time,
};
use lru::LruCache;
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};

use crate::engine::{
    events::{ChunkLoadRequest, ChunkLoadedEvent, ChunkUnloadEvent, ResizeSimulationEvent},
    world::{
        cell::WorldCell,
        chunk::{CHUNK_CELL_COUNT, CHUNK_SIZE, ChunkData, ChunkKey, ChunkView},
        grid::ActiveWorldGrid,
    },
};

pub mod cell;
pub mod chunk;
pub mod grid;

pub const DEFAULT_ACTIVE_RADIUS_X: u32 = 7; // Active simulation view
pub const DEFAULT_ACTIVE_RADIUS_Y: u32 = 7; // Active simulation view
pub const PRELOAD_EXT_RADIUS: u32 = 3; // Fetch boundary (outer)
pub const PRELOAD_TRIGGER: u32 = 2; // Trigger fetch if active view gets chunks away from outer edge
pub const CACHE_CHUNK_SIZE: usize = (((DEFAULT_ACTIVE_RADIUS_X * 2)
    * (DEFAULT_ACTIVE_RADIUS_Y * 2))
    * (PRELOAD_EXT_RADIUS * 2)) as usize;

/// Engine-side storage for lightweight metadata of active chunks.
#[derive(Resource)]
pub struct WorldStore {
    pub active_chunks: FxHashMap<ChunkKey, ChunkData>,
    pub cached_chunks: LruCache<ChunkKey, ChunkData, FxBuildHasher>,
    pub pending_requests: FxHashSet<ChunkKey>,
}

impl Default for WorldStore {
    fn default() -> Self {
        Self {
            active_chunks: FxHashMap::default(),
            cached_chunks: LruCache::with_hasher(
                NonZeroUsize::new(CACHE_CHUNK_SIZE).unwrap(),
                FxBuildHasher,
            ), // TODO: Change
            pending_requests: FxHashSet::default(),
        }
    }
}

#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct WorldFocus {
    pub position: DVec3,
}

#[derive(Resource)]
pub struct ChunkManager {
    pub active_view: Option<ChunkView>,
    pub preload_view: Option<ChunkView>,
    /// The radius of the simulation area (e.g., 1 = 3x3 chunks)
    pub active_radius_x: u32,
    pub active_radius_y: u32,
    /// Extends the preload view beyond the active view by this many chunks.
    pub preload_ext_radius: u32,
    /// If the active view gets within this many chunks of the
    /// preload view's boundary, it triggers a recenter and batch fetch.
    pub preload_trigger: u32,
}

impl Default for ChunkManager {
    fn default() -> Self {
        Self {
            active_view: None,
            active_radius_x: DEFAULT_ACTIVE_RADIUS_X,
            active_radius_y: DEFAULT_ACTIVE_RADIUS_Y,
            preload_view: None,
            preload_ext_radius: PRELOAD_EXT_RADIUS,
            preload_trigger: PRELOAD_TRIGGER,
        }
    }
}

pub fn handle_simulation_resize(
    focus: Res<WorldFocus>,
    time: Res<Time>,
    mut reader: MessageReader<ResizeSimulationEvent>,
    mut manager: ResMut<ChunkManager>,
    mut grid: ResMut<ActiveWorldGrid>,
    mut store: ResMut<WorldStore>,
    mut unload_writer: MessageWriter<ChunkUnloadEvent>,
    mut load_writer: MessageWriter<ChunkLoadRequest>,
) {
    for event in reader.read() {
        let center = ChunkKey::from_dvec3(focus.position);
        let new_active_view = ChunkView::from_rect_xy(
            center,
            event.new_active_radius_x as i32,
            event.new_active_radius_y as i32,
        );

        if let Some(old_active) = manager.active_view {
            // BACKUP: Extract ALL active chunks directly into their existing ChunkData vectors.
            // This safely preserves the physics states before we shatter the grid's stride.
            for key in old_active.iter() {
                if let Some(chunk_data) = store.active_chunks.get_mut(&key) {
                    // Ensure the vec has the correct capacity, then overwrite
                    if chunk_data.cells.len() != CHUNK_CELL_COUNT {
                        chunk_data
                            .cells
                            .resize(CHUNK_CELL_COUNT, WorldCell::default());
                    }
                    grid.extract_chunk_into(key, &mut chunk_data.cells);
                    chunk_data.last_simulated = time.elapsed_secs_f64();
                }
            }

            // EVICT: Move demoted chunks from Active to LRU Cache
            for key in old_active.iter().filter(|k| !new_active_view.contains(k)) {
                // remove() takes ownership of the ChunkData out of the active map
                if let Some(chunk_data) = store.active_chunks.remove(&key) {
                    // Push to cache, handling potential overflow evictions to disk
                    if let Some((evicted_key, evicted_data)) =
                        store.cached_chunks.push(key, chunk_data)
                    {
                        unload_writer.write(ChunkUnloadEvent {
                            key: evicted_key,
                            data: evicted_data,
                        });
                    }
                }
            }
        }

        // MUTATE STRIDE: Resize the grid in-place
        let span_chunks_x = (event.new_active_radius_x * 2 + 1) as i32;
        let span_chunks_y = (event.new_active_radius_y * 2 + 1) as i32;

        let new_width = span_chunks_x * (CHUNK_SIZE as i32);
        let new_height = span_chunks_y * (CHUNK_SIZE as i32);
        let new_origin = new_active_view.min.to_ivec2() * (CHUNK_SIZE as i32);

        grid.resize_in_place(new_width, new_height, new_origin);

        // RESTORE + PROMOTE: Repopulate the new grid from every available source
        for key in new_active_view.iter() {
            if store.active_chunks.contains_key(&key) {
                // Already retained (was in both old and new view, never evicted)
                let cells = store.active_chunks[&key].cells.clone();
                grid.load_chunk(key, &cells);
            } else if let Some(chunk_data) = store.cached_chunks.pop(&key) {
                // Was evicted to cache during this resize or previously
                grid.load_chunk(key, &chunk_data.cells);
                store.active_chunks.insert(key, chunk_data);
            } else if store.pending_requests.insert(key) {
                // Cold miss — request from disk/generator
                load_writer.write(ChunkLoadRequest { key });
            }
        }

        // Update view
        manager.active_radius_x = event.new_active_radius_x;
        manager.active_radius_y = event.new_active_radius_y;

        // View is fully populated — set it so manage_chunk_window knows the current state
        manager.active_view = Some(new_active_view);
        manager.preload_view = None;
    }
}

pub fn manage_chunk_window(
    focus: Res<WorldFocus>,
    time: Res<Time>,
    mut manager: ResMut<ChunkManager>,
    mut grid: ResMut<ActiveWorldGrid>,
    mut store: ResMut<WorldStore>,
    mut unload_writer: MessageWriter<ChunkUnloadEvent>,
    mut load_writer: MessageWriter<ChunkLoadRequest>,
) {
    let center = ChunkKey::from_dvec3(focus.position);
    let new_active_view = ChunkView::from_rect_xy(
        center,
        manager.active_radius_x as i32,
        manager.active_radius_y as i32,
    );

    // Handle Active Simulation View (Promotions & Demotions)
    if manager.active_view != Some(new_active_view) {
        if let Some(old_active) = manager.active_view {
            // Evict chunks that fell out of the active simulation view
            for key in old_active.iter().filter(|k| !new_active_view.contains(k)) {
                if let Some(mut chunk_data) = store.active_chunks.remove(&key) {
                    // ZERO-ALLOCATION EXTRACTION
                    if chunk_data.cells.len() != CHUNK_CELL_COUNT {
                        chunk_data
                            .cells
                            .resize(CHUNK_CELL_COUNT, WorldCell::default());
                    }
                    grid.extract_chunk_into(key, &mut chunk_data.cells);
                    chunk_data.last_simulated = time.elapsed_secs_f64();

                    // Push demoted chunk into the LRU Cache.
                    if let Some((evicted_key, evicted_data)) =
                        store.cached_chunks.push(key, chunk_data)
                    {
                        unload_writer.write(ChunkUnloadEvent {
                            key: evicted_key,
                            data: evicted_data,
                        });
                    }
                }
            }
        }

        // Shift the Toroidal Grid's window anchor
        grid.shift_window(new_active_view.min.to_ivec2() * (CHUNK_SIZE as i32));

        // Promote chunks entering the active simulation view
        let old_active_ref = manager.active_view.as_ref();
        for key in new_active_view
            .iter()
            .filter(|k| old_active_ref.map_or(true, |old| !old.contains(k)))
        {
            if let Some(chunk_data) = store.cached_chunks.pop(&key) {
                grid.load_chunk(key, &chunk_data.cells);
                store.active_chunks.insert(key, chunk_data);
            } else if !store.active_chunks.contains_key(&key) && store.pending_requests.insert(key)
            {
                load_writer.write(ChunkLoadRequest { key });
            }
        }
        manager.active_view = Some(new_active_view);
    }

    // Trigger Check for Disk I/O Batching
    let mut requires_preload_fetch = false;

    if let Some(current_preload) = manager.preload_view {
        // Create a "danger zone" view around the active view
        let danger_view = ChunkView::from_rect_xy(
            center,
            (manager.active_radius_x + manager.preload_trigger) as i32,
            (manager.active_radius_y + manager.preload_trigger) as i32,
        );

        // If our danger zone bleeds outside the preloaded chunks, we must fetch
        if !current_preload.contains(&danger_view.min)
            || !current_preload.contains(&danger_view.max)
        {
            requires_preload_fetch = true;
        }
    } else {
        requires_preload_fetch = true; // First time setup
    }

    // Batch Fetching from Disk (Only happens when trigger is hit)
    if requires_preload_fetch {
        let new_preload_view = ChunkView::from_rect_xy(
            center,
            (manager.active_radius_x + manager.preload_ext_radius) as i32,
            (manager.active_radius_y + manager.preload_ext_radius) as i32,
        );

        let old_preload_ref = manager.preload_view.as_ref();

        for key in new_preload_view
            .iter()
            .filter(|k| old_preload_ref.map_or(true, |old| !old.contains(k)))
        {
            // If it's totally cold, ask morbid-app to spin up the disk
            if !store.active_chunks.contains_key(&key)
                && !store.cached_chunks.contains(&key)
                && store.pending_requests.insert(key)
            {
                load_writer.write(ChunkLoadRequest { key });
            }
        }

        manager.preload_view = Some(new_preload_view);
    }
}

pub fn handle_chunk_loaded(
    time: Res<Time>,
    mut reader: MessageReader<ChunkLoadedEvent>,
    manager: Res<ChunkManager>,
    mut store: ResMut<WorldStore>,
    mut grid: ResMut<ActiveWorldGrid>,
    mut unload_writer: MessageWriter<ChunkUnloadEvent>,
) {
    for event in reader.read() {
        store.pending_requests.remove(&event.key);

        let mut chunk_data = event.data.clone();
        let current_time = time.elapsed_secs_f64();
        let delta_secs = current_time - event.data.last_simulated;

        if delta_secs > 1.0 {
            fast_forward_chunk(&mut chunk_data, delta_secs);
        }

        chunk_data.last_simulated = current_time;

        let is_active = manager
            .active_view
            .map_or(false, |v| v.contains(&event.key));

        if is_active {
            grid.load_chunk(event.key, &chunk_data.cells);
            store.active_chunks.insert(event.key, chunk_data);
        } else {
            // The chunk arrived, but the player already walked away. Stow it in Cache.
            if let Some((evicted_key, evicted_data)) =
                store.cached_chunks.push(event.key, chunk_data)
            {
                unload_writer.write(ChunkUnloadEvent {
                    key: evicted_key,
                    data: evicted_data,
                });
            }
        }
    }
}

/// Dummy implementation for fast-forwarding chunk physics (e.g., settling water/sand)
/// Fast-forwards chunk physics (e.g., settling water/blood) to catch up with missed time.
/// Note: This is a single pass calculation to catch up with missed time and not a 1:1 simulation.
fn fast_forward_chunk(_chunk_data: &mut ChunkData, _delta_secs: f64) {
    //todo!("Implement fast-forward physics")
}
