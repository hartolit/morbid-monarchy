//! Material identities stored in a cell's material fields, and their physical properties.
//!
//! Identifiers are persisted in saved chunks, so an existing value never changes meaning; new
//! materials take unassigned values. Properties are shared by the cell rules, the body physics
//! and (later) the GPU kernels, so each material is described once.

use bytemuck::{Pod, Zeroable};

use crate::cell::WorldCell;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
#[repr(transparent)]
pub struct TerrainMat(pub u8);

impl TerrainMat {
    // 4 bits (at most 15)
    pub const EMPTY: Self = Self(0);
    pub const TERRAIN_STONE: Self = Self(1);
    pub const TERRAIN_DIRT: Self = Self(2);
    pub const TERRAIN_SANDSTONE: Self = Self(3);
    pub const TERRAIN_DENSE_SNOW: Self = Self(4);
    pub const TERRAIN_METAL: Self = Self(5);
    pub const TERRAIN_CORRUPTION: Self = Self(6);

    /// Maps solid bedrock to the loose material it breaks down into when eroded.
    /// Inverse of [`GranularMat::to_terrain`], so erosion followed by compression round-trips.
    /// Returns `None` for materials that resist mechanical erosion.
    pub fn to_granular(self) -> Option<GranularMat> {
        match self {
            Self::TERRAIN_DIRT => Some(GranularMat::GRANULAR_DIRT),
            Self::TERRAIN_SANDSTONE => Some(GranularMat::GRANULAR_SAND),
            Self::TERRAIN_STONE => Some(GranularMat::GRANULAR_GRAVEL),
            Self::TERRAIN_DENSE_SNOW => Some(GranularMat::GRANULAR_SNOW),
            Self::TERRAIN_CORRUPTION => Some(GranularMat::GRANULAR_CORRUPTION),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
#[repr(transparent)]
pub struct SurfaceMat(pub u8);

impl SurfaceMat {
    // 4 bits (at most 15)
    pub const EMPTY: Self = Self(0);
    pub const SURFACE_FIRE: Self = Self(1);
    pub const SURFACE_FOLIAGE: Self = Self(2);
    pub const SURFACE_WOOD: Self = Self(3);
    pub const SURFACE_FLESH: Self = Self(4);
    pub const SURFACE_BONE: Self = Self(5);
    pub const SURFACE_ROT: Self = Self(6);
    pub const SURFACE_ASH: Self = Self(7);
    pub const SURFACE_SNOW: Self = Self(8);
    pub const SURFACE_CLAY: Self = Self(9);
    // 10 is unassigned: ice is a solid phase of the liquid layer (`FluidMat::FLUID_ICE`).
    pub const SURFACE_METAL: Self = Self(11);
    pub const SURFACE_GLASS: Self = Self(12);
    pub const SURFACE_CORRUPTION: Self = Self(13);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
#[repr(transparent)]
pub struct GranularMat(pub u8);

impl GranularMat {
    // 4 bits (at most 15)
    pub const EMPTY: Self = Self(0);
    pub const GRANULAR_DIRT: Self = Self(1);
    pub const GRANULAR_SAND: Self = Self(2);
    pub const GRANULAR_MUD: Self = Self(3);
    pub const GRANULAR_GRAVEL: Self = Self(4);
    pub const GRANULAR_SNOW: Self = Self(5);
    pub const GRANULAR_LIQUID_METAL: Self = Self(6);
    pub const GRANULAR_CORRUPTION: Self = Self(7);

    /// Geologically maps a loose granular material to its solid bedrock equivalent under high compression.
    pub fn to_terrain(self) -> TerrainMat {
        match self {
            Self::GRANULAR_SAND => TerrainMat::TERRAIN_SANDSTONE,
            Self::GRANULAR_SNOW => TerrainMat::TERRAIN_DENSE_SNOW,
            Self::GRANULAR_DIRT | Self::GRANULAR_MUD => TerrainMat::TERRAIN_DIRT,
            Self::GRANULAR_GRAVEL | Self::GRANULAR_LIQUID_METAL => TerrainMat::TERRAIN_STONE,
            Self::GRANULAR_CORRUPTION => TerrainMat::TERRAIN_CORRUPTION,
            _ => TerrainMat::TERRAIN_STONE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
#[repr(transparent)]
pub struct FluidMat(pub u8);

impl FluidMat {
    // 3 bits (at most 7)
    pub const EMPTY: Self = Self(0);
    pub const FLUID_WATER: Self = Self(1);
    pub const FLUID_MAGMA: Self = Self(2);
    pub const FLUID_BLOOD: Self = Self(3);
    pub const FLUID_ACID: Self = Self(4);
    pub const FLUID_OIL: Self = Self(5);
    pub const FLUID_CORRUPTION: Self = Self(6);
    pub const FLUID_ICE: Self = Self(7);

    /// Frozen liquids are solid: they never flow, nothing flows into them, and bodies and
    /// loose material rest on them. Freezing and melting are plain material swaps, so a column
    /// keeps its volume through every phase change.
    #[inline(always)]
    pub const fn is_solid(self) -> bool {
        self.0 == Self::FLUID_ICE.0
    }

    /// Water-like liquids (and ice) that smother flames and quench magma into rock.
    #[inline(always)]
    pub const fn is_coolant(self) -> bool {
        matches!(
            self,
            Self::FLUID_WATER
                | Self::FLUID_BLOOD
                | Self::FLUID_ACID
                | Self::FLUID_CORRUPTION
                | Self::FLUID_ICE
        )
    }

    /// The solid this liquid freezes into, if it freezes.
    #[inline(always)]
    pub const fn frozen(self) -> Option<Self> {
        match self {
            Self::FLUID_WATER => Some(Self::FLUID_ICE),
            _ => None,
        }
    }

    /// The liquid this solid melts into, if it melts.
    #[inline(always)]
    pub const fn melted(self) -> Option<Self> {
        match self {
            Self::FLUID_ICE => Some(Self::FLUID_WATER),
            _ => None,
        }
    }
}

/// Bits of the cell's compass field: the direction a liquid last moved, which biases its next
/// move so flows keep momentum.
pub struct CompassFlags;

impl CompassFlags {
    pub const FACING_N: u8 = 1 << 0;
    pub const FACING_S: u8 = 1 << 1;
    pub const FACING_E: u8 = 1 << 2;
    pub const FACING_W: u8 = 1 << 3;
}

/// Steepest stable slope of loose material, in elevation units of height difference between
/// neighbouring cells. `u16::MAX` means the material does not avalanche.
#[inline(always)]
pub fn granular_repose(material: GranularMat) -> u16 {
    match material {
        GranularMat::GRANULAR_LIQUID_METAL => 0,
        GranularMat::GRANULAR_MUD => 1,
        GranularMat::GRANULAR_SAND | GranularMat::GRANULAR_CORRUPTION => 2,
        GranularMat::GRANULAR_SNOW => 3,
        GranularMat::GRANULAR_GRAVEL | GranularMat::GRANULAR_DIRT => 4,
        _ => u16::MAX,
    }
}

/// Steepest bedrock slope that holds, in elevation units between neighbours; steeper erodes.
#[inline(always)]
pub fn terrain_repose(material: TerrainMat) -> u16 {
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
pub fn terrain_crush_hardness(material: TerrainMat) -> f32 {
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
pub fn ground_softness(cell: &WorldCell) -> f32 {
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
