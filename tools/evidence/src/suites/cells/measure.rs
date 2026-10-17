//! Checkpoint D: what the material phases cost at the qualification window, dense against
//! sparse, at idle, mixed and full activity, on the selected adapter.
//!
//! Every scenario runs on generated terrain (seed 69) in a radius-6 window and is measured over
//! a fixed number of ticks after a warm-up, submitted one at a time (each tick waits for its
//! readback before the next is submitted, so latency is a round trip, not throughput). Per tick it
//! records the CPU time spent encoding and submitting, the round trip to decoded results, the
//! device time of the three passes when timestamps are available, the tiles and cells the
//! scheduling visited, bytes moved by purpose and process allocations around the port's calls.
//! The structural expectations are checked: an idle world visits no tile, dense visits every
//! tile, every tick balances, and nothing is allocated on the device while measuring.

use std::time::{Duration, Instant};

use world_model::{
    command::{AmbientTemperature, Command, EditTool, MaterialEdit},
    generation::WorldGenerator,
    glam::IVec2,
    rules::SimulationRules,
    spatial::ChunkKey,
};
use world_runtime::port::{AdmittedCommand, Outcome, PhysicalWorld, TickReport, TransferCounters};

use super::{
    super::{Check, alloc, driver::Driver},
    twin::Setup,
};

const RADIUS: u32 = 6;
const WARMUP: u32 = 10;
const MEASURED: u32 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Activity {
    /// Settled terrain with the deterministic rules: nothing is awake.
    Idle,
    /// The deterministic rules on settled terrain, plus eight water strokes a tick moving
    /// across the window: activity around the strokes only.
    Mixed,
    /// Every rule on generated terrain: foliage grows and spreads wherever it is hydrated.
    Growing,
    /// A climate change every tick wakes every resident cell.
    Full,
}

struct Sample {
    encode: Duration,
    report: TickReport,
    /// Process allocations inside the port's submit and poll calls.
    allocations: u64,
}

