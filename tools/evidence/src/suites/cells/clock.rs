//! The fixed logical clock through the runtime and the GPU world: whole ticks from frame time, the
//! catch-up cap, and the same world at the same tick whatever the presentation rate.

use std::time::Duration;

use world_model::{
    cell::WorldCell,
    command::{Command, EditTool, MaterialEdit},
    glam::IVec2,
    material::{FluidMat, TerrainMat},
    spatial::{CHUNK_CELL_COUNT, ChunkKey},
};
use world_runtime::{
    ids::{CommandId, Tick},
    ledger::ResidencyOutcome,
    port::SlotRef,
    runtime::{Runtime, RuntimeConfig},
};

use super::{
    super::{
        Check,
        transfers::{frames_until, runtime_upload},
    },
    intentions::{BED_ELEVATION, flat, physics_only},
    twin::{Setup, at},
};

fn runtime(adapter: &str, setup: Setup, config: RuntimeConfig) -> Result<Runtime, String> {
    let world = gpu_world::world::GpuWorld::open(setup.config(adapter))
        .map_err(|error| format!("open: {error:?}"))?;
    Runtime::new(Box::new(world), config).map_err(|error| format!("runtime: {error:?}"))
}

/// The baseline's `fixed_clock_runs_whole_ticks_and_caps_catch_up`, through the GPU world: 3.5
/// tick intervals run 3 ticks; a one-second stall runs the cap of 4 and discards the rest.
pub fn fixed_clock(adapter: &str, check: &mut Check) -> Result<(), String> {
    let config = RuntimeConfig {
        tick_rate_hz: 60.0,
        max_ticks_per_frame: 4,
        ..RuntimeConfig::default()
    };
    let mut runtime = runtime(adapter, Setup::new(ChunkKey::new(0, 0, 0), 1), config)?;
    let first = runtime.frame(Duration::from_secs_f64(3.5 / 60.0));
    check.equal("ticks due after 3.5 intervals", first.ticks_due, 3);
    owed_ticks(&mut runtime)?;
    check.equal(
        "last tick after 3.5 intervals",
        runtime.last_submitted_tick(),
        Some(Tick::new(2)),
    );
    let second = runtime.frame(Duration::from_secs(1));
    check.equal("ticks due after a one-second frame", second.ticks_due, 4);
    owed_ticks(&mut runtime)?;
    check.equal(
        "last tick after the stall",
        runtime.last_submitted_tick(),
        Some(Tick::new(6)),
    );
    check.equal("discarded ticks", runtime.discarded_ticks(), 56);
    Ok(())
}

/// Lets the runtime submit the ticks it still owes. The device may refuse some for in-flight
/// pressure; they stay due (within the cap) and go out on later, empty frames.
fn owed_ticks(runtime: &mut Runtime) -> Result<(), String> {
    let started = std::time::Instant::now();
    loop {
        let report = runtime.frame(Duration::ZERO);
        let counters = runtime.counters();
        if report.ticks_due == 0 && counters.ticks_observed == counters.ticks_submitted {
            return Ok(());
        }
        if started.elapsed() > super::super::driver::DEVICE_PATIENCE {
            return Err("owed ticks never drained".to_owned());
        }
        std::thread::sleep(Duration::from_micros(200));
    }
}

/// One frame pacing: the frame durations, repeating.
struct Pacing {
    name: &'static str,
    frames: &'static [f64],
}

const PACINGS: [Pacing; 4] = [
    Pacing {
        name: "30 Hz",
        frames: &[1.0 / 30.0],
    },
    Pacing {
        name: "60 Hz",
        frames: &[1.0 / 60.0],
    },
    Pacing {
        name: "144 Hz",
        frames: &[1.0 / 144.0],
    },
    // A 60 Hz stream with a 400 ms stall every seventh frame: each stall owes 24 ticks, four run.
    Pacing {
        name: "60 Hz with stalls",
        frames: &[
            1.0 / 60.0,
            1.0 / 60.0,
            1.0 / 60.0,
            1.0 / 60.0,
            1.0 / 60.0,
            1.0 / 60.0,
            0.4,
        ],
    },
];

