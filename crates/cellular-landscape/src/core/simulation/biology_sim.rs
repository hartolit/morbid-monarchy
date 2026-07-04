use bevy::math::IVec2;
use rand::{Rng, RngExt};

use crate::core::{
    simulation::GridEvent,
    world::{
        cell::{FluidMat, GranularMat, SurfaceMat, TerrainMat, WorldCell},
        grid::CellGridReadView,
    },
};

// The absolute physical limit that water can be wicked upwards by roots.
const MAX_CAPILLARY_LIFT: u32 = 2;

#[inline(always)]
pub fn step_biology<R: Rng + ?Sized>(
    cell: &mut WorldCell,
    old_cell: &WorldCell,
    view: CellGridReadView,
    world_pos: IVec2,
    rng: &mut R,
    _local_events: &mut Vec<GridEvent>,
) {
    let surface = old_cell.surface_mat();
    let fluid = old_cell.fluid_mat();

    if surface != SurfaceMat::EMPTY && surface != SurfaceMat::SURFACE_FOLIAGE {
        return;
    }

    let is_fertile_ground = old_cell.terrain_mat() == TerrainMat::TERRAIN_DIRT
        || old_cell.granular_mat() == GranularMat::GRANULAR_DIRT
        || old_cell.granular_mat() == GranularMat::GRANULAR_MUD;

    if !is_fertile_ground && surface == SurfaceMat::EMPTY {
        return;
    }

    let my_crust_height = old_cell.elevation() as u32 + old_cell.granular_vol() as u32;

    let mut is_hydrated =
        old_cell.granular_mat() == GranularMat::GRANULAR_MUD || fluid == FluidMat::FLUID_WATER;
    let is_drowning = fluid != FluidMat::EMPTY && old_cell.fluid_vol() > 15;

    let mut fertile_neighbors = 0;

    let search_dirs = [
        IVec2::new(0, 1),
        IVec2::new(1, 0),
        IVec2::new(0, -1),
        IVec2::new(-1, 0),
    ];

    for &dir in &search_dirs {
        let n_pos = world_pos + dir;

        if let Some((_, n_cell)) = view.get_cell(n_pos) {
            let n_crust_height = n_cell.elevation() as u32 + n_cell.granular_vol() as u32;
            let vertical_delta = my_crust_height.abs_diff(n_crust_height);

            if surface == SurfaceMat::EMPTY
                && n_cell.surface_mat() == SurfaceMat::SURFACE_FOLIAGE
                && n_cell.surface_state() >= 2
                && vertical_delta <= 1
            {
                fertile_neighbors += 1;
            }

            if !is_hydrated && vertical_delta <= MAX_CAPILLARY_LIFT {
                if n_cell.fluid_mat() == FluidMat::FLUID_WATER
                    || n_cell.fluid_mat() == FluidMat::FLUID_BLOOD
                {
                    is_hydrated = true;
                }
            }
        }
    }

    if surface == SurfaceMat::EMPTY {
        if is_fertile_ground && is_hydrated && !is_drowning {
            if (fertile_neighbors > 0 && rng.random_ratio(1, 12)) || rng.random_ratio(1, 5000) {
                cell.set_surface_mat(SurfaceMat::SURFACE_FOLIAGE);
                cell.set_surface_state(1);
            }
        }
    } else if surface == SurfaceMat::SURFACE_FOLIAGE {
        let state = old_cell.surface_state();

        if is_drowning || !is_hydrated {
            if state <= 1 {
                cell.set_surface_mat(SurfaceMat::EMPTY);
                cell.set_surface_state(0);
            } else {
                cell.set_surface_state(state - 1);
            }
        } else {
            if state < 10 && rng.random_ratio(1, 6) {
                cell.set_surface_state(state + 1);
            }
        }
    }
}
