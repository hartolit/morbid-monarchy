//! Adversarial cases: deliberate faults the checks must catch, transfers at capacity and at the
//! elevation ceiling, seams, sleeping receivers, activity edge cases and coordinates.

use world_model::{
    cell::WorldCell,
    command::{AmbientTemperature, Command, EditTool, MaterialEdit},
    glam::IVec2,
    material::{FluidMat, GranularMat, SurfaceMat, TerrainMat},
    observation::WorldTotals,
    rules::SimulationRules,
    spatial::{CHUNK_CELL_COUNT, CHUNK_SIZE, ChunkKey},
};
use world_runtime::{
    ids::{ChunkRevision, Tick},
    port::{CommandResult, Outcome, PhysicalWorld},
};

use super::{
    super::{Check, driver::Driver},
    intentions::{BED_ELEVATION, chaotic, flat, physics_only},
    twin::{Setup, Twin, at},
};

const SIZE: i32 = CHUNK_SIZE as i32;

fn edit(tool: EditTool, x: i32, y: i32, radius: i32, strength: u8) -> Command {
    Command::EditMaterial(
        MaterialEdit::new(tool, IVec2::new(x, y), radius, strength).expect("supported"),
    )
}

/// The absolute cell of in-chunk `(x, y)` of `key`.
fn cell_of(key: ChunkKey, x: i32, y: i32) -> IVec2 {
    IVec2::new(key.key.x * SIZE + x, key.key.y * SIZE + y)
}

fn totals_of(results: &[CommandResult]) -> Result<WorldTotals, String> {
    results
        .iter()
        .find_map(|result| match result {
            CommandResult::Totals(totals) => Some(*totals),
            _ => None,
        })
        .ok_or_else(|| "no totals".to_owned())
}

/// The suite must reject an unmatched transfer: donors drain liquid their receivers drop. The
/// tick's own accounting and the comparison with the reference must both see it, and the world
/// must be consistent again once the fault is switched off.
pub fn unmatched_transfer_detected(adapter: &str, check: &mut Check) -> Result<(), String> {
    let key = ChunkKey::new(1, -2, 0);
    let mut cells = flat(TerrainMat::TERRAIN_STONE, BED_ELEVATION);
    cells[at(30, 30)].inject_fluid(FluidMat::FLUID_WATER, 300);
    let setup = Setup::new(key, 1).rules(physics_only()).faults();
    let mut twin = Twin::open(adapter, setup, &[(key, cells.clone())])?;
    let before = totals_of(&twin.step(&[Command::MeasureTotals])?)?;
    let faults = twin
        .world()
        .fault_controller()
        .ok_or("no fault controller")?;
    faults.unmatched_transfers(true);
    twin.run(20)?;
    faults.unmatched_transfers(false);
    let lost =
        before.fluid_volume() - totals_of(&twin.step(&[Command::MeasureTotals])?)?.fluid_volume();
    check.that(twin.inconsistent_ticks > 0, || {
        "the unbalanced ticks were not reported".to_owned()
    });
    check.that(twin.report_mismatches > 0, || {
        "the reference comparison did not notice".to_owned()
    });
    check.that(lost > 0, || "the specimen lost no liquid".to_owned());
    check.metric("specimen_inconsistent_ticks", twin.inconsistent_ticks);
    check.metric("specimen_lost_units", lost);
    // Without the fault the same session balances again.
    let inconsistent = twin.inconsistent_ticks;
    twin.run(20)?;
    check.equal(
        "inconsistent ticks after the fault",
        twin.inconsistent_ticks,
        inconsistent,
    );
    Ok(())
}

