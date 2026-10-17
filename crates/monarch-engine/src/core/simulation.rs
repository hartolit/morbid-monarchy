mod biology_sim;
mod fire_sim;
mod granular_sim;
mod liquid_sim;
mod thermal_sim;

use bevy::{
    ecs::{
        resource::Resource,
        system::{Local, Res, ResMut},
    },
    math::IVec2,
    time::Time,
};
use flume::{Receiver, Sender};
use rand::{RngExt, SeedableRng, rngs::SmallRng};
use rayon::iter::{
    IndexedParallelIterator, IntoParallelRefIterator, IntoParallelRefMutIterator, ParallelIterator,
};
use std::{
    sync::atomic::{AtomicU8, Ordering},
    time::Instant,
};

use crate::core::counters::EngineCounters;

use crate::core::world::grid::{
    WAKE_ACTIVE_FRESH, WAKE_ACTIVE_MIN, WAKE_ASLEEP, WAKE_CHANGE_RADIUS, schedule_wake,
};
use crate::prelude::ActiveWorldGrid;
use crate::prelude::CellGridReadView;
use world_model::{
    config::{DEFAULT_AMBIENT_TEMPERATURE_C, DEFAULT_MAX_TICKS_PER_FRAME, DEFAULT_TICK_RATE_HZ},
    material::{FluidMat, SurfaceMat, TerrainMat},
    rules::{BIOLOGY_ODDS_DENOMINATOR, FIRE_ODDS_DENOMINATOR, FOLIAGE_MATURE_STATE, granular_runs},
};

/// Lower bound on the tick rate so a zeroed configuration cannot produce an infinite step.
pub const MIN_TICK_RATE_HZ: f64 = 1.0;

/// Read-only view of which cells may donate mass during the current tick.
///
/// Exchange arbitration only considers donors, and every donor's neighbours are scheduled at
/// least at halo level, so any transfer a donor commits to is always matched by its receiver.
#[derive(Clone, Copy)]
pub(crate) struct DonorMask<'a> {
    wake: &'a [AtomicU8],
}

impl DonorMask<'_> {
    #[inline(always)]
    pub(crate) fn can_donate(&self, idx: usize) -> bool {
        self.wake[idx].load(Ordering::Relaxed) >= WAKE_ACTIVE_MIN
    }
}

pub enum GridEvent {
    SpawnTerrainParticle { pos: IVec2, material: TerrainMat },
    SpawnFluidParticle { pos: IVec2, material: FluidMat },
    ApplyDamage { pos: IVec2, amount: u32 },
    PlaySound { pos: IVec2, sound_id: u8 },
}

#[derive(Resource, Debug, Clone)]
pub struct SimulationConfig {
    pub run_liquid: bool,
    pub run_biology: bool,
    pub run_granular: bool,
    pub run_fire: bool,
    pub run_thermal: bool,
    /// Fixed cellular automaton rate. Decoupled from the render frame rate so flow speeds
    /// do not change with the monitor refresh or frame time.
    pub tick_rate_hz: f64,
    /// Catch-up budget per frame. When a frame takes longer than this many ticks, the
    /// remaining backlog is dropped so the simulation slows down instead of spiralling.
    pub max_ticks_per_frame: u32,
    /// Air temperature in degrees Celsius. Below freezing, still water crystallises from
    /// shorelines and existing ice; above it, ice melts back from its exposed edges.
    pub ambient_temperature_c: f32,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            run_liquid: true,
            run_biology: true,
            run_granular: true,
            run_fire: true,
            run_thermal: true,
            tick_rate_hz: DEFAULT_TICK_RATE_HZ,
            max_ticks_per_frame: DEFAULT_MAX_TICKS_PER_FRAME,
            ambient_temperature_c: DEFAULT_AMBIENT_TEMPERATURE_C,
        }
    }
}

