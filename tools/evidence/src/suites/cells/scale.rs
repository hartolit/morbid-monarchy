//! Stochastic rules, dense against sparse execution, long runs and the largest work: keyed growth
//! and fire, dense/sparse agreement with every rule on, settlement over thousands of ticks, totals
//! beyond 32 bits, and a fully active world whose tile list exceeds one dispatch dimension.

use world_model::{
    cell::WorldCell,
    command::{AmbientTemperature, Command, EditTool, MaterialEdit},
    generation::WorldGenerator,
    glam::IVec2,
    material::{FluidMat, GranularMat, SurfaceMat, TerrainMat},
    observation::WorldTotals,
    spatial::{CHUNK_CELL_COUNT, CHUNK_SIZE, ChunkKey},
};
use world_runtime::port::{CellReport, CommandResult};

use super::{
    super::Check,
    intentions::{BED_ELEVATION, chaotic, flat, mixer, physics_only},
    twin::{Setup, Twin, at},
};

const SIZE: i32 = CHUNK_SIZE as i32;

/// Dirt with a water film, seeding foliage, a burning oil pool, magma beside fuel and ash.
fn living_chunk(seed: u64) -> Vec<WorldCell> {
    let mut next = mixer(seed);
    let mut cells = flat(TerrainMat::TERRAIN_DIRT, BED_ELEVATION);
    for (index, cell) in cells.iter_mut().enumerate() {
        let (x, y) = (index % CHUNK_SIZE, index / CHUNK_SIZE);
        cell.set_elevation(BED_ELEVATION + (next() % 3) as u16);
        if x < 32 && next().is_multiple_of(3) {
            cell.inject_fluid(FluidMat::FLUID_WATER, 2);
        }
        if next().is_multiple_of(40) {
            cell.set_surface_mat(SurfaceMat::SURFACE_FOLIAGE);
            cell.set_surface_state(3);
        }
        if (40..48).contains(&x) && (40..48).contains(&y) {
            cell.inject_fluid(FluidMat::FLUID_OIL, 6);
            if x == 44 && y == 44 {
                cell.set_surface_mat(SurfaceMat::SURFACE_FIRE);
            }
        }
        if (50..54).contains(&x) && (10..14).contains(&y) {
            cell.set_surface_mat(SurfaceMat::SURFACE_ASH);
            cell.set_granular_mat(GranularMat::GRANULAR_MUD);
            cell.set_granular_vol(3);
        }
    }
    cells[at(20, 50)].inject_fluid(FluidMat::FLUID_MAGMA, 20);
    cells[at(21, 50)].set_surface_mat(SurfaceMat::SURFACE_WOOD);
    cells
}

fn growth(cells: &[WorldCell]) -> usize {
    cells
        .iter()
        .filter(|c| c.surface_mat() == SurfaceMat::SURFACE_FOLIAGE)
        .count()
}

/// Growth and fire with keyed draws follow the reference exactly, reproduce in a new session with
/// the same seed, and differ with another seed.
pub fn keyed_growth_and_fire(adapter: &str, check: &mut Check) -> Result<(), String> {
    let key = ChunkKey::new(5, 7, 0);
    let cells = living_chunk(0xB10);
    let run = |seed: u64,
               check: &mut Check,
               label: &str|
     -> Result<(Vec<WorldCell>, CellTotals), String> {
        let mut twin = Twin::open(
            adapter,
            Setup::new(key, 1).seed(seed).ambient(18.0),
            &[(key, cells.clone())],
        )?;
        let mut sums = CellTotals::default();
        for _ in 0..400 {
            twin.step(&[])?;
            sums.add(&twin.last.map(|last| last.cells).unwrap_or_default());
        }
        let captured = twin.compare_cells(check, label)?;
        twin.finish(check, label);
        Ok((captured[0].1.clone(), sums))
    };
    let (first, sums) = run(11, check, "seed 11")?;
    let (again, _) = run(11, check, "seed 11 again")?;
    let (other, _) = run(12, check, "seed 12")?;
    check.equal(
        "cells differing between two sessions with one seed",
        first.iter().zip(&again).filter(|(a, b)| a != b).count(),
        0,
    );
    check.that(first != other, || {
        "another seed gave the same world".to_owned()
    });
    check.that(growth(&first) > growth(&cells), || {
        "nothing grew".to_owned()
    });
    check.that(sums.burn > 0, || "nothing burned".to_owned());
    check.that(sums.uptake > 0, || "no foliage drew water".to_owned());
    check.metric("foliage_before", growth(&cells) as u64);
    check.metric("foliage_after", growth(&first) as u64);
    check.metric("burned_units", sums.burn);
    check.metric("uptake_units", sums.uptake);
    Ok(())
}