/// Opens a one-chunk GPU world, captures the activation tick's result and returns it.
fn first_tick(
    adapter: &str,
    setup: Setup,
    cells: &[WorldCell],
) -> Result<(Driver, Vec<WorldCell>), String> {
    let mut driver = Driver::open(setup.config(adapter))?;
    driver.next_tick = Tick::new(setup.first_tick);
    let session = driver.session();
    let slot = driver
        .world
        .reserve_slot(session, setup.center)
        .map_err(|refusal| format!("reserve: {refusal:?}"))?;
    driver
        .world
        .upload_chunk(slot, ChunkRevision::new(1), cells)
        .map_err(|refusal| format!("upload: {refusal:?}"))?;
    let ticket = driver
        .world
        .capture_chunk(slot)
        .map_err(|refusal| format!("capture: {refusal:?}"))?;
    driver
        .submit(&[])
        .map_err(|refusal| format!("tick: {refusal:?}"))?;
    driver.poll_until("capture", |driver| {
        driver
            .outcomes
            .iter()
            .any(|outcome| matches!(outcome, Outcome::CaptureReady { ticket: ready, .. } if *ready == ticket))
    })?;
    let mut captured = vec![WorldCell::default(); CHUNK_CELL_COUNT];
    driver
        .world
        .take_capture(ticket, &mut captured)
        .map_err(|refusal| format!("take: {refusal:?}"))?;
    driver.settle()?;
    Ok((driver, captured))
}

/// Several donors whose only lower neighbour is one pit: the pit takes from the fullest donor
/// only, exactly half the head difference, and the others keep their liquid this tick. The same
/// for granular donors around a granular pit (the steepest wins). Then the conflict runs on
/// against the reference.
pub fn donor_conflicts(adapter: &str, check: &mut Check) -> Result<(), String> {
    let key = ChunkKey::new(-6, -6, 0);
    let mut cells = flat(TerrainMat::TERRAIN_STONE, BED_ELEVATION);
    let pit = at(20, 20);
    cells[pit].set_elevation(BED_ELEVATION - 10);
    for (index, volume) in [
        (at(20, 21), 30u16),
        (at(21, 20), 50),
        (at(20, 19), 10),
        (at(19, 20), 49),
    ] {
        cells[index].inject_fluid(FluidMat::FLUID_WATER, volume);
    }
    // Granular: a pit at (40, 40) and sand donors at different heights.
    let sand_pit = at(40, 40);
    cells[sand_pit].set_elevation(BED_ELEVATION - 8);
    for (index, volume) in [(at(40, 41), 6u16), (at(41, 40), 12), (at(39, 40), 9)] {
        cells[index].set_granular_mat(GranularMat::GRANULAR_SAND);
        cells[index].set_granular_vol(volume);
    }
    let setup = Setup::new(key, 1).rules(physics_only()).first_tick(1);
    let (_driver, after) = first_tick(adapter, setup, &cells)?;
    // Liquid: the 50-unit donor wins; (150 - 90) / 2 = 30 units move.
    check.equal(
        "pit water",
        (after[pit].fluid_mat(), after[pit].fluid_vol()),
        (FluidMat::FLUID_WATER, 30),
    );
    check.equal("winning donor", after[at(21, 20)].fluid_vol(), 20);
    for (index, volume) in [(at(20, 21), 30u16), (at(20, 19), 10), (at(19, 20), 49)] {
        check.equal(
            &format!("losing donor {index}"),
            after[index].fluid_vol(),
            volume,
        );
    }
    // Granular (tick 1 is odd, so granular runs): the 12-unit pile is steepest and moves its
    // largest step, MAX_GRANULAR_TRANSFER = 2.
    check.equal("sand pit", after[sand_pit].granular_vol(), 2);
    check.equal("winning sand donor", after[at(41, 40)].granular_vol(), 10);
    check.equal(
        "losing sand donors",
        (
            after[at(40, 41)].granular_vol(),
            after[at(39, 40)].granular_vol(),
        ),
        (6, 9),
    );
    // And on, against the reference.
    let mut twin = Twin::open(adapter, setup, &[(key, cells)])?;
    twin.run(300)?;
    twin.compare_cells(check, "conflict")?;
    twin.finish(check, "conflict");
    Ok(())
}

