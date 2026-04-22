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
) {
    let width = grid_res.width;
    let height = grid_res.height;

    grid_res.swap_buffers();
    let grid_mut = grid_res.into_inner();

    let tick = grid_mut.tick;
    let read_buffer = &grid_mut.back_buffer;
    let global_tx = event_queue.tx.clone();

    grid_mut.cells.par_iter_mut().enumerate().for_each_init(
        || (global_tx.clone(), SmallRng::from_rng(&mut rand::rng())),
        |(tx, rng), (idx, cell)| {
            let pos = ActiveWorldGrid::index_to_pos(idx, width);
            let old_cell = &read_buffer[idx];

            // ---------------------------------------------------------
            // DETERMINISTIC PHYSICS
            // ---------------------------------------------------------
            if tick % 2 == 0 {
                liquid_sim::step_liquid(cell, old_cell, read_buffer, width, height, rng, pos);
            }

            // ---------------------------------------------------------
            // SPARSE BIOLOGY
            // ---------------------------------------------------------
            if rng.random_ratio(1, 10) {
                biology_sim::step_biology(cell, old_cell, read_buffer, width, height, rng, tx, pos);
            }
        },
    );

    grid_mut.cells_dirty = true;
}
