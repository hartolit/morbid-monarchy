//! The baseline's ten simulation intentions (`monarch-engine` `core::simulation::tests`, removed
//! in 06) on the GPU, each also compared tick for tick with the CPU reference. Fixtures are one
//! 64 × 64 chunk whose neighbours are not resident (a closed boundary, as the baseline's grid
//! edge was), placed at keys away from the origin so the hashes see other absolute coordinates.

use world_model::{
    cell::WorldCell,
    command::{AmbientTemperature, Command},
    material::{FluidMat, GranularMat, SurfaceMat, TerrainMat},
    observation::WorldTotals,
    rules::SimulationRules,
    spatial::{CHUNK_CELL_COUNT, CHUNK_SIZE, ChunkKey},
};
use world_runtime::port::CommandResult;

use super::{
    super::Check,
    twin::{Setup, Twin, at},
};

pub const BED_ELEVATION: u16 = 100;
/// The baseline tests' settling limit.
pub const SETTLE_TICK_LIMIT: u32 = 6000;
const SIZE: usize = CHUNK_SIZE;

pub fn flat(terrain: TerrainMat, elevation: u16) -> Vec<WorldCell> {
    let mut cell = WorldCell::default();
    cell.set_terrain_mat(terrain);
    cell.set_elevation(elevation);
    vec![cell; CHUNK_CELL_COUNT]
}

/// The baseline tests' `physics_only`: no biology and no fire.
pub fn physics_only() -> SimulationRules {
    SimulationRules::DETERMINISTIC
}

/// A one-chunk twin at `key` (its neighbours not resident).
pub fn single(adapter: &str, setup: Setup, cells: Vec<WorldCell>) -> Result<Twin, String> {
    let key = setup.center;
    Twin::open(adapter, setup, &[(key, cells)])
}

fn totals(twin: &mut Twin) -> Result<WorldTotals, String> {
    match twin.step(&[Command::MeasureTotals])?[0] {
        CommandResult::Totals(totals) => Ok(totals),
        other => Err(format!("totals: {other:?}")),
    }
}

/// Liquid heads of wet cells: highest minus lowest.
fn head_spread(cells: &[WorldCell], only_bed: Option<u16>) -> u32 {
    let heads: Vec<u32> = cells
        .iter()
        .filter(|c| c.fluid_vol() > 0 && only_bed.is_none_or(|bed| c.elevation() == bed))
        .map(|c| c.liquid_head())
        .collect();
    heads.iter().max().copied().unwrap_or(0) - heads.iter().min().copied().unwrap_or(0)
}

fn count(cells: &[WorldCell], material: FluidMat) -> usize {
    cells.iter().filter(|c| c.fluid_mat() == material).count()
}

fn run_dam_break(
    adapter: &str,
    check: &mut Check,
    setup: Setup,
    label: &str,
) -> Result<(), String> {
    let mut cells = flat(TerrainMat::TERRAIN_STONE, BED_ELEVATION);
    cells[at(32, 32)].set_fluid_mat(FluidMat::FLUID_WATER);
    cells[at(32, 32)].set_fluid_vol(400);
    let initial = WorldTotals::of(&cells);
    let mut twin = single(adapter, setup.rules(physics_only()), cells)?;
    let settled = twin.run_until_asleep(SETTLE_TICK_LIMIT)?;
    check.that(settled.is_some(), || {
        format!("{label}: the pool never fell asleep")
    });
    check.metric(
        &format!("{label}_settled_after_ticks"),
        settled.unwrap_or(0),
    );
    let after = totals(&mut twin)?;
    check.equal(
        &format!("{label}: fluid"),
        after.fluid_volume(),
        initial.fluid_volume(),
    );
    check.equal(&format!("{label}: solid"), after.solid(), initial.solid());
    let captured = twin.compare_cells(check, label)?;
    let cells = &captured[0].1;
    check.that(cells[at(32, 32)].fluid_vol() < 10, || {
        format!("{label}: the column did not spread")
    });
    let spread = head_spread(cells, None);
    check.that(spread <= 1, || {
        format!("{label}: the pool rests from a spread of {spread}")
    });
    twin.finish(check, label);
    Ok(())
}

