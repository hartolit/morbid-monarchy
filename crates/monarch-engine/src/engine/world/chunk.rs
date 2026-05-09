use bevy::math::{DVec3, IVec2, IVec3};
use bitcode::{Decode, Encode};

use crate::engine::{entities::SerializedEntity, world::cell::WorldCell};

pub const CHUNK_SIZE: usize = 64;
pub const CHUNK_CELL_COUNT: usize = CHUNK_SIZE * CHUNK_SIZE;

#[derive(Clone, Encode, Decode)]
pub struct ChunkData {
    pub last_simulated: f64,
    pub theme: ChunkTheme,
    pub cells: Vec<WorldCell>,
    pub serialized_entities: Vec<SerializedEntity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub struct ChunkTheme(pub u8);

impl ChunkTheme {
    pub const GRASS_PLAINS: Self = Self(0);
    pub const OCEAN: Self = Self(1);
    pub const DESERT: Self = Self(2);
    pub const CAVE: Self = Self(3);
    pub const FOREST: Self = Self(4);
    pub const SWAMP: Self = Self(5);
    pub const TUNDRA: Self = Self(6);
    pub const MOUNTAIN: Self = Self(7);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ChunkKey {
    pub key: IVec3,
}

impl ChunkKey {
    #[inline(always)]
    pub fn from_dvec3(pos: DVec3) -> Self {
        let chunk_f64 = CHUNK_SIZE as f64;
        Self {
            key: IVec3::new(
                (pos.x / chunk_f64).floor() as i32,
                (pos.y / chunk_f64).floor() as i32,
                (pos.z / chunk_f64).floor() as i32,
            ),
        }
    }

    /// Returns the center of the chunk.
    #[inline(always)]
    pub fn center(&self) -> DVec3 {
        let chunk_f64 = CHUNK_SIZE as f64;
        let half_chunk = chunk_f64 / 2.0;

        DVec3::new(
            (self.key.x as f64 * chunk_f64) + half_chunk,
            (self.key.y as f64 * chunk_f64) + half_chunk,
            (self.key.z as f64 * chunk_f64) + half_chunk,
        )
    }

    #[inline(always)]
    pub fn to_ivec2(&self) -> IVec2 {
        IVec2::new(self.key.x, self.key.y)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkView {
    pub min: ChunkKey,
    pub max: ChunkKey,
}

/// Creates a default [`ChunkView`] with both `min` and `max` set to [`ChunkKey::default()`].
/// Which is equal to `DVec3::default()` or `(0, 0, 0)`.
impl Default for ChunkView {
    fn default() -> Self {
        Self {
            min: ChunkKey::default(),
            max: ChunkKey::default(),
        }
    }
}

impl ChunkView {
    /// Returns `true` if the view contains zero chunks.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.min.key.x > self.max.key.x
            || self.min.key.y > self.max.key.y
            || self.min.key.z > self.max.key.z
    }

    /// Creates a cubic bounding box centered on a specific chunk.
    /// A `radius` of 1 results in a 3x3x3 volume (27 chunks).
    #[inline]
    pub fn from_cubic(center: ChunkKey, radius: i32) -> Self {
        Self {
            min: ChunkKey {
                key: center.key - IVec3::splat(radius),
            },
            max: ChunkKey {
                key: center.key + IVec3::splat(radius),
            },
        }
    }

    /// Creates a cuboid bounding box centered at `center_pos`.
    /// `h_chunk_radius` controls the horizontal spread (X and Z axes).
    /// `v_chunk_radius` controls the vertical spread (Y axis).
    /// A `chunk_radius` of 1 results in a 3x3x3 volume (27 chunks).
    #[inline]
    pub fn from_cuboid(center: ChunkKey, h_chunk_radius: i32, v_chunk_radius: i32) -> Self {
        let extent = IVec3::new(h_chunk_radius, v_chunk_radius, h_chunk_radius);
        Self {
            min: ChunkKey {
                key: center.key - extent,
            },
            max: ChunkKey {
                key: center.key + extent,
            },
        }
    }

    /// Creates a flat top-down rect on the X/Y plane.
    /// `radius_x` and `radius_y` controls the spread along the X and Y axes, leaving Z at 0.
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

    /// Returns `true` if the given `chunk` is contained within this bounding box.
    #[inline]
    pub fn contains(&self, chunk: &ChunkKey) -> bool {
        chunk.key.x >= self.min.key.x
            && chunk.key.x <= self.max.key.x
            && chunk.key.y >= self.min.key.y
            && chunk.key.y <= self.max.key.y
            && chunk.key.z >= self.min.key.z
            && chunk.key.z <= self.max.key.z
    }

    /// Iterates all chunks within this bounding box.
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

    /// Iterates chunks in expanding concentric shells.
    /// Zero-Allocation (uses stack arrays [r, -r] and chaining).
    #[inline]
    pub fn iter_concentric(&self, center: ChunkKey) -> impl Iterator<Item = ChunkKey> + '_ {
        // Finds the maximum possible distance to ANY edge (min or max)
        let max_r = (self.max.key.x - center.key.x)
            .abs()
            .max((center.key.x - self.min.key.x).abs())
            .max((self.max.key.y - center.key.y).abs())
            .max((center.key.y - self.min.key.y).abs())
            .max((self.max.key.z - center.key.z).abs())
            .max((center.key.z - self.min.key.z).abs());

        // The Center (r=0) - Handle separately to avoid branching/allocs in the loop
        std::iter::once(center)
            .chain(
                // The Shells (r=1..max)
                (1..=max_r).flat_map(move |r| {
                    // Top/Bottom Faces (Fixed Z)
                    let z_faces = (-r..=r).flat_map(move |x| {
                        (-r..=r).flat_map(move |y| {
                            [r, -r].into_iter().map(move |z| ChunkKey {
                                key: IVec3::new(
                                    center.key.x + x,
                                    center.key.y + y,
                                    center.key.z + z,
                                ),
                            })
                        })
                    });

                    // Front/Back Bands (Fixed Y, Inner Z)
                    let y_bands = (-r..=r).flat_map(move |x| {
                        (-(r - 1)..r).flat_map(move |z| {
                            [r, -r].into_iter().map(move |y| ChunkKey {
                                key: IVec3::new(
                                    center.key.x + x,
                                    center.key.y + y,
                                    center.key.z + z,
                                ),
                            })
                        })
                    });

                    // Left/Right Bands (Fixed X, Inner Y, Inner Z)
                    let x_bands = (-(r - 1)..r).flat_map(move |y| {
                        (-(r - 1)..r).flat_map(move |z| {
                            [r, -r].into_iter().map(move |x| ChunkKey {
                                key: IVec3::new(
                                    center.key.x + x,
                                    center.key.y + y,
                                    center.key.z + z,
                                ),
                            })
                        })
                    });

                    z_faces.chain(y_bands).chain(x_bands)
                }),
            )
            // Clip to view bounds
            .filter(move |k| self.contains(k))
    }
}
