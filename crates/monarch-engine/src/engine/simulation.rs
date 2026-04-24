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

use crate::prelude::{ActiveWorldGrid, MaterialId};

pub enum GridEvent {
    SpawnParticle { pos: IVec2, material: MaterialId },
    ApplyDamage { pos: IVec2, amount: u32 },
    PlaySound { pos: IVec2, sound_id: u8 },
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
    let width = grid_res.width;
    let height = grid_res.height;

    grid_res.swap_buffers();
    let grid_mut = grid_res.into_inner();

    let tick = grid_mut.tick;
    let read_buffer = &grid_mut.back_buffer;
    let global_tx = event_queue.tx.clone();

    let run_liquid = config.run_liquid;
    let run_biology = config.run_biology;

    grid_mut.cells.par_iter_mut().enumerate().for_each_init(
        || (global_tx.clone(), SmallRng::from_rng(&mut rand::rng())),
        |(tx, rng), (idx, cell)| {
            let pos = ActiveWorldGrid::index_to_pos(idx, width);
            let old_cell = &read_buffer[idx];

            // Lock-Free Wake Propagation Check
            let mut should_simulate = old_cell.is_awake();

            // If we are asleep, check if any 8 neighbors are awake. If they are,
            // they might flow into us or interact with us, so we MUST wake up to receive them.
            if !should_simulate {
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let nx = (pos.x + dx).rem_euclid(width);
                        let ny = (pos.y + dy).rem_euclid(height);
                        let n_idx = (ny * width + nx) as usize;

                        if read_buffer[n_idx].is_awake() {
                            should_simulate = true;
                            break;
                        }
                    }
                    if should_simulate {
                        break;
                    }
                }
            }

            // If absolute dead space, branch predictor skips everything.
            // The `cell` is already an exact copy of `old_cell` due to swap_buffers().
            if !should_simulate {
                return;
            }

            // Default to sleeping next frame unless a system mutates us
            cell.sleep();

            // Deterministic simulation step
            if run_liquid && tick % 2 == 0 {
                liquid_sim::step_liquid(cell, old_cell, read_buffer, width, height, pos, tick);
            }

            // Non-deterministic simulation step
            if run_biology && rng.random_ratio(1, 10) {
                biology_sim::step_biology(cell, old_cell, read_buffer, width, height, rng, tx, pos);
            }

            // If the cell's memory footprint changed AT ALL during the step, it wakes up.
            if cell != old_cell {
                cell.wake();
            } else if cell.terrain.material == MaterialId::ORGANIC_FOLIAGE
                && cell.terrain.state < 10
            {
                // Special case: Stochastic processes (like plant growth) that failed
                // their RNG roll this frame must stay awake to try again next frame.
                cell.wake();
            }
        },
    );

    grid_mut.cells_dirty = true;
}
