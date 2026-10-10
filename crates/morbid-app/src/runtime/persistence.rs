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
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

/// Directory the interactive application uses, relative to the working directory.
pub const WORLD_DATA_DIR: &str = "world_data";
const DB_FILE: &str = "save.redb";

// Batch threshold size for flushing chunks to disk.
const SAVE_BATCH_SIZE_THRESHOLD: usize = 64;

#[derive(Resource, Clone)]
pub struct WorldDatabase(pub Arc<RedbChunkStorage>);

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

/// Load and save work, accumulated for baseline measurement. Counting never alters which
/// chunks are loaded, generated or saved.
#[derive(Resource, Default)]
pub struct PersistenceCounters {
    pub load_requests: u64,
    pub loads_completed: u64,
    pub unload_events: u64,
    pub save_batches: u64,
    pub save_tasks_completed: u64,
    pub exit_flush_chunks: u64,
    /// Outcomes recorded inside I/O and generation tasks.
    pub tasks: Arc<PersistenceTaskCounters>,
}

/// Counters updated from worker tasks. Each is an independent statistic, so relaxed ordering
/// is sufficient: no other memory is published through them.
#[derive(Default)]
pub struct PersistenceTaskCounters {
    pub loads_from_store: AtomicU64,
    pub loads_generated: AtomicU64,
    pub read_errors: AtomicU64,
    pub decode_errors: AtomicU64,
    pub chunks_written: AtomicU64,
    pub bytes_encoded: AtomicU64,
    pub write_failures: AtomicU64,
}

/// Opens (or creates) `dir/save.redb`.
pub fn initialize_database(dir: &Path) -> WorldDatabase {
    if !dir.exists() {
        std::fs::create_dir_all(dir).expect("Failed to create world_data directory");
    }

    let db_path = dir.join(DB_FILE);
    let db = Arc::new(Database::create(db_path).expect("Failed to initialize redb database"));

    let storage = RedbChunkStorage::new(db).expect("Failed to initialize chunk storage backend");

    WorldDatabase(Arc::new(storage))
}

pub fn handle_load_requests(
    mut commands: Commands,
    mut reader: MessageReader<ChunkLoadRequest>,
    db: Res<WorldDatabase>,
    seed: Res<WorldSeed>,
    mut counters: ResMut<PersistenceCounters>,
) {
    let io_pool = IoTaskPool::get();
    let compute_pool = AsyncComputeTaskPool::get();
    let world_seed = seed.0;

    for request in reader.read() {
        let key = request.key;
        let storage_clone = db.0.clone();
        let task_counters = counters.tasks.clone();
        counters.load_requests += 1;

        let task = io_pool.spawn(async move {
            let cached_chunk =
                match storage_clone.read_chunk(key, |bytes| bitcode::decode::<CellChunk>(bytes)) {
                    Ok(Some(Ok(data))) => Some(data),
                    Ok(Some(Err(e))) => {
                        task_counters.decode_errors.fetch_add(1, Ordering::Relaxed);
                        error!("Corruption decoding chunk {:?}: {}", key, e);
                        None
                    }
                    Ok(None) => None,
                    Err(e) => {
                        task_counters.read_errors.fetch_add(1, Ordering::Relaxed);
                        error!("Storage read error for chunk {:?}: {}", key, e);
                        None
                    }
                };

            let data = if let Some(chunk_data) = cached_chunk {
                task_counters
                    .loads_from_store
                    .fetch_add(1, Ordering::Relaxed);
                chunk_data
            } else {
                task_counters
                    .loads_generated
                    .fetch_add(1, Ordering::Relaxed);
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
    mut counters: ResMut<PersistenceCounters>,
) {
    for event in reader.read() {
        counters.unload_events += 1;
        save_queue.chunks.push((event.key, event.data.clone()));
    }
}

pub fn process_save_queue(
    mut commands: Commands,
    time: Res<Time>,
    mut timer: ResMut<SaveTimer>,
    mut save_queue: ResMut<ChunkSaveQueue>,
    db: Res<WorldDatabase>,
    mut counters: ResMut<PersistenceCounters>,
) {
    if save_queue.chunks.is_empty() {
        return;
    }

    timer.0.tick(time.delta());

    if timer.0.is_finished() || save_queue.chunks.len() >= SAVE_BATCH_SIZE_THRESHOLD {
        let chunks_to_save = std::mem::take(&mut save_queue.chunks);
        let storage_clone = db.0.clone();
        let pool = IoTaskPool::get();
        let task_counters = counters.tasks.clone();
        counters.save_batches += 1;

        let task = pool.spawn(async move {
            let encoded_payloads: Vec<_> = chunks_to_save
                .into_iter()
                .map(|(k, data)| (k, bitcode::encode(&data)))
                .collect();

            let batch_refs: Vec<_> = encoded_payloads
                .iter()
                .map(|(k, bytes)| (*k, bytes.as_slice()))
                .collect();
            let encoded_bytes: usize = batch_refs.iter().map(|(_, bytes)| bytes.len()).sum();
            task_counters
                .bytes_encoded
                .fetch_add(encoded_bytes as u64, Ordering::Relaxed);

            if let Err(e) = storage_clone.write_batch(&batch_refs) {
                task_counters.write_failures.fetch_add(1, Ordering::Relaxed);
                error!("Failed to execute batch chunk save: {}", e);
            } else {
                task_counters
                    .chunks_written
                    .fetch_add(batch_refs.len() as u64, Ordering::Relaxed);
            }
        });

        commands.spawn(ChunkSaveTask(task));
    }
}

pub fn emergency_flush_on_exit(
    mut exit_events: MessageReader<AppExit>,
    mut queue: ResMut<ChunkSaveQueue>,
    db: Res<WorldDatabase>,
    mut counters: ResMut<PersistenceCounters>,
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
    counters.exit_flush_chunks += chunks_to_save.len() as u64;

    let encoded_payloads: Vec<_> = chunks_to_save
        .into_iter()
        .map(|(k, data)| (k, bitcode::encode(&data)))
        .collect();

    let batch_refs: Vec<_> = encoded_payloads
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
    mut counters: ResMut<PersistenceCounters>,
) {
    for (entity, mut task) in &mut task_query {
        if let Some(loaded_event) = block_on(future::poll_once(&mut task.0)) {
            counters.loads_completed += 1;
            writer.write(loaded_event);
            commands.entity(entity).despawn();
        }
    }
}

pub fn poll_save_tasks(
    mut commands: Commands,
    mut task_query: Query<(Entity, &mut ChunkSaveTask)>,
    mut counters: ResMut<PersistenceCounters>,
) {
    for (entity, mut task) in &mut task_query {
        if block_on(future::poll_once(&mut task.0)).is_some() {
            counters.save_tasks_completed += 1;
            commands.entity(entity).despawn();
        }
    }
}
