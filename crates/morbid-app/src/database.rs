use bevy::{
    prelude::*,
    tasks::{IoTaskPool, Task, block_on, futures_lite::future},
};
use monarch_engine::world::{
    chunk::{ChunkData, ChunkKey},
    events::{ChunkLoadRequest, ChunkLoadedEvent, ChunkUnloadEvent},
};
use redb::{Database, ReadableDatabase, TableDefinition};
use std::{path::PathBuf, sync::Arc};

const WORLD_DATA_DIR: &str = "world_data";
const DB_FILE: &str = "save.redb";

// Clean schema: Key is [X, Y, Z], Value is the compressed bitcode byte array
const CHUNKS_TABLE: TableDefinition<[i32; 3], &[u8]> = TableDefinition::new("chunks");

/// A cloneable wrapper around the thread-safe Database to be shared across tasks.
#[derive(Resource, Clone)]
pub struct WorldDatabase(pub Arc<Database>);

#[derive(Component)]
pub struct ChunkLoadTask(Task<ChunkLoadedEvent>);

#[derive(Component)]
pub struct ChunkSaveTask(Task<()>);

/// Bootstraps the database file and ensures the directory exists before the game starts.
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

/// Listens for requests from the engine and spins up background database reads
pub fn handle_load_requests(
    mut commands: Commands,
    mut reader: MessageReader<ChunkLoadRequest>,
    db: Res<WorldDatabase>,
) {
    let pool = IoTaskPool::get();

    for request in reader.read() {
        let key = request.key;
        let db_clone = db.0.clone();

        let task = pool.spawn(async move {
            let data = match load_chunk_from_db(&db_clone, key) {
                Ok(Some(chunk_data)) => chunk_data,
                Ok(None) => ChunkData::generate(key), // Not in DB, generate new
                Err(e) => {
                    error!(
                        "Database read error for chunk {:?}: {}. Generating chunk.",
                        key, e
                    );
                    ChunkData::generate(key)
                }
            };

            ChunkLoadedEvent { key, data }
        });

        commands.spawn(ChunkLoadTask(task));
    }
}

/// Listens for unloads from the engine and spins up background database writes
pub fn handle_unload_events(
    mut commands: Commands,
    mut reader: MessageReader<ChunkUnloadEvent>,
    db: Res<WorldDatabase>,
) {
    let pool = IoTaskPool::get();

    for event in reader.read() {
        let data = event.data.clone();
        let key = event.key;
        let db_clone = db.0.clone();

        let task = pool.spawn(async move {
            if let Err(e) = save_chunk_to_db(&db_clone, key, &data) {
                error!("Failed to save chunk {:?} to database: {}", key, e);
            }
        });

        commands.spawn(ChunkSaveTask(task));
    }
}

/// Polls active load tasks and funnels the data back into the engine via MessageWriter.
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

/// Cleans up finished save tasks so we don't leak entities.
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

// --- Internal DB I/O & Generation ---

fn ensure_chunks_table(db: &Database) -> Result<(), Box<dyn std::error::Error>> {
    let write_txn = db.begin_write()?;
    {
        let _table = write_txn.open_table(CHUNKS_TABLE)?;
    }
    write_txn.commit()?;

    Ok(())
}

fn save_chunk_to_db(
    db: &Database,
    key: ChunkKey,
    data: &ChunkData,
) -> Result<(), Box<dyn std::error::Error>> {
    let encoded: Vec<u8> = bitcode::encode(data);

    // Atomic write transaction
    let write_txn = db.begin_write()?;
    {
        let mut table = write_txn.open_table(CHUNKS_TABLE)?;
        table.insert([key.key.x, key.key.y, key.key.z], encoded.as_slice())?;
    }

    // Commit to disk safely (ACID guaranteed)
    write_txn.commit()?;

    Ok(())
}

fn load_chunk_from_db(
    db: &Database,
    key: ChunkKey,
) -> Result<Option<ChunkData>, Box<dyn std::error::Error>> {
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
