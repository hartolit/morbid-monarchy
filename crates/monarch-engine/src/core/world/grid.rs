use bevy::{ecs::resource::Resource, math::IVec2};
use spatial_lib::grid::{GridReadView, ToroidalGrid};
use std::sync::atomic::{AtomicU8, Ordering};

use crate::core::world::{
    DEFAULT_ACTIVE_RADIUS_X, DEFAULT_ACTIVE_RADIUS_Y, cell::WorldCell, chunk::CHUNK_SIZE,
};

pub type CellGridReadView<'a> = GridReadView<'a, WorldCell>;

// Wake byte levels. Mass moves between cells by pull-based exchange, which is only conservative
// when the donor and the receiver both run in the same tick. Cells are therefore split into
// donors (active) and receive-only halo cells, and every active cell is guaranteed to have its
// whole 3x3 neighbourhood scheduled at halo level or higher.

/// Asleep: the cell is skipped entirely.
pub const WAKE_ASLEEP: u8 = 0;
/// Halo: the cell may receive mass from an active neighbour but never donates or reacts.
pub const WAKE_HALO: u8 = 1;
/// Lowest level at which a cell runs every rule and may donate mass.
pub const WAKE_ACTIVE_MIN: u8 = 2;
/// Level granted to a changed cell. It stays active for `WAKE_ACTIVE_FRESH - WAKE_ACTIVE_MIN + 1`
/// ticks without further change, which survives the interleaved granular cadence.
pub const WAKE_ACTIVE_FRESH: u8 = 3;
/// Chebyshev radius around a changed cell that becomes active. Liquid trickles depend on the
/// receiver's neighbours, so a change can alter decisions two cells away.
pub const WAKE_CHANGE_RADIUS: i32 = 2;

/// Schedules `center` and its neighbours for the next tick: cells within `active_radius` are
/// raised to `active_level`, and one further ring is raised to halo so every active cell can
/// always hand mass to any of its neighbours.
#[inline(always)]
pub fn schedule_wake(
    next_wake: &[AtomicU8],
    view: &CellGridReadView,
    center: IVec2,
    active_level: u8,
    active_radius: i32,
) {
    let halo_radius = active_radius + 1;
    for dy in -halo_radius..=halo_radius {
        for dx in -halo_radius..=halo_radius {
            let level = if dx.abs() <= active_radius && dy.abs() <= active_radius {
                active_level
            } else {
                WAKE_HALO
            };
            if let Some(idx) = view.get_index(center + IVec2::new(dx, dy)) {
                next_wake[idx].fetch_max(level, Ordering::Relaxed);
            }
        }
    }
}

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
        let window_origin = IVec2::new(
            -(DEFAULT_ACTIVE_RADIUS_X as i32) * (CHUNK_SIZE as i32),
            -(DEFAULT_ACTIVE_RADIUS_Y as i32) * (CHUNK_SIZE as i32),
        );

        Self::new(
            span_chunks_x * (CHUNK_SIZE as i32),
            span_chunks_y * (CHUNK_SIZE as i32),
            window_origin,
        )
    }
}

impl ActiveWorldGrid {
    /// Allocates a grid of `width` x `height` cells whose lowest corner sits at `window_origin`.
    /// Every cell starts awake so the first tick processes the whole window.
    pub fn new(width: i32, height: i32, window_origin: IVec2) -> Self {
        let size = (width * height) as usize;
        Self {
            spatial: ToroidalGrid::new(width, height, window_origin),
            back_buffer: vec![WorldCell::default(); size].into_boxed_slice(),
            wake_buffer: (0..size)
                .map(|_| AtomicU8::new(WAKE_ASLEEP))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            next_wake_buffer: (0..size)
                .map(|_| AtomicU8::new(WAKE_ACTIVE_FRESH))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            cells_dirty: true,
            tick: 0,
        }
    }

    pub fn resize_buffers(&mut self) {
        let new_size = (self.spatial.width * self.spatial.height) as usize;

        self.back_buffer = vec![WorldCell::default(); new_size].into_boxed_slice();
        self.wake_buffer = (0..new_size)
            .map(|_| AtomicU8::new(WAKE_ASLEEP))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        self.next_wake_buffer = (0..new_size)
            .map(|_| AtomicU8::new(WAKE_ACTIVE_FRESH)) // Force full-grid simulation wake to process structural mutations
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
            val.store(WAKE_ASLEEP, Ordering::Relaxed);
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

    /// Schedules every cell in the window, for changes that can affect any of them at once.
    pub fn wake_all(&self) {
        for wake in self.next_wake_buffer.iter() {
            wake.store(WAKE_ACTIVE_FRESH, Ordering::Relaxed);
        }
    }

    /// Schedules an externally edited cell (brush strokes, rigid-body deformation) so the
    /// simulation picks the edit up on the next tick.
    #[inline(always)]
    pub fn wake_cell(&self, world_pos: IVec2) {
        let view = GridReadView {
            cells: &self.spatial.cells,
            width: self.spatial.width,
            height: self.spatial.height,
            window_origin: self.spatial.window_origin,
            buffer_head: self.spatial.buffer_head,
        };
        schedule_wake(
            &self.next_wake_buffer,
            &view,
            world_pos,
            WAKE_ACTIVE_FRESH,
            WAKE_CHANGE_RADIUS,
        );
    }
}
