#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode, Pod, Zeroable)]
#[repr(transparent)]
pub struct MetaCell {
    pub top: MetaFlags,
    pub bottom: MetaFlags,
    pub left: MetaFlags,
    pub right: MetaFlags,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode, Pod, Zeroable)]
#[repr(transparent)]
pub struct MetaFlags(pub u16);

impl MetaFlags {
    pub const NONE: Self = Self(0);
    pub const NEARBY_WATER_POOL: Self = Self(1 << 0);
    pub const NEARBY_LAND: Self = Self(1 << 1);
    pub const INFESTED_AREA: Self = Self(1 << 2);
    pub const FOREST_FIRE: Self = Self(1 << 3);

    #[inline(always)]
    pub fn contains(&self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    #[inline(always)]
    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    #[inline(always)]
    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }
}