#[derive(Debug, Default, Clone, Copy)]
struct CellTotals {
    burn: u64,
    uptake: u64,
    process_tiles: u64,
}

impl CellTotals {
    fn add(&mut self, report: &CellReport) {
        self.burn += report.burn;
        self.uptake += report.uptake;
        self.process_tiles += u64::from(report.process_tiles);
    }
}

/// Fields of two reports that must agree between dense and sparse execution: everything but the
/// tiles each visited.
fn same_semantics(a: &CellReport, b: &CellReport) -> bool {
    CellReport {
        candidate_tiles: 0,
        process_tiles: 0,
        active_tiles: 0,
        ..*a
    } == CellReport {
        candidate_tiles: 0,
        process_tiles: 0,
        active_tiles: 0,
        ..*b
    }
}

fn mixed_chunks(center: ChunkKey) -> Vec<(ChunkKey, Vec<WorldCell>)> {
    let mut chunks = Vec::new();
    for dy in -1..=1 {
        for dx in -1..=1 {
            let key = ChunkKey::new(center.key.x + dx, center.key.y + dy, 0);
            let seed = (dx * 7 + dy * 3 + 40) as u64;
            let cells = if (dx + dy) % 2 == 0 {
                chaotic(seed)
            } else {
                living_chunk(seed)
            };
            chunks.push((key, cells));
        }
    }
    chunks
}

/// Dense and sparse execution under the same selected semantics: every tick's accounting and
/// activity agree, the cells agree, and both equal the reference, with every rule on.
pub fn dense_sparse_agreement(adapter: &str, check: &mut Check) -> Result<(), String> {
    let center = ChunkKey::new(-8, 2, 0);
    let chunks = mixed_chunks(center);
    let setup = Setup::new(center, 1).ambient(1.0);
    let mut sparse = Twin::open(adapter, setup, &chunks)?;
    let mut dense = Twin::open(adapter, setup.dense(), &chunks)?;
    let mut disagreeing = 0;
    let (mut sparse_tiles, mut dense_tiles) = (0u64, 0u64);
    let stroke = Command::EditMaterial(
        MaterialEdit::new(
            EditTool::Lava,
            IVec2::new(center.key.x * SIZE + 5, center.key.y * SIZE + 60),
            3,
            30,
        )
        .expect("supported"),
    );
    for tick in 0..300 {
        let commands: &[Command] = if tick % 50 == 10 {
            std::slice::from_ref(&stroke)
        } else {
            &[]
        };
        sparse.step(commands)?;
        dense.step(commands)?;
        let (a, b) = (
            sparse.last.map(|last| last.cells).unwrap_or_default(),
            dense.last.map(|last| last.cells).unwrap_or_default(),
        );
        sparse_tiles += u64::from(a.process_tiles);
        dense_tiles += u64::from(b.process_tiles);
        if !same_semantics(&a, &b) {
            disagreeing += 1;
            if disagreeing <= 3 {
                check.fail(format!("tick {tick}: sparse {a:?} dense {b:?}"));
            }
        }
    }
    check.equal("ticks where dense and sparse disagree", disagreeing, 0);
    let a = sparse.compare_cells(check, "sparse")?;
    let b = dense.compare_cells(check, "dense")?;
    let differing: usize = a
        .iter()
        .zip(&b)
        .map(|((_, x), (_, y))| x.iter().zip(y).filter(|(p, q)| p != q).count())
        .sum();
    check.equal("cells differing between dense and sparse", differing, 0);
    check.that(sparse_tiles < dense_tiles, || {
        "sparse visited as many tiles as dense".to_owned()
    });
    check.metric("sparse_process_tiles", sparse_tiles);
    check.metric("dense_process_tiles", dense_tiles);
    sparse.finish(check, "sparse");
    dense.finish(check, "dense");
    Ok(())
}

