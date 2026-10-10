use bevy::math::IVec2;

use crate::core::{
    utils::spatial_hash_extended,
    world::{
        cell::{FluidMat, SurfaceMat, WorldCell},
        grid::CellGridReadView,
    },
};

/// Temperature at which water and ice trade places.
pub const FREEZING_POINT_C: f32 = 0.0;

/// Coolant volume boiled off per tick for every magma column sharing an edge.
const STEAM_LOSS_PER_CONTACT: u16 = 2;
/// Odds per tick, per degree below freezing, that a nucleated water column freezes.
const FREEZE_CHANCE_PER_DEGREE: f32 = 0.02;
/// Odds per tick, per degree above freezing, that an exposed ice column melts.
const MELT_CHANCE_PER_DEGREE: f32 = 0.002;
/// Upper bound on either phase-change chance, so even extreme temperatures advance a front
/// over several ticks instead of flashing a whole pool at once.
const MAX_PHASE_CHANCE: f32 = 0.5;
const CHANCE_RESOLUTION: u32 = 1 << 16;
/// Salts the per-cell hash so phase rolls are uncorrelated with the other passes' hashes.
const PHASE_HASH_CADENCE: u32 = 7;

const CONTACT_DIRS: [IVec2; 4] = [
    IVec2::new(0, 1),
    IVec2::new(1, 0),
    IVec2::new(0, -1),
    IVec2::new(-1, 0),
];

const NEIGHBOR_DIRS: [IVec2; 8] = [
    IVec2::new(0, 1),
    IVec2::new(1, 1),
    IVec2::new(1, 0),
    IVec2::new(1, -1),
    IVec2::new(0, -1),
    IVec2::new(-1, -1),
    IVec2::new(-1, 0),
    IVec2::new(-1, 1),
];

/// What a column's neighbours look like from the back buffer.
#[derive(Default)]
struct Surroundings {
    /// Edge-sharing neighbours holding magma or a coolant.
    magma_contacts: u16,
    coolant_contacts: u16,
    /// Any neighbour is molten or burning.
    heated: bool,
    /// Any neighbour is ice; freezing water grows from it.
    touches_ice: bool,
    /// Any neighbour is dry; freezing water nucleates along shorelines.
    touches_dry: bool,
    /// Any neighbour is not ice; melting ice gives way from such edges.
    touches_unfrozen: bool,
}

impl Surroundings {
    fn survey(view: CellGridReadView, world_pos: IVec2) -> Self {
        let mut surroundings = Self::default();
        for dir in NEIGHBOR_DIRS {
            let Some((_, neighbor)) = view.get_cell(world_pos + dir) else {
                continue;
            };
            let fluid = neighbor.fluid_mat();
            surroundings.heated |= fluid == FluidMat::FLUID_MAGMA
                || neighbor.surface_mat() == SurfaceMat::SURFACE_FIRE;
            surroundings.touches_ice |= fluid.is_solid();
            surroundings.touches_dry |= fluid == FluidMat::EMPTY;
            surroundings.touches_unfrozen |= !fluid.is_solid();

            if CONTACT_DIRS.contains(&dir) {
                if fluid == FluidMat::FLUID_MAGMA {
                    surroundings.magma_contacts += 1;
                } else if fluid.is_coolant() {
                    surroundings.coolant_contacts += 1;
                }
            }
        }
        surroundings
    }
}

/// Deterministic per-cell, per-tick roll; identical inputs always give the same outcome.
#[inline(always)]
fn rolls(chance: f32, world_pos: IVec2, tick: u32) -> bool {
    let threshold = (chance.clamp(0.0, MAX_PHASE_CHANCE) * CHANCE_RESOLUTION as f32) as u32;
    spatial_hash_extended(world_pos, tick, 0, PHASE_HASH_CADENCE) % CHANCE_RESOLUTION < threshold
}

/// Resolves heat exchange and phase changes for one column, returning `true` while a phase
/// change is still pending so the caller keeps the column awake until it happens.
///
/// Runs after the liquid pass and reads the post-flow column from `cell`, so whatever freezes,
/// melts or solidifies is exactly what is left after this tick's transfers. Neighbours are read
/// from the back buffer, which keeps reactions symmetric across an edge.
#[inline(always)]
pub fn step_thermal(
    cell: &mut WorldCell,
    view: CellGridReadView,
    world_pos: IVec2,
    tick: u32,
    ambient_temperature_c: f32,
) -> bool {
    let fluid = cell.fluid_mat();
    if fluid == FluidMat::EMPTY {
        return false;
    }
    let surroundings = Surroundings::survey(view, world_pos);

    if fluid == FluidMat::FLUID_MAGMA {
        if surroundings.coolant_contacts > 0 {
            cell.quench();
        }
        return false;
    }

    if fluid.is_solid() {
        if surroundings.heated {
            cell.thaw();
            return false;
        }
        let warmth = ambient_temperature_c - FREEZING_POINT_C;
        if warmth > 0.0 && surroundings.touches_unfrozen {
            if rolls(warmth * MELT_CHANCE_PER_DEGREE, world_pos, tick) {
                cell.thaw();
            }
            return true;
        }
        return false;
    }

    if fluid.is_coolant() && surroundings.magma_contacts > 0 {
        cell.drain_fluid(STEAM_LOSS_PER_CONTACT * surroundings.magma_contacts);
    }

    let chill = FREEZING_POINT_C - ambient_temperature_c;
    let nucleated = surroundings.touches_ice || surroundings.touches_dry;
    if chill > 0.0 && nucleated && !surroundings.heated && cell.fluid_mat().frozen().is_some() {
        if rolls(chill * FREEZE_CHANCE_PER_DEGREE, world_pos, tick) {
            cell.freeze();
        }
        return true;
    }
    false
}
