use lru::LruCache;
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};
use std::num::NonZeroUsize;

use crate::chunk::Chunk;
use crate::math::ChunkKey;

/// Engine-agnostic storage for the topological state of chunks.
#[derive(Debug)]
pub struct ChunkStore<T, M = ()> {
    pub active: FxHashMap<ChunkKey, Chunk<T, M>>,
    pub cached: LruCache<ChunkKey, Chunk<T, M>, FxBuildHasher>,
    pub pending: FxHashSet<ChunkKey>,
}

impl<T, M> ChunkStore<T, M> {
    pub fn new(cache_capacity: usize) -> Self {
        Self {
            active: FxHashMap::default(),
            cached: LruCache::with_hasher(
                NonZeroUsize::new(cache_capacity)
                    .expect("Cache capacity must be mathematically non-zero"),
                FxBuildHasher,
            ),
            pending: FxHashSet::default(),
        }
    }
}
