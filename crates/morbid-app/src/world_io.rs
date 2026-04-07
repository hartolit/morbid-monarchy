use bevy::{
    prelude::*,
    tasks::{IoTaskPool, Task, block_on, futures_lite::future},
};
use monarch_engine::world::{
    chunk::{CHUNK_CELL_COUNT, ChunkData, ChunkKey, ChunkTheme},
    events::{ChunkLoadRequest, ChunkLoadedEvent, ChunkUnloadEvent},
    types::{MaterialId, Pixel, PixelFlags, WorldCell},
};
use std::{
    io::{Read, Write},
    path::PathBuf,
};

const WORLD_DATA_DIR: &str = "world_data";

#[derive(Component)]
pub struct ChunkLoadTask(Task<ChunkLoadedEvent>);

#[derive(Component)]
pub struct ChunkSaveTask(Task<()>);

/// Listens for requests from the engine and spins up background tasks to load or generate them.
pub fn handle_load_requests(mut commands: Commands, mut reader: MessageReader<ChunkLoadRequest>) {
    let pool = IoTaskPool::get();

    for request in reader.read() {
        let key = request.key;

        let task = pool.spawn(async move {
            let path = get_chunk_path(key);

            let data = if path.exists() {
                load_chunk_from_disk(&path).unwrap_or_else(|_| generate_fallback_chunk())
            } else {
                generate_fallback_chunk()
            };

            ChunkLoadedEvent { key, data }
        });

        commands.spawn(ChunkLoadTask(task));
    }
}

/// Listens for unloads from the engine and spins up background tasks to persist them to disk.
pub fn handle_unload_events(mut commands: Commands, mut reader: MessageReader<ChunkUnloadEvent>) {
    let pool = IoTaskPool::get();

    for event in reader.read() {
        // Clone the payload so it can be moved into the async block
        let data = event.data.clone();
        let key = event.key;

        let task = pool.spawn(async move {
            let path = get_chunk_path(key);
            if let Err(e) = save_chunk_to_disk(&path, &data) {
                error!("Failed to save chunk {:?}: {}", key, e);
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

// --- Internal File I/O & Generation ---

fn get_chunk_path(key: ChunkKey) -> PathBuf {
    PathBuf::from(WORLD_DATA_DIR).join(format!(
        "chunk_{}_{}_{}.bin",
        key.key.x, key.key.y, key.key.z
    ))
}

/// Procedurally generates a baseline chunk if one does not exist on disk.
fn generate_fallback_chunk() -> ChunkData {
    let mut cells = vec![WorldCell::default(); CHUNK_CELL_COUNT];

    for cell in cells.iter_mut() {
        cell.terrain = Pixel {
            material: MaterialId::DIRT,
            state: 0,
            variant: 0,
            flags: PixelFlags::IS_SOLID,
        };
    }

    ChunkData {
        last_simulated: 0.0, // Will be stamped correctly by the engine's catch-up pass
        theme: ChunkTheme::GRASS_PLAINS,
        cells,
        serialized_entities: Vec::new(),
    }
}

fn save_chunk_to_disk(path: &PathBuf, data: &ChunkData) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let encoded: Vec<u8> = bitcode::encode(data);
    let mut file = std::fs::File::create(path)?;
    file.write_all(&encoded)?;

    Ok(())
}

fn load_chunk_from_disk(path: &PathBuf) -> std::io::Result<ChunkData> {
    let mut file = std::fs::File::open(path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    let data: ChunkData = bitcode::decode(&buffer)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    Ok(data)
}
