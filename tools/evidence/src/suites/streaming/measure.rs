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

/// Frame CPU times, the session's share of them by owner, and allocations over a number of
/// frames.
#[derive(Default)]
struct Phase {
    frame_ms: Vec<f64>,
    /// The session's frame time by owner (`FrameTimes`): the runtime, the residency owner, the
    /// checkpoint owner and the copies no tick took along.
    owners_ms: [Vec<f64>; 4],
    /// Every thread's allocations, and this thread's, which runs the session; the rest are the
    /// store's workers' (and any thread wgpu runs).
    allocations: u64,
    main_allocations: u64,
    frames: u32,
}

impl Phase {
    /// Runs one frame of one tick, paced like a display so the device keeps up and the time
    /// measured is the frame's CPU cost. Allocations count to the next frame, so the workers'
    /// during the pause belong to the frame that gave them work.
    fn frame(&mut self, live: &mut Live) {
        let process = alloc::snapshot().0;
        let main = alloc::thread_allocations();
        let started = Instant::now();
        live.session.frame_ticks(1);
        self.frame_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        let times = live.session.frame_times();
        for (series, time) in self.owners_ms.iter_mut().zip([
            times.runtime,
            times.residency,
            times.checkpoints,
            times.transfers,
        ]) {
            series.push(time.as_secs_f64() * 1000.0);
        }
        std::thread::sleep(Duration::from_micros(500));
        self.allocations += alloc::snapshot().0 - process;
        self.main_allocations += alloc::thread_allocations() - main;
        self.frames += 1;
    }
}

fn phase(live: &mut Live, frames: u32) -> Phase {
    let mut phase = Phase::default();
    for _ in 0..frames {
        phase.frame(live);
    }
    phase
}

