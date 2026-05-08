use bevy::math::IVec2;
use rand::{Rng, RngExt};

use crate::engine::{
    simulation::GridEvent,
    world::{
        cell::{FluidMat, SurfaceMat, WorldCell},
        grid::GridReadView,
    },
};

#[inline(always)]
pub fn step_fire<R: Rng + ?Sized>(
    cell: &mut WorldCell,
    old_cell: &WorldCell,
    view: GridReadView,
    world_pos: IVec2,
    rng: &mut R,
    _local_events: &mut Vec<GridEvent>,
) {
    let surface = old_cell.surface_mat();

    // Extinguish immediately if submerged in a non-combustible fluid (Water, Blood, Acid)
    let fluid = old_cell.fluid_mat();
    if fluid != FluidMat::EMPTY && fluid != FluidMat::FLUID_OIL {
        if surface == SurfaceMat::SURFACE_FIRE {
            cell.set_surface_mat(SurfaceMat::EMPTY);
            cell.set_surface_state(0);
        }
        return;
    }

    // Ignition & Spread Phase
    if is_combustible(surface) {
        let mut fire_neighbors = 0;
        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }

                if let Some((_, n_cell)) = view.get_cell(world_pos + IVec2::new(dx, dy)) {
                    if n_cell.surface_mat() == SurfaceMat::SURFACE_FIRE {
                        fire_neighbors += 1;
                    }
                }
            }
        }

        // Higher chance to catch fire if surrounded by more fire
        if fire_neighbors > 0 && rng.random_ratio(fire_neighbors as u32, 15) {
            cell.set_surface_mat(SurfaceMat::SURFACE_FIRE);
            cell.set_surface_state(0);
        }
    }
    // Active Burning Phase
    else if surface == SurfaceMat::SURFACE_FIRE {
        let state = old_cell.surface_state();

        // Active Burning Phase
        // TODO: change
        if state < 20 {
            cell.set_surface_state(state.saturating_add(1));
        } else {
            // Burn out: 50% chance to leave Ash, 50% chance to leave nothing
            if rng.random_ratio(1, 2) {
                cell.set_surface_mat(SurfaceMat::SURFACE_ASH);
            } else {
                cell.set_surface_mat(SurfaceMat::EMPTY);
            }
            cell.set_surface_state(0);
        }
    }
}

#[inline(always)]
fn is_combustible(mat: SurfaceMat) -> bool {
    matches!(
        mat,
        SurfaceMat::SURFACE_FOLIAGE
            | SurfaceMat::SURFACE_WOOD
            | SurfaceMat::SURFACE_FLESH
            | SurfaceMat::SURFACE_ROT
    )
}
