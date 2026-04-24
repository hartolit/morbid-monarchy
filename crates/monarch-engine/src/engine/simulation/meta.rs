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

pub struct MetaData {
    pub kind: MetaDataKind, // maybe small array here?
    pub neighbor: Neighbor,
}