/// Runs frames of one tick until `done` holds.
fn phase_until(
    live: &mut Live,
    what: &str,
    mut done: impl FnMut(&world_runtime::session::WorldSession) -> bool,
) -> Result<Phase, String> {
    let mut phase = Phase::default();
    while !done(&live.session) {
        if phase.frames > 5000 {
            return Err(format!("{what}: not done after {} frames", phase.frames));
        }
        phase.frame(live);
    }
    Ok(phase)
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
    let frames = f64::from(phase.frames.max(1));
    check.metric(
        &format!("{label}_allocations_per_frame"),
        phase.allocations as f64 / frames,
    );
    check.metric(
        &format!("{label}_session_thread_allocations_per_frame"),
        phase.main_allocations as f64 / frames,
    );
    check.metric(
        &format!("{label}_other_thread_allocations_per_frame"),
        (phase.allocations - phase.main_allocations) as f64 / frames,
    );
    for (owner, mut series) in ["runtime", "residency", "checkpoints", "transfers"]
        .into_iter()
        .zip(phase.owners_ms)
    {
        let mean = series.iter().sum::<f64>() / frames;
        check.metric(&format!("{label}_{owner}_ms_mean"), mean);
        check.metric(
            &format!("{label}_{owner}_ms_p95"),
            percentile(&mut series, 0.95),
        );
    }
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
    let mut walking = Phase::default();
    for step in 1..=moves {
        live.session.set_window(window(step, 0, RADIUS));
        let mut frames = 0;
        loop {
            walking.frame(&mut live);
            frames += 1;
            let residency = live.session.residency();
            if residency.window() == window(step, 0, RADIUS) && residency.settled() {
                break;
            }
            if frames > 2000 {
                return Err(format!("move {step} did not settle"));
            }
        }
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

/// Bytes this process has had written to storage (`write_bytes` of `/proc/self/io`): the
/// store's rows and every page redb rewrote for them. `None` where the kernel does not say.
fn storage_bytes_written() -> Option<u64> {
    let io = std::fs::read_to_string("/proc/self/io").ok()?;
    io.lines()
        .find_map(|line| line.strip_prefix("write_bytes: "))
        .and_then(|value| value.trim().parse().ok())
}

/// What one interval's run measured.
struct Cadence {
    saves: u64,
    chunks_per_save: Vec<f64>,
    /// Ticks from a save's request to its publication.
    save_ticks: Vec<f64>,
    /// The most ticks after the latest published checkpoint at any measured frame: what a
    /// crash at that frame would have lost.
    exposure_max: u64,
}

/// Periodic checkpoints at each of `intervals` (ticks; `None` is off) under the frozen scenario
/// `profile`/`case`, paced at its tick rate as the client runs: bytes per checkpoint on the
/// device, back to the CPU and to storage, ticks from a save's request to its publication, the
/// most simulated time a crash would have lost at any frame, and the frame's CPU time and
/// allocations. Warm-up as the scenario says, then 900 measured ticks per interval. The store is
/// on the workspace's disk, not in the system's temporary directory.
fn checkpoint_cadence(
    adapter: &str,
    check: &mut Check,
    profile: &str,
    case: &str,
    intervals: &[Option<u64>],
) -> Result<(), String> {
    use crate::scenario::{self, Family};
    use world_model::{
        body::BodyHandle,
        command::{AmbientTemperature, Command, ElevationScale},
        spatial::{CHUNK_SIZE, ChunkKey},
    };
    use world_runtime::{
        ids::{CommandId, Tick},
        receipts::ReceiptState,
    };

    const MEASURED_TICKS: u64 = 900;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let loaded = scenario::load(&scenario::scenario_path(
        &root.join("scenarios"),
        profile,
        case,
    ))?;
    let s = &loaded.scenario;
    let start = IVec2::from(s.camera.start_cell);
    let size = CHUNK_SIZE as i32;
    let focus = ChunkKey::new(start.x.div_euclid(size), start.y.div_euclid(size), 0);
    let [radius, _] = s.world.active_radius_chunks;
    let rules = SimulationRules {
        liquid: s.world.families.contains(&Family::Liquid),
        granular: s.world.families.contains(&Family::Granular),
        thermal: s.world.families.contains(&Family::Thermal),
        fire: s.world.families.contains(&Family::Fire),
        biology: s.world.families.contains(&Family::Biology),
    };
    let bodies: u32 = s.bodies.iter().map(|group| group.count).sum();
    let ambient =
        AmbientTemperature::celsius(s.world.ambient_temperature_c).map_err(|e| e.to_string())?;
    let scale = ElevationScale::new(s.world.elevation_scale).map_err(|e| e.to_string())?;
    let tick = Duration::from_secs_f64(1.0 / s.world.tick_rate_hz);
    let window = window(focus.key.x, focus.key.y, radius);
    check.metric(
        &format!("{profile}_{case}_window_chunks"),
        window.chunk_count(),
    );
    check.metric(&format!("{profile}_{case}_bodies"), bodies);
    for interval in intervals {
        let label = format!(
            "{profile}_{case}_{}",
            interval.map_or("off".to_owned(), |ticks| format!("every_{ticks}"))
        );
        // On the disk saved worlds live on: a sync there is what a save waits for.
        let scratch = Scratch::on_disk("cadence")?;
        let path = scratch.path("world.redb");
        world::create(&path, window, rules)?;
        let store = open_store(&path, StoreFaults::new())?;
        let mut config = session_config();
        config.checkpoints.autosave_ticks = *interval;
        let mut live = open(adapter, store, SessionEpoch::new(2), config, |gpu| {
            gpu.transfer.bodies = bodies.next_power_of_two().max(64);
            gpu.ambient_temperature = ambient;
            gpu.elevation_scale = scale;
        })?;
        live.settle()?;
        let first = live
            .session
            .runtime()
            .last_submitted_tick()
            .map_or(0, |tick| tick.value() + 1);
        let total = s.run.warmup_ticks + MEASURED_TICKS;
        let mut next_id = 1u64;
        let mut acknowledged = 0u64;
        let mut admitted_through = None;
        let mut measured = Phase::default();
        let mut before = None;
        let mut cadence = Cadence {
            saves: 0,
            chunks_per_save: Vec::new(),
            save_ticks: Vec::new(),
            exposure_max: 0,
        };
        // The save the receipt describes, and the tick it was first seen requested at.
        let mut seen: Option<(u64, u64, bool)> = None;
        let mut deadline = Instant::now();
        loop {
            let next = live
                .session
                .runtime()
                .last_submitted_tick()
                .map_or(0, |tick| tick.value() + 1);
            let t = next - first;
            if t >= total {
                break;
            }
            // The scenario's commands for the next tick, admitted once for that tick.
            if admitted_through.is_none_or(|through| through < t) {
                let mut commands = Vec::new();
                for group in s.bodies.iter().filter(|group| group.at_tick == t) {
                    commands.extend(
                        group
                            .spawns()
                            .map_err(|e| e.to_string())?
                            .into_iter()
                            .map(Command::SpawnBody),
                    );
                }
                for action in &s.actions {
                    // The first sphere spawned is the driven one.
                    let driven = action.drives_at(t).then(|| BodyHandle::new(0, 0));
                    commands.extend(action.commands_at(t, driven).map_err(|e| e.to_string())?);
                }
                for command in commands {
                    live.session
                        .runtime_mut()
                        .submit_at(CommandId::new(next_id), command, Tick::new(next))
                        .map_err(|error| format!("{command:?}: {error:?}"))?;
                    next_id += 1;
                }
                admitted_through = Some(t);
            }
            let measuring = t >= s.run.warmup_ticks;
            if measuring && before.is_none() {
                before = Some((
                    live.session.runtime().counters(),
                    *live.session.residency().summary(),
                    storage_bytes_written(),
                ));
            }
            deadline += tick;
            if measuring {
                measured.frame(&mut live);
            } else {
                live.session.frame_ticks(1);
            }
            // Release the receipts of finished commands, oldest first, as the client does.
            while acknowledged + 1 < next_id {
                match live
                    .session
                    .runtime()
                    .receipt(CommandId::new(acknowledged + 1))
                    .map(|receipt| receipt.state)
                {
                    Some(ReceiptState::Completed { .. } | ReceiptState::Abandoned { .. }) => {
                        acknowledged += 1;
                    }
                    _ => break,
                }
            }
            if acknowledged > 0 {
                let _ = live
                    .session
                    .runtime_mut()
                    .acknowledge(CommandId::new(acknowledged));
            }
            let checkpoints = live.session.checkpoints();
            if let Some(receipt) = checkpoints.receipt() {
                let id = receipt.id.value();
                if seen.is_none_or(|(save, ..)| save != id) {
                    seen = Some((id, next, false));
                }
                if let Some((save, requested, false)) = seen
                    && receipt.committed().is_some()
                {
                    seen = Some((save, requested, true));
                    if measuring {
                        cadence.saves += 1;
                        cadence.chunks_per_save.push(receipt.chunks as f64);
                        cadence.save_ticks.push((next - requested) as f64);
                    }
                }
            }
            if measuring {
                let latest = checkpoints.latest().tick.map_or(0, Tick::value);
                cadence.exposure_max = cadence.exposure_max.max(next.saturating_sub(latest));
            }
            let now = Instant::now();
            if deadline > now {
                std::thread::sleep(deadline - now);
            } else {
                deadline = now;
            }
        }
        let (counters_before, residency_before, written_before) =
            before.ok_or("no measured ticks")?;
        let written = storage_bytes_written()
            .zip(written_before)
            .map(|(after, before)| after - before);
        let counters = live.session.runtime().counters();
        let residency = *live.session.residency().summary();
        report(check, &label, measured);
        check.metric(&format!("{label}_measured_ticks"), MEASURED_TICKS);
        check.metric(&format!("{label}_saves"), cadence.saves);
        check.metric(
            &format!("{label}_snapshot_readback_bytes"),
            counters.readback.snapshots - counters_before.readback.snapshots,
        );
        check.metric(
            &format!("{label}_snapshot_copy_bytes"),
            counters.snapshot_copy_bytes - counters_before.snapshot_copy_bytes,
        );
        check.metric(
            &format!("{label}_chunk_rows_written"),
            residency.writes_committed - residency_before.writes_committed,
        );
        if let Some(written) = written {
            check.metric(&format!("{label}_storage_bytes_written"), written);
        }
        check.metric(
            &format!("{label}_write_age_ms_max"),
            residency.write_age_ms.max,
        );
        let mut chunks = cadence.chunks_per_save;
        check.metric(
            &format!("{label}_chunks_per_save_p50"),
            percentile(&mut chunks, 0.5),
        );
        check.metric(
            &format!("{label}_chunks_per_save_max"),
            chunks.iter().copied().fold(0.0, f64::max),
        );
        let mut ticks = cadence.save_ticks;
        check.metric(
            &format!("{label}_save_ticks_p50"),
            percentile(&mut ticks, 0.5),
        );
        check.metric(
            &format!("{label}_save_ticks_max"),
            ticks.iter().copied().fold(0.0, f64::max),
        );
        check.metric(&format!("{label}_exposure_ticks_max"), cadence.exposure_max);
        check.metric(
            &format!("{label}_retained_high_water"),
            residency.retained_high_water,
        );
        check.metric(
            &format!("{label}_discarded_ticks"),
            live.session.runtime().discarded_ticks(),
        );
        check.equal(&format!("{label}: frame waits"), counters.frame_waits, 0);
        if let Some(interval) = interval {
            check.that(cadence.saves >= MEASURED_TICKS / interval, || {
                format!("{label}: {} saves in {MEASURED_TICKS} ticks", cadence.saves)
            });
        }
        let closed = live.session.close(Duration::from_secs(60));
        check.that(closed.saved, || format!("{label}: {closed:?}"));
    }
    Ok(())
}

pub fn checkpoint_cadence_qualification(adapter: &str, check: &mut Check) -> Result<(), String> {
    checkpoint_cadence(
        adapter,
        check,
        "qualification",
        "coupled",
        &[None, Some(300), Some(60)],
    )?;
    checkpoint_cadence(adapter, check, "qualification", "idle", &[Some(60)])
}

pub fn checkpoint_cadence_scale(adapter: &str, check: &mut Check) -> Result<(), String> {
    checkpoint_cadence(
        adapter,
        check,
        "scale",
        "coupled",
        &[None, Some(300), Some(60)],
    )
}
