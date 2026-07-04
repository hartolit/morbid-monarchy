mod biology_sim;
mod fire_sim;
mod granular_sim;
mod liquid_sim;

use bevy::{
    ecs::{
        resource::Resource,
        system::{Res, ResMut},
    },
    math::IVec2,
};
use flume::{Receiver, Sender};
use rand::{RngExt, SeedableRng, rngs::SmallRng};
use rayon::iter::{
    IndexedParallelIterator, IntoParallelRefIterator, IntoParallelRefMutIterator, ParallelIterator,
};
use std::sync::atomic::Ordering;

use crate::core::world::cell::{FluidMat, SurfaceMat, TerrainMat};
use crate::prelude::ActiveWorldGrid;
use crate::prelude::CellGridReadView;

pub enum GridEvent {
    SpawnTerrainParticle { pos: IVec2, material: TerrainMat },
    SpawnFluidParticle { pos: IVec2, material: FluidMat },
    ApplyDamage { pos: IVec2, amount: u32 },
    PlaySound { pos: IVec2, sound_id: u8 },
}

#[derive(Resource)]
pub struct SimulationConfig {
    pub run_liquid: bool,
    pub run_biology: bool,
    pub run_granular: bool,
    pub run_fire: bool,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            run_liquid: true,
            run_biology: true,
            run_granular: true,
            run_fire: true,
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

    let width = grid_mut.spatial.width;
    let height = grid_mut.spatial.height;
    let tick = grid_mut.tick;
    let global_tx = event_queue.tx.clone();

    let run_liquid = config.run_liquid;
    let run_biology = config.run_biology;
    let run_granular = config.run_granular;
    let run_fire = config.run_fire;

    let view = CellGridReadView {
        cells: &grid_mut.back_buffer,
        width,
        height,
        window_origin: grid_mut.spatial.window_origin,
        buffer_head: grid_mut.spatial.buffer_head,
    };

    let spatial_cells = &mut grid_mut.spatial.cells;
    let wake_buf = &*grid_mut.wake_buffer;
    let next_wake_buf = &*grid_mut.next_wake_buffer;
    let back_buf = &*grid_mut.back_buffer;

    // Strict 3-way zip locks the hardware prefetcher into a unified linear stride.
    let events: Vec<GridEvent> = spatial_cells
        .par_iter_mut()
        .zip(wake_buf.par_iter())
        .zip(back_buf.par_iter())
        .enumerate()
        .fold(
            || (Vec::new(), SmallRng::from_rng(&mut rand::rng())),
            |(mut local_events, mut rng), (idx, ((cell, wake_atomic), old_cell))| {
                let wake_val = wake_atomic.load(Ordering::Relaxed);
                if wake_val == 0 {
                    return (local_events, rng);
                }

                let buffer_pos = IVec2::new((idx as i32) % width, (idx as i32) / width);
                let local_pos = IVec2::new(
                    (buffer_pos.x - view.buffer_head.x).rem_euclid(width),
                    (buffer_pos.y - view.buffer_head.y).rem_euclid(height),
                );
                let world_pos = local_pos + view.window_origin;

                if run_granular && tick % 2 != 0 {
                    granular_sim::step_granular(cell, old_cell, view, world_pos, tick);
                }

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

                if run_fire && rng.random_ratio(1, 2) {
                    fire_sim::step_fire(
                        cell,
                        old_cell,
                        view,
                        world_pos,
                        &mut rng,
                        &mut local_events,
                    );
                }

                let mut changed = cell.0 != old_cell.0;

                // Keep the cell alive artificially if it contains active foliage
                // that hasn't reached its maximum biological lifecycle state.
                if !changed
                    && cell.surface_mat() == SurfaceMat::SURFACE_FOLIAGE
                    && cell.surface_state() < 10
                {
                    changed = true;
                }

                // Keep the cell awake artificially while it is burning
                if !changed && cell.surface_mat() == SurfaceMat::SURFACE_FIRE {
                    changed = true;
                }

                if changed {
                    // The cell changed. Apply a TTL of 2 to survive the interleaved passes.
                    next_wake_buf[idx].fetch_max(2, Ordering::Relaxed);
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            if dx == 0 && dy == 0 {
                                continue;
                            }

                            if let Some(n_idx) = view.get_index(world_pos + IVec2::new(dx, dy)) {
                                next_wake_buf[n_idx].fetch_max(2, Ordering::Relaxed);
                            }
                        }
                    }
                } else if wake_val > 1 {
                    next_wake_buf[idx].fetch_max(wake_val - 1, Ordering::Relaxed);
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

    for ev in events {
        let _ = global_tx.send(ev);
    }

    grid_mut.cells_dirty = true;
}