/// Nine chunks of chaotic terrain run until the whole window sleeps, against the reference every
/// tick, with the accounting balanced every tick and the totals unchanged at the end.
pub fn long_settlement(adapter: &str, check: &mut Check) -> Result<(), String> {
    let center = ChunkKey::new(12, -12, 0);
    let mut chunks = Vec::new();
    for dy in -1..=1 {
        for dx in -1..=1 {
            let key = ChunkKey::new(center.key.x + dx, center.key.y + dy, 0);
            chunks.push((key, chaotic((dx * 5 + dy + 90) as u64)));
        }
    }
    let initial = chunks
        .iter()
        .fold(WorldTotals::default(), |mut totals, (_, cells)| {
            for cell in cells {
                totals.add(cell);
            }
            totals
        });
    let mut twin = Twin::open(
        adapter,
        Setup::new(center, 1).rules(physics_only()),
        &chunks,
    )?;
    let settled = twin.run_until_asleep(30_000)?;
    check.that(settled.is_some(), || {
        "the chaotic window never fell asleep".to_owned()
    });
    check.metric("settled_after_ticks", settled.unwrap_or(0));
    let results = twin.step(&[Command::MeasureTotals])?;
    let CommandResult::Totals(after) = results[0] else {
        return Err(format!("totals: {:?}", results[0]));
    };
    check.equal("fluid", after.fluid_volume(), initial.fluid_volume());
    check.equal("solid", after.solid(), initial.solid());
    twin.compare_cells(check, "settled")?;
    twin.finish(check, "settlement");
    Ok(())
}

fn full_column() -> WorldCell {
    let mut cell = WorldCell::default();
    cell.set_terrain_mat(TerrainMat::TERRAIN_DIRT);
    cell.set_elevation(WorldCell::MAX_ELEVATION as u16);
    cell.set_granular_mat(GranularMat::GRANULAR_SAND);
    cell.set_granular_vol(WorldCell::MAX_GRANULAR_VOL);
    cell.inject_fluid(FluidMat::FLUID_WATER, WorldCell::MAX_FLUID_VOL);
    cell
}

/// A radius-8 window of full columns: the totals exceed 32 bits and are exact; strokes at every
/// field's maximum add or remove exactly what the columns allow.
pub fn extreme_capacity(adapter: &str, check: &mut Check) -> Result<(), String> {
    let center = ChunkKey::new(-20, 20, 0);
    let setup = Setup::new(center, 8);
    let window = setup.window();
    let chunks: Vec<(ChunkKey, Vec<WorldCell>)> = window
        .view()
        .iter()
        .map(|key| (key, vec![full_column(); CHUNK_CELL_COUNT]))
        .collect();
    let mut twin = Twin::open(adapter, setup, &chunks)?;
    let cells = (chunks.len() * CHUNK_CELL_COUNT) as u64;
    let expected = WorldTotals {
        fluid: [0, 511 * cells, 0, 0, 0, 0, 0, 0],
        elevation: 4095 * cells,
        granular: 15 * cells,
        cells,
    };
    check.that(expected.elevation > u64::from(u32::MAX), || {
        "the fixture stays within 32 bits".to_owned()
    });
    let center_cell = IVec2::new(center.key.x * SIZE + 32, center.key.y * SIZE + 32);
    let at_max = |tool: EditTool| {
        Command::EditMaterial(MaterialEdit::new(tool, center_cell, 64, 255).expect("supported"))
    };
    let results = twin.step(&[
        Command::MeasureTotals,
        at_max(EditTool::Water),
        at_max(EditTool::Raise),
        at_max(EditTool::Sand),
        Command::MeasureTotals,
        at_max(EditTool::Lower),
        Command::MeasureTotals,
    ])?;
    let totals = |index: usize| match results[index] {
        CommandResult::Totals(totals) => Ok(totals),
        other => Err(format!("totals {index}: {other:?}")),
    };
    check.equal("totals of full columns", totals(0)?, expected);
    check.equal("full columns take nothing more", totals(4)?, expected);
    let report = twin.last.map(|last| last.cells).unwrap_or_default();
    // Radius 64 touches 12,853 cells; lowering 255 from 4,095 each.
    check.equal("lowered units", report.tool_solid_out, 12_853 * 255);
    check.equal(
        "totals after lowering",
        totals(6)?.solid(),
        expected.solid() - 12_853 * 255,
    );
    check.equal(
        "nothing added",
        (report.tool_fluid_in, report.tool_solid_in),
        (0, 0),
    );
    twin.run(4)?;
    twin.finish(check, "capacity");
    Ok(())
}

