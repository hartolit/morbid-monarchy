use std::fs;
use std::io;
use std::path::PathBuf;

use bevy::prelude::Resource;
use bitcode::{Decode, Encode};
use mm_core::{ChunkDelta, ChunkKey, ChunkState, WorldConfig, generate_chunk};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
struct PersistedChunkState {
    key: ChunkKey,
    delta: ChunkDelta,
}

#[derive(Resource, Debug, Clone)]
pub struct ChunkPersistence {
    base_dir: PathBuf,
}

impl Default for ChunkPersistence {
    fn default() -> Self {
        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::new(root.join("world_data"))
    }
}

impl ChunkPersistence {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    pub fn load_chunk(&self, config: &WorldConfig, key: ChunkKey) -> io::Result<Option<ChunkState>> {
        let path = self.chunk_path(key);
        if !path.exists() {
            return Ok(None);
        }

        let bytes = fs::read(path)?;
        let persisted: PersistedChunkState = if cfg!(debug_assertions) {
            let text = String::from_utf8(bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            ron::from_str(&text).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
        } else {
            bitcode::decode(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
        };

        let mut state = ChunkState::new(generate_chunk(config, persisted.key));
        state.delta = persisted.delta;

        Ok(Some(state))
    }

    pub fn save_chunk(&self, state: &ChunkState) -> io::Result<()> {
        let path = self.chunk_path(state.data.key);
        if state.delta.removed_object_ids.is_empty() && state.delta.pixel_overrides.is_empty() {
            if path.exists() {
                fs::remove_file(path)?;
            }
            return Ok(());
        }

        fs::create_dir_all(&self.base_dir)?;
        let persisted = PersistedChunkState {
            key: state.data.key,
            delta: state.delta.clone(),
        };

        if cfg!(debug_assertions) {
            let text = ron::to_string(&persisted)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            fs::write(path, text)
        } else {
            let bytes = bitcode::encode(&persisted);
            fs::write(path, bytes)
        }
    }

    fn chunk_path(&self, key: ChunkKey) -> PathBuf {
        self.base_dir.join(if cfg!(debug_assertions) {
            format!("chunk_{}_{}_{}.ron", key.x, key.y, key.z)
        } else {
            format!("chunk_{}_{}_{}.bin", key.x, key.y, key.z)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ChunkPersistence;
    use mm_core::{ChunkKey, ChunkLocalPixel, ChunkState, WorldConfig, WorldPixel, generate_chunk};

    #[test]
    fn chunk_persistence_round_trip_restores_saved_state() {
        let base_dir = std::env::temp_dir().join(format!(
            "mm-app-persistence-test-{}-{}",
            std::process::id(),
            "round-trip"
        ));
        let persistence = ChunkPersistence::new(base_dir.clone());
        let config = WorldConfig::default();
        let key = ChunkKey { x: 4, y: -2, z: 0 };
        let mut state = ChunkState::new(generate_chunk(&config, key));
        state.set_pixel(ChunkLocalPixel::new(8, 9).unwrap(), WorldPixel::Blood);

        persistence.save_chunk(&state).unwrap();
        let restored = persistence.load_chunk(&config, key).unwrap().unwrap();

        assert_eq!(restored, state);

        if base_dir.exists() {
            let _ = std::fs::remove_dir_all(base_dir);
        }
    }

    #[test]
    fn clean_chunk_state_does_not_leave_persistence_file() {
        let base_dir = std::env::temp_dir().join(format!(
            "mm-app-persistence-test-{}-{}",
            std::process::id(),
            "clean-state"
        ));
        let persistence = ChunkPersistence::new(base_dir.clone());
        let config = WorldConfig::default();
        let key = ChunkKey::ORIGIN;
        let state = ChunkState::new(generate_chunk(&config, key));

        persistence.save_chunk(&state).unwrap();

        assert!(persistence.load_chunk(&config, key).unwrap().is_none());

        if base_dir.exists() {
            let _ = std::fs::remove_dir_all(base_dir);
        }
    }
}