/// Accumulates frame time and converts it into whole simulation ticks.
#[derive(Resource, Default, Debug)]
pub struct SimulationClock {
    accumulated_seconds: f64,
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
    time: Res<Time>,
    mut clock: ResMut<SimulationClock>,
    mut grid: ResMut<ActiveWorldGrid>,
    event_queue: Res<SimulationEventQueue>,
    config: Res<SimulationConfig>,
    mut applied_temperature: Local<Option<f32>>,
    counters: Option<ResMut<EngineCounters>>,
) {
    // Sleeping pools never re-evaluate their phase on their own, so a change of climate has
    // to wake the whole window once.
    if *applied_temperature != Some(config.ambient_temperature_c) {
        if applied_temperature.is_some() {
            grid.wake_all();
        }
        *applied_temperature = Some(config.ambient_temperature_c);
    }

    let tick_seconds = 1.0 / config.tick_rate_hz.max(MIN_TICK_RATE_HZ);
    clock.accumulated_seconds += time.delta_secs_f64();

    let started = Instant::now();
    let mut ticks_run = 0;
    while clock.accumulated_seconds >= tick_seconds && ticks_run < config.max_ticks_per_frame {
        run_simulation_tick(&mut grid, &event_queue.tx, &config);
        clock.accumulated_seconds -= tick_seconds;
        ticks_run += 1;
    }

    let backlog_seconds = clock.accumulated_seconds;
    if ticks_run == config.max_ticks_per_frame {
        clock.accumulated_seconds = clock.accumulated_seconds.min(tick_seconds);
    }

    if let Some(mut counters) = counters {
        let ticks = u64::from(ticks_run);
        counters.ticks += ticks;
        counters.simulation_nanos += started.elapsed().as_nanos() as u64;
        counters.full_copy_bytes += ticks * std::mem::size_of_val(&*grid.spatial.cells) as u64;
        counters.wake_clear_bytes += ticks * grid.next_wake_buffer.len() as u64;
        counters.discarded_simulation_nanos +=
            ((backlog_seconds - clock.accumulated_seconds).max(0.0) * 1e9) as u64;
    }
}

