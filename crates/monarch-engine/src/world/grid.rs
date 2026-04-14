use bevy::{ecs::resource::Resource, math::IVec2};

use crate::world::{
    DEFAULT_ACTIVE_RADIUS_X, DEFAULT_ACTIVE_RADIUS_Y,
    cell::WorldCell,
    chunk::{CHUNK_CELL_COUNT, CHUNK_SIZE, ChunkKey},
};

/// Toroidal (wrapping) grid where cellular automata runs.
#[derive(Resource)]
pub struct ActiveWorldGrid {
    pub width: i32,
    pub height: i32,
    pub cells: Vec<WorldCell>,
    // The world coordinates of the bottom-left corner of the current active window.
    pub window_origin: IVec2,
    pub buffer_head: IVec2,
}

impl Default for ActiveWorldGrid {
    fn default() -> Self {
        let span_chunks_x = (DEFAULT_ACTIVE_RADIUS_X * 2 + 1) as i32;
        let span_chunks_y = (DEFAULT_ACTIVE_RADIUS_Y * 2 + 1) as i32;

        let width = span_chunks_x * (CHUNK_SIZE as i32);
        let height = span_chunks_y * (CHUNK_SIZE as i32);

        let origin_chunk_x = -(DEFAULT_ACTIVE_RADIUS_X as i32);
        let origin_chunk_y = -(DEFAULT_ACTIVE_RADIUS_Y as i32);

        let window_origin = IVec2::new(
            origin_chunk_x * (CHUNK_SIZE as i32),
            origin_chunk_y * (CHUNK_SIZE as i32),
        );

        Self::new(width, height, window_origin)
    }
}

impl ActiveWorldGrid {
    pub fn new(width: i32, height: i32, origin: IVec2) -> Self {
        let size = (width * height) as usize;
        Self {
            width,
            height,
            cells: vec![WorldCell::default(); size],
            window_origin: origin,
            buffer_head: IVec2::ZERO,
        }
    }

    #[inline(always)]
    fn wrap_offset(&self, offset: IVec2) -> IVec2 {
        IVec2::new(
            offset.x.rem_euclid(self.width),
            offset.y.rem_euclid(self.height),
        )
    }

    #[inline(always)]
    pub fn shift_window(&mut self, new_origin: IVec2) {
        let delta = new_origin - self.window_origin;
        self.buffer_head = self.wrap_offset(self.buffer_head + delta);
        self.window_origin = new_origin;
    }

    #[inline(always)]
    pub fn get_index(&self, world_pos: IVec2) -> usize {
        let local_pos = world_pos - self.window_origin;
        let buffer_pos = self.wrap_offset(local_pos + self.buffer_head);

        (buffer_pos.y * self.width + buffer_pos.x) as usize
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
        self.buffer_head = IVec2::ZERO;
    }
}
