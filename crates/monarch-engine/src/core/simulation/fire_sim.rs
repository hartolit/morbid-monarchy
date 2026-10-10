use bevy::math::IVec2;
use rand::{Rng, RngExt};

use crate::core::{
    physics::materials::is_combustible,
    simulation::GridEvent,
    world::{
        cell::{FluidMat, SurfaceMat, WorldCell},
        grid::CellGridReadView,
    },
};

/// Burn ticks a flame survives once it has no liquid fuel left.
const FLAME_LIFETIME_TICKS: u16 = 20;
/// Oil volume consumed per burning tick. The flame is held alive until the pool is gone.
const OIL_BURN_RATE: u16 = 1;
/// Each adjacent heat source adds one chance in this many to ignite per evaluated tick.
const IGNITION_ODDS_DENOMINATOR: u32 = 15;
/// A burnt-out flame leaves ash with odds of one in this many.
const ASH_RESIDUE_ODDS: u32 = 2;

#[inline(always)]
fn is_heat_source(cell: &WorldCell) -> bool {
    cell.surface_mat() == SurfaceMat::SURFACE_FIRE || cell.fluid_mat() == FluidMat::FLUID_MAGMA
}

#[inline(always)]
pub fn step_fire<R: Rng + ?Sized>(
    cell: &mut WorldCell,
    old_cell: &WorldCell,
    view: CellGridReadView,
    world_pos: IVec2,
    rng: &mut R,
    _local_events: &mut Vec<GridEvent>,
) {
    let surface = old_cell.surface_mat();
    let fluid = old_cell.fluid_mat();

    if fluid.is_coolant() {
        if surface == SurfaceMat::SURFACE_FIRE {
            cell.set_surface_mat(SurfaceMat::EMPTY);
            cell.set_surface_state(0);
        }
        return;
    }

    // The burning branch must run before the ignition test: an oil pool is always
    // combustible, so testing ignition first would reset a lit pool every tick and it would
    // never consume its fuel or burn out.
    if surface == SurfaceMat::SURFACE_FIRE {
        // Read the post-flow column so fuel that drained away this tick is not burned twice.
        if cell.fluid_mat() == FluidMat::FLUID_OIL {
            cell.drain_fluid(OIL_BURN_RATE);
            return;
        }

        let state = old_cell.surface_state();
        if state < FLAME_LIFETIME_TICKS {
            cell.set_surface_state(state + 1);
        } else {
            if rng.random_ratio(1, ASH_RESIDUE_ODDS) {
                cell.set_surface_mat(SurfaceMat::SURFACE_ASH);
            } else {
                cell.set_surface_mat(SurfaceMat::EMPTY);
            }
            cell.set_surface_state(0);
        }
        return;
    }

    if !is_combustible(surface, fluid) {
        return;
    }

    // Fuel sitting in a magma column ignites on contact.
    let mut heat_sources = if fluid == FluidMat::FLUID_MAGMA {
        IGNITION_ODDS_DENOMINATOR
    } else {
        0
    };

    for dy in -1..=1 {
        for dx in -1..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }

            if let Some((_, n_cell)) = view.get_cell(world_pos + IVec2::new(dx, dy))
                && is_heat_source(n_cell)
            {
                heat_sources += 1;
            }
        }
    }

    if heat_sources > 0
        && rng.random_ratio(
            heat_sources.min(IGNITION_ODDS_DENOMINATOR),
            IGNITION_ODDS_DENOMINATOR,
        )
    {
        cell.set_surface_mat(SurfaceMat::SURFACE_FIRE);
        cell.set_surface_state(0);
    }
}
