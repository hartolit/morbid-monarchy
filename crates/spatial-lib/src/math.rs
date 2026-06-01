use glam::{DVec3, IVec2, IVec3};

/// A discrete 3D coordinate identifying a specific spatial chunk in the world grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "redb-storage", derive(bitcode::Encode, bitcode::Decode))]
pub struct ChunkKey {
    pub key: IVec3,
}

impl ChunkKey {
    /// Creates a new chunk key from raw integer coordinates.
    #[inline(always)]
    pub fn new(x: i32, y: i32, z: i32) -> Self {
        Self {
            key: IVec3::new(x, y, z),
        }
    }

    /// Derives the chunk key containing a given continuous world position.
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

    /// Returns the exact mathematical center of this chunk in continuous world space.
    #[inline(always)]
    pub fn center(&self, chunk_size: f64) -> DVec3 {
        let half_chunk = chunk_size / 2.0;
        DVec3::new(
            (self.key.x as f64 * chunk_size) + half_chunk,
            (self.key.y as f64 * chunk_size) + half_chunk,
            (self.key.z as f64 * chunk_size) + half_chunk,
        )
    }

    /// Flattens the 3D key into a 2D coordinate on the XZ plane (mapped to XY).
    #[inline(always)]
    pub fn to_ivec2(&self) -> IVec2 {
        IVec2::new(self.key.x, self.key.y)
    }
}

/// An axis-aligned bounding box defining a volume of chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkView {
    pub min: ChunkKey,
    pub max: ChunkKey,
}

/// Creates a default `ChunkView` spanning exactly one chunk at the origin `(0, 0, 0)`.
impl Default for ChunkView {
    fn default() -> Self {
        Self {
            min: ChunkKey::default(),
            max: ChunkKey::default(),
        }
    }
}

impl ChunkView {
    /// Evaluates if the geometric bounds of the view result in a zero-volume container.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.min.key.x > self.max.key.x
            || self.min.key.y > self.max.key.y
            || self.min.key.z > self.max.key.z
    }

    /// Creates a perfect cubic bounding box centered on a specific chunk.
    /// A `radius` of 1 yields a 3x3x3 volume.
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

    /// Creates an asymmetrical cuboid bounding box.
    /// `h_chunk_radius` controls X/Z spread; `v_chunk_radius` controls Y elevation spread.
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

    /// Creates a flat rectangular view locked to the X/Y plane (Z spread is zero).
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

    /// Checks if a specific chunk coordinate exists within this volume.
    #[inline]
    pub fn contains(&self, chunk: &ChunkKey) -> bool {
        chunk.key.x >= self.min.key.x
            && chunk.key.x <= self.max.key.x
            && chunk.key.y >= self.min.key.y
            && chunk.key.y <= self.max.key.y
            && chunk.key.z >= self.min.key.z
            && chunk.key.z <= self.max.key.z
    }

    /// Yields a standard linear iterator over all chunks in the volume.
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

    /// Yields chunks outward from the center in concentric shells.
    /// Designed for zero-allocation generation scheduling prioritized by proximity.
    #[inline]
    pub fn iter_concentric(&self, center: ChunkKey) -> impl Iterator<Item = ChunkKey> + '_ {
        let max_r = (self.max.key.x - center.key.x)
            .abs()
            .max((center.key.x - self.min.key.x).abs())
            .max((self.max.key.y - center.key.y).abs())
            .max((center.key.y - self.min.key.y).abs())
            .max((self.max.key.z - center.key.z).abs())
            .max((center.key.z - self.min.key.z).abs());

        std::iter::once(center)
            .chain((1..=max_r).flat_map(move |r| {
                let z_faces = (-r..=r).flat_map(move |x| {
                    (-r..=r).flat_map(move |y| {
                        [r, -r].into_iter().map(move |z| ChunkKey {
                            key: IVec3::new(center.key.x + x, center.key.y + y, center.key.z + z),
                        })
                    })
                });

                let y_bands = (-r..=r).flat_map(move |x| {
                    (-(r - 1)..r).flat_map(move |z| {
                        [r, -r].into_iter().map(move |y| ChunkKey {
                            key: IVec3::new(center.key.x + x, center.key.y + y, center.key.z + z),
                        })
                    })
                });

                let x_bands = (-(r - 1)..r).flat_map(move |y| {
                    (-(r - 1)..r).flat_map(move |z| {
                        [r, -r].into_iter().map(move |x| ChunkKey {
                            key: IVec3::new(center.key.x + x, center.key.y + y, center.key.z + z),
                        })
                    })
                });

                z_faces.chain(y_bands).chain(x_bands)
            }))
            .filter(move |k| self.contains(k))
    }
}