pub fn dam_break(adapter: &str, check: &mut Check) -> Result<(), String> {
    let key = ChunkKey::new(-3, 2, 0);
    run_dam_break(adapter, check, Setup::new(key, 1), "sparse")?;
    run_dam_break(adapter, check, Setup::new(key, 1).dense(), "dense")
}

pub fn walled_basin(adapter: &str, check: &mut Check) -> Result<(), String> {
    const WALL: usize = 4;
    const WALL_HEIGHT: u16 = 40;
    let mut cells = flat(TerrainMat::TERRAIN_STONE, BED_ELEVATION);
    for y in 0..SIZE {
        for x in 0..SIZE {
            if !(WALL..SIZE - WALL).contains(&x) || !(WALL..SIZE - WALL).contains(&y) {
                cells[at(x, y)].set_elevation(BED_ELEVATION + WALL_HEIGHT);
            }
        }
    }
    cells[at(WALL + 2, WALL + 2)].set_fluid_mat(FluidMat::FLUID_WATER);
    cells[at(WALL + 2, WALL + 2)].set_fluid_vol(WorldCell::MAX_FLUID_VOL);
    let initial = WorldTotals::of(&cells);
    let mut twin = single(
        adapter,
        Setup::new(ChunkKey::new(4, 4, 0), 1).rules(physics_only()),
        cells,
    )?;
    let settled = twin.run_until_asleep(SETTLE_TICK_LIMIT)?;
    check.that(settled.is_some(), || {
        "the basin never came to rest".to_owned()
    });
    check.metric("settled_after_ticks", settled.unwrap_or(0));
    check.equal(
        "fluid",
        totals(&mut twin)?.fluid_volume(),
        initial.fluid_volume(),
    );
    let captured = twin.compare_cells(check, "basin")?;
    let spread = head_spread(&captured[0].1, Some(BED_ELEVATION));
    check.that(spread <= 1, || {
        format!("the basin rests from a spread of {spread}")
    });
    twin.finish(check, "basin");
    Ok(())
}

pub fn granular_pile(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut cells = flat(TerrainMat::TERRAIN_STONE, BED_ELEVATION);
    for dy in 0..3 {
        for dx in 0..3 {
            let cell = &mut cells[at(31 + dx, 31 + dy)];
            cell.set_granular_mat(GranularMat::GRANULAR_SAND);
            cell.set_granular_vol(WorldCell::MAX_GRANULAR_VOL);
        }
    }
    let initial = WorldTotals::of(&cells);
    let mut twin = single(
        adapter,
        Setup::new(ChunkKey::new(0, -5, 0), 1).rules(physics_only()),
        cells,
    )?;
    let settled = twin.run_until_asleep(SETTLE_TICK_LIMIT)?;
    check.that(settled.is_some(), || {
        "the sand pile never came to rest".to_owned()
    });
    check.equal("solid", totals(&mut twin)?.solid(), initial.solid());
    let captured = twin.compare_cells(check, "pile")?;
    let sandy = captured[0]
        .1
        .iter()
        .filter(|c| c.granular_mat() == GranularMat::GRANULAR_SAND)
        .count();
    check.that(sandy > 9, || "the pile did not slide".to_owned());
    twin.finish(check, "pile");
    Ok(())
}

