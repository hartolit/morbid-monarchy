//! Checkpoint D: what bodies cost on the selected adapter.
//!
//! **Scaling** (`body-scaling-bins`, `body-scaling-all-pairs`): 64, 256, 1024 and 4096 bodies
//! on flat stone in a radius-3 window, every material rule off so the body pass is what varies.
//! *Distributed*: radius-2.5 spheres six cells apart, resting. *Dense*: radius 2–5 spheres in
//! up to four layers, attracted to the window's centre for 120 ticks and measured while still
//! attracted, a pile under load. Each tick is submitted alone and waited for (a round trip, not
//! throughput); 64 bodies are sampled a tick, as the runtime's selection does. Per tick: CPU
//! encode-and-submit time, the round trip, device time of the body pass (timestamps), candidate
//! and contact pairs, the solver's residual overlap and approach, bytes moved and process
//! allocations around the port's calls. The default solver runs 16 position and 16 velocity
//! passes; the dense 4096 cluster is also measured at 4 + 4 and 8 + 8.
//!
//! **Legacy workloads** (`body-legacy-workloads`): the frozen `body-contact` scenarios of the
//! legacy baseline (qualification: 64 metal spheres at radius 6; scale: 256 at radius 16, on
//! seed-69 terrain with every rule family) replayed tick for tick through the GPU world, against
//! the scenario files whose SHA-256 the baseline recorded. The baseline's body time is CPU time
//! of the body systems per tick; the GPU's is device time of the body pass, and the CPU cost
//! that remains is the encode time.
//!
//! **Pacing** (`body-pacing`): the runtime presenting frames at 60 Hz of wall time with up to
//! three ticks in flight, at 256 distributed and 4096 dense bodies: ticks submitted and
//! discarded, pressure refusals, frame waits (none are allowed), the in-flight high water, tick
//! latency and the age of applied body samples.

use std::time::{Duration, Instant};

use gpu_world::world::{Broadphase, GpuWorld, PairIterations};
use world_model::{
    body::{BodyHandle, SphereDescription},
    cell::WorldCell,
    command::{AmbientTemperature, BodySpawn, Command, ElevationScale, Target},
    generation::WorldGenerator,
    glam::IVec2,
    material::TerrainMat,
    rules::SimulationRules,
    spatial::ChunkKey,
};
use world_runtime::{
    ids::{CommandId, Tick, WorldId},
    port::{
        AdmittedCommand, BodyReport, Outcome, PhysicalWorld, ResidentWindow, TickReport,
        TransferCounters,
    },
    runtime::{Runtime, RuntimeConfig},
};

use super::{
    super::{Check, alloc, driver::Driver, transfers::runtime_upload},
    scene::{Setup, chunk_cells},
};
use crate::scenario::{self, Family};

const SCALING_RADIUS: u32 = 3;
const SAMPLED: usize = 64;

fn stone(_: IVec2, column: &mut WorldCell) {
    column.set_terrain_mat(TerrainMat::TERRAIN_STONE);
    column.set_elevation(40);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    Distributed,
    Dense,
}

impl Layout {
    fn name(self) -> &'static str {
        match self {
            Self::Distributed => "distributed",
            Self::Dense => "dense",
        }
    }

    fn spawns(self, count: u32) -> Result<Vec<Command>, String> {
        let mut out = Vec::with_capacity(count as usize);
        match self {
            Self::Distributed => {
                let side = (f64::from(count).sqrt().ceil()) as i32;
                let sphere = SphereDescription::new(4.0, 2.5, 7.8).map_err(|e| e.to_string())?;
                for k in 0..count as i32 {
                    let cell = IVec2::new(32, 32) + (IVec2::new(k % side, k / side) - side / 2) * 6;
                    out.push(spawn(sphere, cell, 1.0)?);
                }
            }
            Self::Dense => {
                let per_layer = count.min(1024);
                let side = (f64::from(per_layer).sqrt().ceil()) as i32;
                for k in 0..count as i32 {
                    let layer = k / per_layer as i32;
                    let index = k % per_layer as i32;
                    let radius = 2.0 + (k % 4) as f32;
                    let sphere = SphereDescription::new(radius * radius * 0.25, radius, 2.0)
                        .map_err(|e| e.to_string())?;
                    let cell = IVec2::new(32, 32)
                        + (IVec2::new(index % side, index / side) - side / 2) * 10;
                    out.push(spawn(sphere, cell, 4.0 + 14.0 * layer as f32)?);
                }
            }
        }
        Ok(out)
    }
}

