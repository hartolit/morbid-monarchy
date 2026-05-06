use bevy::{ecs::resource::Resource, math::IVec2};
use rayon::iter::{IntoParallelRefMutIterator, ParallelIterator};
use std::sync::atomic::{AtomicU8, Ordering};

use crate::engine::world::{
    DEFAULT_ACTIVE_RADIUS_X, DEFAULT_ACTIVE_RADIUS_Y,
    cell::WorldCell,
    chunk::{CHUNK_CELL_COUNT, CHUNK_SIZE, ChunkKey},
};

#[derive(Clone, Copy)]
pub struct GridReadView<'a> {
    pub cells: &'a [WorldCell],
    pub width: i32,
    pub height: i32,
    pub window_origin: IVec2,
    pub buffer_head: IVec2,
}

impl<'a> GridReadView<'a> {
    #[inline(always)]
    pub fn get_cell(&self, world_pos: IVec2) -> Option<(usize, &'a WorldCell)> {
        // By casting to u32, negative values underflow to MAX, instantly failing the < width check.
        // This handles both >= 0 and < bounds in a single CPU instruction.
        let lx = (world_pos.x - self.window_origin.x) as u32;
        let ly = (world_pos.y - self.window_origin.y) as u32;

        if lx < self.width as u32 && ly < self.height as u32 {
            let mut bx = lx + self.buffer_head.x as u32;
            if bx >= self.width as u32 {
                bx -= self.width as u32;
            }

            let mut by = ly + self.buffer_head.y as u32;
            if by >= self.height as u32 {
                by -= self.height as u32;
            }

            let idx = (by * (self.width as u32) + bx) as usize;
            Some((idx, &self.cells[idx]))
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn get_index(&self, world_pos: IVec2) -> Option<usize> {
        let lx = (world_pos.x - self.window_origin.x) as u32;
        let ly = (world_pos.y - self.window_origin.y) as u32;

        if lx < self.width as u32 && ly < self.height as u32 {
            let mut bx = lx + self.buffer_head.x as u32;
            if bx >= self.width as u32 {
                bx -= self.width as u32;
            }

            let mut by = ly + self.buffer_head.y as u32;
            if by >= self.height as u32 {
                by -= self.height as u32;
            }

            Some((by * (self.width as u32) + bx) as usize)
        } else {
            None
        }
    }
}

#[derive(Resource)]
pub struct ActiveWorldGrid {
    pub width: i32,
    pub height: i32,
    pub cells: Vec<WorldCell>,
    pub back_buffer: Vec<WorldCell>,
    pub wake_buffer: Vec<AtomicU8>,
    pub next_wake_buffer: Vec<AtomicU8>,
    pub window_origin: IVec2,
    pub buffer_head: IVec2,
    pub cells_dirty: bool,
    pub tick: u32,
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

        let mut wake_buffer = Vec::with_capacity(size);
        let mut next_wake_buffer = Vec::with_capacity(size);
        for _ in 0..size {
            // Start asleep in the current buffer, but AWAKE in the next buffer.
            // When the first frame calls swap_buffers(), this rotates perfectly.
            wake_buffer.push(AtomicU8::new(0));
            next_wake_buffer.push(AtomicU8::new(2));
        }

        Self {
            width,
            height,
            cells: vec![WorldCell::default(); size],
            back_buffer: vec![WorldCell::default(); size],
            wake_buffer,
            next_wake_buffer,
            window_origin: origin,
            buffer_head: IVec2::ZERO,
            cells_dirty: true,
            tick: 0,
        }
    }

    #[inline(always)]
    pub fn index_to_pos(index: usize, width: i32) -> IVec2 {
        IVec2::new((index as i32) % width, (index as i32) / width)
    }

    #[inline(always)]
    pub fn back_buffer_view(&self) -> GridReadView<'_> {
        GridReadView {
            cells: &self.back_buffer,
            width: self.width,
            height: self.height,
            window_origin: self.window_origin,
            buffer_head: self.buffer_head,
        }
    }

    #[inline(always)]
    pub fn swap_buffers(&mut self) {
        std::mem::swap(&mut self.cells, &mut self.back_buffer);
        self.cells.copy_from_slice(&self.back_buffer);

        std::mem::swap(&mut self.wake_buffer, &mut self.next_wake_buffer);

        self.next_wake_buffer
            .par_iter_mut()
            .for_each(|a| *a.get_mut() = 0);

        self.tick = self.tick.wrapping_add(1);
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
        self.cells_dirty = true;
    }

    #[inline(always)]
    pub fn wake_cell(&self, world_pos: IVec2) {
        let lx = (world_pos.x - self.window_origin.x) as u32;
        let ly = (world_pos.y - self.window_origin.y) as u32;
        if lx < self.width as u32 && ly < self.height as u32 {
            let buffer_pos = self.wrap_offset(world_pos - self.window_origin + self.buffer_head);
            let idx = (buffer_pos.y * self.width + buffer_pos.x) as usize;

            self.next_wake_buffer[idx].fetch_max(2, Ordering::Relaxed);

            for dy in -1..=1 {
                for dx in -1..=1 {
                    let nx = (world_pos.x + dx - self.window_origin.x) as u32;
                    let ny = (world_pos.y + dy - self.window_origin.y) as u32;
                    if nx < self.width as u32 && ny < self.height as u32 {
                        let n_bp = self.wrap_offset(
                            world_pos + IVec2::new(dx, dy) - self.window_origin + self.buffer_head,
                        );
                        let n_idx = (n_bp.y * self.width + n_bp.x) as usize;

                        self.next_wake_buffer[n_idx].fetch_max(2, Ordering::Relaxed);
                    }
                }
            }
        }
    }

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

                // Write to NEXT wake buffer
                *self.next_wake_buffer[buffer_idx].get_mut() = 2;

                chunk_idx += 1;
            }
        }
        self.cells_dirty = true;
    }

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

    pub fn resize_in_place(&mut self, new_width: i32, new_height: i32, new_origin: IVec2) {
        let new_size = (new_width * new_height) as usize;

        self.cells.clear();
        self.cells.resize(new_size, WorldCell::default());

        self.back_buffer.clear();
        self.back_buffer.resize(new_size, WorldCell::default());

        self.wake_buffer.clear();
        self.wake_buffer.resize_with(new_size, || AtomicU8::new(0));

        // Start newly resized grids entirely awake
        self.next_wake_buffer.clear();
        self.next_wake_buffer
            .resize_with(new_size, || AtomicU8::new(2));

        self.width = new_width;
        self.height = new_height;
        self.window_origin = new_origin;
        self.buffer_head = IVec2::ZERO;
        self.cells_dirty = true;
    }
}
