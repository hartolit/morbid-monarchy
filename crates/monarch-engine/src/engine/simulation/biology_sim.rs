use bevy::math::IVec2;
use rand::{Rng, RngExt};

use crate::engine::{
    simulation::GridEvent,
    world::{
        cell::{FluidMat, GranularMat, SurfaceMat, TerrainMat, WorldCell},
        grid::GridReadView,
    },
};

#[inline(always)]
pub fn step_biology<R: Rng + ?Sized>(
    cell: &mut WorldCell,
    old_cell: &WorldCell,
    view: GridReadView,
    world_pos: IVec2,
    rng: &mut R,
    _local_events: &mut Vec<GridEvent>,
) {
    // Foliage cannot grow underwater or in magma
    if old_cell.fluid_mat() != FluidMat::EMPTY {
        return;
    }

    let surface = old_cell.surface_mat();

    // Skip if the surface is occupied by an inorganic element (Fire, Stone Wall, etc.)
    if surface != SurfaceMat::EMPTY && surface != SurfaceMat::SURFACE_FOLIAGE {
        return;
    }

    // Determine if the structural substrate can support plant life
    let is_fertile_ground = old_cell.terrain_mat() == TerrainMat::TERRAIN_DIRT
        || old_cell.granular_mat() == GranularMat::GRANULAR_DIRT
        || old_cell.granular_mat() == GranularMat::GRANULAR_MUD;

    if !is_fertile_ground && surface == SurfaceMat::EMPTY {
        return;
    }

    // Scan neighbors for biological wavefronts
    let mut fertile_neighbors = 0;
    for dy in -1..=1 {
        for dx in -1..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let n_pos = world_pos + IVec2::new(dx, dy);

            if let Some((_, n_cell)) = view.get_cell(n_pos) {
                if n_cell.surface_mat() == SurfaceMat::SURFACE_FOLIAGE
                    && n_cell.surface_state() >= 2
                {
                    fertile_neighbors += 1;
                }
            }
        }
    }

    if surface == SurfaceMat::EMPTY {
        // Sprouting Phase: Seed from nearby neighbors or rare spontaneous generation
        if fertile_neighbors > 0 || rng.random_ratio(1, 1000) {
            cell.set_surface_mat(SurfaceMat::SURFACE_FOLIAGE);
            cell.set_surface_state(0);
        }
    } else if surface == SurfaceMat::SURFACE_FOLIAGE {
        // Lifecycle Phase: Advance the biological clock
        let state = old_cell.surface_state();
        if state < 10 {
            cell.set_surface_state(state + 1);
        } else {
            // Decay Phase: The plant dies off.
            // Emergent physics: Dead matter drops 1 volume of Granular Dirt,
            // physically raising the terrain over centuries of biological cycles.
            cell.set_surface_mat(SurfaceMat::EMPTY);
            cell.set_surface_state(0);

            // Only drop dirt if there's room in the volume limit
            if old_cell.granular_vol() < WorldCell::MAX_GRANULAR_VOL {
                if old_cell.granular_mat() == GranularMat::EMPTY
                    || old_cell.granular_mat() == GranularMat::GRANULAR_DIRT
                {
                    cell.set_granular_mat(GranularMat::GRANULAR_DIRT);
                    cell.set_granular_vol(old_cell.granular_vol().saturating_add(1));
                }
            }
        }
    }
}