/// Transfers toward full, frozen and mixed columns, and brush strokes into them: nothing is lost,
/// rejected material stays at its donor, the tools add exactly what fits.
pub fn full_and_frozen_receivers(adapter: &str, check: &mut Check) -> Result<(), String> {
    let key = ChunkKey::new(3, 3, 0);
    let mut cells = flat(TerrainMat::TERRAIN_STONE, BED_ELEVATION);
    // A full column beside a higher head, both walled in: the donor's only lower neighbour is
    // full, so it offers nothing and both keep their water.
    for y in 6..15 {
        for x in 6..15 {
            cells[at(x, y)].set_elevation(900);
        }
    }
    cells[at(10, 10)].set_elevation(BED_ELEVATION);
    cells[at(10, 10)].inject_fluid(FluidMat::FLUID_WATER, WorldCell::MAX_FLUID_VOL);
    cells[at(10, 11)].set_elevation(BED_ELEVATION + 100);
    cells[at(10, 11)].inject_fluid(FluidMat::FLUID_WATER, 500);
    // Water against ice.
    cells[at(30, 30)].inject_fluid(FluidMat::FLUID_ICE, 40);
    cells[at(31, 30)].inject_fluid(FluidMat::FLUID_WATER, 80);
    // Oil flowing into a shallow water column, and water into a deep oil pool.
    cells[at(50, 50)].inject_fluid(FluidMat::FLUID_WATER, 2);
    cells[at(51, 50)].inject_fluid(FluidMat::FLUID_OIL, 60);
    cells[at(50, 20)].inject_fluid(FluidMat::FLUID_OIL, 100);
    cells[at(51, 20)].inject_fluid(FluidMat::FLUID_WATER, 120);
    // A column at the elevation ceiling for the sand brush.
    cells[at(5, 50)].set_elevation(4094);
    let setup = Setup::new(key, 1).rules(physics_only()).ambient(-2.0);
    let mut twin = Twin::open(adapter, setup, &[(key, cells)])?;
    twin.run(50)?;
    let captured = twin.compare_cells(check, "before the strokes")?;
    let cells = &captured[0].1;
    check.equal(
        "ice untouched",
        (cells[at(30, 30)].fluid_mat(), cells[at(30, 30)].fluid_vol()),
        (FluidMat::FLUID_ICE, 40),
    );
    check.equal(
        "full column",
        cells[at(10, 10)].fluid_vol(),
        WorldCell::MAX_FLUID_VOL,
    );
    check.equal("its donor", cells[at(10, 11)].fluid_vol(), 500);
    // Brushes into the full, the frozen and the capped columns.
    let full = cell_of(key, 10, 10);
    let ice = cell_of(key, 30, 30);
    let capped = cell_of(key, 5, 50);
    let results = twin.step(&[
        Command::MeasureTotals,
        edit(EditTool::Water, full.x, full.y, 0, 255),
        edit(EditTool::Water, ice.x, ice.y, 0, 7),
        edit(EditTool::Sand, capped.x, capped.y, 0, 200),
        Command::MeasureTotals,
    ])?;
    let report = twin.last.map(|last| last.cells).unwrap_or_default();
    let before = totals_of(&results[..1])?;
    let after = totals_of(&results[4..])?;
    check.equal(
        "tool fluid in",
        report.tool_fluid_in,
        after.fluid_volume() - before.fluid_volume(),
    );
    check.equal(
        "tool solid in",
        report.tool_solid_in,
        after.solid() - before.solid(),
    );
    // The sand brush on a column one below the ceiling adds 15 loose units and 1 of compaction:
    // the rest of its 200 units do not fit.
    check.that(report.tool_solid_in <= 16, || {
        format!("sand added {}", report.tool_solid_in)
    });
    check.equal("painted into ice", report.tool_fluid_in, 7);
    twin.run(200)?;
    twin.compare_cells(check, "receivers")?;
    twin.finish(check, "receivers");
    Ok(())
}

