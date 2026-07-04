use bevy::{ecs::resource::Resource, math::IVec2};
use spatial_lib::grid::{GridReadView, ToroidalGrid};
use std::sync::atomic::{AtomicU8, Ordering};

use crate::core::world::{
    DEFAULT_ACTIVE_RADIUS_X, DEFAULT_ACTIVE_RADIUS_Y, cell::WorldCell, chunk::CHUNK_SIZE,
};

pub type CellGridReadView<'a> = GridReadView<'a, WorldCell>;

#[derive(Resource)]
pub struct ActiveWorldGrid {
    pub spatial: ToroidalGrid<WorldCell>,
    pub back_buffer: Box<[WorldCell]>,
    pub wake_buffer: Box<[AtomicU8]>,
    pub next_wake_buffer: Box<[AtomicU8]>,
    pub cells_dirty: bool,
    pub tick: u32,
}

impl Default for ActiveWorldGrid {
    fn default() -> Self {
        let span_chunks_x = (DEFAULT_ACTIVE_RADIUS_X * 2 + 1) as i32;
        let span_chunks_y = (DEFAULT_ACTIVE_RADIUS_Y * 2 + 1) as i32;
        let width = span_chunks_x * (CHUNK_SIZE as i32);
        let height = span_chunks_y * (CHUNK_SIZE as i32);

        let window_origin = IVec2::new(
            -(DEFAULT_ACTIVE_RADIUS_X as i32) * (CHUNK_SIZE as i32),
            -(DEFAULT_ACTIVE_RADIUS_Y as i32) * (CHUNK_SIZE as i32),
        );

        let size = (width * height) as usize;
        Self {
            spatial: ToroidalGrid::new(width, height, window_origin),
            back_buffer: vec![WorldCell::default(); size].into_boxed_slice(),
            wake_buffer: (0..size)
                .map(|_| AtomicU8::new(0))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            next_wake_buffer: (0..size)
                .map(|_| AtomicU8::new(2))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            cells_dirty: true,
            tick: 0,
        }
    }
}

impl ActiveWorldGrid {
    pub fn resize_buffers(&mut self) {
        let new_size = (self.spatial.width * self.spatial.height) as usize;

        self.back_buffer = vec![WorldCell::default(); new_size].into_boxed_slice();
        self.wake_buffer = (0..new_size)
            .map(|_| AtomicU8::new(0))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        self.next_wake_buffer = (0..new_size)
            .map(|_| AtomicU8::new(2)) // Force full-grid simulation wake to process structural mutations
            .collect::<Vec<_>>()
            .into_boxed_slice();
        self.cells_dirty = true;
    }

    #[inline(always)]
    pub fn back_buffer_view(&self) -> CellGridReadView<'_> {
        GridReadView {
            cells: &self.back_buffer,
            width: self.spatial.width,
            height: self.spatial.height,
            window_origin: self.spatial.window_origin,
            buffer_head: self.spatial.buffer_head,
        }
    }

    #[inline(always)]
    pub fn swap_buffers(&mut self) {
        std::mem::swap(&mut self.spatial.cells, &mut self.back_buffer);
        self.spatial.cells.copy_from_slice(&self.back_buffer);
        std::mem::swap(&mut self.wake_buffer, &mut self.next_wake_buffer);

        for val in self.next_wake_buffer.iter() {
            val.store(0, Ordering::Relaxed);
        }

        self.tick = self.tick.wrapping_add(1);
    }

    #[inline(always)]
    pub fn get_index(&self, world_pos: IVec2) -> usize {
        self.spatial.get_index(world_pos)
    }

    #[inline(always)]
    pub fn get_cell(&self, world_pos: IVec2) -> WorldCell {
        self.spatial.cells[self.spatial.get_index(world_pos)]
    }

    #[inline(always)]
    pub fn set_cell(&mut self, world_pos: IVec2, cell: WorldCell) {
        let index = self.spatial.get_index(world_pos);
        self.spatial.cells[index] = cell;
        self.cells_dirty = true;
    }

    #[inline(always)]
    pub fn wake_cell(&self, world_pos: IVec2) {
        let width = self.spatial.width;
        let height = self.spatial.height;
        let origin = self.spatial.window_origin;

        let lx = (world_pos.x - origin.x) as u32;
        let ly = (world_pos.y - origin.y) as u32;

        if lx < width as u32 && ly < height as u32 {
            let buffer_pos = self
                .spatial
                .wrap_offset(world_pos - origin + self.spatial.buffer_head);
            let idx = (buffer_pos.y * width + buffer_pos.x) as usize;

            self.next_wake_buffer[idx].fetch_max(2, Ordering::Relaxed);

            for dy in -1..=1 {
                for dx in -1..=1 {
                    let nx = (world_pos.x + dx - origin.x) as u32;
                    let ny = (world_pos.y + dy - origin.y) as u32;
                    if nx < width as u32 && ny < height as u32 {
                        let n_bp = self.spatial.wrap_offset(
                            world_pos + IVec2::new(dx, dy) - origin + self.spatial.buffer_head,
                        );
                        let n_idx = (n_bp.y * width + n_bp.x) as usize;

                        self.next_wake_buffer[n_idx].fetch_max(2, Ordering::Relaxed);
                    }
                }
            }
        }
    }
}
