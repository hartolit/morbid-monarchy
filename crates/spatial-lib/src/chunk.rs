use std::fmt::Debug;

pub mod manager;
pub mod math;
pub mod storage;

/// A rigid spatial container for dense voxel or cellular data.
///
/// Enforces a physically contiguous memory block via `Box<[T]>`. This prohibits
/// accidental vector resizing/reallocations during runtime while preventing
/// the stack overflows associated with massive raw arrays.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "redb-storage", derive(bitcode::Encode, bitcode::Decode))]
pub struct Chunk<T, M = ()> {
    pub cells: Box<[T]>,
    pub metadata: M,
}

impl<T: Default + Clone, M: Default> Chunk<T, M> {
    /// Instantiates a locked-capacity chunk filled with default data.
    #[inline(always)]
    pub fn new_filled(size: usize, metadata: M) -> Self {
        Self {
            cells: vec![T::default(); size].into_boxed_slice(),
            metadata,
        }
    }
}

impl<T, M> Chunk<T, M> {
    /// Consumes a standard `Vec<T>`, locking its length permanently.
    #[inline(always)]
    pub fn from_vec(cells: Vec<T>, metadata: M) -> Self {
        Self {
            cells: cells.into_boxed_slice(),
            metadata,
        }
    }
}
