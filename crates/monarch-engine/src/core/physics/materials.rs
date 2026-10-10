use crate::core::world::cell::{FluidMat, GranularMat, SurfaceMat, TerrainMat, WorldCell};

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
        TerrainMat::TERRAIN_DENSE_SNOW => 3,
        TerrainMat::TERRAIN_DIRT => 4,
        _ => u16::MAX,
    }
}

/// Energy multiplier for crushing one elevation unit of bedrock, relative to packed dirt.
/// Rigid bodies plough through soil but mostly roll over rock.
#[inline(always)]
pub fn get_terrain_crush_hardness(material: TerrainMat) -> f32 {
    match material {
        TerrainMat::TERRAIN_DENSE_SNOW => 0.5,
        TerrainMat::TERRAIN_SANDSTONE | TerrainMat::TERRAIN_CORRUPTION => 1.5,
        TerrainMat::TERRAIN_STONE => 3.0,
        TerrainMat::TERRAIN_METAL => 12.0,
        _ => 1.0,
    }
}

#[inline(always)]
pub fn is_combustible(surface: SurfaceMat, fluid: FluidMat) -> bool {
    matches!(
        surface,
        SurfaceMat::SURFACE_FOLIAGE
            | SurfaceMat::SURFACE_WOOD
            | SurfaceMat::SURFACE_FLESH
            | SurfaceMat::SURFACE_ROT
    ) || fluid == FluidMat::FLUID_OIL
}

/// Bulk physical response of a liquid, shared by the cellular solver and rigid bodies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidProperties {
    /// Density relative to water. Bodies with a lower relative density float.
    pub relative_density: f32,
    /// Exponential velocity decay rate (1/s) applied to a fully submerged body.
    pub drag_rate: f32,
    /// Divides the head difference into the volume moved per tick. A divisor of 2 equalises
    /// two columns in one step; larger divisors flow slower and, because transfers below one
    /// unit round to zero, also act as a yield stress that lets viscous fronts hold a slope.
    pub flow_divisor: u32,
    /// Whether head differences too small to divide still trickle one unit at a time until the
    /// surface is level. Viscous melts and sludges keep their slope instead.
    pub levels_out: bool,
    /// Multiplier on incoming flux when rolling for bed erosion. Zero disables scouring.
    pub erosion_factor: u32,
}

const NO_FLUID: FluidProperties = FluidProperties {
    relative_density: 0.0,
    drag_rate: 0.0,
    flow_divisor: 2,
    levels_out: false,
    erosion_factor: 0,
};

#[inline(always)]
pub const fn fluid_properties(material: FluidMat) -> FluidProperties {
    match material {
        FluidMat::FLUID_WATER => FluidProperties {
            relative_density: 1.0,
            drag_rate: 1.5,
            flow_divisor: 2,
            levels_out: true,
            erosion_factor: 1,
        },
        FluidMat::FLUID_MAGMA => FluidProperties {
            relative_density: 3.1,
            drag_rate: 8.0,
            flow_divisor: 3,
            levels_out: false,
            erosion_factor: 0,
        },
        FluidMat::FLUID_BLOOD => FluidProperties {
            relative_density: 1.06,
            drag_rate: 2.0,
            flow_divisor: 2,
            levels_out: true,
            erosion_factor: 1,
        },
        FluidMat::FLUID_ACID => FluidProperties {
            relative_density: 1.2,
            drag_rate: 1.8,
            flow_divisor: 2,
            levels_out: true,
            erosion_factor: 4,
        },
        FluidMat::FLUID_OIL => FluidProperties {
            relative_density: 0.9,
            drag_rate: 3.0,
            flow_divisor: 3,
            levels_out: true,
            erosion_factor: 0,
        },
        FluidMat::FLUID_CORRUPTION => FluidProperties {
            relative_density: 1.5,
            drag_rate: 4.0,
            flow_divisor: 4,
            levels_out: false,
            erosion_factor: 2,
        },
        // Ice never flows; its density only matters once it melts back into water.
        FluidMat::FLUID_ICE => FluidProperties {
            relative_density: 0.92,
            drag_rate: 0.0,
            flow_divisor: 2,
            levels_out: false,
            erosion_factor: 0,
        },
        _ => NO_FLUID,
    }
}

/// How far a body sinks into the top of a column, in world units of sinkage for the reference
/// load (see `DeformationProfile::rut_depth_scale`). Loose material is softer than the bedrock
/// it weathers from; rock, metal and ice do not yield to rolling at all.
#[inline(always)]
pub fn get_ground_softness(cell: &WorldCell) -> f32 {
    if cell.fluid_mat().is_solid() {
        return 0.0;
    }
    if cell.granular_vol() > 0 {
        return match cell.granular_mat() {
            GranularMat::GRANULAR_SAND => 1.5,
            GranularMat::GRANULAR_MUD | GranularMat::GRANULAR_SNOW => 2.0,
            GranularMat::GRANULAR_DIRT | GranularMat::GRANULAR_CORRUPTION => 1.0,
            GranularMat::GRANULAR_GRAVEL => 0.5,
            _ => 0.0,
        };
    }
    match cell.terrain_mat() {
        TerrainMat::TERRAIN_DIRT => 1.0,
        TerrainMat::TERRAIN_DENSE_SNOW => 1.5,
        TerrainMat::TERRAIN_CORRUPTION => 0.5,
        TerrainMat::TERRAIN_SANDSTONE => 0.25,
        _ => 0.0,
    }
}
