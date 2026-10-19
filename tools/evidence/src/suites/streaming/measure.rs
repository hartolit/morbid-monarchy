//! Measurements of streaming and saving at the qualification radius, and the ordinary resident
//! simulation's traffic without either.

use std::time::{Duration, Instant};

use world_model::{command::EditTool, glam::IVec2, rules::SimulationRules};
use world_runtime::{ids::SessionEpoch, port::TransferCounters};
use world_store::faults::StoreFaults;

use super::{
    super::{Check, alloc},
    residency::stroke,
    world::{self, Live, Scratch, open, open_store, session_config, window},
};

/// The qualification profile's radius.
const RADIUS: u32 = 6;

fn percentile(values: &mut [f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    values[((values.len() - 1) as f64 * p).round() as usize]
}

/// Frame CPU times and process allocations over a number of frames.
struct Phase {
    frame_ms: Vec<f64>,
    allocations: u64,
    frames: u32,
}

/// Runs `frames` frames of one tick, paced like a display so the device keeps up and the time
/// measured is the frame's CPU cost.
fn phase(live: &mut Live, frames: u32) -> Phase {
    let mut frame_ms = Vec::with_capacity(frames as usize);
    let (before, _) = alloc::snapshot();
    for _ in 0..frames {
        let started = Instant::now();
        live.session.frame_ticks(1);
        frame_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        std::thread::sleep(Duration::from_micros(500));
    }
    let (after, _) = alloc::snapshot();
    Phase {
        frame_ms,
        allocations: after - before,
        frames,
    }
}

/// Runs frames of one tick, paced the same way, until `done` holds.
fn phase_until(
    live: &mut Live,
    what: &str,
    mut done: impl FnMut(&world_runtime::session::WorldSession) -> bool,
) -> Result<Phase, String> {
    let mut frame_ms = Vec::new();
    let (before, _) = alloc::snapshot();
    while !done(&live.session) {
        if frame_ms.len() > 5000 {
            return Err(format!("{what}: not done after {} frames", frame_ms.len()));
        }
        let started = Instant::now();
        live.session.frame_ticks(1);
        frame_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        std::thread::sleep(Duration::from_micros(500));
    }
    let (after, _) = alloc::snapshot();
    let frames = frame_ms.len() as u32;
    Ok(Phase {
        frame_ms,
        allocations: after - before,
        frames,
    })
}

/// Completes the submitted ticks without submitting more.
fn drain(live: &mut Live) -> Result<(), String> {
    let started = Instant::now();
    loop {
        let counters = live.session.runtime().counters();
        if counters.ticks_observed == counters.ticks_submitted {
            return Ok(());
        }
        if started.elapsed() > Duration::from_secs(20) {
            return Err("submitted ticks were never observed".to_owned());
        }
        live.session.frame_ticks(0);
        std::thread::sleep(Duration::from_micros(100));
    }
}

fn report(check: &mut Check, label: &str, mut phase: Phase) {
    let max = phase.frame_ms.iter().copied().fold(0.0, f64::max);
    check.metric(
        &format!("{label}_frame_cpu_ms_p50"),
        percentile(&mut phase.frame_ms, 0.5),
    );
    check.metric(
        &format!("{label}_frame_cpu_ms_p95"),
        percentile(&mut phase.frame_ms, 0.95),
    );
    check.metric(
        &format!("{label}_frame_cpu_ms_p99"),
        percentile(&mut phase.frame_ms, 0.99),
    );
    check.metric(&format!("{label}_frame_cpu_ms_max"), max);
    check.metric(
        &format!("{label}_allocations_per_frame"),
        phase.allocations as f64 / f64::from(phase.frames.max(1)),
    );
}

/// Chunk upload bytes, snapshot readback bytes, snapshot device-copy bytes, all readback bytes,
/// ticks submitted and transfer-only submissions between two counter readings.
fn delta(after: &TransferCounters, before: &TransferCounters) -> [u64; 6] {
    [
        after.upload.chunks - before.upload.chunks,
        after.readback.snapshots - before.readback.snapshots,
        after.snapshot_copy_bytes - before.snapshot_copy_bytes,
        after.readback.total() - before.readback.total(),
        after.ticks_submitted - before.ticks_submitted,
        after.transfer_submissions - before.transfer_submissions,
    ]
}

/// Walks the qualification window east chunk by chunk with changed chunks among those leaving,
/// and saves halfway: bytes moved per chunk, retained memory, load and write latency, frames per
/// move, and the frame's CPU time and allocations against a resident baseline.
pub fn streaming_measure(adapter: &str, check: &mut Check) -> Result<(), String> {
    let scratch = Scratch::new("measure")?;
    let path = scratch.path("world.redb");
    world::create(&path, window(0, 0, RADIUS), SimulationRules::ALL)?;
    let store = open_store(&path, StoreFaults::new())?;
    let config = session_config();
    let retained_budget = config.residency.retained_bytes;
    let mut live = open(adapter, store, SessionEpoch::new(2), config, |_| {})?;
    let settle_started = Instant::now();
    let settle_frames = live.settle()?;
    check.metric("settle_frames", settle_frames);
    check.metric("settle_ms", settle_started.elapsed().as_secs_f64() * 1000.0);
    check.metric(
        "settle_upload_bytes",
        live.session.runtime().counters().upload.chunks,
    );

    let resident = phase(&mut live, 300);
    report(check, "resident", resident);

    // A save of the resident window alone, its frames against the resident ones.
    let before = live.session.runtime().counters();
    live.session.save();
    let saving = phase_until(&mut live, "the resident save", |session| {
        session
            .checkpoints()
            .receipt()
            .is_some_and(|receipt| receipt.finished())
    })?;
    let after = live.session.runtime().counters();
    check.that(
        live.session
            .checkpoints()
            .receipt()
            .is_some_and(|receipt| receipt.committed().is_some()),
        || {
            format!(
                "the resident save: {:?}",
                live.session.checkpoints().receipt()
            )
        },
    );
    let [_, streamed, copied, _, _, transfers] = delta(&after, &before);
    check.metric("save_frames", saving.frames);
    check.metric("save_snapshot_readback_bytes", streamed);
    check.metric("save_snapshot_device_copy_bytes", copied);
    check.metric("save_transfer_submissions", transfers);
    check.metric(
        "save_chunks_written",
        live.session
            .checkpoints()
            .receipt()
            .map_or(0, |receipt| receipt.chunks),
    );
    report(check, "saving", saving);

    // Change chunks of the two westernmost columns, which leave first.
    for row in -(RADIUS as i32)..=RADIUS as i32 {
        for column in [-6, -5] {
            stroke(
                &mut live,
                EditTool::Raise,
                IVec2::new(column * 64 + 20, row * 64 + 30),
                1,
            )?;
        }
    }
    let before = live.session.runtime().counters();
    let moves = 8;
    let mut frames_per_move = Vec::new();
    let mut walking = Phase {
        frame_ms: Vec::new(),
        allocations: 0,
        frames: 0,
    };
    for step in 1..=moves {
        live.session.set_window(window(step, 0, RADIUS));
        let mut frames = 0;
        let started = alloc::snapshot().0;
        loop {
            let begun = Instant::now();
            live.session.frame_ticks(1);
            walking
                .frame_ms
                .push(begun.elapsed().as_secs_f64() * 1000.0);
            std::thread::sleep(Duration::from_micros(500));
            frames += 1;
            let residency = live.session.residency();
            if residency.window() == window(step, 0, RADIUS) && residency.settled() {
                break;
            }
            if frames > 2000 {
                return Err(format!("move {step} did not settle"));
            }
        }
        walking.allocations += alloc::snapshot().0 - started;
        walking.frames += frames;
        frames_per_move.push(f64::from(frames));
        if step == moves / 2 {
            live.session.save();
        }
    }
    live.until("the walk's writes and save", |session| {
        !session.residency().busy()
            && session
                .checkpoints()
                .receipt()
                .is_some_and(|receipt| receipt.finished())
    })?;
    let after = live.session.runtime().counters();
    let [uploaded, streamed, copied, readback, ticks, transfers] = delta(&after, &before);
    let chunks_entered = u64::from(moves as u32) * (2 * u64::from(RADIUS) + 1);
    report(check, "walking", walking);
    check.metric("moves", moves);
    check.metric("chunks_entered", chunks_entered);
    check.metric("upload_bytes_per_entered_chunk", uploaded / chunks_entered);
    check.metric("snapshot_readback_bytes", streamed);
    check.metric("snapshot_device_copy_bytes", copied);
    check.metric("walking_readback_bytes_per_tick", readback / ticks.max(1));
    check.metric("transfer_submissions", transfers);
    check.metric(
        "frames_per_move_max",
        frames_per_move.iter().copied().fold(0.0, f64::max),
    );
    check.metric(
        "frames_per_move_mean",
        frames_per_move.iter().sum::<f64>() / frames_per_move.len() as f64,
    );
    let summary = *live.session.residency().summary();
    check.metric("retained_high_water_bytes", summary.retained_high_water);
    check.metric("snapshots_changed", summary.snapshots_changed);
    check.metric("snapshots_unchanged", summary.snapshots_unchanged);
    check.metric("load_age_ms_max", summary.load_age_ms.max);
    check.metric(
        "load_age_ms_mean",
        summary.load_age_ms.mean().unwrap_or(0.0),
    );
    check.metric("write_age_ms_max", summary.write_age_ms.max);
    check.metric("loads_generated", summary.loads_generated);
    check.metric("loads_stored", summary.loads_stored);
    check.equal("frame waits", after.frame_waits, 0);
    check.equal(
        "maintenance waits during frames",
        after.maintenance_waits,
        0,
    );
    check.that(
        live.session
            .checkpoints()
            .receipt()
            .is_some_and(|receipt| receipt.committed().is_some()),
        || "the save during the walk did not commit".to_owned(),
    );
    check.that(summary.retained_high_water <= retained_budget, || {
        format!(
            "{} retained bytes above the budget {retained_budget}",
            summary.retained_high_water
        )
    });
    let closed = live.session.close(Duration::from_secs(60));
    check.that(closed.saved, || format!("{closed:?}"));
    Ok(())
}

/// The ordinary resident simulation (rules on, water flowing, no window move, no save) moves no
/// chunk and no snapshot, and reads back only its bounded per-tick records.
pub fn resident_no_readback(adapter: &str, check: &mut Check) -> Result<(), String> {
    let scratch = Scratch::new("resident")?;
    let path = scratch.path("world.redb");
    world::create(&path, window(0, 0, RADIUS), SimulationRules::ALL)?;
    let store = open_store(&path, StoreFaults::new())?;
    let mut live = open(
        adapter,
        store,
        SessionEpoch::new(2),
        session_config(),
        |_| {},
    )?;
    live.settle()?;
    stroke(&mut live, EditTool::Water, IVec2::new(0, 0), 6)?;
    let before = live.session.runtime().counters();
    let resident = phase(&mut live, 600);
    report(check, "resident", resident);
    drain(&mut live)?;
    let after = live.session.runtime().counters();
    let [uploaded, streamed, copied, readback, ticks, transfers] = delta(&after, &before);
    check.equal("chunk upload bytes", uploaded, 0);
    check.equal("snapshot readback bytes", streamed, 0);
    check.equal("snapshot device copy bytes", copied, 0);
    check.equal(
        "capture readback bytes",
        after.readback.captures - before.readback.captures,
        0,
    );
    check.equal("transfer submissions", transfers, 0);
    check.metric("ticks", ticks);
    check.metric("readback_bytes_per_tick", readback / ticks.max(1));
    check.metric(
        "upload_bytes_per_tick",
        (after.upload.total() - before.upload.total()) / ticks.max(1),
    );
    // The tick's own records (results, events, accounting, samples), never world data.
    check.that(readback / ticks.max(1) < 64 * 1024, || {
        format!("{} bytes read back per tick", readback / ticks.max(1))
    });
    let summary = *live.session.residency().summary();
    check.equal(
        "chunks loaded, all before the measurement",
        summary.loads_generated,
        169,
    );
    let closed = live.session.close(Duration::from_secs(60));
    check.that(closed.saved, || format!("{closed:?}"));
    Ok(())
}
