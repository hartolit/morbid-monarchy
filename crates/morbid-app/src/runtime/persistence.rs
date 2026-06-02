use bevy::{
    app::AppExit,
    prelude::*,
    tasks::{AsyncComputeTaskPool, IoTaskPool, Task, block_on, futures_lite::future},
};
use monarch_engine::prelude::*;
use redb::Database;
use spatial_lib::prelude::{
    math::ChunkKey,
    storage::{ChunkStorage, redb_backend::RedbChunkStorage},
};
use std::{path::PathBuf, sync::Arc};

const WORLD_DATA_DIR: &str = "world_data";
const DB_FILE: &str = "save.redb";

/// Encapsulates the thread-safe spatial-lib DB backend.
#[derive(Resource, Clone)]
pub struct WorldDatabase(pub Arc<RedbChunkStorage>);

/// An accumulator queue to batch disk writes per-frame.
#[derive(Resource, Default)]
pub struct ChunkSaveQueue {
    pub chunks: Vec<(ChunkKey, CellChunk)>,
}

#[derive(Resource)]
pub struct SaveTimer(pub Timer);

#[derive(Resource)]
pub struct WorldSeed(pub u32);

#[derive(Component)]
pub struct ChunkLoadTask(Task<ChunkLoadedEvent>);

#[derive(Component)]
pub struct ChunkSaveTask(Task<()>);

pub fn initialize_database() -> WorldDatabase {
    let dir = PathBuf::from(WORLD_DATA_DIR);
    if !dir.exists() {
        std::fs::create_dir_all(&dir).expect("Failed to create world_data directory");
    }

    let db_path = dir.join(DB_FILE);
    let db = Arc::new(Database::create(db_path).expect("Failed to initialize redb database"));

    // Abstract the physical layout generation to the spatial-lib driver
    let storage = RedbChunkStorage::new(db).expect("Failed to initialize chunk storage backend");

    WorldDatabase(Arc::new(storage))
}

pub fn handle_load_requests(
    mut commands: Commands,
    mut reader: MessageReader<ChunkLoadRequest>,
    db: Res<WorldDatabase>,
    seed: Res<WorldSeed>,
) {
    let io_pool = IoTaskPool::get();
    let compute_pool = AsyncComputeTaskPool::get();
    let world_seed = seed.0;

    for request in reader.read() {
        let key = request.key;
        let storage_clone = db.0.clone();

        let task = io_pool.spawn(async move {
            // Zero-copy decoding: bitcode operates directly on the redb mmap slice
            let cached_chunk =
                match storage_clone.read_chunk(key, |bytes| bitcode::decode::<CellChunk>(bytes)) {
                    Ok(Some(Ok(data))) => Some(data),
                    Ok(Some(Err(e))) => {
                        error!("Corruption decoding chunk {:?}: {}", key, e);
                        None
                    }
                    Ok(None) => None,
                    Err(e) => {
                        error!("Storage read error for chunk {:?}: {}", key, e);
                        None
                    }
                };

            let data = if let Some(chunk_data) = cached_chunk {
                chunk_data
            } else {
                let gen_task = compute_pool.spawn(async move {
                    let generator = WorldGenerator::new(world_seed);
                    generator.generate_chunk(key)
                });
                gen_task.await
            };

            ChunkLoadedEvent { key, data }
        });

        commands.spawn(ChunkLoadTask(task));
    }
}

pub fn handle_unload_events(
    mut reader: MessageReader<ChunkUnloadEvent>,
    mut save_queue: ResMut<ChunkSaveQueue>,
) {
    for event in reader.read() {
        save_queue.chunks.push((event.key, event.data.clone()));
    }
}

pub fn process_save_queue(
    mut commands: Commands,
    time: Res<Time>,
    mut timer: ResMut<SaveTimer>,
    mut save_queue: ResMut<ChunkSaveQueue>,
    db: Res<WorldDatabase>,
) {
    if save_queue.chunks.is_empty() {
        return;
    }

    timer.0.tick(time.delta());

    if timer.0.is_finished() || save_queue.chunks.len() >= 50 {
        let chunks_to_save = std::mem::take(&mut save_queue.chunks);
        let storage_clone = db.0.clone();
        let pool = IoTaskPool::get();

        let task = pool.spawn(async move {
            let encoded_payloads: Vec<(ChunkKey, Vec<u8>)> = chunks_to_save
                .into_iter()
                .map(|(k, data)| (k, bitcode::encode(&data)))
                .collect();

            let batch_refs: Vec<(ChunkKey, &[u8])> = encoded_payloads
                .iter()
                .map(|(k, bytes)| (*k, bytes.as_slice()))
                .collect();

            if let Err(e) = storage_clone.write_batch(&batch_refs) {
                error!("Failed to execute batch chunk save: {}", e);
            }
        });

        commands.spawn(ChunkSaveTask(task));
    }
}

pub fn emergency_flush_on_exit(
    mut exit_events: MessageReader<AppExit>,
    mut queue: ResMut<ChunkSaveQueue>,
    db: Res<WorldDatabase>,
) {
    let mut exiting = false;
    for _ in exit_events.read() {
        exiting = true;
    }

    if !exiting || queue.chunks.is_empty() {
        return;
    }

    info!(
        "AppExit detected! Synchronously flushing {} pending chunks...",
        queue.chunks.len()
    );

    let chunks_to_save = std::mem::take(&mut queue.chunks);
    let encoded_payloads: Vec<(ChunkKey, Vec<u8>)> = chunks_to_save
        .into_iter()
        .map(|(k, data)| (k, bitcode::encode(&data)))
        .collect();

    let batch_refs: Vec<(ChunkKey, &[u8])> = encoded_payloads
        .iter()
        .map(|(k, bytes)| (*k, bytes.as_slice()))
        .collect();

    if let Err(e) = db.0.write_batch(&batch_refs) {
        error!("CRITICAL: Failed to flush chunks on shutdown: {}", e);
    } else {
        info!("Emergency flush complete. Safe to terminate.");
    }
}

pub fn poll_load_tasks(
    mut commands: Commands,
    mut task_query: Query<(Entity, &mut ChunkLoadTask)>,
    mut writer: MessageWriter<ChunkLoadedEvent>,
) {
    for (entity, mut task) in &mut task_query {
        if let Some(loaded_event) = block_on(future::poll_once(&mut task.0)) {
            writer.write(loaded_event);
            commands.entity(entity).despawn();
        }
    }
}

pub fn poll_save_tasks(
    mut commands: Commands,
    mut task_query: Query<(Entity, &mut ChunkSaveTask)>,
) {
    for (entity, mut task) in &mut task_query {
        if block_on(future::poll_once(&mut task.0)).is_some() {
            commands.entity(entity).despawn();
        }
    }
}