/// SplitMix64, for seeded fixtures independent of any rule hash.
pub fn mixer(seed: u64) -> impl FnMut() -> u64 {
    let mut state = seed;
    move || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// The baseline's chaotic terrain: relief, sand and water at random, seeded.
pub fn chaotic(seed: u64) -> Vec<WorldCell> {
    let mut next = mixer(seed);
    let mut cells = flat(TerrainMat::TERRAIN_DIRT, BED_ELEVATION);
    for cell in &mut cells {
        cell.set_elevation(BED_ELEVATION + (next() % 24) as u16);
        if next().is_multiple_of(3) {
            cell.set_granular_mat(GranularMat::GRANULAR_SAND);
            cell.set_granular_vol(1 + (next() % 15) as u16);
        }
        if next().is_multiple_of(4) {
            cell.set_fluid_mat(FluidMat::FLUID_WATER);
            cell.set_fluid_vol(1 + (next() % 63) as u16);
        }
    }
    cells
}

pub fn chaotic_terrain(adapter: &str, check: &mut Check) -> Result<(), String> {
    for (setup, label) in [
        (Setup::new(ChunkKey::new(7, -1, 0), 1), "sparse"),
        (Setup::new(ChunkKey::new(7, -1, 0), 1).dense(), "dense"),
    ] {
        let cells = chaotic(0x5EED);
        let initial = WorldTotals::of(&cells);
        let mut twin = single(adapter, setup.rules(physics_only()), cells)?;
        for tick in 0..600u32 {
            twin.step(&[])?;
            let report = twin.last.map(|last| last.cells).unwrap_or_default();
            check.that(report.consistent(), || {
                format!("{label}: tick {tick} does not balance: {report:?}")
            });
            if tick % 150 == 149 {
                let now = totals(&mut twin)?;
                check.equal(
                    &format!("{label}: fluid at {tick}"),
                    now.fluid_volume(),
                    initial.fluid_volume(),
                );
                check.equal(
                    &format!("{label}: solid at {tick}"),
                    now.solid(),
                    initial.solid(),
                );
            }
        }
        twin.compare_cells(check, label)?;
        twin.finish(check, label);
    }
    Ok(())
}

pub fn oil_patch(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut cells = flat(TerrainMat::TERRAIN_STONE, BED_ELEVATION);
    for cell in &mut cells {
        cell.inject_fluid(FluidMat::FLUID_WATER, 40);
    }
    for dy in 0..3 {
        for dx in 0..3 {
            cells[at(31 + dx, 31 + dy)].inject_fluid(FluidMat::FLUID_OIL, 30);
        }
    }
    let initial = WorldTotals::of(&cells);
    let mut twin = single(
        adapter,
        Setup::new(ChunkKey::new(-1, -1, 0), 1).rules(physics_only()),
        cells,
    )?;
    twin.run(600)?;
    check.equal(
        "fluid",
        totals(&mut twin)?.fluid_volume(),
        initial.fluid_volume(),
    );
    let captured = twin.compare_cells(check, "oil")?;
    let oil = count(&captured[0].1, FluidMat::FLUID_OIL);
    check.that((1..=9).contains(&oil), || {
        format!("oil spread to {oil} cells")
    });
    twin.finish(check, "oil");
    Ok(())
}

pub fn freeze_and_thaw(adapter: &str, check: &mut Check) -> Result<(), String> {
    const WALL: usize = 8;
    const WALL_HEIGHT: u16 = 40;
    let mut cells = flat(TerrainMat::TERRAIN_STONE, BED_ELEVATION);
    let pond = WALL..SIZE - WALL;
    for y in 0..SIZE {
        for x in 0..SIZE {
            if pond.contains(&x) && pond.contains(&y) {
                cells[at(x, y)].inject_fluid(FluidMat::FLUID_WATER, 20);
            } else {
                cells[at(x, y)].set_elevation(BED_ELEVATION + WALL_HEIGHT);
            }
        }
    }
    let initial = WorldTotals::of(&cells);
    let pond_cells = pond.len() * pond.len();
    let setup = Setup::new(ChunkKey::new(2, -6, 0), 1)
        .rules(physics_only())
        .ambient(-10.0);
    let mut twin = single(adapter, setup, cells)?;
    let frozen = twin.run_until_asleep(SETTLE_TICK_LIMIT)?;
    check.that(frozen.is_some(), || "freezing never finished".to_owned());
    let captured = twin.compare_cells(check, "frozen")?;
    check.equal(
        "ice cells",
        count(&captured[0].1, FluidMat::FLUID_ICE),
        pond_cells,
    );
    check.equal(
        "fluid after freezing",
        totals(&mut twin)?.fluid_volume(),
        initial.fluid_volume(),
    );
    // The climate change itself wakes the frozen pond: no separate wake call, unlike the baseline
    // test, which woke the grid by hand.
    let warm = AmbientTemperature::celsius(20.0).map_err(|error| error.to_string())?;
    twin.step(&[Command::SetAmbientTemperature(warm)])?;
    let thawed = twin.run_until_asleep(SETTLE_TICK_LIMIT)?;
    check.that(thawed.is_some(), || "thawing never finished".to_owned());
    let captured = twin.compare_cells(check, "thawed")?;
    check.equal(
        "ice after thawing",
        count(&captured[0].1, FluidMat::FLUID_ICE),
        0,
    );
    check.equal(
        "fluid after thawing",
        totals(&mut twin)?.fluid_volume(),
        initial.fluid_volume(),
    );
    twin.finish(check, "climate");
    Ok(())
}

pub fn ice_floor(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut cells = flat(TerrainMat::TERRAIN_STONE, BED_ELEVATION);
    cells[at(20, 20)].inject_fluid(FluidMat::FLUID_ICE, 10);
    cells[at(21, 20)].inject_fluid(FluidMat::FLUID_WATER, 200);
    let setup = Setup::new(ChunkKey::new(-9, 9, 0), 1)
        .rules(physics_only())
        .ambient(-5.0);
    let mut twin = single(adapter, setup, cells)?;
    twin.run(200)?;
    let captured = twin.compare_cells(check, "ice")?;
    let block = captured[0].1[at(20, 20)];
    check.equal(
        "ice block",
        (block.fluid_mat(), block.fluid_vol()),
        (FluidMat::FLUID_ICE, 10),
    );
    twin.finish(check, "ice");
    Ok(())
}

pub fn magma_quench_and_boil(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut cells = flat(TerrainMat::TERRAIN_DIRT, BED_ELEVATION);
    cells[at(10, 10)].set_fluid_mat(FluidMat::FLUID_MAGMA);
    cells[at(10, 10)].set_fluid_vol(20);
    cells[at(11, 10)].set_fluid_mat(FluidMat::FLUID_WATER);
    cells[at(11, 10)].set_fluid_vol(20);
    let thermal_only = SimulationRules {
        thermal: true,
        ..SimulationRules::NONE
    };
    let mut twin = single(
        adapter,
        Setup::new(ChunkKey::new(1, 1, 0), 1).rules(thermal_only),
        cells,
    )?;
    // The activation tick ran the reaction; capture right after it would cost another tick, so
    // the quench is checked on the capture of the next one (nothing else reacts).
    let report = twin.last.map(|last| last.cells).unwrap_or_default();
    check.equal("quenched units", report.quench, 20);
    check.equal("boiled units", report.boil, 2);
    let captured = twin.compare_cells(check, "magma")?;
    let rock = captured[0].1[at(10, 10)];
    check.equal("rock material", rock.fluid_mat(), FluidMat::EMPTY);
    check.equal(
        "rock terrain",
        rock.terrain_mat(),
        TerrainMat::TERRAIN_STONE,
    );
    check.equal("rock elevation", rock.elevation(), BED_ELEVATION + 20);
    check.that(captured[0].1[at(11, 10)].fluid_vol() < 20, || {
        "the water did not boil".to_owned()
    });
    twin.finish(check, "magma");
    Ok(())
}

pub fn oil_burns_out(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut cells = flat(TerrainMat::TERRAIN_STONE, BED_ELEVATION);
    cells[at(20, 20)].set_fluid_mat(FluidMat::FLUID_OIL);
    cells[at(20, 20)].set_fluid_vol(12);
    cells[at(20, 20)].set_surface_mat(SurfaceMat::SURFACE_FIRE);
    let fire_only = SimulationRules {
        fire: true,
        ..SimulationRules::NONE
    };
    let mut twin = single(
        adapter,
        Setup::new(ChunkKey::new(-4, 0, 0), 1).rules(fire_only),
        cells,
    )?;
    // The activation tick already ran the fire rules once.
    let mut burned = twin.last.map_or(0, |last| last.cells.burn);
    for _ in 0..1000 {
        twin.step(&[])?;
        burned += twin.last.map_or(0, |last| last.cells.burn);
    }
    check.equal("oil burned", burned, 12);
    let captured = twin.compare_cells(check, "fire")?;
    let pool = captured[0].1[at(20, 20)];
    check.equal("pool fluid", pool.fluid_mat(), FluidMat::EMPTY);
    check.that(pool.surface_mat() != SurfaceMat::SURFACE_FIRE, || {
        "the fire did not go out".to_owned()
    });
    twin.finish(check, "fire");
    Ok(())
}