/// The same tick-scheduled inputs at four presentation pacings, including overload: the world at
/// one logical tick is identical in all of them, and the stalled run reports the simulated time it
/// discarded instead of running more ticks per frame.
pub fn presentation_rates(adapter: &str, check: &mut Check) -> Result<(), String> {
    const TARGET: u64 = 160;
    let key = ChunkKey::new(-1, 6, 0);
    let mut cells = flat(TerrainMat::TERRAIN_STONE, BED_ELEVATION);
    cells[at(30, 30)].inject_fluid(FluidMat::FLUID_WATER, 200);
    let origin = IVec2::new(key.key.x * 64, key.key.y * 64);
    let strokes = [(12, 10, 10), (40, 50, 12), (75, 20, 44)];
    let mut worlds: Vec<(&'static str, Vec<WorldCell>, u64)> = Vec::new();
    for pacing in &PACINGS {
        let setup = Setup::new(key, 1).rules(physics_only());
        let mut runtime = runtime(adapter, setup, RuntimeConfig::default())?;
        let slot = runtime_upload(&mut runtime, key, &cells)?;
        let start = runtime
            .last_submitted_tick()
            .map_or(0, |tick| tick.value() + 1);
        for (index, (tick, x, y)) in strokes.iter().enumerate() {
            let command = Command::EditMaterial(
                MaterialEdit::new(EditTool::Water, origin + IVec2::new(*x, *y), 2, 30)
                    .expect("supported"),
            );
            runtime
                .submit_at(
                    CommandId::new(index as u64 + 1),
                    command,
                    Tick::new(start + tick),
                )
                .map_err(|error| format!("submit: {error:?}"))?;
        }
        let mut frame = 0;
        let mut stalls = 0u64;
        while runtime
            .last_submitted_tick()
            .is_none_or(|tick| tick.value() < start + TARGET - 4)
        {
            let seconds = pacing.frames[frame % pacing.frames.len()];
            stalls += u64::from(seconds > 0.1);
            runtime.frame(Duration::from_secs_f64(seconds));
            frame += 1;
            // The frames are simulated, not slept: let the device finish this frame's ticks
            // before the next frame, as a real frame of this length would. Otherwise in-flight
            // backpressure, not the pacing, would decide how many ticks stay owed.
            idle(&mut runtime)?;
        }
        // Every pacing ends within the catch-up cap of the target; single ticks reach it exactly.
        while runtime
            .last_submitted_tick()
            .is_some_and(|tick| tick.value() < start + TARGET)
        {
            runtime.frame_ticks(1);
        }
        check.equal(
            &format!("{}: last tick", pacing.name),
            runtime.last_submitted_tick(),
            Some(Tick::new(start + TARGET)),
        );
        // The capture holds the cells at the end of the tick after the target, in every run.
        let captured = capture(&mut runtime, slot)?;
        let discarded = runtime.discarded_ticks();
        check.metric(&format!("{}_frames", pacing.name), frame as u64);
        check.metric(&format!("{}_discarded_ticks", pacing.name), discarded);
        check.metric(
            &format!("{}_discarded_ms", pacing.name),
            (discarded as f64 * runtime.tick_duration().as_secs_f64() * 1000.0).round() as u64,
        );
        let ledger = runtime.ledger().summary();
        check.equal(
            &format!("{}: inconsistent ticks", pacing.name),
            ledger.inconsistent_ticks,
            0,
        );
        check.metric(&format!("{}_stalls", pacing.name), stalls);
        if stalls > 0 {
            // A 400 ms stall owes 24 ticks at 60 Hz; the cap of 4 runs and 20 are discarded.
            check.equal(
                &format!("{}: discarded ticks", pacing.name),
                discarded,
                20 * stalls,
            );
        }
        worlds.push((pacing.name, captured, discarded));
    }
    let (_, reference, _) = &worlds[0];
    for (name, cells, _) in &worlds[1..] {
        let differing = cells.iter().zip(reference).filter(|(a, b)| a != b).count();
        check.equal(
            &format!("cells of {name} differing from 30 Hz"),
            differing,
            0,
        );
    }
    let discarded: Vec<u64> = worlds.iter().map(|(_, _, discarded)| *discarded).collect();
    check.equal("discards without stalls", &discarded[..3], &[0, 0, 0][..]);
    check.that(discarded[3] > 0, || {
        "the stalled run discarded nothing".to_owned()
    });
    Ok(())
}

/// Polls without submitting until every submitted tick has been observed.
fn idle(runtime: &mut Runtime) -> Result<(), String> {
    let started = std::time::Instant::now();
    loop {
        let counters = runtime.counters();
        if counters.ticks_observed == counters.ticks_submitted {
            return Ok(());
        }
        if started.elapsed() > super::super::driver::DEVICE_PATIENCE {
            return Err("submitted ticks were never observed".to_owned());
        }
        runtime.frame_ticks(0);
        std::thread::sleep(Duration::from_micros(100));
    }
}

/// Captures a resident chunk through the runtime at the end of the next tick.
fn capture(runtime: &mut Runtime, slot: SlotRef) -> Result<Vec<WorldCell>, String> {
    let ticket = runtime
        .capture_chunk(slot)
        .map_err(|error| format!("capture: {error:?}"))?;
    let mut ready = Vec::new();
    frames_until(runtime, "capture", |runtime| {
        runtime.drain_residency(&mut ready);
        ready
            .iter()
            .any(|outcome| matches!(outcome, ResidencyOutcome::CaptureReady { ticket: done, .. } if *done == ticket))
    })?;
    let mut cells = vec![WorldCell::default(); CHUNK_CELL_COUNT];
    runtime
        .take_capture(ticket, &mut cells)
        .map_err(|refusal| format!("take: {refusal:?}"))?;
    Ok(cells)
}
