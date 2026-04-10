use bevy::{ecs::resource::Resource, math::IVec2};

use crate::world::{
    chunk::{CHUNK_CELL_COUNT, CHUNK_SIZE, ChunkKey},
    types::WorldCell,
};

/// Toroidal (wrapping) grid where cellular automata runs.
#[derive(Resource)]
pub struct ActiveWorldGrid {
    pub width: i32,
    pub height: i32,
    pub cells: Vec<WorldCell>,
    // The world coordinates of the bottom-left corner of the current active window.
    pub window_origin: IVec2,
}

impl ActiveWorldGrid {
    pub fn new(width: i32, height: i32, origin: IVec2) -> Self {
        let size = (width * height) as usize;
        Self {
            width,
            height,
            cells: vec![WorldCell::default(); size],
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

    /// Injects a chunk's data from disk/RAM into the active grid.
    #[inline(always)]
    pub fn load_chunk(&mut self, chunk_key: ChunkKey, chunk_cells: &[WorldCell]) {
        debug_assert_eq!(chunk_cells.len(), CHUNK_CELL_COUNT);

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

    /// Extracts a chunk's data directly into an existing buffer. (Zero Allocation)
    #[inline(always)]
    pub fn extract_chunk_into(&self, chunk_key: ChunkKey, dest: &mut [WorldCell]) {
        debug_assert_eq!(dest.len(), CHUNK_CELL_COUNT);

        let world_origin = chunk_key.to_ivec2() * (CHUNK_SIZE as i32);
        let chunk_span = CHUNK_SIZE as i32;

        let mut chunk_idx = 0;
        for y in 0..chunk_span {
            for x in 0..chunk_span {
                let world_pos = IVec2::new(world_origin.x + x, world_origin.y + y);
                let buffer_idx = self.get_index(world_pos);

                dest[chunk_idx] = self.cells[buffer_idx];
                chunk_idx += 1;
            }
        }
    }

    /// Resizes the grid's underlying vector in-place and updates the math variables.
    /// WARNING: This changes the stride. Caller MUST extract data before calling this.
    pub fn resize_in_place(&mut self, new_width: i32, new_height: i32, new_origin: IVec2) {
        let new_size = (new_width * new_height) as usize;

        // This will expand capacity if needed, or simply truncate the len if shrinking.
        // It does not drop the underlying allocation.
        self.cells.clear();
        self.cells.resize(new_size, WorldCell::default());

        self.width = new_width;
        self.height = new_height;
        self.window_origin = new_origin;
    }
}
