use bevy::{
    app::AppExit,
    prelude::*,
    tasks::{AsyncComputeTaskPool, IoTaskPool, Task, block_on, futures_lite::future},
};
use monarch_engine::prelude::*;
use redb::{Database, ReadableDatabase, TableDefinition};
use std::{path::PathBuf, sync::Arc};

const WORLD_DATA_DIR: &str = "world_data";
const DB_FILE: &str = "save.redb";

const CHUNKS_TABLE: TableDefinition<[i32; 3], &[u8]> = TableDefinition::new("chunks");

#[derive(Resource, Clone)]
pub struct WorldDatabase(pub Arc<Database>);

/// An accumulator queue so we can batch disk writes per-frame.
#[derive(Resource, Default)]
pub struct ChunkSaveQueue {
    pub chunks: Vec<(ChunkKey, ChunkData)>,
}

/// A timer to flush the save queue to disk periodically.
#[derive(Resource)]
pub struct SaveTimer(pub Timer);

/// Stores the active simulation seed to replace hardcoded values.
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
    let db = Database::create(db_path).expect("Failed to initialize redb database");
    ensure_chunks_table(&db).expect("Failed to initialize chunks table");

    WorldDatabase(Arc::new(db))
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
        let db_clone = db.0.clone();

        let task = io_pool.spawn(async move {
            // Read the DB and let the non-Send error drop immediately.
            let cached_chunk = match load_chunk_from_db(&db_clone, key) {
                Ok(Some(chunk_data)) => Some(chunk_data),
                Ok(None) => None,
                Err(e) => {
                    error!(
                        "Database read error for chunk {:?}: {}. Generating chunk.",
                        key, e
                    );
                    None
                }
            };

            let data = if let Some(chunk_data) = cached_chunk {
                chunk_data
            } else {
                // Disk miss - Offload heavy CPU math to the Compute Pool!
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

    // Flush if timer finished OR if queue is getting too large (prevent RAM spikes)
    if timer.0.is_finished() || save_queue.chunks.len() >= 50 {
        let chunks_to_save = std::mem::take(&mut save_queue.chunks);
        let db_clone = db.0.clone();
        let pool = IoTaskPool::get();

        let task = pool.spawn(async move {
            if let Err(e) = save_chunks_to_db_batch(&db_clone, &chunks_to_save) {
                error!("Failed to execute batch chunk save: {}", e);
            }
        });

        commands.spawn(ChunkSaveTask(task));
    }
}

/// Intercepts application shutdown and forces a synchronous, blocking write
/// to guarantee no chunks are lost in the ether when the user quits.
pub fn emergency_flush_on_exit(
    mut exit_events: MessageReader<AppExit>,
    mut queue: ResMut<ChunkSaveQueue>,
    db: Res<WorldDatabase>,
) {
    let mut exiting = false;
    for _ in exit_events.read() {
        exiting = true;
    }

    if !exiting {
        return;
    }

    if queue.chunks.is_empty() {
        info!("Save queue is empty. Shutting down cleanly.");
        return;
    }

    info!(
        "AppExit detected! Synchronously flushing {} pending chunks to disk...",
        queue.chunks.len()
    );

    let chunks_to_save = std::mem::take(&mut queue.chunks);

    if let Err(e) = save_chunks_to_db_batch(&db.0, &chunks_to_save) {
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

// --- Internal DB I/O ---

fn ensure_chunks_table(db: &Database) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let write_txn = db.begin_write()?;
    {
        let _table = write_txn.open_table(CHUNKS_TABLE)?;
    }
    write_txn.commit()?;
    Ok(())
}

fn save_chunks_to_db_batch(
    db: &Database,
    chunks: &[(ChunkKey, ChunkData)],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let write_txn = db.begin_write()?;
    {
        let mut table = write_txn.open_table(CHUNKS_TABLE)?;
        for (key, data) in chunks {
            let encoded: Vec<u8> = bitcode::encode(data);
            table.insert([key.key.x, key.key.y, key.key.z], encoded.as_slice())?;
        }
    }

    write_txn.commit()?;

    Ok(())
}

fn load_chunk_from_db(
    db: &Database,
    key: ChunkKey,
) -> Result<Option<ChunkData>, Box<dyn std::error::Error + Send + Sync>> {
    let read_txn = db.begin_read()?;
    let table = read_txn.open_table(CHUNKS_TABLE)?;

    if let Some(access) = table.get([key.key.x, key.key.y, key.key.z])? {
        let bytes = access.value();
        let data: ChunkData = bitcode::decode(bytes)?;
        Ok(Some(data))
    } else {
        Ok(None)
    }
}
