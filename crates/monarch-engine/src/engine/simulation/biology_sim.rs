use bevy::math::IVec2;
use flume::Sender;
use rand::{Rng, RngExt};

use crate::engine::{
    simulation::GridEvent,
    world::cell::{MaterialId, WorldCell},
};

#[inline(always)]
pub fn step_biology<R: Rng + ?Sized>(
    cell: &mut WorldCell,
    old_cell: &WorldCell,
    read_buffer: &[WorldCell],
    width: i32,
    height: i32,
    world_pos: IVec2,
    window_origin: IVec2,
    buffer_head: IVec2,
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

            let nx = world_pos.x + dx;
            let ny = world_pos.y + dy;

            let lx = nx - window_origin.x;
            let ly = ny - window_origin.y;

            if lx >= 0 && lx < width && ly >= 0 && ly < height {
                let bx = (lx + buffer_head.x).rem_euclid(width);
                let by = (ly + buffer_head.y).rem_euclid(height);
                let n_idx = (by * width + bx) as usize;

                let n_cell = &read_buffer[n_idx];
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
