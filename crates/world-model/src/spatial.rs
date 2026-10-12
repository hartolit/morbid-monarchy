//! Chunk geometry, absolute chunk keys and the axis convention between cells and render space.
//!
//! The world is a plane of cells addressed by absolute integer coordinates `(x, y)`. Chunks are
//! `CHUNK_SIZE` x `CHUNK_SIZE` cells; chunk `(cx, cy)` holds cells `cx*64 .. cx*64+63` by
//! `cy*64 .. cy*64+63`, stored row-major (`index = local_y * 64 + local_x`).
//!
//! Render space is y-up. A cell's height sits at the mesh vertex `(x, height, -y)`: cell `y`
//! grows toward render `-z`. Two conversions from a render position back to a cell are in use
//! and mean different things, so both are named here:
//! - [`nearest_cell`] rounds: the cell whose vertex is closest (body centres);
//! - [`quad_cell`] floors: the cell at the lower corner of the quad containing the point
//!   (pointer hits, brushes, the observer's floor).

use glam::{DVec3, IVec2, IVec3};

/// Cells along each side of a chunk.
pub const CHUNK_SIZE: usize = 64;
pub const CHUNK_CELL_COUNT: usize = CHUNK_SIZE * CHUNK_SIZE;

/// The cell whose mesh vertex is nearest the render-space point `(x, _, z)`.
#[inline(always)]
pub fn nearest_cell(x: f32, z: f32) -> IVec2 {
    IVec2::new(x.round() as i32, (-z).round() as i32)
}

/// The cell at the lower corner of the mesh quad containing the render-space point `(x, _, z)`.
#[inline(always)]
pub fn quad_cell(x: f32, z: f32) -> IVec2 {
    IVec2::new(x.floor() as i32, (-z).floor() as i32)
}

/// Render-space `(x, z)` of a cell's vertex.
#[inline(always)]
pub fn cell_render_xz(cell: IVec2) -> (f32, f32) {
    (cell.x as f32, -(cell.y as f32))
}

/// An absolute chunk identity: three signed components.
///
/// The planar world uses `x` and `y`; every key the application produces has `z = 0`. The third
/// component exists because saved rows are keyed by `[i32; 3]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ChunkKey {
    pub key: IVec3,
}

impl ChunkKey {
    #[inline(always)]
    pub fn new(x: i32, y: i32, z: i32) -> Self {
        Self {
            key: IVec3::new(x, y, z),
        }
    }

    /// The chunk containing a continuous position given in cell units on each axis.
    #[inline(always)]
    pub fn from_world(pos: DVec3, chunk_size: f64) -> Self {
        Self {
            key: IVec3::new(
                (pos.x / chunk_size).floor() as i32,
                (pos.y / chunk_size).floor() as i32,
                (pos.z / chunk_size).floor() as i32,
            ),
        }
    }

    /// The planar `(x, y)` components, dropping `z`.
    #[inline(always)]
    pub fn to_ivec2(&self) -> IVec2 {
        IVec2::new(self.key.x, self.key.y)
    }

    /// Absolute coordinates of the chunk's first cell.
    #[inline(always)]
    pub fn origin_cell(&self) -> IVec2 {
        self.to_ivec2() * CHUNK_SIZE as i32
    }
}

/// An inclusive box of chunk keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ChunkView {
    pub min: ChunkKey,
    pub max: ChunkKey,
}

impl ChunkView {
    /// A planar rectangle around `center`: `radius_x` and `radius_y` chunks on each side, with
    /// the centre's `z`.
    #[inline]
    pub fn from_rect_xy(center: ChunkKey, radius_x: i32, radius_y: i32) -> Self {
        let extent = IVec3::new(radius_x, radius_y, 0);
        Self {
            min: ChunkKey {
                key: center.key - extent,
            },
            max: ChunkKey {
                key: center.key + extent,
            },
        }
    }

    #[inline]
    pub fn contains(&self, chunk: &ChunkKey) -> bool {
        chunk.key.x >= self.min.key.x
            && chunk.key.x <= self.max.key.x
            && chunk.key.y >= self.min.key.y
            && chunk.key.y <= self.max.key.y
            && chunk.key.z >= self.min.key.z
            && chunk.key.z <= self.max.key.z
    }

    /// Every key in the box, `x` outermost, then `y`, then `z`.
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = ChunkKey> + '_ {
        let min = self.min.key;
        let max = self.max.key;
        (min.x..=max.x)
            .flat_map(move |x| (min.y..=max.y).map(move |y| (x, y)))
            .flat_map(move |(x, y)| {
                (min.z..=max.z).map(move |z| ChunkKey {
                    key: IVec3::new(x, y, z),
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_cell_conversions_differ_only_where_they_should() {
        // Just past a vertex toward +x and toward cell -y (render +z).
        assert_eq!(nearest_cell(3.4, -5.4), IVec2::new(3, 5));
        assert_eq!(quad_cell(3.4, -5.4), IVec2::new(3, 5));
        assert_eq!(nearest_cell(3.6, -5.6), IVec2::new(4, 6));
        assert_eq!(quad_cell(3.6, -5.6), IVec2::new(3, 5));
        // Negative cells: floor goes down, not toward zero.
        assert_eq!(quad_cell(-0.5, 0.5), IVec2::new(-1, -1));
        assert_eq!(cell_render_xz(IVec2::new(7, -2)), (7.0, 2.0));
    }

    #[test]
    fn keys_floor_negative_positions() {
        let key = |x, y| ChunkKey::from_world(DVec3::new(x, y, 0.0), CHUNK_SIZE as f64);
        assert_eq!(key(32.0, 32.0), ChunkKey::new(0, 0, 0));
        assert_eq!(key(-0.5, -0.5), ChunkKey::new(-1, -1, 0));
        assert_eq!(key(200.0, 230.0), ChunkKey::new(3, 3, 0));
        assert_eq!(ChunkKey::new(-2, 3, 0).origin_cell(), IVec2::new(-128, 192));
    }

    #[test]
    fn rect_view_holds_the_window_and_iterates_every_key_once() {
        let view = ChunkView::from_rect_xy(ChunkKey::new(1, -1, 0), 2, 1);
        let keys: Vec<ChunkKey> = view.iter().collect();
        assert_eq!(keys.len(), 5 * 3);
        assert!(keys.iter().all(|key| view.contains(key) && key.key.z == 0));
        assert!(!view.contains(&ChunkKey::new(4, -1, 0)));
        assert!(!view.contains(&ChunkKey::new(1, -1, 1)));
    }
}