/// A radius-16 window of generated terrain fully woken by a climate change: 69,696 tiles, more
/// than one dispatch dimension holds, all processed, in sparse and in dense execution, with the
/// same accounting.
pub fn full_activity_dispatch_split(adapter: &str, check: &mut Check) -> Result<(), String> {
    let center = ChunkKey::new(3, -4, 0);
    let generator = WorldGenerator::new(69);
    let mut reports: Vec<Vec<CellReport>> = Vec::new();
    for dense in [false, true] {
        let mut setup = Setup::new(center, 16).rules(physics_only());
        if dense {
            setup = setup.dense();
        }
        let chunks: Vec<(ChunkKey, Vec<WorldCell>)> = setup
            .window()
            .view()
            .iter()
            .map(|key| (key, generator.generate_chunk(key).cells.into_vec()))
            .collect();
        let tiles = chunks.len() as u32 * 64;
        let mut driver = super::super::driver::Driver::open(setup.config(adapter))?;
        for group in chunks.chunks(32) {
            for (key, cells) in group {
                driver.stage_upload(*key, cells)?;
            }
            driver
                .submit(&[])
                .map_err(|refusal| format!("tick: {refusal:?}"))?;
            driver.settle()?;
        }
        let cold = AmbientTemperature::celsius(-3.0).map_err(|error| error.to_string())?;
        let mut run = Vec::new();
        for tick in 0..3 {
            let commands = if tick == 0 {
                vec![Command::SetAmbientTemperature(cold)]
            } else {
                Vec::new()
            };
            driver.outcomes.clear();
            let ids = driver
                .submit(&commands)
                .map_err(|refusal| format!("tick: {refusal:?}"))?;
            driver.settle()?;
            let report = driver
                .outcomes
                .iter()
                .find_map(|outcome| match outcome {
                    world_runtime::port::Outcome::TickCompleted(report) => Some(report.cells),
                    _ => None,
                })
                .ok_or("no report")?;
            if tick == 0 {
                check.equal(
                    &format!("dense {dense}: configured"),
                    driver.result(ids[0]),
                    Some(CommandResult::Configured),
                );
                check.equal(
                    &format!("dense {dense}: tiles processed"),
                    report.process_tiles,
                    tiles,
                );
                check.equal(
                    &format!("dense {dense}: cells awake"),
                    u64::from(report.awake_cells),
                    u64::from(tiles) * 64,
                );
            }
            check.that(report.consistent(), || {
                format!("dense {dense}: tick {tick} does not balance")
            });
            run.push(report);
        }
        check.metric(
            if dense { "dense_tiles" } else { "sparse_tiles" },
            u64::from(tiles),
        );
        reports.push(run);
    }
    for (tick, (a, b)) in reports[0].iter().zip(&reports[1]).enumerate() {
        check.that(same_semantics(a, b), || {
            format!("tick {tick}: sparse {a:?} dense {b:?}")
        });
    }
    Ok(())
}
