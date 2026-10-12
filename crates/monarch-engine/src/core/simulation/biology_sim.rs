use bevy::math::IVec2;
use rand::{Rng, RngExt};

use crate::core::{
    simulation::{FOLIAGE_MATURE_STATE, GridEvent},
    world::grid::CellGridReadView,
};
use world_model::{
    cell::WorldCell,
    material::{FluidMat, GranularMat, SurfaceMat, TerrainMat},
};

// The absolute physical limit that water can be wicked upwards by roots.
const MAX_CAPILLARY_LIFT: u32 = 2;
// Water a growing plant draws from its own column per growth step.
const FOLIAGE_WATER_USE: u16 = 2;

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

    if surface != SurfaceMat::EMPTY
        && surface != SurfaceMat::SURFACE_FOLIAGE
        && surface != SurfaceMat::SURFACE_ASH
    {
        return;
    }

    let is_fertile_ground = old_cell.terrain_mat() == TerrainMat::TERRAIN_DIRT
        || old_cell.granular_mat() == GranularMat::GRANULAR_DIRT
        || old_cell.granular_mat() == GranularMat::GRANULAR_MUD;

    if !is_fertile_ground && (surface == SurfaceMat::EMPTY || surface == SurfaceMat::SURFACE_ASH) {
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

            if (surface == SurfaceMat::EMPTY || surface == SurfaceMat::SURFACE_ASH)
                && n_cell.surface_mat() == SurfaceMat::SURFACE_FOLIAGE
                && n_cell.surface_state() >= 2
                && vertical_delta <= 1
            {
                fertile_neighbors += 1;
            }

            if !is_hydrated
                && vertical_delta <= MAX_CAPILLARY_LIFT
                && (n_cell.fluid_mat() == FluidMat::FLUID_WATER
                    || n_cell.fluid_mat() == FluidMat::FLUID_BLOOD)
            {
                is_hydrated = true;
            }
        }
    }

    if surface == SurfaceMat::EMPTY || surface == SurfaceMat::SURFACE_ASH {
        if is_fertile_ground && is_hydrated && !is_drowning {
            let has_ash = surface == SurfaceMat::SURFACE_ASH;

            // Ash acts as fertilizer, significantly boosting the probability of grass taking root
            let neighbor_chance = if has_ash { 1 } else { 3 };
            let spont_chance = if has_ash { 150 } else { 800 };

            if (fertile_neighbors > 0 && rng.random_ratio(1, neighbor_chance))
                || rng.random_ratio(1, spont_chance)
            {
                cell.set_surface_mat(SurfaceMat::SURFACE_FOLIAGE);
                cell.set_surface_state(1); // Overwrites the ash or empty space
            }
        }
    } else if surface == SurfaceMat::SURFACE_FOLIAGE {
        let state = old_cell.surface_state();

        if is_hydrated && !is_drowning && state < FOLIAGE_MATURE_STATE {
            // Grow aggressively if requirements are met
            if rng.random_ratio(1, 3) {
                cell.set_surface_state(state + 1);
            }

            // Consume water locally to create a sink which naturally draws in from surrounding cells
            let current_fluid = cell.fluid_mat();
            if current_fluid == FluidMat::FLUID_WATER || current_fluid == FluidMat::FLUID_BLOOD {
                cell.drain_fluid(FOLIAGE_WATER_USE);
            } else if cell.granular_mat() == GranularMat::GRANULAR_MUD {
                // Alternatively, dry out local mud
                if rng.random_ratio(1, 10) {
                    cell.set_granular_mat(GranularMat::GRANULAR_DIRT);
                }
            }
        }
    }
}
