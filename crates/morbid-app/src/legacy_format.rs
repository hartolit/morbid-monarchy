//! The baseline's chunk rows, as the baseline's own chunk type.
//!
//! `world_store::legacy` owns the bytes of a `spatial_chunks` row and is tested against the
//! old writer's fixtures; this module only converts between that row content and the engine's
//! `CellChunk`. Prompt 06 removes it with the baseline.

use monarch_engine::core::world::chunk::{CellChunk, ChunkMetadata};
use spatial_lib::chunk::Chunk;
use world_store::legacy::{self, LegacyChunk};

/// Encodes a chunk exactly as the baseline writer did.
pub fn encode(chunk: &CellChunk) -> Vec<u8> {
    legacy::encode(&LegacyChunk {
        cells: chunk.cells.clone(),
        last_simulated: chunk.metadata.last_simulated,
        theme: chunk.metadata.theme,
    })
}

/// The engine's chunk for a decoded row.
pub fn into_cell_chunk(chunk: LegacyChunk) -> CellChunk {
    Chunk {
        cells: chunk.cells,
        metadata: ChunkMetadata {
            last_simulated: chunk.last_simulated,
            theme: chunk.theme,
        },
    }
}