/// Magma and coolant meeting at the elevation ceiling: no unit disappears, what does not fit stays
/// liquid magma, and the region falls asleep instead of retrying for ever.
pub fn ceiling_quench(adapter: &str, check: &mut Check) -> Result<(), String> {
    let key = ChunkKey::new(-2, 5, 0);
    let mut cells = flat(TerrainMat::TERRAIN_STONE, 4000);
    // The whole chunk a checkerboard at and just below the ceiling, magma on the west half and
    // water on the east: they meet along x = 31 | 32.
    for y in 0..64 {
        for x in 0..64 {
            let cell = &mut cells[at(x, y)];
            cell.set_elevation(if (x + y) % 2 == 0 { 4095 } else { 4093 });
            if x < 32 {
                cell.inject_fluid(FluidMat::FLUID_MAGMA, 40);
            } else {
                cell.inject_fluid(FluidMat::FLUID_WATER, 30);
            }
        }
    }
    let initial = WorldTotals::of(&cells);
    let setup = Setup::new(key, 1).rules(physics_only());
    let mut twin = Twin::open(adapter, setup, &[(key, cells)])?;
    // The activation tick already reacted.
    let first = twin.last.map(|last| last.cells).unwrap_or_default();
    let mut quench = first.quench;
    let mut boil = first.boil;
    let mut rejected = u64::from(first.rejected_transfers);
    let settled = loop {
        twin.step(&[])?;
        let report = twin.last.map(|last| last.cells).unwrap_or_default();
        quench += report.quench;
        boil += report.boil;
        rejected += u64::from(report.rejected_transfers);
        if report.awake_cells == 0 {
            break Some(twin.totals.ticks);
        }
        if twin.totals.ticks > 6000 {
            break None;
        }
    };
    check.that(settled.is_some(), || {
        "the ceiling region never fell asleep".to_owned()
    });
    let after = totals_of(&twin.step(&[Command::MeasureTotals])?)?;
    // Every unit is accounted for: fluid lost only to quench and boiling, solid gained by quench.
    check.equal(
        "fluid",
        after.fluid_volume() + quench + boil,
        initial.fluid_volume(),
    );
    check.equal("solid", after.solid(), initial.solid() + quench);
    check.that(rejected > 0, || "no transfer met the ceiling".to_owned());
    let captured = twin.compare_cells(check, "ceiling")?;
    let liquid_magma_at_ceiling = captured[0]
        .1
        .iter()
        .filter(|c| {
            c.elevation() == 4095 && c.fluid_mat() == FluidMat::FLUID_MAGMA && c.fluid_vol() > 0
        })
        .count();
    check.that(liquid_magma_at_ceiling > 0, || {
        "no magma was left liquid at the ceiling".to_owned()
    });
    check.metric("quench_units", quench);
    check.metric("rejected_transfers", rejected);
    twin.finish(check, "ceiling");
    Ok(())
}

/// Flow across a four-chunk junction at negative keys and across tile seams, sparse and dense.
pub fn seam_flow(adapter: &str, check: &mut Check) -> Result<(), String> {
    for (setup, label) in [
        (
            Setup::new(ChunkKey::new(-1, -1, 0), 1).rules(physics_only()),
            "sparse",
        ),
        (
            Setup::new(ChunkKey::new(-1, -1, 0), 1)
                .rules(physics_only())
                .dense(),
            "dense",
        ),
    ] {
        let mut chunks = Vec::new();
        for y in -2..=0 {
            for x in -2..=0 {
                let key = ChunkKey::new(x, y, 0);
                let mut cells = flat(TerrainMat::TERRAIN_DIRT, BED_ELEVATION);
                // Columns diagonally across the junction of chunks (-1,-1)..(0,0), and a column
                // and a sand pile on the tile seams 7 | 8 of the far chunk (-2,-2).
                if x == -1 && y == -1 {
                    cells[at(63, 63)].inject_fluid(FluidMat::FLUID_WATER, 300);
                }
                if x == 0 && y == 0 {
                    cells[at(0, 0)].inject_fluid(FluidMat::FLUID_WATER, 300);
                }
                if x == -2 && y == -2 {
                    cells[at(7, 7)].inject_fluid(FluidMat::FLUID_WATER, 60);
                    cells[at(8, 8)].set_granular_mat(GranularMat::GRANULAR_SAND);
                    cells[at(8, 8)].set_granular_vol(15);
                }
                chunks.push((key, cells));
            }
        }
        let mut twin = Twin::open(adapter, setup, &chunks)?;
        twin.run(300)?;
        let captured = twin.compare_cells(check, label)?;
        // The two chunks without a source got their water across the seams.
        let reached = |x: i32, y: i32| {
            captured
                .iter()
                .find(|(key, _)| key.key.x == x && key.key.y == y)
                .is_some_and(|(_, cells)| cells.iter().any(|c| c.fluid_vol() > 0))
        };
        check.that(reached(0, -1) && reached(-1, 0), || {
            format!("{label}: the junction flow did not cross both seams")
        });
        let far = captured
            .iter()
            .find(|(key, _)| key.key.x == -2 && key.key.y == -2)
            .map(|(_, cells)| cells.clone())
            .unwrap_or_default();
        // Sand slid from (8, 8) into the tile of (7, 7), and water from (7, 7) into tiles east.
        let sand_crossed = far.get(at(7, 7)).is_some_and(|c| c.granular_vol() > 0);
        let water_crossed = far
            .iter()
            .enumerate()
            .any(|(index, c)| index % CHUNK_SIZE >= 8 && c.fluid_vol() > 0);
        check.that(sand_crossed && water_crossed, || {
            format!("{label}: the tile seam was not crossed (sand {sand_crossed}, water {water_crossed})")
        });
        twin.finish(check, label);
    }
    Ok(())
}