/// Advances the cellular automaton by exactly one tick.
pub fn run_simulation_tick(
    grid_mut: &mut ActiveWorldGrid,
    event_tx: &Sender<GridEvent>,
    config: &SimulationConfig,
) {
    grid_mut.swap_buffers();

    let width = grid_mut.spatial.width;
    let height = grid_mut.spatial.height;
    let tick = grid_mut.tick;

    let run_liquid = config.run_liquid;
    let run_biology = config.run_biology;
    let run_granular = config.run_granular && granular_runs(u64::from(tick));
    let run_fire = config.run_fire;
    let run_thermal = config.run_thermal;
    let ambient_temperature_c = config.ambient_temperature_c;

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
    let donors = DonorMask { wake: wake_buf };

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
                if wake_val == WAKE_ASLEEP {
                    return (local_events, rng);
                }
                let is_active = wake_val >= WAKE_ACTIVE_MIN;

                let buffer_pos = IVec2::new((idx as i32) % width, (idx as i32) / width);
                let local_pos = IVec2::new(
                    (buffer_pos.x - view.buffer_head.x).rem_euclid(width),
                    (buffer_pos.y - view.buffer_head.y).rem_euclid(height),
                );
                let world_pos = local_pos + view.window_origin;

                if run_granular {
                    granular_sim::step_granular(
                        cell, old_cell, view, donors, is_active, world_pos, tick,
                    );
                }

                if run_liquid {
                    liquid_sim::step_liquid(
                        cell, old_cell, view, donors, is_active, world_pos, tick,
                    );
                }

                // Halo cells exist only to complete mass exchanges started by active neighbours.
                let mut phase_change_pending = false;
                if is_active {
                    if run_thermal {
                        phase_change_pending = thermal_sim::step_thermal(
                            cell,
                            view,
                            world_pos,
                            tick,
                            ambient_temperature_c,
                        );
                    }

                    if run_biology && rng.random_ratio(1, BIOLOGY_ODDS_DENOMINATOR) {
                        biology_sim::step_biology(
                            cell,
                            old_cell,
                            view,
                            world_pos,
                            &mut rng,
                            &mut local_events,
                        );
                    }

                    if run_fire && rng.random_ratio(1, FIRE_ODDS_DENOMINATOR) {
                        fire_sim::step_fire(
                            cell,
                            old_cell,
                            view,
                            world_pos,
                            &mut rng,
                            &mut local_events,
                        );
                    }
                }

                let mut changed = cell.0 != old_cell.0;

                if is_active && !changed {
                    // Keep the cell alive artificially if it contains active foliage
                    // that hasn't reached its maximum biological lifecycle state, while it is
                    // burning, or while it is about to freeze or melt.
                    let surface = cell.surface_mat();
                    changed = (surface == SurfaceMat::SURFACE_FOLIAGE
                        && cell.surface_state() < FOLIAGE_MATURE_STATE)
                        || surface == SurfaceMat::SURFACE_FIRE
                        || phase_change_pending;
                }

                if changed {
                    schedule_wake(
                        next_wake_buf,
                        &view,
                        world_pos,
                        WAKE_ACTIVE_FRESH,
                        WAKE_CHANGE_RADIUS,
                    );
                } else if wake_val > WAKE_ACTIVE_MIN {
                    schedule_wake(next_wake_buf, &view, world_pos, wake_val - 1, 0);
                }

                (local_events, rng)
            },
        )
        .map(|(events, _)| events)
        .reduce(Vec::new, |mut a, mut b| {
            a.append(&mut b);
            a
        });

    for ev in events {
        let _ = event_tx.send(ev);
    }

    grid_mut.cells_dirty = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::{system::RunSystemOnce, world::World};
    use std::time::Duration;
    use world_model::{cell::WorldCell, material::GranularMat};

    const GRID_SIZE: i32 = 48;
    const BED_ELEVATION: u16 = 100;
    const SETTLE_TICK_LIMIT: usize = 6000;

    fn flat_grid(terrain: TerrainMat) -> ActiveWorldGrid {
        let mut grid = ActiveWorldGrid::new(GRID_SIZE, GRID_SIZE, IVec2::ZERO);
        for cell in grid.spatial.cells.iter_mut() {
            cell.set_terrain_mat(terrain);
            cell.set_elevation(BED_ELEVATION);
        }
        grid
    }

    fn edit_cell(grid: &mut ActiveWorldGrid, pos: IVec2, edit: impl FnOnce(&mut WorldCell)) {
        let mut cell = grid.get_cell(pos);
        edit(&mut cell);
        grid.set_cell(pos, cell);
    }

    fn physics_only() -> SimulationConfig {
        SimulationConfig {
            run_biology: false,
            run_fire: false,
            ..SimulationConfig::default()
        }
    }

    fn sum_cells(grid: &ActiveWorldGrid, measure: impl Fn(&WorldCell) -> u64) -> u64 {
        grid.spatial.cells.iter().map(measure).sum()
    }

    fn fluid_volume(grid: &ActiveWorldGrid) -> u64 {
        sum_cells(grid, |c| c.fluid_vol() as u64)
    }

    fn solid_volume(grid: &ActiveWorldGrid) -> u64 {
        sum_cells(grid, |c| c.elevation() as u64 + c.granular_vol() as u64)
    }

    fn is_asleep(grid: &ActiveWorldGrid) -> bool {
        grid.next_wake_buffer
            .iter()
            .all(|wake| wake.load(Ordering::Relaxed) == 0)
    }

    /// Runs ticks until no cell is scheduled to wake, returning the tick count.
    fn run_until_asleep(grid: &mut ActiveWorldGrid, config: &SimulationConfig) -> Option<usize> {
        let queue = SimulationEventQueue::default();
        for tick in 1..=SETTLE_TICK_LIMIT {
            run_simulation_tick(grid, &queue.tx, config);
            if is_asleep(grid) {
                return Some(tick);
            }
        }
        None
    }

    #[test]
    fn dam_break_conserves_liquid_and_settles() {
        let mut grid = flat_grid(TerrainMat::TERRAIN_STONE);
        let center = IVec2::splat(GRID_SIZE / 2);
        edit_cell(&mut grid, center, |c| {
            c.set_fluid_mat(FluidMat::FLUID_WATER);
            c.set_fluid_vol(400);
        });
        let initial_fluid = fluid_volume(&grid);
        let initial_solid = solid_volume(&grid);

        let settled_after = run_until_asleep(&mut grid, &physics_only());

        assert_eq!(
            fluid_volume(&grid),
            initial_fluid,
            "liquid volume must be conserved"
        );
        assert_eq!(
            solid_volume(&grid),
            initial_solid,
            "erosion must conserve solid mass"
        );
        assert!(
            settled_after.is_some(),
            "a level pool must reach a fixed point instead of dithering forever"
        );
        assert!(
            grid.get_cell(center).fluid_vol() < 10,
            "the column must have spread out"
        );

        // A one-unit-per-cell staircase is also free of two-unit steps; the pool must instead
        // come to rest level to within the single unit an integer volume cannot split.
        let wet_heads: Vec<u32> = grid
            .spatial
            .cells
            .iter()
            .filter(|c| c.fluid_vol() > 0)
            .map(|c| c.elevation() as u32 + c.granular_vol() as u32 + c.fluid_vol() as u32)
            .collect();
        let highest = wet_heads.iter().max().copied().unwrap_or_default();
        let lowest = wet_heads.iter().min().copied().unwrap_or_default();
        assert!(
            highest - lowest <= 1,
            "pool settled as a mound from {lowest} to {highest}"
        );
    }

    #[test]
    fn walled_basin_fills_level_and_rests() {
        const WALL: i32 = 4;
        const WALL_HEIGHT: u16 = 40;
        let mut grid = flat_grid(TerrainMat::TERRAIN_STONE);
        let interior = WALL..GRID_SIZE - WALL;
        for y in 0..GRID_SIZE {
            for x in 0..GRID_SIZE {
                if !interior.contains(&x) || !interior.contains(&y) {
                    edit_cell(&mut grid, IVec2::new(x, y), |c| {
                        c.set_elevation(BED_ELEVATION + WALL_HEIGHT);
                    });
                }
            }
        }
        let corner = IVec2::splat(WALL + 2);
        edit_cell(&mut grid, corner, |c| {
            c.set_fluid_mat(FluidMat::FLUID_WATER);
            c.set_fluid_vol(WorldCell::MAX_FLUID_VOL);
        });
        let initial_fluid = fluid_volume(&grid);

        let settled_after = run_until_asleep(&mut grid, &physics_only());

        assert_eq!(fluid_volume(&grid), initial_fluid);
        assert!(settled_after.is_some(), "the basin never came to rest");
        let heads: Vec<u32> = grid
            .spatial
            .cells
            .iter()
            .filter(|c| c.elevation() == BED_ELEVATION && c.fluid_vol() > 0)
            .map(|c| BED_ELEVATION as u32 + c.fluid_vol() as u32)
            .collect();
        let highest = heads.iter().max().copied().unwrap_or_default();
        let lowest = heads.iter().min().copied().unwrap_or_default();
        assert!(
            highest - lowest <= 1,
            "basin settled from {lowest} to {highest}"
        );
    }

    #[test]
    fn granular_pile_conserves_mass_and_settles() {
        let mut grid = flat_grid(TerrainMat::TERRAIN_STONE);
        let center = IVec2::splat(GRID_SIZE / 2);
        for dy in -1..=1 {
            for dx in -1..=1 {
                edit_cell(&mut grid, center + IVec2::new(dx, dy), |c| {
                    c.set_granular_mat(GranularMat::GRANULAR_SAND);
                    c.set_granular_vol(WorldCell::MAX_GRANULAR_VOL);
                });
            }
        }
        let initial_solid = solid_volume(&grid);

        let settled_after = run_until_asleep(&mut grid, &physics_only());

        assert_eq!(solid_volume(&grid), initial_solid);
        assert!(settled_after.is_some(), "a sand pile must come to rest");
    }

    #[test]
    fn chaotic_terrain_conserves_liquid_and_solid_mass() {
        const STRESS_SEED: u64 = 0x5EED;
        const STRESS_TICKS: usize = 600;
        const RELIEF: u16 = 24;

        let mut rng = SmallRng::seed_from_u64(STRESS_SEED);
        let mut grid = flat_grid(TerrainMat::TERRAIN_DIRT);
        for y in 0..GRID_SIZE {
            for x in 0..GRID_SIZE {
                edit_cell(&mut grid, IVec2::new(x, y), |c| {
                    c.set_elevation(BED_ELEVATION + rng.random_range(0..RELIEF));
                    if rng.random_ratio(1, 3) {
                        c.set_granular_mat(GranularMat::GRANULAR_SAND);
                        c.set_granular_vol(rng.random_range(1..=WorldCell::MAX_GRANULAR_VOL));
                    }
                    if rng.random_ratio(1, 4) {
                        c.set_fluid_mat(FluidMat::FLUID_WATER);
                        c.set_fluid_vol(rng.random_range(1..64));
                    }
                });
            }
        }
        let initial_fluid = fluid_volume(&grid);
        let initial_solid = solid_volume(&grid);

        let queue = SimulationEventQueue::default();
        let config = physics_only();
        for _ in 0..STRESS_TICKS {
            run_simulation_tick(&mut grid, &queue.tx, &config);
            assert_eq!(fluid_volume(&grid), initial_fluid);
            assert_eq!(solid_volume(&grid), initial_solid);
        }
    }

    #[test]
    fn oil_poured_into_a_lake_stays_a_patch() {
        let mut grid = flat_grid(TerrainMat::TERRAIN_STONE);
        for cell in grid.spatial.cells.iter_mut() {
            cell.inject_fluid(FluidMat::FLUID_WATER, 40);
        }
        let center = IVec2::splat(GRID_SIZE / 2);
        for dy in -1..=1 {
            for dx in -1..=1 {
                edit_cell(&mut grid, center + IVec2::new(dx, dy), |c| {
                    c.inject_fluid(FluidMat::FLUID_OIL, 30);
                });
            }
        }
        let initial_fluid = fluid_volume(&grid);

        let queue = SimulationEventQueue::default();
        for _ in 0..600 {
            run_simulation_tick(&mut grid, &queue.tx, &physics_only());
        }

        let oil_cells = grid
            .spatial
            .cells
            .iter()
            .filter(|c| c.fluid_mat() == FluidMat::FLUID_OIL)
            .count();
        assert_eq!(
            fluid_volume(&grid),
            initial_fluid,
            "mixing must not delete volume"
        );
        assert!(
            (1..=9).contains(&oil_cells),
            "oil spread to {oil_cells} cells"
        );
    }

    #[test]
    fn still_water_freezes_from_the_shore_and_thaws_back() {
        const WALL: i32 = 8;
        const WALL_HEIGHT: u16 = 40;
        let mut grid = flat_grid(TerrainMat::TERRAIN_STONE);
        let pond = WALL..GRID_SIZE - WALL;
        for y in 0..GRID_SIZE {
            for x in 0..GRID_SIZE {
                edit_cell(&mut grid, IVec2::new(x, y), |c| {
                    if pond.contains(&x) && pond.contains(&y) {
                        c.inject_fluid(FluidMat::FLUID_WATER, 20);
                    } else {
                        c.set_elevation(BED_ELEVATION + WALL_HEIGHT);
                    }
                });
            }
        }
        let initial_fluid = fluid_volume(&grid);
        let pond_cells = pond.len() * pond.len();
        let count = |grid: &ActiveWorldGrid, material: FluidMat| {
            grid.spatial
                .cells
                .iter()
                .filter(|c| c.fluid_mat() == material)
                .count()
        };

        let mut climate = physics_only();
        climate.ambient_temperature_c = -10.0;
        let frozen_after = run_until_asleep(&mut grid, &climate);
        assert!(frozen_after.is_some(), "freezing never finished");
        assert_eq!(count(&grid, FluidMat::FLUID_ICE), pond_cells);
        assert_eq!(fluid_volume(&grid), initial_fluid);

        climate.ambient_temperature_c = 20.0;
        grid.wake_all();
        let thawed_after = run_until_asleep(&mut grid, &climate);
        assert!(thawed_after.is_some(), "thawing never finished");
        assert_eq!(count(&grid, FluidMat::FLUID_ICE), 0);
        assert_eq!(fluid_volume(&grid), initial_fluid);
    }

    #[test]
    fn ice_is_a_floor_that_liquid_cannot_enter() {
        let mut grid = flat_grid(TerrainMat::TERRAIN_STONE);
        let ice = IVec2::new(20, 20);
        edit_cell(&mut grid, ice, |c| {
            c.inject_fluid(FluidMat::FLUID_ICE, 10);
        });
        edit_cell(&mut grid, ice + IVec2::X, |c| {
            c.inject_fluid(FluidMat::FLUID_WATER, 200);
        });

        let mut keep_frozen = physics_only();
        keep_frozen.ambient_temperature_c = -5.0;
        let queue = SimulationEventQueue::default();
        for _ in 0..200 {
            run_simulation_tick(&mut grid, &queue.tx, &keep_frozen);
        }

        let block = grid.get_cell(ice);
        assert_eq!(
            (block.fluid_mat(), block.fluid_vol()),
            (FluidMat::FLUID_ICE, 10)
        );
    }

    #[test]
    fn magma_touching_water_solidifies_and_boils_it() {
        let mut grid = flat_grid(TerrainMat::TERRAIN_DIRT);
        let magma_pos = IVec2::new(10, 10);
        let water_pos = IVec2::new(11, 10);
        let magma_volume = 20;
        let water_volume = 20;
        edit_cell(&mut grid, magma_pos, |c| {
            c.set_fluid_mat(FluidMat::FLUID_MAGMA);
            c.set_fluid_vol(magma_volume);
        });
        edit_cell(&mut grid, water_pos, |c| {
            c.set_fluid_mat(FluidMat::FLUID_WATER);
            c.set_fluid_vol(water_volume);
        });

        let thermal_only = SimulationConfig {
            run_liquid: false,
            run_biology: false,
            run_granular: false,
            run_fire: false,
            ..SimulationConfig::default()
        };
        let queue = SimulationEventQueue::default();
        run_simulation_tick(&mut grid, &queue.tx, &thermal_only);

        let rock = grid.get_cell(magma_pos);
        assert_eq!(rock.fluid_mat(), FluidMat::EMPTY);
        assert_eq!(rock.terrain_mat(), TerrainMat::TERRAIN_STONE);
        assert_eq!(rock.elevation(), BED_ELEVATION + magma_volume);
        assert!(grid.get_cell(water_pos).fluid_vol() < water_volume);
    }

    #[test]
    fn burning_oil_consumes_its_fuel_and_burns_out() {
        let mut grid = flat_grid(TerrainMat::TERRAIN_STONE);
        let pool = IVec2::new(20, 20);
        edit_cell(&mut grid, pool, |c| {
            c.set_fluid_mat(FluidMat::FLUID_OIL);
            c.set_fluid_vol(12);
            c.set_surface_mat(SurfaceMat::SURFACE_FIRE);
        });

        let fire_only = SimulationConfig {
            run_liquid: false,
            run_biology: false,
            run_granular: false,
            run_thermal: false,
            ..SimulationConfig::default()
        };
        let queue = SimulationEventQueue::default();
        for _ in 0..1000 {
            run_simulation_tick(&mut grid, &queue.tx, &fire_only);
        }

        let burnt = grid.get_cell(pool);
        assert_eq!(
            burnt.fluid_mat(),
            FluidMat::EMPTY,
            "the oil must be consumed"
        );
        assert_ne!(
            burnt.surface_mat(),
            SurfaceMat::SURFACE_FIRE,
            "the fire must go out"
        );
    }

    #[test]
    fn fixed_clock_runs_whole_ticks_and_caps_catch_up() {
        let mut world = World::new();
        world.insert_resource(ActiveWorldGrid::new(8, 8, IVec2::ZERO));
        world.init_resource::<SimulationClock>();
        world.init_resource::<SimulationEventQueue>();
        world.insert_resource(SimulationConfig {
            tick_rate_hz: 60.0,
            max_ticks_per_frame: 4,
            ..SimulationConfig::default()
        });

        let mut time = Time::<()>::default();
        time.advance_by(Duration::from_secs_f64(3.5 / 60.0));
        world.insert_resource(time);
        world
            .run_system_once(simulate_world)
            .expect("simulate_world must run");
        assert_eq!(world.resource::<ActiveWorldGrid>().tick, 3);

        world
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f64(1.0));
        world
            .run_system_once(simulate_world)
            .expect("simulate_world must run");
        assert_eq!(
            world.resource::<ActiveWorldGrid>().tick,
            7,
            "catch-up is capped"
        );
    }
}
