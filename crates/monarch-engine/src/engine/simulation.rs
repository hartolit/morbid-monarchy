mod biology_sim;
pub mod liquid_sim;

use bevy::{
    ecs::{
        resource::Resource,
        system::{Res, ResMut},
    },
    math::IVec2,
};
use flume::{Receiver, Sender};
use rand::{RngExt, SeedableRng, rngs::SmallRng};
use rayon::iter::{IndexedParallelIterator, IntoParallelRefMutIterator, ParallelIterator};
use std::sync::atomic::Ordering;

use crate::engine::world::cell::{FluidMat, TerrainMat};
use crate::prelude::{ActiveWorldGrid, GridReadView};

pub enum GridEvent {
    SpawnTerrainParticle {
        pos: IVec2,
        material: crate::engine::world::cell::TerrainMat,
    },
    SpawnFluidParticle {
        pos: IVec2,
        material: crate::engine::world::cell::FluidMat,
    },
    ApplyDamage {
        pos: IVec2,
        amount: u32,
    },
    PlaySound {
        pos: IVec2,
        sound_id: u8,
    },
}

#[derive(Resource)]
pub struct SimulationConfig {
    pub run_liquid: bool,
    pub run_biology: bool,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            run_liquid: true,
            run_biology: true,
        }
    }
}

#[derive(Resource)]
pub struct SimulationEventQueue {
    pub tx: Sender<GridEvent>,
    pub rx: Receiver<GridEvent>,
}

impl Default for SimulationEventQueue {
    fn default() -> Self {
        let (tx, rx) = flume::unbounded();
        Self { tx, rx }
    }
}

pub fn simulate_world(
    mut grid_res: ResMut<ActiveWorldGrid>,
    event_queue: Res<SimulationEventQueue>,
    config: Res<SimulationConfig>,
) {
    grid_res.swap_buffers();
    let grid_mut = grid_res.into_inner();

    let width = grid_mut.width;
    let height = grid_mut.height;
    let tick = grid_mut.tick;
    let global_tx = event_queue.tx.clone();

    let run_liquid = config.run_liquid;
    let run_biology = config.run_biology;

    let view = GridReadView {
        cells: &grid_mut.back_buffer,
        width,
        height,
        window_origin: grid_mut.window_origin,
        buffer_head: grid_mut.buffer_head,
    };

    let wake_buf = &grid_mut.wake_buffer;
    let next_wake_buf = &grid_mut.next_wake_buffer;

    // Rayon fold creates thread-local variables. Zero lock contention on the global channel.
    let events: Vec<GridEvent> = grid_mut
        .cells
        .par_iter_mut()
        .enumerate()
        .fold(
            || (Vec::new(), SmallRng::from_rng(&mut rand::rng())),
            |(mut local_events, mut rng), (idx, cell)| {
                // Lock-Free O(1) Asleep Elimination
                if wake_buf[idx].load(Ordering::Relaxed) == 0 {
                    return (local_events, rng);
                }

                let buffer_pos = ActiveWorldGrid::index_to_pos(idx, width);
                let local_pos = IVec2::new(
                    (buffer_pos.x - view.buffer_head.x).rem_euclid(width),
                    (buffer_pos.y - view.buffer_head.y).rem_euclid(height),
                );

                let world_pos = local_pos + view.window_origin;
                let old_cell = &view.cells[idx];

                if run_liquid && tick % 2 == 0 {
                    liquid_sim::step_liquid(cell, old_cell, view, world_pos, tick);
                }

                if run_biology && rng.random_ratio(1, 10) {
                    biology_sim::step_biology(
                        cell,
                        old_cell,
                        view,
                        world_pos,
                        &mut rng,
                        &mut local_events,
                    );
                }

                let mut changed = cell.0 != old_cell.0;

                if !changed
                    && cell.terrain_mat() == TerrainMat::FOLIAGE
                    && cell.terrain_state() < 10
                {
                    changed = true;
                }

                if changed {
                    // The cell changed, wake it and its neighbors for the next frame
                    next_wake_buf[idx].store(1, Ordering::Relaxed);
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            if dx == 0 && dy == 0 {
                                continue;
                            }

                            if let Some(n_idx) = view.get_index(world_pos + IVec2::new(dx, dy)) {
                                next_wake_buf[n_idx].store(1, Ordering::Relaxed);
                            }
                        }
                    }
                } else if run_liquid && tick % 2 != 0 {
                    // We wake the cell on odd ticks to keep it alive for the next even tick.
                    next_wake_buf[idx].store(1, Ordering::Relaxed);
                }

                (local_events, rng)
            },
        )
        .map(|(events, _)| events)
        .reduce(
            || Vec::new(),
            |mut a, mut b| {
                a.append(&mut b);
                a
            },
        );

    // Bulk flush the channel in the main thread
    for ev in events {
        let _ = global_tx.send(ev);
    }

    grid_mut.cells_dirty = true;
}
