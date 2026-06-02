use crate::math::ChunkKey;
use glam::IVec2;

/// A purely mathematical toroidal view over an isolated memory block for safe parallel lookups.
#[derive(Clone, Copy)]
pub struct GridReadView<'a, T> {
    pub cells: &'a [T],
    pub width: i32,
    pub height: i32,
    pub window_origin: IVec2,
    pub buffer_head: IVec2,
}

impl<'a, T> GridReadView<'a, T> {
    #[inline(always)]
    pub fn get_cell(&self, world_pos: IVec2) -> Option<(usize, &'a T)> {
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

/// A toroidal buffer. Maps an infinite 2D plane onto a fixed 1D array.
#[derive(Debug, Clone)]
pub struct ToroidalGrid<T> {
    pub width: i32,
    pub height: i32,
    pub cells: Box<[T]>,
    pub window_origin: IVec2,
    pub buffer_head: IVec2,
}

impl<T: Default + Clone> ToroidalGrid<T> {
    pub fn new(width: i32, height: i32, origin: IVec2) -> Self {
        Self {
            width,
            height,
            cells: vec![T::default(); (width * height) as usize].into_boxed_slice(),
            window_origin: origin,
            buffer_head: IVec2::ZERO,
        }
    }

    #[inline(always)]
    pub fn wrap_offset(&self, offset: IVec2) -> IVec2 {
        IVec2::new(
            offset.x.rem_euclid(self.width),
            offset.y.rem_euclid(self.height),
        )
    }

    /// Shifts the active projection window without moving data in memory.
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

    /// Injects an isolated chunk of data directly into the active toroidal projection.
    #[inline(always)]
    pub fn load_chunk(&mut self, chunk_key: ChunkKey, chunk_size: usize, chunk_cells: &[T]) {
        let world_origin = chunk_key.to_ivec2() * (chunk_size as i32);
        let span = chunk_size as i32;

        debug_assert_eq!(chunk_cells.len(), chunk_size * chunk_size);

        let mut chunk_idx = 0;
        for y in 0..span {
            for x in 0..span {
                let world_pos = IVec2::new(world_origin.x + x, world_origin.y + y);
                let buffer_idx = self.get_index(world_pos);
                self.cells[buffer_idx] = chunk_cells[chunk_idx].clone();
                chunk_idx += 1;
            }
        }
    }

    /// Extracts data from the toroidal projection out into an isolated dense array.
    #[inline(always)]
    pub fn extract_chunk_into(&self, chunk_key: ChunkKey, chunk_size: usize, dest: &mut [T]) {
        let world_origin = chunk_key.to_ivec2() * (chunk_size as i32);
        let span = chunk_size as i32;

        debug_assert_eq!(dest.len(), chunk_size * chunk_size);

        let mut chunk_idx = 0;
        for y in 0..span {
            for x in 0..span {
                let world_pos = IVec2::new(world_origin.x + x, world_origin.y + y);
                let buffer_idx = self.get_index(world_pos);
                dest[chunk_idx] = self.cells[buffer_idx].clone();
                chunk_idx += 1;
            }
        }
    }

    /// Obliterates the current projection and re-allocates the underlying arrays.
    pub fn resize_in_place(&mut self, new_width: i32, new_height: i32, new_origin: IVec2) {
        let new_size = (new_width * new_height) as usize;
        self.cells = vec![T::default(); new_size].into_boxed_slice();
        self.width = new_width;
        self.height = new_height;
        self.window_origin = new_origin;
        self.buffer_head = IVec2::ZERO;
    }
}
