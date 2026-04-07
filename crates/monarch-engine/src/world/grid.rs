use bevy::{ecs::resource::Resource, math::IVec2};

use crate::world::{
    chunk::{CHUNK_CELL_COUNT, CHUNK_SIZE, ChunkKey},
    types::{Pixel, WorldCell},
};

/// Toroidal (wrapping) grid where cellular automata runs.
#[derive(Resource)]
pub struct ActiveWorldGrid {
    pub width: i32,
    pub height: i32,
    pub cells: Box<[WorldCell]>,
    // The world coordinates of the bottom-left corner of the current active window.
    pub window_origin: IVec2,
}

impl ActiveWorldGrid {
    pub fn new(width: i32, height: i32, origin: IVec2) -> Self {
        let size = (width * height) as usize;
        Self {
            width,
            height,
            cells: vec![WorldCell::default(); size].into_boxed_slice(),
            window_origin: origin,
        }
    }

    #[inline(always)]
    pub fn get_index(&self, world_pos: IVec2) -> usize {
        let buffer_x = world_pos.x.rem_euclid(self.width);
        let buffer_y = world_pos.y.rem_euclid(self.height);

        (buffer_y * self.width + buffer_x) as usize
    }

    #[inline(always)]
    pub fn get_cell(&self, world_pos: IVec2) -> WorldCell {
        self.cells[self.get_index(world_pos)]
    }

    #[inline(always)]
    pub fn set_cell(&mut self, world_pos: IVec2, cell: WorldCell) {
        let index = self.get_index(world_pos);
        self.cells[index] = cell;
    }

    /// Injects a chunk's data from disk into the active grid.
    #[inline(always)]
    pub fn load_chunk(&mut self, chunk_key: ChunkKey, chunk_cells: &[WorldCell; CHUNK_CELL_COUNT]) {
        let world_origin = chunk_key.to_ivec2() * (CHUNK_SIZE as i32);
        let chunk_span = CHUNK_SIZE as i32;

        let mut chunk_idx = 0;
        for y in 0..chunk_span {
            for x in 0..chunk_span {
                let world_pos = IVec2::new(world_origin.x + x, world_origin.y + y);
                let buffer_idx = self.get_index(world_pos);

                self.cells[buffer_idx] = chunk_cells[chunk_idx];
                chunk_idx += 1;
            }
        }
    }

    /// Extracts a chunk's data from the active grid for saving/unloading.
    #[inline(always)]
    pub fn unload_chunk(&self, chunk_key: ChunkKey) -> Box<[WorldCell; CHUNK_CELL_COUNT]> {
        let world_origin = chunk_key.to_ivec2() * (CHUNK_SIZE as i32);
        let chunk_span = CHUNK_SIZE as i32;

        // Allocate directly on the heap
        let mut chunk_cells = vec![WorldCell::default(); CHUNK_CELL_COUNT].into_boxed_slice();

        let mut chunk_idx = 0;
        for y in 0..chunk_span {
            for x in 0..chunk_span {
                let world_pos = IVec2::new(world_origin.x + x, world_origin.y + y);
                let buffer_idx = self.get_index(world_pos);

                chunk_cells[chunk_idx] = self.cells[buffer_idx];
                chunk_idx += 1;
            }
        }

        // Safely downcast the Box<[WorldCell]> back to Box<[WorldCell; 4096]>
        chunk_cells.try_into().unwrap()
    }

    /// Extracts the old chunk from the grid, and immediately overwrites that
    /// modular space with the new chunk data.
    #[inline(always)]
    pub fn swap_boundary_chunks(
        &mut self,
        evicted_key: ChunkKey,
        incoming_key: ChunkKey,
        incoming_cells: &[WorldCell; CHUNK_CELL_COUNT],
    ) -> Box<[WorldCell; CHUNK_CELL_COUNT]> {
        let old_chunk = self.unload_chunk(evicted_key);
        self.load_chunk(incoming_key, incoming_cells);
        old_chunk
    }
}
