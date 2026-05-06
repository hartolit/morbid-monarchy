use bevy::math::IVec2;
use rand::{Rng, RngExt};

use crate::engine::{
    simulation::GridEvent,
    world::{
        cell::{FluidMat, SurfaceMat, TerrainMat, WorldCell},
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
    local_events: &mut Vec<GridEvent>,
) {
    if old_cell.fluid_mat() != FluidMat::EMPTY || old_cell.surface_mat() != SurfaceMat::EMPTY {
        return;
    }

    let terrain = old_cell.terrain_mat();
    if terrain != TerrainMat::SAND && terrain != TerrainMat::FOLIAGE {
        return;
    }

    let mut wave_front_neighbors = 0;
    for dy in -1..=1 {
        for dx in -1..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let n_pos = world_pos + IVec2::new(dx, dy);

            if let Some((_, n_cell)) = view.get_cell(n_pos) {
                if n_cell.terrain_mat() == TerrainMat::FOLIAGE && n_cell.terrain_state() < 2 {
                    wave_front_neighbors += 1;
                }
            }
        }
    }

    if terrain == TerrainMat::SAND {
        if wave_front_neighbors > 0 || rng.random_ratio(1, 1000) {
            cell.set_terrain_mat(TerrainMat::FOLIAGE);
            cell.set_terrain_state(0);
        }
    } else if terrain == TerrainMat::FOLIAGE {
        let state = old_cell.terrain_state();
        if state < 10 {
            cell.set_terrain_state(state + 1);
        } else {
            cell.set_terrain_mat(TerrainMat::SAND);
            cell.set_terrain_state(0);

            local_events.push(GridEvent::SpawnTerrainParticle {
                pos: world_pos,
                material: TerrainMat::SAND,
            });
        }
    }
}