/// A settled world, then a change two cells from a sleeping donor across a tile seam: the
/// donor's trickle depends on its receiver's neighbour, so the wake radius must reach it. Then a
/// stroke beside a sleeping tile: the sleeping receivers take the flow.
pub fn sleeping_receivers(adapter: &str, check: &mut Check) -> Result<(), String> {
    let key = ChunkKey::new(0, 0, 0);
    let mut cells = flat(TerrainMat::TERRAIN_STONE, BED_ELEVATION);
    // One unit at x = 7: too little to divide, nothing lower to trickle into. A fixed point.
    cells[at(7, 10)].inject_fluid(FluidMat::FLUID_WATER, 1);
    for setup in [
        Setup::new(key, 1).rules(physics_only()),
        Setup::new(key, 1).rules(physics_only()).dense(),
    ] {
        let dense = setup.execution == gpu_world::world::Execution::Dense;
        let label = if dense { "dense" } else { "sparse" };
        let mut twin = Twin::open(adapter, setup, &[(key, cells.clone())])?;
        let settled = twin.run_until_asleep(100)?;
        check.that(settled.is_some(), || {
            format!("{label}: the fixture never slept")
        });
        // Lower (9, 10) by one: (8, 10), (8, 9) and (8, 11) now drain lower, so the donor at
        // (7, 10), two cells from the edit and in the neighbouring tile, may trickle its unit.
        let lowered = cell_of(key, 9, 10);
        twin.step(&[edit(EditTool::Lower, lowered.x, lowered.y, 0, 1)])?;
        let report = twin.last.map(|last| last.cells).unwrap_or_default();
        check.that(report.fluid_out >= 1, || {
            format!("{label}: the two-away donor did not move")
        });
        let captured = twin.compare_cells(check, &format!("{label} halo"))?;
        check.equal(
            &format!("{label}: donor"),
            captured[0].1[at(7, 10)].fluid_vol(),
            0,
        );
        // A big stroke beside a sleeping tile.
        twin.run_until_asleep(400)?;
        let stroke = cell_of(key, 30, 30);
        twin.step(&[edit(EditTool::Water, stroke.x, stroke.y, 3, 200)])?;
        twin.run(40)?;
        twin.compare_cells(check, &format!("{label} stroke"))?;
        twin.finish(check, label);
    }
    Ok(())
}

/// An asleep world does no cell work: no candidate, no process tile, no awake cell, tick after
/// tick (the lists and counters reset). One small edit wakes only its neighbourhood.
pub fn zero_active_tiles(adapter: &str, check: &mut Check) -> Result<(), String> {
    let center = ChunkKey::new(2, 2, 0);
    let mut chunks = Vec::new();
    for y in 1..=3 {
        for x in 1..=3 {
            chunks.push((
                ChunkKey::new(x, y, 0),
                flat(TerrainMat::TERRAIN_STONE, BED_ELEVATION),
            ));
        }
    }
    let mut twin = Twin::open(adapter, Setup::new(center, 1), &chunks)?;
    let settled = twin.run_until_asleep(20)?;
    check.that(settled.is_some(), || {
        "the flat world never slept".to_owned()
    });
    for tick in 0..30 {
        twin.step(&[])?;
        let report = twin.last.map(|last| last.cells).unwrap_or_default();
        let idle = (
            report.candidate_tiles,
            report.process_tiles,
            report.awake_cells,
            report.active_tiles,
        );
        if idle != (0, 0, 0, 0) {
            check.fail(format!("idle tick {tick}: {report:?}"));
            break;
        }
    }
    // A radius-0 edit in the middle of a tile wakes that tile's cells within 3 of it: one tile
    // and nothing else; at a tile corner, four.
    let middle = cell_of(center, 20, 20);
    twin.step(&[edit(EditTool::Raise, middle.x, middle.y, 0, 1)])?;
    let report = twin.last.map(|last| last.cells).unwrap_or_default();
    check.equal("process tiles of a mid-tile edit", report.process_tiles, 1);
    twin.run_until_asleep(20)?;
    let corner = cell_of(center, 16, 16);
    twin.step(&[edit(EditTool::Raise, corner.x, corner.y, 0, 1)])?;
    let report = twin.last.map(|last| last.cells).unwrap_or_default();
    check.equal("process tiles of a corner edit", report.process_tiles, 4);
    twin.finish(check, "idle");
    Ok(())
}

