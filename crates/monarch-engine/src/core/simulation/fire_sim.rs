use bevy::math::IVec2;
use rand::{Rng, RngExt};

use crate::core::{simulation::GridEvent, world::grid::CellGridReadView};
use world_model::{
    cell::WorldCell,
    material::{FluidMat, SurfaceMat, is_combustible},
    rules::{ASH_RESIDUE_ODDS, FLAME_LIFETIME_TICKS, IGNITION_ODDS_DENOMINATOR, OIL_BURN_RATE},
};

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
