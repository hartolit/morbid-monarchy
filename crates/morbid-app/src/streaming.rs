use std::{
    collections::HashSet,
    fs,
    io,
    path::{Path, PathBuf},
};

use bevy::{
    math::IVec2,
    prelude::Resource,
    tasks::{block_on, poll_once, AsyncComputeTaskPool, Task},
};
use monarch_engine::world::{
    ChunkWindowDelta, PersistedChunk, StoredChunk, WorldState, chunk_theme, empty_chunk, generate_chunk,
};

const WORLD_SAVE_ROOT: &str = "runtime/world/chunks";

#[derive(Resource)]
pub struct ChunkStreamingState {
    save_root: PathBuf,
    in_flight_loads: HashSet<IVec2>,
    in_flight_saves: HashSet<IVec2>,
    load_tasks: Vec<ChunkLoadTask>,
    save_tasks: Vec<ChunkSaveTask>,
}

impl Default for ChunkStreamingState {
    fn default() -> Self {
        Self {
            save_root: PathBuf::from(WORLD_SAVE_ROOT),
            in_flight_loads: HashSet::new(),
            in_flight_saves: HashSet::new(),
            load_tasks: Vec::new(),
            save_tasks: Vec::new(),
        }
    }
}

impl ChunkStreamingState {
    pub fn schedule_window_delta(&mut self, world: &mut WorldState, delta: ChunkWindowDelta) {
        for world_chunk in delta.exited_chunks {
            if self.in_flight_saves.contains(&world_chunk) {
                continue;
            }

            let Some(chunk) = world.extract_chunk(world_chunk) else {
                continue;
            };

            self.spawn_save_task(world_chunk, chunk);
        }

        for world_chunk in delta.entered_chunks {
            if self.in_flight_loads.contains(&world_chunk) {
                continue;
            }

            world.apply_chunk(world_chunk, empty_chunk(chunk_theme(world_chunk)));
            self.spawn_load_task(world_chunk);
        }
    }

    pub fn poll(&mut self, world: &mut WorldState) -> bool {
        let mut changed = false;
        let mut load_index = 0;
        while load_index < self.load_tasks.len() {
            let Some(outcome) = block_on(poll_once(&mut self.load_tasks[load_index].task)) else {
                load_index += 1;
                continue;
            };

            self.in_flight_loads.remove(&outcome.world_chunk);
            if world.active_grid.contains_world_chunk(outcome.world_chunk) {
                world.apply_chunk(outcome.world_chunk, outcome.chunk);
                changed = true;
            }
            self.load_tasks.swap_remove(load_index);
        }

        let mut save_index = 0;
        while save_index < self.save_tasks.len() {
            let Some(outcome) = block_on(poll_once(&mut self.save_tasks[save_index].task)) else {
                save_index += 1;
                continue;
            };

            self.in_flight_saves.remove(&outcome.world_chunk);
            outcome.result.ok();
            self.save_tasks.swap_remove(save_index);
        }

        changed
    }

    fn spawn_load_task(&mut self, world_chunk: IVec2) {
        let save_root = self.save_root.clone();
        let task = AsyncComputeTaskPool::get().spawn(async move {
            let chunk = load_or_generate_chunk(&save_root, world_chunk);
            ChunkLoadOutcome { world_chunk, chunk }
        });

        self.in_flight_loads.insert(world_chunk);
        self.load_tasks.push(ChunkLoadTask { task });
    }

    fn spawn_save_task(&mut self, world_chunk: IVec2, chunk: PersistedChunk) {
        let save_root = self.save_root.clone();
        let task = AsyncComputeTaskPool::get().spawn(async move {
            let result = save_chunk_to_disk(&save_root, world_chunk, chunk);
            ChunkSaveOutcome { world_chunk, result }
        });

        self.in_flight_saves.insert(world_chunk);
        self.save_tasks.push(ChunkSaveTask { task });
    }
}

struct ChunkLoadTask {
    task: Task<ChunkLoadOutcome>,
}

struct ChunkSaveTask {
    task: Task<ChunkSaveOutcome>,
}

struct ChunkLoadOutcome {
    world_chunk: IVec2,
    chunk: PersistedChunk,
}

struct ChunkSaveOutcome {
    world_chunk: IVec2,
    result: io::Result<()>,
}

fn load_or_generate_chunk(save_root: &Path, world_chunk: IVec2) -> PersistedChunk {
    let file_path = chunk_file_path(save_root, world_chunk);
    match fs::read(&file_path)
        .ok()
        .and_then(|bytes| bitcode::decode::<StoredChunk>(&bytes).ok())
    {
        Some(stored_chunk) => PersistedChunk::from_stored(stored_chunk),
        None => generate_chunk(world_chunk),
    }
}

fn save_chunk_to_disk(save_root: &Path, world_chunk: IVec2, chunk: PersistedChunk) -> io::Result<()> {
    fs::create_dir_all(save_root)?;
    let file_path = chunk_file_path(save_root, world_chunk);
    let bytes = bitcode::encode(&chunk.into_stored());
    fs::write(file_path, bytes)
}

fn chunk_file_path(save_root: &Path, world_chunk: IVec2) -> PathBuf {
    save_root.join(format!("{}_{}.bin", world_chunk.x, world_chunk.y))
}