/// Immature foliage stays awake only while it can grow (after 07). A field of dry young foliage
/// on dirt, with a level water trench through its middle chunk: the foliage the trench waters
/// grows to maturity, the dry foliage keeps its state, and then the whole world sleeps, which
/// it never did while dry foliage held its neighbourhood awake. Water poured on dry foliage
/// wakes it and it grows. The device equals the reference tick for tick throughout.
pub fn dry_foliage_sleeps(adapter: &str, check: &mut Check) -> Result<(), String> {
    use world_model::rules::FOLIAGE_MATURE_STATE;
    let center = ChunkKey::new(2, 2, 0);
    let young = |column: &mut WorldCell| {
        column.set_surface_mat(SurfaceMat::SURFACE_FOLIAGE);
        column.set_surface_state(1);
    };
    // The trench: two units below the bank (capillary reach), filled level with it, on stone so
    // nothing takes root in it.
    let trench = 20..24;
    let mut chunks = Vec::new();
    for y in 1..=3 {
        for x in 1..=3 {
            let key = ChunkKey::new(x, y, 0);
            let mut cells = flat(TerrainMat::TERRAIN_DIRT, BED_ELEVATION);
            for (index, column) in cells.iter_mut().enumerate() {
                let (cx, _) = (index as i32 % SIZE, index as i32 / SIZE);
                if key == center && trench.contains(&cx) {
                    column.set_terrain_mat(TerrainMat::TERRAIN_STONE);
                    column.set_elevation(BED_ELEVATION - 2);
                    column.inject_fluid(FluidMat::FLUID_WATER, 2);
                } else {
                    young(column);
                }
            }
            chunks.push((key, cells));
        }
    }
    let mut twin = Twin::open(adapter, Setup::new(center, 1), &chunks)?;
    let asleep = twin.run_until_asleep(4_000)?;
    check.that(asleep.is_some(), || {
        format!(
            "the world never slept: {:?}",
            twin.last.map(|last| last.cells)
        )
    });
    check.metric("ticks_until_asleep", asleep.unwrap_or(0));
    let captured = twin.compare_cells(check, "settled")?;
    let cells = &captured
        .iter()
        .find(|(key, _)| *key == center)
        .ok_or("no centre chunk")?
        .1;
    let banks: Vec<u16> = (0..SIZE)
        .flat_map(|y| [at(19, y as usize), at(24, y as usize)])
        .map(|index| cells[index].surface_state())
        .collect();
    check.that(
        banks.iter().all(|state| *state == FOLIAGE_MATURE_STATE),
        || format!("watered foliage did not mature: states {banks:?}"),
    );
    let dry: Vec<u16> = [at(5, 5), at(40, 40), at(17, 30), at(26, 30)]
        .into_iter()
        .map(|index| cells[index].surface_state())
        .collect();
    check.equal("dry foliage states", dry, vec![1, 1, 1, 1]);

    // Water poured on dry foliage far from the trench wakes it, and it grows.
    let target = cell_of(ChunkKey::new(1, 1, 0), 30, 30);
    twin.step(&[edit(EditTool::Water, target.x, target.y, 2, 4)])?;
    check.that(
        twin.last.is_some_and(|last| last.cells.process_tiles > 0),
        || "the poured water woke nothing".to_owned(),
    );
    twin.run(600)?;
    let captured = twin.compare_cells(check, "watered")?;
    let cells = &captured
        .iter()
        .find(|(key, _)| *key == ChunkKey::new(1, 1, 0))
        .ok_or("no watered chunk")?
        .1;
    // Growth is a keyed draw per sampled tick, so count the stroke's cells that grew.
    let grown = (28..33)
        .flat_map(|y| (28..33).map(move |x| at(x, y)))
        .filter(|index| cells[*index].surface_state() > 1)
        .count();
    check.metric("watered_cells_grown", grown);
    check.that(grown > 0, || {
        "no foliage under the poured water grew".to_owned()
    });
    twin.finish(check, "dry foliage");
    Ok(())
}

