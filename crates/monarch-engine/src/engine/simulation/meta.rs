pub struct Neighbor {
    pub top: MetaData,
    pub bottom: MetaData,
    pub left: MetaData,
    pub right: MetaData,
}

pub enum MetaDataKind {
    NearbyWaterPool,
    NearbyLand,
    InfestedArea,
    ForestFire,
}

pub enum MetaData {
    NearbyWaterPool(Neighbor),
    NearbyLand(Neighbor),
    InfestedArea(Neighbor),
    ForestFire(Neighbor),
}
