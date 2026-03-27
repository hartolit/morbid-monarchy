use std::fs;
use std::io;
use std::path::PathBuf;

use bevy::prelude::Resource;
use mm_core::{ChunkKey, ChunkState};

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

    pub fn load_chunk(&self, key: ChunkKey) -> io::Result<Option<ChunkState>> {
        let path = self.chunk_path(key);
        if !path.exists() {
            return Ok(None);
        }

        let bytes = fs::read(path)?;
        let state = if cfg!(debug_assertions) {
            let text = String::from_utf8(bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            ron::from_str(&text).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
        } else {
            bitcode::decode(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
        };

        Ok(Some(state))
    }

    pub fn save_chunk(&self, state: &ChunkState) -> io::Result<()> {
        fs::create_dir_all(&self.base_dir)?;
        let path = self.chunk_path(state.snapshot.key);

        if cfg!(debug_assertions) {
            let text = ron::to_string(state)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            fs::write(path, text)
        } else {
            let bytes = bitcode::encode(state);
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
    use mm_core::{ChunkKey, ChunkState, WorldConfig, generate_chunk};

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
        let state = ChunkState::new(generate_chunk(&config, key));

        persistence.save_chunk(&state).unwrap();
        let restored = persistence.load_chunk(key).unwrap().unwrap();

        assert_eq!(restored, state);

        if base_dir.exists() {
            let _ = std::fs::remove_dir_all(base_dir);
        }
    }
}