fn spawn(sphere: SphereDescription, cell: IVec2, drop: f32) -> Result<Command, String> {
    BodySpawn::with(sphere, Target::Column(cell), drop)
        .map(Command::SpawnBody)
        .map_err(|error| error.to_string())
}

struct Sample {
    encode: Duration,
    report: TickReport,
    bodies: BodyReport,
    /// Process allocations inside the port's submit call and inside its polls.
    submit_allocations: u64,
    poll_allocations: u64,
}

fn percentile(values: &mut [f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    values[((values.len() - 1) as f64 * p).round() as usize]
}

/// Submits one tick of `commands`, timing the encode-and-submit call, and waits for its
/// reports.
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
    let submitted = alloc::snapshot().0;
    driver.settle()?;
    let polled = alloc::snapshot().0;
    let (mut report, mut bodies) = (None, None);
    for outcome in &driver.outcomes {
        match outcome {
            Outcome::TickCompleted(completed) if completed.tick == tick => {
                report = Some(*completed)
            }
            Outcome::BodiesCompleted { tick: of, report } if *of == tick => bodies = Some(*report),
            _ => {}
        }
    }
    Ok(Sample {
        encode,
        report: report.ok_or("no tick report")?,
        bodies: bodies.ok_or("no body report")?,
        submit_allocations: submitted - allocations_before,
        poll_allocations: polled - submitted,
    })
}

/// The live handles after `spawns` succeeded, from the outcomes of their tick.
fn spawned(driver: &Driver, into: &mut Vec<BodyHandle>) {
    for outcome in &driver.outcomes {
        if let Outcome::Command(command) = outcome
            && let world_runtime::port::CommandResult::BodySpawned { body, .. } = command.result
        {
            into.push(body);
        }
    }
}

/// Rotates the sampled set through the live bodies, `SAMPLED` a tick.
fn rotate(driver: &mut Driver, live: &[BodyHandle], cursor: &mut usize) {
    driver.observe.clear();
    for _ in 0..SAMPLED.min(live.len()) {
        driver.observe.push(live[*cursor % live.len()]);
        *cursor += 1;
    }
}

