use crate::engine::world::cell::{GranularMat, SurfaceMat, TerrainMat};

#[inline(always)]
pub fn get_granular_repose(material: GranularMat) -> u16 {
    match material {
        GranularMat::GRANULAR_LIQUID_METAL => 0,
        GranularMat::GRANULAR_MUD => 1,
        GranularMat::GRANULAR_SAND | GranularMat::GRANULAR_CORRUPTION => 2,
        GranularMat::GRANULAR_SNOW => 3,
        GranularMat::GRANULAR_GRAVEL | GranularMat::GRANULAR_DIRT => 4,
        _ => u16::MAX,
    }
}

#[inline(always)]
pub fn get_terrain_repose(material: TerrainMat) -> u16 {
    match material {
        TerrainMat::TERRAIN_SANDSTONE => 2,
        TerrainMat::TERRAIN_ICE => 3,
        TerrainMat::TERRAIN_DIRT => 4,
        _ => u16::MAX,
    }
}

#[inline(always)]
pub fn is_combustible(material: SurfaceMat) -> bool {
    matches!(
        material,
        SurfaceMat::SURFACE_FOLIAGE
            | SurfaceMat::SURFACE_WOOD
            | SurfaceMat::SURFACE_FLESH
            | SurfaceMat::SURFACE_ROT
    )
}
