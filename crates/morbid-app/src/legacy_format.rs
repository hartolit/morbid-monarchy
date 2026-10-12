//! The baseline's saved-chunk encoding: the value bytes of `spatial_chunks` rows.
//!
//! The old writer bitcode-encoded `CellChunk` directly, so its bytes were defined by the derived
//! encoding of `Chunk { cells: Box<[WorldCell]>, metadata: ChunkMetadata }`. The model types no
//! longer carry a codec. This module keeps the encoding stable by mirroring that exact shape,
//! newtypes included, in private record types. The fixtures in `tests/fixtures/legacy-v0`
//! were written by the old writer, and the tests decode them and re-encode them byte for byte.
//!
//! Prompt 05 replaces this with a versioned format and an import from these rows.

use bitcode::{Decode, Encode};
use monarch_engine::core::world::chunk::{CellChunk, ChunkMetadata};
use spatial_lib::chunk::Chunk;
use world_model::{cell::WorldCell, generation::ChunkTheme};

#[derive(Encode, Decode)]
struct RecordCell(u64);

#[derive(Encode, Decode)]
struct RecordTheme(u8);

#[derive(Encode, Decode)]
struct RecordMetadata {
    last_simulated: f64,
    theme: RecordTheme,
}

#[derive(Encode, Decode)]
struct RecordChunk {
    cells: Box<[RecordCell]>,
    metadata: RecordMetadata,
}

/// Encodes a chunk exactly as the baseline writer did.
pub fn encode(chunk: &CellChunk) -> Vec<u8> {
    let record = RecordChunk {
        cells: chunk.cells.iter().map(|cell| RecordCell(cell.0)).collect(),
        metadata: RecordMetadata {
            last_simulated: chunk.metadata.last_simulated,
            theme: RecordTheme(chunk.metadata.theme.0),
        },
    };
    bitcode::encode(&record)
}

/// Decodes a stored row. A decoding error means the row is corrupt or not in this format; it
/// is not an absent chunk.
pub fn decode(bytes: &[u8]) -> Result<CellChunk, bitcode::Error> {
    let record: RecordChunk = bitcode::decode(bytes)?;
    Ok(Chunk {
        cells: record.cells.iter().map(|cell| WorldCell(cell.0)).collect(),
        metadata: ChunkMetadata {
            last_simulated: record.metadata.last_simulated,
            theme: ChunkTheme(record.metadata.theme.0),
        },
    })
}
