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
    let buffer_head = grid_res.buffer_head;
    let window_origin = grid_res.window_origin;

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
            let buffer_pos = ActiveWorldGrid::index_to_pos(idx, width);

            // Map the physical memory buffer coordinate to the logical screen coordinate
            let local_pos = IVec2::new(
                (buffer_pos.x - buffer_head.x).rem_euclid(width),
                (buffer_pos.y - buffer_head.y).rem_euclid(height),
            );

            // Map the screen coordinate to absolute global world space
            let world_pos = local_pos + window_origin;

            let old_cell = &read_buffer[idx];

            // Lock-Free Wake Propagation Check
            let mut should_simulate = old_cell.is_awake();

            // If we are asleep, check if any 8 neighbors are awake.
            // We MUST respect the Active Window bounds here, not wrap the Toroidal array.
            if !should_simulate {
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let lx = local_pos.x + dx;
                        let ly = local_pos.y + dy;

                        if lx >= 0 && lx < width && ly >= 0 && ly < height {
                            let bx = (lx + buffer_head.x).rem_euclid(width);
                            let by = (ly + buffer_head.y).rem_euclid(height);
                            let n_idx = (by * width + bx) as usize;

                            if read_buffer[n_idx].is_awake() {
                                should_simulate = true;
                                break;
                            }
                        }
                    }
                    if should_simulate {
                        break;
                    }
                }
            }

            if !should_simulate {
                return;
            }

            cell.sleep();

            if run_liquid && tick % 2 == 0 {
                liquid_sim::step_liquid(
                    cell,
                    old_cell,
                    read_buffer,
                    width,
                    height,
                    world_pos,
                    window_origin,
                    buffer_head,
                    tick,
                );
            }

            if run_biology && rng.random_ratio(1, 10) {
                biology_sim::step_biology(
                    cell,
                    old_cell,
                    read_buffer,
                    width,
                    height,
                    world_pos,
                    window_origin,
                    buffer_head,
                    rng,
                    tx,
                );
            }

            if cell != old_cell {
                cell.wake();
            } else if cell.terrain.material == MaterialId::ORGANIC_FOLIAGE
                && cell.terrain.state < 10
            {
                cell.wake();
            }
        },
    );

    grid_mut.cells_dirty = true;
}
