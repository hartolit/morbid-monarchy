use bevy::math::IVec2;
use flume::Sender;
use rand::{Rng, RngExt};

use crate::engine::{
    simulation::GridEvent,
    world::{
        cell::{MaterialId, WorldCell},
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
    tx: &mut Sender<GridEvent>,
) {
    if old_cell.fluid.material != MaterialId::EMPTY
        || old_cell.surface.material != MaterialId::EMPTY
    {
        return;
    }

    let terrain = old_cell.terrain;
    if terrain.material != MaterialId::LOOSE_SAND && terrain.material != MaterialId::ORGANIC_FOLIAGE
    {
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
                if n_cell.terrain.material == MaterialId::ORGANIC_FOLIAGE
                    && n_cell.terrain.state < 2
                {
                    wave_front_neighbors += 1;
                }
            }
        }
    }

    if terrain.material == MaterialId::LOOSE_SAND {
        if wave_front_neighbors > 0 {
            cell.terrain.material = MaterialId::ORGANIC_FOLIAGE;
            cell.terrain.state = 0;
        } else if rng.random_ratio(1, 1000) {
            cell.terrain.material = MaterialId::ORGANIC_FOLIAGE;
            cell.terrain.state = 0;
        }
    } else if terrain.material == MaterialId::ORGANIC_FOLIAGE {
        if terrain.state < 10 {
            cell.terrain.state += 1;
        } else {
            cell.terrain.material = MaterialId::LOOSE_SAND;
            cell.terrain.state = 0;

            let _ = tx.send(GridEvent::SpawnParticle {
                pos: world_pos,
                material: MaterialId::LOOSE_SAND,
            });
        }
    }
}