/// Records the measured samples under `label` and checks what must hold while measuring.
fn report(
    check: &mut Check,
    label: &str,
    samples: &[Sample],
    before: &TransferCounters,
    after: &TransferCounters,
    passes: u32,
) {
    let ticks = samples.len() as f64;
    let per_tick = |value: u64| (value as f64 / ticks).round() as u64;
    let mean = |pick: &dyn Fn(&Sample) -> f64| samples.iter().map(pick).sum::<f64>() / ticks;
    let mut encode: Vec<f64> = samples
        .iter()
        .map(|s| s.encode.as_secs_f64() * 1e6)
        .collect();
    let mut round_trip: Vec<f64> = samples
        .iter()
        .map(|s| s.report.latency.as_secs_f64() * 1e3)
        .collect();
    check.metric(
        &format!("{label}_encode_us_median"),
        percentile(&mut encode, 0.5).round(),
    );
    check.metric(
        &format!("{label}_encode_us_p95"),
        percentile(&mut encode, 0.95).round(),
    );
    check.metric(
        &format!("{label}_round_trip_ms_median"),
        (percentile(&mut round_trip, 0.5) * 100.0).round() / 100.0,
    );
    check.metric(
        &format!("{label}_round_trip_ms_p95"),
        (percentile(&mut round_trip, 0.95) * 100.0).round() / 100.0,
    );
    let phases: Vec<_> = samples.iter().filter_map(|s| s.report.phases).collect();
    if phases.len() == samples.len() {
        let mut bodies: Vec<f64> = phases
            .iter()
            .map(|p| p.bodies.as_secs_f64() * 1e6)
            .collect();
        let mut total: Vec<f64> = samples
            .iter()
            .filter_map(|s| s.report.gpu_time)
            .map(|t| t.as_secs_f64() * 1e6)
            .collect();
        let median = percentile(&mut bodies, 0.5);
        check.metric(&format!("{label}_gpu_bodies_us_median"), median.round());
        check.metric(
            &format!("{label}_gpu_bodies_us_p95"),
            percentile(&mut bodies, 0.95).round(),
        );
        check.metric(
            &format!("{label}_gpu_bodies_us_max"),
            percentile(&mut bodies, 1.0).round(),
        );
        check.metric(
            &format!("{label}_gpu_tick_us_median"),
            percentile(&mut total, 0.5).round(),
        );
        check.metric(
            &format!("{label}_gpu_bodies_us_per_pass"),
            (median / f64::from(passes.max(1)) * 10.0).round() / 10.0,
        );
    } else {
        check.note(format!(
            "{label}: pass timestamps unavailable on this adapter"
        ));
    }
    check.metric(
        &format!("{label}_substeps_per_tick"),
        mean(&|s| f64::from(s.bodies.substeps)),
    );
    check.metric(&format!("{label}_passes_per_tick"), passes);
    check.metric(
        &format!("{label}_candidate_pairs_per_tick"),
        mean(&|s| s.bodies.candidate_pairs as f64).round(),
    );
    check.metric(
        &format!("{label}_pair_visits_per_tick"),
        mean(&|s| s.bodies.pair_visits as f64).round(),
    );
    check.metric(
        &format!("{label}_contact_pairs_per_tick"),
        mean(&|s| f64::from(s.bodies.contact_pairs)).round(),
    );
    check.metric(
        &format!("{label}_terrain_contacts_per_tick"),
        mean(&|s| f64::from(s.bodies.terrain_contacts)).round(),
    );
    let max = |pick: &dyn Fn(&Sample) -> f32| samples.iter().map(pick).fold(0.0f32, f32::max);
    check.metric(
        &format!("{label}_residual_overlap_max"),
        f64::from(max(&|s| s.bodies.residual_overlap)),
    );
    check.metric(
        &format!("{label}_residual_approach_max"),
        f64::from(max(&|s| s.bodies.residual_approach)),
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
        &format!("{label}_submit_allocations_per_tick"),
        per_tick(samples.iter().map(|s| s.submit_allocations).sum()),
    );
    check.metric(
        &format!("{label}_poll_allocations_per_tick"),
        per_tick(samples.iter().map(|s| s.poll_allocations).sum()),
    );
    check.equal(
        &format!("{label}: buffers created while measuring"),
        after.buffers_created,
        before.buffers_created,
    );
    check.equal(
        &format!("{label}: frame waits"),
        after.frame_waits,
        before.frame_waits,
    );
    let inconsistent = samples
        .iter()
        .filter(|s| !s.report.cells.consistent() || !s.bodies.consistent())
        .count();
    check.equal(&format!("{label}: inconsistent ticks"), inconsistent, 0);
}

fn scaling_run(
    adapter: &str,
    check: &mut Check,
    broadphase: Broadphase,
    layout: Layout,
    count: u32,
    iterations: PairIterations,
) -> Result<(), String> {
    let label = format!(
        "{}_{}_{count}{}",
        match broadphase {
            Broadphase::Bins => "bins",
            Broadphase::AllPairs => "all_pairs",
        },
        layout.name(),
        if iterations == PairIterations::DEFAULT {
            String::new()
        } else {
            format!("_{}x{}", iterations.position, iterations.velocity)
        }
    );
    let setup = Setup {
        radius: SCALING_RADIUS,
        bodies: 4096,
        samples: SAMPLED as u32,
        broadphase,
        pair_iterations: iterations,
        timestamps: true,
        ..Setup::default()
    };
    let mut driver = Driver::open(setup.config(adapter))?;
    let keys: Vec<ChunkKey> = setup.window().view().iter().collect();
    for group in keys.chunks(32) {
        for key in group {
            driver.stage_upload(*key, &chunk_cells(*key, &stone))?;
        }
        step(&mut driver, &[])?;
    }
    let mut live = Vec::with_capacity(count as usize);
    for batch in layout.spawns(count)?.chunks(256) {
        step(&mut driver, batch)?;
        spawned(&driver, &mut live);
    }
    check.equal(
        &format!("{label}: bodies spawned"),
        live.len() as u32,
        count,
    );
    let center = Target::Column(IVec2::new(32, 32));
    let commands = |layout: Layout| -> Vec<Command> {
        match layout {
            Layout::Distributed => Vec::new(),
            Layout::Dense => vec![Command::AttractBodies { target: center }],
        }
    };
    let warmup = match layout {
        Layout::Distributed => 30,
        Layout::Dense => 120,
    };
    let mut cursor = 0;
    for _ in 0..warmup {
        rotate(&mut driver, &live, &mut cursor);
        step(&mut driver, &commands(layout))?;
    }
    let before = driver.world.counters();
    let mut samples = Vec::with_capacity(30);
    for _ in 0..30 {
        rotate(&mut driver, &live, &mut cursor);
        samples.push(step(&mut driver, &commands(layout))?);
    }
    let after = driver.world.counters();
    let substeps = samples.first().map_or(0, |s| s.bodies.substeps);
    let passes = substeps * (iterations.position + iterations.velocity + 2);
    report(check, &label, &samples, &before, &after, passes);
    // Every body sampled and every tick's body report agrees on the live count.
    check.that(samples.iter().all(|s| s.bodies.live == count), || {
        format!("{label}: a tick lost a body")
    });
    Ok(())
}

