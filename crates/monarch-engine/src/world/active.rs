use std::mem;

use bevy::{
    ecs::resource::Resource,
    math::{IVec2, UVec2},
};

use crate::world::{
    chunk::{PersistedChunk, Pixel, CHUNK_PIXELS, CHUNK_SIDE},
    generation::{chunk_theme, empty_chunk},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveGridConfig {
    pub loaded_chunks: UVec2,
    pub visible_chunks: UVec2,
}

impl ActiveGridConfig {
    pub const DEFAULT_LOADED_CHUNKS: UVec2 = UVec2::new(10, 10);
    pub const DEFAULT_VISIBLE_CHUNKS: UVec2 = UVec2::new(8, 8);

    pub fn sanitized(self) -> Self {
        let visible_chunks = UVec2::new(self.visible_chunks.x.max(1), self.visible_chunks.y.max(1));
        let loaded_chunks = UVec2::new(
            self.loaded_chunks.x.max(visible_chunks.x),
            self.loaded_chunks.y.max(visible_chunks.y),
        );

        Self {
            loaded_chunks,
            visible_chunks,
        }
    }

    pub fn loaded_pixel_dimensions(self) -> UVec2 {
        self.loaded_chunks * CHUNK_SIDE as u32
    }

    pub fn visible_pixel_dimensions(self) -> UVec2 {
        self.visible_chunks * CHUNK_SIDE as u32
    }

    pub fn half_loaded_chunks(self) -> IVec2 {
        IVec2::new(
            (self.loaded_chunks.x / 2) as i32,
            (self.loaded_chunks.y / 2) as i32,
        )
    }

    pub fn loaded_chunks_i32(self) -> IVec2 {
        IVec2::new(self.loaded_chunks.x as i32, self.loaded_chunks.y as i32)
    }

    pub fn half_visible_pixels(self) -> IVec2 {
        let visible_pixel_dimensions = self.visible_pixel_dimensions();
        IVec2::new(
            (visible_pixel_dimensions.x / 2) as i32,
            (visible_pixel_dimensions.y / 2) as i32,
        )
    }
}

impl Default for ActiveGridConfig {
    fn default() -> Self {
        Self {
            loaded_chunks: Self::DEFAULT_LOADED_CHUNKS,
            visible_chunks: Self::DEFAULT_VISIBLE_CHUNKS,
        }
        .sanitized()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveGridView {
    pub loaded_world_chunk_min: IVec2,
    pub loaded_world_pixel_min: IVec2,
    pub loaded_pixel_dimensions: UVec2,
    pub visible_world_pixel_min: IVec2,
    pub visible_pixel_dimensions: UVec2,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChunkWindowDelta {
    pub entered_chunks: Vec<IVec2>,
    pub exited_chunks: Vec<IVec2>,
}

impl ChunkWindowDelta {
    pub fn is_empty(&self) -> bool {
        self.entered_chunks.is_empty() && self.exited_chunks.is_empty()
    }

    pub fn full_window(chunks: Vec<IVec2>) -> Self {
        Self {
            entered_chunks: chunks,
            exited_chunks: Vec::new(),
        }
    }

    pub fn append(&mut self, mut other: Self) {
        self.entered_chunks.append(&mut other.entered_chunks);
        self.exited_chunks.append(&mut other.exited_chunks);
    }
}

#[derive(Debug, Clone)]
pub struct ToroidalGrid {
    config: ActiveGridConfig,
    pixels: Vec<Pixel>,
    world_chunk_min: IVec2,
}

impl ToroidalGrid {
    pub fn new(config: ActiveGridConfig, player_world_pixel: IVec2) -> Self {
        let config = config.sanitized();
        let pixel_dimensions = config.loaded_pixel_dimensions();
        let mut grid = Self {
            config,
            pixels: vec![Pixel::EMPTY; pixel_dimensions.x as usize * pixel_dimensions.y as usize],
            world_chunk_min: IVec2::ZERO,
        };
        grid.reset_around(player_world_pixel);
        grid
    }

    pub fn config(&self) -> ActiveGridConfig {
        self.config
    }

    pub fn loaded_pixel_dimensions(&self) -> UVec2 {
        self.config.loaded_pixel_dimensions()
    }

    pub fn visible_pixel_dimensions(&self) -> UVec2 {
        self.config.visible_pixel_dimensions()
    }

    pub fn active_world_pixel_min(&self) -> IVec2 {
        self.world_chunk_min * CHUNK_SIDE as i32
    }

    pub fn visible_world_pixel_min(&self, player_world_pixel: IVec2) -> IVec2 {
        player_world_pixel - self.config.half_visible_pixels()
    }

    pub fn view(&self, player_world_pixel: IVec2) -> ActiveGridView {
        ActiveGridView {
            loaded_world_chunk_min: self.world_chunk_min,
            loaded_world_pixel_min: self.active_world_pixel_min(),
            loaded_pixel_dimensions: self.loaded_pixel_dimensions(),
            visible_world_pixel_min: self.visible_world_pixel_min(player_world_pixel),
            visible_pixel_dimensions: self.visible_pixel_dimensions(),
        }
    }

    pub fn world_to_chunk(&self, world_pixel: IVec2) -> IVec2 {
        IVec2::new(
            world_pixel.x.div_euclid(CHUNK_SIDE as i32),
            world_pixel.y.div_euclid(CHUNK_SIDE as i32),
        )
    }

    pub fn get_world_pixel(&self, world_pixel: IVec2) -> Pixel {
        self.pixels[self.buffer_index(world_pixel)]
    }

    pub fn set_world_pixel(&mut self, world_pixel: IVec2, pixel: Pixel) {
        let index = self.buffer_index(world_pixel);
        self.pixels[index] = pixel;
    }

    pub fn contains_world_chunk(&self, world_chunk: IVec2) -> bool {
        let loaded_chunks = self.config.loaded_chunks_i32();
        let max = self.world_chunk_min + loaded_chunks;

        world_chunk.x >= self.world_chunk_min.x
            && world_chunk.x < max.x
            && world_chunk.y >= self.world_chunk_min.y
            && world_chunk.y < max.y
    }

    pub fn window_chunks(&self) -> Vec<IVec2> {
        let max = self.world_chunk_min + self.config.loaded_chunks_i32();
        let mut chunks = Vec::with_capacity((self.config.loaded_chunks.x * self.config.loaded_chunks.y) as usize);

        for chunk_y in self.world_chunk_min.y..max.y {
            for chunk_x in self.world_chunk_min.x..max.x {
                chunks.push(IVec2::new(chunk_x, chunk_y));
            }
        }

        chunks
    }

    pub fn full_window_delta(&self) -> ChunkWindowDelta {
        ChunkWindowDelta::full_window(self.window_chunks())
    }

    pub fn copy_visible_window_pixels(&self, player_world_pixel: IVec2) -> Vec<Pixel> {
        let view = self.view(player_world_pixel);
        let mut output = Vec::with_capacity(
            view.visible_pixel_dimensions.x as usize * view.visible_pixel_dimensions.y as usize,
        );

        for y in 0..view.visible_pixel_dimensions.y as i32 {
            for x in 0..view.visible_pixel_dimensions.x as i32 {
                let world_pixel = view.visible_world_pixel_min + IVec2::new(x, y);
                output.push(self.get_world_pixel(world_pixel));
            }
        }

        output
    }

    pub fn recenter_around(&mut self, player_world_pixel: IVec2) -> ChunkWindowDelta {
        let target_chunk = self.world_to_chunk(player_world_pixel);
        let target_min = target_chunk - self.config.half_loaded_chunks();

        if target_min == self.world_chunk_min {
            return ChunkWindowDelta::default();
        }

        let previous_min = self.world_chunk_min;
        let previous_max = previous_min + self.config.loaded_chunks_i32();
        let next_max = target_min + self.config.loaded_chunks_i32();
        let mut delta = ChunkWindowDelta::default();

        for chunk_y in previous_min.y..previous_max.y {
            for chunk_x in previous_min.x..previous_max.x {
                let world_chunk = IVec2::new(chunk_x, chunk_y);
                let still_present = world_chunk.x >= target_min.x
                    && world_chunk.x < next_max.x
                    && world_chunk.y >= target_min.y
                    && world_chunk.y < next_max.y;

                if !still_present {
                    delta.exited_chunks.push(world_chunk);
                }
            }
        }

        for chunk_y in target_min.y..next_max.y {
            for chunk_x in target_min.x..next_max.x {
                let world_chunk = IVec2::new(chunk_x, chunk_y);
                let already_present = world_chunk.x >= previous_min.x
                    && world_chunk.x < previous_max.x
                    && world_chunk.y >= previous_min.y
                    && world_chunk.y < previous_max.y;

                if !already_present {
                    delta.entered_chunks.push(world_chunk);
                }
            }
        }

        self.world_chunk_min = target_min;
        delta
    }

    fn reset_around(&mut self, player_world_pixel: IVec2) {
        let center_chunk = self.world_to_chunk(player_world_pixel);
        self.world_chunk_min = center_chunk - self.config.half_loaded_chunks();

        for world_chunk in self.window_chunks() {
            self.clear_chunk(world_chunk);
        }
    }

    pub fn read_chunk(&self, world_chunk: IVec2) -> PersistedChunk {
        let mut pixels = Box::new([Pixel::EMPTY; CHUNK_PIXELS]);
        let world_origin = world_chunk * CHUNK_SIDE as i32;

        for local_y in 0..CHUNK_SIDE as i32 {
            for local_x in 0..CHUNK_SIDE as i32 {
                let local_index = local_y as usize * CHUNK_SIDE + local_x as usize;
                let world_pixel = world_origin + IVec2::new(local_x, local_y);
                pixels[local_index] = self.get_world_pixel(world_pixel);
            }
        }

        PersistedChunk {
            theme: chunk_theme(world_chunk),
            pixels,
            entities: Vec::new(),
        }
    }

    pub fn write_chunk(&mut self, world_chunk: IVec2, chunk: &PersistedChunk) {
        if !self.contains_world_chunk(world_chunk) {
            return;
        }

        let world_origin = world_chunk * CHUNK_SIDE as i32;

        for local_y in 0..CHUNK_SIDE as i32 {
            for local_x in 0..CHUNK_SIDE as i32 {
                let local_index = local_y as usize * CHUNK_SIDE + local_x as usize;
                let world_pixel = world_origin + IVec2::new(local_x, local_y);
                self.set_world_pixel(world_pixel, chunk.pixels[local_index]);
            }
        }
    }

    fn clear_chunk(&mut self, world_chunk: IVec2) {
        let placeholder = empty_chunk(chunk_theme(world_chunk));
        self.write_chunk(world_chunk, &placeholder);
    }

    fn buffer_index(&self, world_pixel: IVec2) -> usize {
        let dimensions = self.loaded_pixel_dimensions();
        let buffer_x = world_pixel.x.rem_euclid(dimensions.x as i32) as usize;
        let buffer_y = world_pixel.y.rem_euclid(dimensions.y as i32) as usize;
        buffer_y * dimensions.x as usize + buffer_x
    }
}

#[derive(Resource, Clone)]
pub struct WorldState {
    pub player_world_pixel: IVec2,
    pub active_grid: ToroidalGrid,
    pending_chunk_window_delta: ChunkWindowDelta,
}

impl Default for WorldState {
    fn default() -> Self {
        let player_world_pixel = IVec2::ZERO;
        let active_grid = ToroidalGrid::new(ActiveGridConfig::default(), player_world_pixel);
        let pending_chunk_window_delta = active_grid.full_window_delta();
        Self {
            player_world_pixel,
            active_grid,
            pending_chunk_window_delta,
        }
    }
}

impl WorldState {
    pub fn move_player_by(&mut self, delta: IVec2) {
        if delta == IVec2::ZERO {
            return;
        }

        self.player_world_pixel += delta;
        let window_delta = self.active_grid.recenter_around(self.player_world_pixel);
        self.pending_chunk_window_delta.append(window_delta);
    }

    pub fn player_view_pixel(&self) -> IVec2 {
        self.player_world_pixel - self.active_grid.visible_world_pixel_min(self.player_world_pixel)
    }

    pub fn take_chunk_window_delta(&mut self) -> ChunkWindowDelta {
        mem::take(&mut self.pending_chunk_window_delta)
    }

    pub fn apply_chunk(&mut self, world_chunk: IVec2, chunk: PersistedChunk) {
        self.active_grid.write_chunk(world_chunk, &chunk);
    }

    pub fn extract_chunk(&self, world_chunk: IVec2) -> Option<PersistedChunk> {
        Some(self.active_grid.read_chunk(world_chunk))
    }
}
