use bevy::{
    ecs::{
        message::{MessageReader, MessageWriter},
        resource::Resource,
        system::{Res, ResMut},
    },
    math::DVec3,
    time::Time,
};

use spatial_lib::{manager::ChunkManager as SpatialManager, math::ChunkKey, store::ChunkStore};

use crate::engine::{
    events::{ChunkLoadRequest, ChunkLoadedEvent, ChunkUnloadEvent, ResizeSimulationEvent},
    world::{
        chunk::{CHUNK_SIZE, ChunkMetadata},
        grid::ActiveWorldGrid,
    },
};

pub mod cell;
pub mod chunk;
pub mod grid;

pub const DEFAULT_ACTIVE_RADIUS_X: u32 = 6;
pub const DEFAULT_ACTIVE_RADIUS_Y: u32 = 6;
pub const PRELOAD_EXT_RADIUS: u32 = 4;
pub const PRELOAD_TRIGGER: u32 = 2;

pub const CACHE_CHUNK_SIZE: usize = (((DEFAULT_ACTIVE_RADIUS_X * 2)
    * (DEFAULT_ACTIVE_RADIUS_Y * 2))
    * (PRELOAD_EXT_RADIUS * 2)) as usize;

#[derive(Resource)]
pub struct WorldStore {
    pub inner: ChunkStore<crate::engine::world::cell::WorldCell, ChunkMetadata>,
}

impl Default for WorldStore {
    fn default() -> Self {
        Self {
            inner: ChunkStore::new(CACHE_CHUNK_SIZE),
        }
    }
}

#[derive(Resource)]
pub struct WorldManager {
    pub inner: SpatialManager,
}

impl Default for WorldManager {
    fn default() -> Self {
        Self {
            inner: SpatialManager::new(
                DEFAULT_ACTIVE_RADIUS_X,
                DEFAULT_ACTIVE_RADIUS_Y,
                PRELOAD_EXT_RADIUS,
                PRELOAD_TRIGGER,
                CHUNK_SIZE,
            ),
        }
    }
}

#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct WorldFocus {
    pub position: DVec3,
}

pub fn handle_simulation_resize(
    focus: Res<WorldFocus>,
    mut reader: MessageReader<ResizeSimulationEvent>,
    mut manager: ResMut<WorldManager>,
    mut grid: ResMut<ActiveWorldGrid>,
    mut store: ResMut<WorldStore>,
    mut unload_writer: MessageWriter<ChunkUnloadEvent>,
    mut load_writer: MessageWriter<ChunkLoadRequest>,
) {
    for event in reader.read() {
        let center = ChunkKey::from_world(focus.position, CHUNK_SIZE as f64);
        let window_events = manager.inner.handle_resize(
            center,
            event.new_active_radius_x,
            event.new_active_radius_y,
            &mut store.inner,
            &mut grid.spatial,
        );

        grid.resize_buffers();

        for key in window_events.loads_requested {
            load_writer.write(ChunkLoadRequest { key });
        }
        for (key, data) in window_events.unloads_emitted {
            unload_writer.write(ChunkUnloadEvent { key, data });
        }
    }
}

pub fn manage_chunk_window(
    focus: Res<WorldFocus>,
    mut manager: ResMut<WorldManager>,
    mut grid: ResMut<ActiveWorldGrid>,
    mut store: ResMut<WorldStore>,
    mut unload_writer: MessageWriter<ChunkUnloadEvent>,
    mut load_writer: MessageWriter<ChunkLoadRequest>,
) {
    let center = ChunkKey::from_world(focus.position, CHUNK_SIZE as f64);
    let window_events = manager
        .inner
        .manage_window(center, &mut store.inner, &mut grid.spatial);

    for key in window_events.loads_requested {
        load_writer.write(ChunkLoadRequest { key });
    }
    for (key, data) in window_events.unloads_emitted {
        unload_writer.write(ChunkUnloadEvent { key, data });
    }
}

pub fn handle_chunk_loaded(
    time: Res<Time>,
    mut reader: MessageReader<ChunkLoadedEvent>,
    mut manager: ResMut<WorldManager>,
    mut store: ResMut<WorldStore>,
    mut grid: ResMut<ActiveWorldGrid>,
    mut unload_writer: MessageWriter<ChunkUnloadEvent>,
) {
    for event in reader.read() {
        let mut chunk_data = event.data.clone();
        chunk_data.metadata.last_simulated = time.elapsed_secs_f64();

        if let Some((evicted_key, evicted_data)) = manager.inner.handle_chunk_loaded(
            event.key,
            chunk_data,
            &mut store.inner,
            &mut grid.spatial,
        ) {
            unload_writer.write(ChunkUnloadEvent {
                key: evicted_key,
                data: evicted_data,
            });
        }
    }
}