/// A sleeping pond wakes when the climate changes, and a sleeping region wakes when a rule is
/// switched back on (the baseline woke only on climate changes).
pub fn climate_and_rule_reactivation(adapter: &str, check: &mut Check) -> Result<(), String> {
    let key = ChunkKey::new(-7, 4, 0);
    let mut cells = flat(TerrainMat::TERRAIN_STONE, BED_ELEVATION);
    for y in 0..64 {
        for x in 0..64 {
            if (8..56).contains(&x) && (8..56).contains(&y) {
                cells[at(x, y)].inject_fluid(FluidMat::FLUID_WATER, 10);
            } else {
                cells[at(x, y)].set_elevation(BED_ELEVATION + 30);
            }
        }
    }
    let mut twin = Twin::open(
        adapter,
        Setup::new(key, 1).rules(physics_only()),
        &[(key, cells)],
    )?;
    check.that(twin.run_until_asleep(200)?.is_some(), || {
        "the pond never slept".to_owned()
    });
    let cold = AmbientTemperature::celsius(-15.0).map_err(|error| error.to_string())?;
    let results = twin.step(&[Command::SetAmbientTemperature(cold)])?;
    check.equal("configured", results[0], CommandResult::Configured);
    let report = twin.last.map(|last| last.cells).unwrap_or_default();
    check.equal(
        "cells woken by the climate",
        report.awake_cells as usize,
        CHUNK_CELL_COUNT,
    );
    check.equal("tiles woken by the climate", report.process_tiles, 64);
    check.that(twin.run_until_asleep(3000)?.is_some(), || {
        "freezing never finished".to_owned()
    });
    let captured = twin.compare_cells(check, "frozen")?;
    check.that(
        captured[0]
            .1
            .iter()
            .any(|c| c.fluid_mat() == FluidMat::FLUID_ICE),
        || "nothing froze".to_owned(),
    );
    // Liquid off; a fresh column cannot flow and the region falls asleep around it.
    let still = SimulationRules {
        liquid: false,
        ..physics_only()
    };
    let warm = AmbientTemperature::celsius(15.0).map_err(|error| error.to_string())?;
    twin.step(&[
        Command::SetSimulationRules(still),
        Command::SetAmbientTemperature(warm),
    ])?;
    check.that(twin.run_until_asleep(4000)?.is_some(), || {
        "the warm still world never slept".to_owned()
    });
    let column = cell_of(key, 4, 4);
    twin.step(&[edit(EditTool::Water, column.x, column.y, 1, 100)])?;
    check.that(twin.run_until_asleep(50)?.is_some(), || {
        "the still column never slept".to_owned()
    });
    // Switching liquid back on wakes everything once, and the column flows.
    twin.step(&[Command::SetSimulationRules(physics_only())])?;
    let report = twin.last.map(|last| last.cells).unwrap_or_default();
    check.equal(
        "cells woken by the rules",
        report.awake_cells as usize,
        CHUNK_CELL_COUNT,
    );
    check.that(report.fluid_out > 0, || {
        "the column did not flow after liquid came back".to_owned()
    });
    twin.run(100)?;
    twin.compare_cells(check, "rules")?;
    twin.finish(check, "reactivation");
    Ok(())
}