fn percentile(values: &mut [f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    let rank = ((values.len() - 1) as f64 * p).round() as usize;
    values[rank]
}

fn tick_commands(activity: Activity, tick: u32) -> Vec<Command> {
    match activity {
        Activity::Idle | Activity::Growing => Vec::new(),
        Activity::Mixed => (0..8)
            .map(|k| {
                let x = ((tick * 37 + k * 101) % 800) as i32 - 400;
                let y = ((tick * 23 + k * 59) % 800) as i32 - 400;
                Command::EditMaterial(
                    MaterialEdit::new(EditTool::Water, IVec2::new(x, y), 6, 4).expect("supported"),
                )
            })
            .collect(),
        Activity::Full => {
            let degrees = if tick.is_multiple_of(2) { 12.0 } else { 13.0 };
            vec![Command::SetAmbientTemperature(
                AmbientTemperature::celsius(degrees).expect("supported"),
            )]
        }
    }
}

fn run(adapter: &str, check: &mut Check, dense: bool, activity: Activity) -> Result<(), String> {
    let label = format!(
        "{}_{}",
        if dense { "dense" } else { "sparse" },
        match activity {
            Activity::Idle => "idle",
            Activity::Mixed => "mixed",
            Activity::Growing => "growing",
            Activity::Full => "full",
        }
    );
    let rules = match activity {
        Activity::Idle | Activity::Mixed => SimulationRules::DETERMINISTIC,
        Activity::Growing | Activity::Full => SimulationRules::ALL,
    };
    let mut setup = Setup::new(ChunkKey::new(0, 0, 0), RADIUS).rules(rules);
    setup.timestamps = true;
    if dense {
        setup = setup.dense();
    }
    let mut driver = Driver::open(setup.config(adapter))?;
    let generator = WorldGenerator::new(69);
    let keys: Vec<ChunkKey> = setup.window().view().iter().collect();
    let tiles = keys.len() as u32 * 64;
    for group in keys.chunks(32) {
        for key in group {
            driver.stage_upload(*key, &generator.generate_chunk(*key).cells)?;
        }
        driver
            .submit(&[])
            .map_err(|refusal| format!("activation: {refusal:?}"))?;
        driver.settle()?;
    }
    // An idle or mixed world starts asleep; generated water is level, so this is short.
    if matches!(activity, Activity::Idle | Activity::Mixed) {
        let mut asleep = false;
        for _ in 0..2000 {
            let report = step(&mut driver, &[])?.report;
            if report.cells.awake_cells == 0 {
                asleep = true;
                break;
            }
        }
        check.that(asleep, || {
            format!("{label}: the generated window never slept")
        });
    }
    for tick in 0..WARMUP {
        step(&mut driver, &tick_commands(activity, tick))?;
    }
    let before: TransferCounters = driver.world.counters();
    let mut samples = Vec::with_capacity(MEASURED as usize);
    for tick in 0..MEASURED {
        samples.push(step(&mut driver, &tick_commands(activity, WARMUP + tick))?);
    }
    let after = driver.world.counters();
    let allocations: u64 = samples.iter().map(|sample| sample.allocations).sum();
    let ticks = f64::from(MEASURED);
    let per_tick = |value: u64| (value as f64 / ticks).round() as u64;

    let mut encode: Vec<f64> = samples
        .iter()
        .map(|s| s.encode.as_secs_f64() * 1e6)
        .collect();
    let mut latency: Vec<f64> = samples
        .iter()
        .map(|s| s.report.latency.as_secs_f64() * 1e3)
        .collect();
    let processed: Vec<u32> = samples
        .iter()
        .map(|s| s.report.cells.process_tiles)
        .collect();
    let awake: u64 = samples
        .iter()
        .map(|s| u64::from(s.report.cells.awake_cells))
        .sum();
    let changed: u64 = samples
        .iter()
        .map(|s| u64::from(s.report.cells.changed_cells))
        .sum();
    let processed_mean = processed.iter().map(|tiles| f64::from(*tiles)).sum::<f64>() / ticks;
    check.metric(&format!("{label}_tiles"), u64::from(tiles));
    check.metric(
        &format!("{label}_process_tiles_mean"),
        processed_mean.round() as u64,
    );
    check.metric(
        &format!("{label}_process_fraction_permille"),
        (processed_mean / f64::from(tiles) * 1000.0).round() as u64,
    );
    check.metric(&format!("{label}_awake_cells_per_tick"), per_tick(awake));
    check.metric(
        &format!("{label}_changed_cells_per_tick"),
        per_tick(changed),
    );
    check.metric(
        &format!("{label}_encode_us_median"),
        percentile(&mut encode, 0.5).round() as u64,
    );
    check.metric(
        &format!("{label}_encode_us_p95"),
        percentile(&mut encode, 0.95).round() as u64,
    );
    check.metric(
        &format!("{label}_round_trip_ms_median"),
        (percentile(&mut latency, 0.5) * 100.0).round() / 100.0,
    );
    check.metric(
        &format!("{label}_round_trip_ms_p95"),
        (percentile(&mut latency, 0.95) * 100.0).round() / 100.0,
    );
    let phases: Vec<_> = samples.iter().filter_map(|s| s.report.phases).collect();
    if phases.len() == samples.len() {
        for (name, pick) in [("commands", 0usize), ("activity", 1), ("material", 2)] {
            let mut values: Vec<f64> = phases
                .iter()
                .map(|p| [p.commands, p.activity, p.material][pick].as_secs_f64() * 1e6)
                .collect();
            check.metric(
                &format!("{label}_gpu_{name}_us_median"),
                percentile(&mut values, 0.5).round() as u64,
            );
            check.metric(
                &format!("{label}_gpu_{name}_us_p95"),
                percentile(&mut values, 0.95).round() as u64,
            );
        }
    } else {
        check.note(format!(
            "{label}: pass timestamps unavailable on this adapter"
        ));
    }
    // Bytes the material phases write per tick, from the visited cells (an analytical count of
    // the kernels' stores, not a hardware counter): a proposal and a resolution word and a
    // scratch cell per awake cell (every cell in dense execution) and a committed cell per change.
    let visited = if dense {
        u64::from(tiles) * 64 * u64::from(MEASURED)
    } else {
        awake
    };
    let committed = if dense {
        u64::from(tiles) * 64 * u64::from(MEASURED)
    } else {
        changed
    };
    check.metric(
        &format!("{label}_material_store_bytes_per_tick"),
        per_tick(visited * 16 + committed * 8),
    );
    check.metric(
        &format!("{label}_upload_bytes_per_tick"),
        per_tick(after.upload.total() - before.upload.total()),
    );
    check.metric(
        &format!("{label}_readback_bytes_per_tick"),
        per_tick(after.readback.total() - before.readback.total()),
    );
    check.metric(
        &format!("{label}_device_clear_bytes_per_tick"),
        per_tick(after.device_clear_bytes - before.device_clear_bytes),
    );
    check.metric(
        &format!("{label}_process_allocations_per_tick"),
        per_tick(allocations),
    );
    check.equal(
        &format!("{label}: buffers created while measuring"),
        after.buffers_created,
        before.buffers_created,
    );
    check.equal(&format!("{label}: frame waits"), after.frame_waits, 0);
    let inconsistent = samples
        .iter()
        .filter(|s| !s.report.cells.consistent())
        .count();
    check.equal(&format!("{label}: inconsistent ticks"), inconsistent, 0);
    match (dense, activity) {
        (false, Activity::Idle) => {
            check.that(processed.iter().all(|tiles| *tiles == 0), || {
                format!("{label}: an idle tick visited tiles: {processed:?}")
            });
        }
        (true, _) => check.that(processed.iter().all(|t| *t == tiles), || {
            format!("{label}: dense skipped tiles")
        }),
        (false, Activity::Full) => check.that(processed.iter().all(|t| *t == tiles), || {
            format!("{label}: a woken tick skipped tiles")
        }),
        (false, Activity::Mixed | Activity::Growing) => check
            .that(processed_mean < f64::from(tiles), || {
                format!("{label}: partial activity visited every tile")
            }),
    }
    let plan = driver.world.plan();
    check.metric(
        "plan_flow_bytes",
        plan.buffer(gpu_world::plan::BufferRole::Flow).size,
    );
    check.metric(
        "plan_scratch_bytes",
        plan.buffer(gpu_world::plan::BufferRole::Scratch).size,
    );
    check.metric(
        "plan_activity_bytes",
        plan.buffer(gpu_world::plan::BufferRole::Activity).size,
    );
    check.metric("plan_terrain_bytes", plan.terrain_bytes);
    check.metric("plan_device_bytes", plan.device_bytes);
    Ok(())
}

/// Submits one tick of `commands`, timing the encode-and-submit call, and waits for its report.
fn step(driver: &mut Driver, commands: &[Command]) -> Result<Sample, String> {
    let tick = driver.next_tick;
    driver.outcomes.clear();
    let admitted: Vec<AdmittedCommand> = commands
        .iter()
        .map(|command| AdmittedCommand {
            id: driver.next_id(),
            command: *command,
        })
        .collect();
    let allocations_before = alloc::snapshot().0;
    let started = Instant::now();
    driver
        .submit_admitted(&admitted)
        .map_err(|refusal| format!("submit: {refusal:?}"))?;
    let encode = started.elapsed();
    driver.settle()?;
    let allocations = alloc::snapshot().0 - allocations_before;
    let report = driver
        .outcomes
        .iter()
        .find_map(|outcome| match outcome {
            Outcome::TickCompleted(report) if report.tick == tick => Some(*report),
            _ => None,
        })
        .ok_or("no report")?;
    Ok(Sample {
        encode,
        report,
        allocations,
    })
}

pub fn activity_measurement(adapter: &str, check: &mut Check) -> Result<(), String> {
    for dense in [false, true] {
        for activity in [
            Activity::Idle,
            Activity::Mixed,
            Activity::Growing,
            Activity::Full,
        ] {
            if dense && matches!(activity, Activity::Mixed | Activity::Growing) {
                continue;
            }
            run(adapter, check, dense, activity)?;
        }
    }
    Ok(())
}
