use bevy::{
    ecs::{
        message::{MessageReader, MessageWriter},
        system::{Res, ResMut},
    },
    time::Time,
};

use crate::world::{
    chunk::{CHUNK_CELL_COUNT, CHUNK_SIZE, ChunkData, ChunkKey, ChunkView},
    events::{ChunkLoadRequest, ChunkLoadedEvent, ChunkUnloadEvent},
    grid::ActiveWorldGrid,
    types::{ChunkManager, WorldCell, WorldFocus, WorldStore},
};

pub mod chunk;
pub mod events;
pub mod grid;
pub mod types;

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
    // Creates a flat top-down view centered around the player's position
    let new_view = ChunkView::from_cuboid(center, manager.view_radius as i32, 0);

    if let Some(old_view) = manager.current_view {
        if old_view == new_view {
            return;
        }

        // Filters evicted chunks and unloads them (Zero-Allocation)
        for key in old_view.iter().filter(|k| !new_view.contains(k)) {
            if let Some(mut chunk_data) = store.active_chunks.remove(&key) {
                // Extract simulated pixels data from the grid
                chunk_data.cells = grid.unload_chunk(key);

                chunk_data.last_simulated = time.elapsed_secs_f64();

                unload_writer.write(ChunkUnloadEvent {
                    key,
                    data: chunk_data,
                });
            }
        }
    }

    // Shift the Toroidal Grid's window anchor
    grid.window_origin = new_view.min.to_ivec2() * (CHUNK_SIZE as i32);

    // Requests incoming chunks (Zero-Allocation)
    let old_view_ref = manager.current_view.as_ref();
    for key in new_view
        .iter()
        .filter(|k| old_view_ref.map_or(true, |old| !old.contains(k)))
    {
        if !store.active_chunks.contains_key(&key) && store.pending_requests.insert(key) {
            load_writer.write(ChunkLoadRequest { key });
        }
    }

    manager.current_view = Some(new_view);
}

pub fn handle_chunk_loaded(
    time: Res<Time>,
    mut reader: MessageReader<ChunkLoadedEvent>,
    mut store: ResMut<WorldStore>,
    mut grid: ResMut<ActiveWorldGrid>,
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
        grid.load_chunk(event.key, &event.data.cells);
        store.active_chunks.insert(event.key, event.data.clone());
    }
}

/// Dummy implementation for fast-forwarding chunk physics (e.g., settling water/sand)
fn fast_forward_chunk(_chunk_data: &mut ChunkData, _delta_secs: f64) {
    // TODO: Calculate how many ticks `missed_seconds` represents.
    // Run simplified, large-timestep cellular automata passes to stabilize the environment
    // before the player sees it.
}
