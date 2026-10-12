use spatial_lib::chunk::Chunk;
use world_model::{
    cell::WorldCell,
    generation::{ChunkTheme, GeneratedChunk},
};

/// What the baseline keeps beside a chunk's cells.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct ChunkMetadata {
    pub last_simulated: f64,
    pub theme: ChunkTheme,
}

pub type CellChunk = Chunk<WorldCell, ChunkMetadata>;

/// A freshly generated chunk as the baseline stores it: never simulated yet.
pub fn generated_cell_chunk(generated: GeneratedChunk) -> CellChunk {
    Chunk {
        cells: generated.cells,
        metadata: ChunkMetadata {
            last_simulated: 0.0,
            theme: generated.theme,
        },
    }
}