fn scaling(adapter: &str, check: &mut Check, broadphase: Broadphase) -> Result<(), String> {
    let started = Instant::now();
    for layout in [Layout::Distributed, Layout::Dense] {
        for count in [64, 256, 1024, 4096] {
            scaling_run(
                adapter,
                check,
                broadphase,
                layout,
                count,
                PairIterations::DEFAULT,
            )?;
        }
    }
    if broadphase == Broadphase::Bins {
        for iterations in [
            PairIterations {
                position: 4,
                velocity: 4,
            },
            PairIterations {
                position: 8,
                velocity: 8,
            },
        ] {
            scaling_run(adapter, check, broadphase, Layout::Dense, 4096, iterations)?;
        }
    }
    check.metric("wall_seconds", started.elapsed().as_secs_f64().round());
    Ok(())
}

pub fn scaling_bins(adapter: &str, check: &mut Check) -> Result<(), String> {
    scaling(adapter, check, Broadphase::Bins)
}

pub fn scaling_all_pairs(adapter: &str, check: &mut Check) -> Result<(), String> {
    scaling(adapter, check, Broadphase::AllPairs)
}

/// The legacy baseline's frozen `body-contact` scenarios, replayed through the GPU world.
pub fn legacy_workloads(adapter: &str, check: &mut Check) -> Result<(), String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for (profile, legacy_body_ms, legacy_sha) in [
        (
            "qualification",
            5.32,
            "2888cc8356aaeae99304e49b1c71b31ca1689f33aa9d72f00db32c4c4fab7d6b",
        ),
        (
            "scale",
            16.99,
            "60e6c22eaa9d236b5aa5bc048bda4a7426744fa62343cb6a215ba00174a68887",
        ),
    ] {
        let path = scenario::scenario_path(&root.join("scenarios"), profile, "body-contact");
        let loaded = scenario::load(&path)?;
        check.equal(
            &format!("{profile}: scenario identity (the baseline's file)"),
            loaded.sha256.as_str(),
            legacy_sha,
        );
        let s = &loaded.scenario;
        // The baseline's window follows the camera, which starts over this cell.
        let start = IVec2::from(s.camera.start_cell);
        let size = world_model::spatial::CHUNK_SIZE as i32;
        let focus = ChunkKey::new(start.x.div_euclid(size), start.y.div_euclid(size), 0);
        let [rx, ry] = s.world.active_radius_chunks;
        let window = ResidentWindow::new(focus, rx, ry).map_err(|e| format!("window: {e:?}"))?;
        let mut config = gpu_world::world::GpuWorldConfig::new(
            WorldId::new(u64::from(s.world.seed)),
            adapter,
            window,
        );
        config.allow_software = super::super::allow_software();
        config.rules = SimulationRules {
            liquid: s.world.families.contains(&Family::Liquid),
            granular: s.world.families.contains(&Family::Granular),
            thermal: s.world.families.contains(&Family::Thermal),
            fire: s.world.families.contains(&Family::Fire),
            biology: s.world.families.contains(&Family::Biology),
        };
        config.ambient_temperature = AmbientTemperature::celsius(s.world.ambient_temperature_c)
            .map_err(|e| e.to_string())?;
        config.elevation_scale =
            ElevationScale::new(s.world.elevation_scale).map_err(|e| e.to_string())?;
        config.timestamps = true;
        config.transfer.body_samples_per_tick = SAMPLED as u32;
        let mut driver = Driver::open(config)?;
        driver.tick_rate_hz = s.world.tick_rate_hz;
        let generator = WorldGenerator::new(s.world.seed);
        let keys: Vec<ChunkKey> = window.view().iter().collect();
        let slots = driver.world.capabilities().chunk_upload_slots as usize;
        for group in keys.chunks(slots) {
            for key in group {
                driver.stage_upload(*key, &generator.generate_chunk(*key).cells)?;
            }
            step(&mut driver, &[])?;
        }
        let ticks = s.run.warmup_ticks + s.run.measured_ticks;
        let mut live = Vec::new();
        let mut cursor = 0;
        let mut samples = Vec::with_capacity(s.run.measured_ticks as usize);
        let mut before = None;
        let mut totals = BodyReport::default();
        for t in 0..ticks {
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
                commands.extend(action.commands_at(t, None).map_err(|e| e.to_string())?);
            }
            rotate(&mut driver, &live, &mut cursor);
            if t == s.run.warmup_ticks {
                before = Some(driver.world.counters());
            }
            let sample = step(&mut driver, &commands)?;
            spawned(&driver, &mut live);
            if t >= s.run.warmup_ticks {
                totals.add(&sample.bodies);
                samples.push(sample);
            }
        }
        let after = driver.world.counters();
        let label = format!("legacy_{profile}");
        let substeps = samples.first().map_or(0, |s| s.bodies.substeps);
        let iterations = PairIterations::DEFAULT;
        let passes = substeps * (iterations.position + iterations.velocity + 2);
        report(
            check,
            &label,
            &samples,
            &before.ok_or("no measured ticks")?,
            &after,
            passes,
        );
        let count: u32 = s.bodies.iter().map(|group| group.count).sum();
        check.equal(&format!("{label}: bodies"), live.len() as u32, count);
        check.metric(&format!("{label}_bodies"), count);
        check.metric(&format!("{label}_window_chunks"), keys.len());
        check.metric(&format!("{label}_measured_ticks"), samples.len());
        check.metric(
            &format!("{label}_legacy_body_cpu_ms_per_tick"),
            legacy_body_ms,
        );
        // The baseline tested every unordered pair in every substep.
        let legacy_pairs = u64::from(count) * u64::from(count - 1) / 2 * u64::from(substeps);
        check.metric(&format!("{label}_legacy_pair_tests_per_tick"), legacy_pairs);
        check.metric(&format!("{label}_craters"), totals.craters);
        check.metric(&format!("{label}_ruts"), totals.ruts);
        check.metric(&format!("{label}_splashes"), totals.splashes);
        check.metric(&format!("{label}_claims_lost"), totals.claims_lost);
    }
    Ok(())
}