/// A chunk that streams in is woken with its boundary dependants; a retired chunk wakes the
/// border it leaves, which becomes a closed boundary.
pub fn chunk_activation_and_retirement(adapter: &str, check: &mut Check) -> Result<(), String> {
    let center = ChunkKey::new(-3, -3, 0);
    let east = ChunkKey::new(-2, -3, 0);
    let mut dry = flat(TerrainMat::TERRAIN_STONE, BED_ELEVATION);
    dry[at(62, 30)].set_elevation(BED_ELEVATION - 5);
    let mut wet = flat(TerrainMat::TERRAIN_STONE, BED_ELEVATION);
    wet[at(0, 30)].inject_fluid(FluidMat::FLUID_WATER, 200);
    let mut twin = Twin::open(
        adapter,
        Setup::new(center, 1).rules(physics_only()),
        &[(center, dry)],
    )?;
    check.that(twin.run_until_asleep(20)?.is_some(), || {
        "the dry chunk never slept".to_owned()
    });
    twin.activate(&[(east, wet)])?;
    let report = twin.last.map(|last| last.cells).unwrap_or_default();
    // The new chunk is awake throughout, and the bordering tiles of the asleep chunk too.
    check.that(report.awake_cells as usize > CHUNK_CELL_COUNT, || {
        format!("activation woke {} cells", report.awake_cells)
    });
    twin.run(60)?;
    let captured = twin.compare_cells(check, "activation")?;
    let crossed = captured
        .iter()
        .find(|(key, _)| *key == center)
        .is_some_and(|(_, cells)| cells.iter().any(|c| c.fluid_vol() > 0));
    check.that(crossed, || "no water crossed into the old chunk".to_owned());
    twin.run_until_asleep(3000)?;
    twin.retire(east)?;
    twin.step(&[])?;
    let report = twin.last.map(|last| last.cells).unwrap_or_default();
    check.that(report.process_tiles >= 8, || {
        format!("retirement woke {} tiles", report.process_tiles)
    });
    twin.run(60)?;
    twin.compare_cells(check, "retirement")?;
    twin.finish(check, "streaming");
    Ok(())
}

/// The same absolute chunks resident in windows with different origins (and negative keys) give
/// the same world: kernels address by window but hash by absolute position.
pub fn rebased_window(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut chunks = Vec::new();
    for y in 2..=4 {
        for x in -6..=-4 {
            let key = ChunkKey::new(x, y, 0);
            let mut cells = chaotic(0x7E57 + (x * 31 + y) as u64);
            cells[at(10, 10)].set_surface_mat(SurfaceMat::SURFACE_FOLIAGE);
            chunks.push((key, cells));
        }
    }
    let narrow = Setup::new(ChunkKey::new(-5, 3, 0), 1);
    let wide = Setup::new(ChunkKey::new(-4, 4, 0), 2);
    let mut first = Twin::open(adapter, narrow, &chunks)?;
    let mut second = Twin::open(adapter, wide, &chunks)?;
    let edits = [edit(EditTool::Water, -5 * SIZE + 30, 3 * SIZE + 30, 4, 50)];
    first.step(&edits)?;
    second.step(&edits)?;
    first.run(150)?;
    second.run(150)?;
    let a = first.compare_cells(check, "narrow window")?;
    let b = second.compare_cells(check, "wide window")?;
    let differing: usize = a
        .iter()
        .map(|(key, cells)| {
            let other = &b
                .iter()
                .find(|(other, _)| other == key)
                .expect("same chunks")
                .1;
            cells.iter().zip(other).filter(|(x, y)| x != y).count()
        })
        .sum();
    check.equal("cells differing between the windows", differing, 0);
    first.finish(check, "narrow");
    second.finish(check, "wide");
    Ok(())
}

/// Across the low tick word's rollover the keys change and the granular cadence continues; the
/// GPU follows the reference through it.
pub fn tick_rollover(adapter: &str, check: &mut Check) -> Result<(), String> {
    let key = ChunkKey::new(9, -9, 0);
    let mut cells = chaotic(0x2032);
    for cell in cells.iter_mut().step_by(7) {
        cell.set_surface_mat(SurfaceMat::SURFACE_FOLIAGE);
    }
    let start = (1u64 << 32) - 6;
    let mut twin = Twin::open(
        adapter,
        Setup::new(key, 1).first_tick(start),
        &[(key, cells)],
    )?;
    twin.run(20)?;
    let reached = twin.driver.next_tick;
    check.that(reached > Tick::new(1 << 32), || {
        format!("the run stopped at {reached:?}")
    });
    twin.compare_cells(check, "rollover")?;
    twin.finish(check, "rollover");
    Ok(())
}
