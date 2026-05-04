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

use crate::engine::world::cell::TerrainMat;
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

    grid_mut.cells.par_iter_mut().enumerate().for_each_init(
        || (global_tx.clone(), SmallRng::from_rng(&mut rand::rng())),
        |(tx, rng), (idx, cell)| {
            let buffer_pos = ActiveWorldGrid::index_to_pos(idx, width);

            let local_pos = IVec2::new(
                (buffer_pos.x - view.buffer_head.x).rem_euclid(width),
                (buffer_pos.y - view.buffer_head.y).rem_euclid(height),
            );

            let world_pos = local_pos + view.window_origin;
            let old_cell = &view.cells[idx];

            let mut should_simulate = old_cell.is_awake();

            if !should_simulate {
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }

                        let n_pos = world_pos + IVec2::new(dx, dy);
                        if let Some((_, n_cell)) = view.get_cell(n_pos) {
                            if n_cell.is_awake() {
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
                liquid_sim::step_liquid(cell, old_cell, view, world_pos, tick);
            }

            if run_biology && rng.random_ratio(1, 10) {
                biology_sim::step_biology(cell, old_cell, view, world_pos, rng, tx);
            }

            if cell.0 != old_cell.0 {
                cell.wake();
            } else if cell.terrain_mat() == TerrainMat::FOLIAGE && cell.terrain_state() < 10 {
                cell.wake();
            }
        },
    );

    grid_mut.cells_dirty = true;
}