/// The runtime presenting frames at 60 Hz of wall time.
pub fn pacing(adapter: &str, check: &mut Check) -> Result<(), String> {
    for (layout, count) in [(Layout::Distributed, 256u32), (Layout::Dense, 4096)] {
        let label = format!("paced_{}_{count}", layout.name());
        let setup = Setup {
            radius: SCALING_RADIUS,
            bodies: 4096,
            samples: SAMPLED as u32,
            timestamps: true,
            ..Setup::default()
        };
        let world = GpuWorld::open(setup.config(adapter)).map_err(|e| format!("open: {e:?}"))?;
        // Room to admit every spawn up front (the default admits 1024 waiting commands).
        let config = RuntimeConfig {
            pending_commands: 8192,
            receipts: 16384,
            ..RuntimeConfig::default()
        };
        let mut runtime =
            Runtime::new(Box::new(world), config).map_err(|e| format!("runtime: {e:?}"))?;
        for key in setup.window().view().iter() {
            runtime_upload(&mut runtime, key, &chunk_cells(key, &stone))?;
        }
        let mut id = 0u64;
        let first = runtime.last_submitted_tick().map_or(0, |t| t.value() + 1) + 1;
        for (k, command) in layout.spawns(count)?.into_iter().enumerate() {
            id += 1;
            runtime
                .submit_at(
                    CommandId::new(id),
                    command,
                    Tick::new(first + k as u64 / 256),
                )
                .map_err(|e| format!("spawn: {e:?}"))?;
        }
        // Run until spawned and, for the cluster, attracted together.
        let warm = first + u64::from(count) / 256 + if layout == Layout::Dense { 120 } else { 30 };
        let attract = |runtime: &mut Runtime, tick: u64, id: &mut u64| -> Result<(), String> {
            if layout == Layout::Dense {
                *id += 1;
                runtime
                    .submit_at(
                        CommandId::new(*id),
                        Command::AttractBodies {
                            target: Target::Column(IVec2::new(32, 32)),
                        },
                        Tick::new(tick),
                    )
                    .map_err(|e| format!("attract: {e:?}"))?;
            }
            Ok(())
        };
        for tick in first..warm {
            attract(&mut runtime, tick, &mut id)?;
        }
        while runtime
            .last_submitted_tick()
            .is_none_or(|t| t.value() < warm)
        {
            runtime.frame_ticks(1);
        }
        let controlled = BodyHandle::new(0, 0);
        runtime.set_controlled(Some(controlled));
        // Three seconds of 60 Hz frames, slept to their deadlines.
        let frame = Duration::from_secs_f64(1.0 / 60.0);
        let counters_before = runtime.counters();
        let ledger_before = *runtime.ledger().summary();
        let discarded_before = runtime.discarded_ticks();
        let mut frame_cpu = Vec::new();
        let mut submitted = Vec::new();
        let mut pressured = 0u32;
        let start = Instant::now();
        let mut last = start;
        let mut scheduled = warm + 1;
        for index in 1..=180u32 {
            let next = runtime.last_submitted_tick().map_or(0, |t| t.value() + 1);
            while scheduled < next + 8 {
                attract(&mut runtime, scheduled, &mut id)?;
                scheduled += 1;
            }
            let deadline = start + frame * index;
            if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            let now = Instant::now();
            let began = Instant::now();
            let report = runtime.frame(now - last);
            frame_cpu.push(began.elapsed().as_secs_f64() * 1e3);
            last = now;
            submitted.push(f64::from(report.ticks_submitted));
            pressured += u32::from(report.pressure.is_some());
        }
        let wall = start.elapsed().as_secs_f64();
        let counters = runtime.counters();
        let ledger = *runtime.ledger().summary();
        let body_summary = *runtime.bodies().summary();
        let ticks = counters.ticks_submitted - counters_before.ticks_submitted;
        check.metric(&format!("{label}_frames"), 180);
        check.metric(
            &format!("{label}_wall_seconds"),
            (wall * 100.0).round() / 100.0,
        );
        check.metric(&format!("{label}_ticks_submitted"), ticks);
        check.metric(
            &format!("{label}_ticks_per_second"),
            (ticks as f64 / wall * 10.0).round() / 10.0,
        );
        check.metric(
            &format!("{label}_ticks_discarded"),
            runtime.discarded_ticks() - discarded_before,
        );
        check.metric(&format!("{label}_frames_under_pressure"), pressured);
        check.metric(
            &format!("{label}_in_flight_high_water"),
            counters.in_flight_high_water,
        );
        check.metric(
            &format!("{label}_frame_cpu_ms_median"),
            (percentile(&mut frame_cpu, 0.5) * 1000.0).round() / 1000.0,
        );
        check.metric(
            &format!("{label}_frame_cpu_ms_p95"),
            (percentile(&mut frame_cpu, 0.95) * 1000.0).round() / 1000.0,
        );
        check.metric(
            &format!("{label}_frame_cpu_ms_max"),
            (percentile(&mut frame_cpu, 1.0) * 1000.0).round() / 1000.0,
        );
        let latency_count = ledger.tick_latency_ms.count - ledger_before.tick_latency_ms.count;
        let latency_sum = ledger.tick_latency_ms.sum - ledger_before.tick_latency_ms.sum;
        check.metric(
            &format!("{label}_tick_latency_ms_mean"),
            (latency_sum / latency_count.max(1) as f64 * 100.0).round() / 100.0,
        );
        check.metric(
            &format!("{label}_tick_latency_ms_max_whole_run"),
            (ledger.tick_latency_ms.max * 100.0).round() / 100.0,
        );
        check.metric(
            &format!("{label}_sample_age_ticks_max_whole_run"),
            body_summary.sample_age_ticks.max,
        );
        check.metric(
            &format!("{label}_sample_age_ticks_mean_whole_run"),
            body_summary.sample_age_ticks.mean().unwrap_or(0.0),
        );
        if let Some(last) = runtime.bodies().body(controlled).and_then(|t| t.last) {
            let age = runtime
                .last_submitted_tick()
                .map_or(0, |tick| tick.value().saturating_sub(last.tick));
            check.metric(&format!("{label}_controlled_sample_age_ticks_at_end"), age);
        }
        check.equal(&format!("{label}: frame waits"), counters.frame_waits, 0);
        check.that(counters.in_flight_high_water <= 3, || {
            format!("{label}: {} ticks in flight", counters.in_flight_high_water)
        });
        check.equal(
            &format!("{label}: inconsistent ticks"),
            ledger.inconsistent_ticks,
            0,
        );
    }
    Ok(())
}
