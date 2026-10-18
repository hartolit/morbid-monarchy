//! Controls through the runtime's command owner, and the CPU consumer of body observations
//! (checkpoint C).
//!
//! Commands enter through [`Runtime::submit_at`] under caller-chosen identities, so a retry is
//! the same identity submitted again; receipts hold each outcome until acknowledged. Body
//! observations reach the runtime's [`BodyMonitor`], which a consumer drains when it chooses.

use std::collections::BTreeMap;

use gpu_world::world::GpuWorld;
use world_model::{
    body::{BodyHandle, BodyPreset, JUMP_IMPULSE},
    cell::WorldCell,
    command::{BodySpawn, Command, Direction, Drive, Target},
    glam::IVec2,
    material::{FluidMat, GranularMat, TerrainMat},
    observation::{BodyCounters, BodySample},
    spatial::ChunkKey,
};
use world_runtime::{
    bodies::{BodyEvent, BodyMonitor},
    ids::{CommandId, Tick},
    port::{CommandResult, Outcome},
    receipts::{Admission, AdmissionError, ReceiptState},
    runtime::{Runtime, RuntimeConfig},
};

use super::{
    super::{
        Check,
        transfers::{completed, runtime_upload},
    },
    scene::{Scene, Setup, chunk_cells},
};

fn stone(_: IVec2, column: &mut WorldCell) {
    column.set_terrain_mat(TerrainMat::TERRAIN_STONE);
    column.set_elevation(40);
}

/// Sand west of x = 32, a water basin east of it: landings crater, plunges splash.
fn sand_and_water(cell: IVec2, column: &mut WorldCell) {
    if cell.x < 32 {
        column.set_elevation(40);
        column.set_granular_mat(GranularMat::GRANULAR_SAND);
        column.set_granular_vol(4);
    } else {
        column.set_terrain_mat(TerrainMat::TERRAIN_STONE);
        column.set_elevation(24);
        column.set_fluid_mat(FluidMat::FLUID_WATER);
        column.set_fluid_vol(20);
    }
}

fn spawn_at(cell: IVec2, drop: f32) -> Command {
    Command::SpawnBody(BodySpawn::new(BodyPreset::Metal, cell, drop).expect("valid"))
}

fn open_runtime(
    adapter: &str,
    setup: Setup,
    config: RuntimeConfig,
    events_per_tick: Option<u32>,
    shape: fn(IVec2, &mut WorldCell),
) -> Result<Runtime, String> {
    let mut world_config = setup.config(adapter);
    if let Some(events) = events_per_tick {
        world_config.transfer.events_per_tick = events;
    }
    let world = GpuWorld::open(world_config).map_err(|error| format!("open: {error:?}"))?;
    let mut runtime =
        Runtime::new(Box::new(world), config).map_err(|error| format!("runtime: {error:?}"))?;
    let view = setup.window().view();
    for y in view.min.key.y..=view.max.key.y {
        for x in view.min.key.x..=view.max.key.x {
            let key = ChunkKey::new(x, y, 0);
            runtime_upload(&mut runtime, key, &chunk_cells(key, &shape))?;
        }
    }
    Ok(runtime)
}

/// Runs paced frames of one tick each until `tick` has been submitted, then polls until every
/// submitted tick has been observed.
fn run_through(runtime: &mut Runtime, tick: u64) -> Result<(), String> {
    while runtime
        .last_submitted_tick()
        .is_none_or(|last| last.value() < tick)
    {
        runtime.frame_ticks(1);
    }
    settle(runtime)
}

fn settle(runtime: &mut Runtime) -> Result<(), String> {
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
        std::thread::sleep(std::time::Duration::from_micros(100));
    }
}

fn next(runtime: &Runtime) -> u64 {
    runtime
        .last_submitted_tick()
        .map_or(0, |tick| tick.value() + 1)
}

fn submit(
    runtime: &mut Runtime,
    id: u64,
    command: Command,
    tick: u64,
) -> Result<Admission, String> {
    runtime
        .submit_at(CommandId::new(id), command, Tick::new(tick))
        .map_err(|error| format!("submit {id}: {error:?}"))
}

fn latest(runtime: &Runtime, body: BodyHandle) -> Result<BodySample, String> {
    runtime
        .bodies()
        .body(body)
        .and_then(|tracked| tracked.last)
        .ok_or_else(|| format!("no sample of {body:?}"))
}

fn applied_at(runtime: &Runtime, id: u64) -> Option<Tick> {
    match runtime.receipt(CommandId::new(id))?.state {
        ReceiptState::Completed { tick, .. } => Some(tick),
        _ => None,
    }
}

/// Controls through the runtime: a drive and a jump submitted twice under one identity apply
/// once (a duplicate before and after completion is answered from the receipt), a jump in the
/// air is refused as not grounded, one identity cannot name two commands, a removed or
/// never-issued handle is stale for every body command, a full table refuses a spawn with
/// its capacity while a freed slot is reused under its next generation, and an acknowledged
/// identity cannot be submitted again.
pub fn controls(adapter: &str, check: &mut Check) -> Result<(), String> {
    let setup = Setup {
        bodies: 4,
        samples: 8,
        ..Setup::default()
    };
    let mut runtime = open_runtime(adapter, setup, RuntimeConfig::default(), None, stone)?;
    let start = next(&runtime) + 2;
    for (k, cell) in [IVec2::new(16, 30), IVec2::new(32, 30), IVec2::new(48, 30)]
        .iter()
        .enumerate()
    {
        submit(&mut runtime, 1 + k as u64, spawn_at(*cell, 1.0), start)?;
    }
    run_through(&mut runtime, start + 40)?;
    let mut bodies = Vec::new();
    for id in 1..=3 {
        match completed(&runtime, CommandId::new(id)) {
            Some(CommandResult::BodySpawned { body, .. }) => bodies.push(body),
            other => return Err(format!("spawn {id}: {other:?}")),
        }
    }
    for body in &bodies {
        check.that(latest(&runtime, *body)?.grounded, || {
            format!("{body:?} never landed")
        });
    }

    // A drive, submitted again before and after it applied.
    let east = Drive::controls(Direction::new(1.0, 0.0).map_err(|error| error.to_string())?);
    let drive = Command::DriveBody {
        body: bodies[0],
        drive: east,
    };
    let at = next(&runtime) + 1;
    check.equal(
        "drive admitted",
        submit(&mut runtime, 10, drive, at)?,
        Admission::Admitted,
    );
    check.equal(
        "drive resubmitted while pending",
        submit(&mut runtime, 10, drive, at)?,
        Admission::Duplicate(ReceiptState::Pending),
    );
    run_through(&mut runtime, at)?;
    check.equal(
        "drive outcome",
        completed(&runtime, CommandId::new(10)),
        Some(CommandResult::Driven { body: bodies[0] }),
    );
    check.equal(
        "drive applied at",
        applied_at(&runtime, 10),
        Some(Tick::new(at)),
    );
    let driven = latest(&runtime, bodies[0])?;
    let still = latest(&runtime, bodies[1])?;
    check.that(
        driven.velocity.x > 1.0 && driven.velocity.z.abs() < 0.1,
        || format!("the driven body is not moving east: {driven:?}"),
    );
    check.that(still.speed() < 0.1, || {
        format!("an undriven body moved: {still:?}")
    });
    let now = next(&runtime);
    let after = submit(&mut runtime, 10, drive, now)?;
    check.that(
        matches!(after, Admission::Duplicate(ReceiptState::Completed { .. })),
        || format!("drive resubmitted after completion: {after:?}"),
    );
    let later = next(&runtime) + 2;
    run_through(&mut runtime, later)?;
    check.equal(
        "drive applied at, after the retry",
        applied_at(&runtime, 10),
        Some(Tick::new(at)),
    );

    // A jump submitted three times applies once; a jump in the air is refused.
    let jump = Command::JumpBody { body: bodies[1] };
    let at = next(&runtime) + 1;
    check.equal(
        "jump admitted",
        submit(&mut runtime, 11, jump, at)?,
        Admission::Admitted,
    );
    for _ in 0..2 {
        check.equal(
            "jump resubmitted",
            submit(&mut runtime, 11, jump, at)?,
            Admission::Duplicate(ReceiptState::Pending),
        );
    }
    submit(&mut runtime, 12, jump, at + 1)?;
    run_through(&mut runtime, at)?;
    let rising = latest(&runtime, bodies[1])?;
    check.equal(
        "jump outcome",
        completed(&runtime, CommandId::new(11)),
        Some(CommandResult::Jumped {
            body: bodies[1],
            jumped: true,
        }),
    );
    check.that(
        rising.velocity.y > 0.8 * JUMP_IMPULSE && rising.velocity.y < 1.2 * JUMP_IMPULSE,
        || format!("one jump impulse expected: {rising:?}"),
    );
    run_through(&mut runtime, at + 1)?;
    check.equal(
        "jump in the air",
        completed(&runtime, CommandId::new(12)),
        Some(CommandResult::Jumped {
            body: bodies[1],
            jumped: false,
        }),
    );
    let falling = latest(&runtime, bodies[1])?;
    check.that(falling.velocity.y < rising.velocity.y, || {
        format!("the refused jump added speed: {rising:?} then {falling:?}")
    });

    // One identity, two commands.
    let at = next(&runtime) + 1;
    submit(&mut runtime, 13, drive, at)?;
    check.equal(
        "a second command under a held identity",
        runtime.submit_at(CommandId::new(13), jump, Tick::new(at)),
        Err(AdmissionError::Conflict(CommandId::new(13))),
    );

    // Stale handles.
    let removed = bodies[2];
    let at = next(&runtime) + 1;
    submit(&mut runtime, 20, Command::RemoveBody { body: removed }, at)?;
    let stale_commands = [
        Command::DriveBody {
            body: removed,
            drive: east,
        },
        Command::JumpBody { body: removed },
        Command::RemoveBody { body: removed },
        Command::DriveBody {
            body: BodyHandle::new(999, 0),
            drive: east,
        },
        Command::JumpBody {
            body: BodyHandle::new(0, 7),
        },
    ];
    for (k, command) in stale_commands.iter().enumerate() {
        submit(&mut runtime, 21 + k as u64, *command, at + 1)?;
    }
    run_through(&mut runtime, at + 1)?;
    check.that(
        matches!(completed(&runtime, CommandId::new(20)), Some(CommandResult::BodyRemoved { body, .. }) if body == removed),
        || format!("removal: {:?}", completed(&runtime, CommandId::new(20))),
    );
    for (k, command) in stale_commands.iter().enumerate() {
        let body = command.body().expect("a body command");
        check.equal(
            &format!("stale {command:?}"),
            completed(&runtime, CommandId::new(21 + k as u64)),
            Some(CommandResult::StaleBody { body }),
        );
    }

    // Capacity: two live bodies and four slots.
    let at = next(&runtime) + 1;
    for k in 0..3u64 {
        submit(
            &mut runtime,
            30 + k,
            spawn_at(IVec2::new(20 + 10 * k as i32, 45), 1.0),
            at,
        )?;
    }
    run_through(&mut runtime, at)?;
    check.that(
        matches!(completed(&runtime, CommandId::new(30)), Some(CommandResult::BodySpawned { body, .. }) if body == BodyHandle::new(2, 1)),
        || format!("freed slot reuse: {:?}", completed(&runtime, CommandId::new(30))),
    );
    check.that(
        matches!(completed(&runtime, CommandId::new(31)), Some(CommandResult::BodySpawned { body, .. }) if body == BodyHandle::new(3, 0)),
        || format!("last slot: {:?}", completed(&runtime, CommandId::new(31))),
    );
    check.equal(
        "spawn into a full table",
        completed(&runtime, CommandId::new(32)),
        Some(CommandResult::BodyCapacity { capacity: 4 }),
    );

    // Acknowledged identities are gone for good.
    runtime
        .acknowledge(CommandId::new(32))
        .map_err(|error| format!("acknowledge: {error:?}"))?;
    check.that(
        matches!(
            runtime.submit_at(CommandId::new(10), drive, Tick::new(next(&runtime))),
            Err(AdmissionError::Expired { .. })
        ),
        || "an acknowledged identity was admitted again".to_owned(),
    );
    let latency = runtime
        .receipt(CommandId::new(32))
        .and_then(|receipt| receipt.input_latency_ticks());
    check.metric("unknown_outcomes", runtime.unknown_outcomes());
    if let Some(latency) = latency {
        check.metric("scheduled_input_latency_ticks", latency);
    }
    check.equal("unknown outcomes", runtime.unknown_outcomes(), 0);
    Ok(())
}

/// Sums of delivered events per body: landings, liquid entries, contacts, craters, splashes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Delivered {
    counts: [u32; 5],
    spawned: u32,
    removals: u32,
    removed: Option<BodyCounters>,
}

fn record(log: &mut BTreeMap<BodyHandle, Delivered>, events: &[BodyEvent]) {
    for event in events {
        let entry = log.entry(event.body()).or_default();
        match *event {
            BodyEvent::Spawned { .. } => entry.spawned += 1,
            BodyEvent::Landed { count, .. } => entry.counts[0] += count,
            BodyEvent::EnteredLiquid { count, .. } => entry.counts[1] += count,
            BodyEvent::TouchedBody { count, .. } => entry.counts[2] += count,
            BodyEvent::Cratered { count, .. } => entry.counts[3] += count,
            BodyEvent::Splashed { count, .. } => entry.counts[4] += count,
            BodyEvent::Removed { counters, .. } => {
                entry.removals += 1;
                entry.removed = Some(counters);
            }
        }
    }
}

fn counts_of(counters: &BodyCounters) -> [u32; 5] {
    [
        counters.landings,
        counters.liquid_entries,
        counters.body_contacts,
        counters.craters,
        counters.splashes,
    ]
}

/// Checks that every removed body's delivered events add up to the counters it was removed
/// with, and that it was reported spawned and removed exactly once.
fn exact(check: &mut Check, label: &str, log: &BTreeMap<BodyHandle, Delivered>) -> u32 {
    let mut removed = 0;
    for (body, delivered) in log {
        check.equal(
            &format!("{label}: {body:?} spawn events"),
            delivered.spawned,
            1,
        );
        check.equal(
            &format!("{label}: {body:?} removal events"),
            delivered.removals,
            1,
        );
        match delivered.removed {
            Some(counters) => {
                removed += 1;
                check.equal(
                    &format!("{label}: {body:?} events against its final counters"),
                    delivered.counts,
                    counts_of(&counters),
                );
            }
            None => check.fail(format!("{label}: {body:?} was never reported removed")),
        }
    }
    removed
}

/// The consumer: bodies landing in sand and plunging into water, lifted every 40 ticks, with
/// a ring of eight events drained only every 25 ticks, six samples a tick for 30 bodies, four
/// cosmetic events a tick on the device, and slots freed and reused mid-run. Every body's
/// delivered landings, liquid entries, contacts, craters and splashes equal the counters its
/// removal reported, each spawn and removal is delivered once, the saturated ring deferred
/// rather than dropped, cosmetic events overflowed and were counted, samples were applied
/// within the frames in flight, and the consumer reacted to the controlled body's landings by
/// jumping it. Replays of a recorded outcome stream with samples delayed, reversed and
/// duplicated, and spawns delivered after the first samples, deliver the same events.
pub fn observation_delivery(adapter: &str, check: &mut Check) -> Result<(), String> {
    let setup = Setup {
        bodies: 32,
        samples: 6,
        ..Setup::default()
    };
    let config = RuntimeConfig {
        body_events: 8,
        cosmetic_events: 16,
        ..RuntimeConfig::default()
    };
    let mut runtime = open_runtime(adapter, setup, config, Some(4), sand_and_water)?;
    let max_in_flight = runtime.capabilities().max_in_flight;
    let mut id = 100u64;
    let mut issue = |runtime: &mut Runtime, command: Command, tick: u64| -> Result<u64, String> {
        id += 1;
        submit(runtime, id, command, tick)?;
        Ok(id)
    };
    let columns: Vec<IVec2> = (0..30)
        .map(|k| IVec2::new(8 + (k % 10) * 6, 20 + (k / 10) * 10))
        .collect();
    let start = next(&runtime) + 2;
    let mut spawns = Vec::new();
    for (k, cell) in columns.iter().enumerate() {
        spawns.push(issue(
            &mut runtime,
            spawn_at(*cell, 24.0 + (k % 3) as f32 * 8.0),
            start,
        )?);
    }
    run_through(&mut runtime, start)?;
    let mut live: Vec<BodyHandle> = Vec::new();
    for spawn in &spawns {
        match completed(&runtime, CommandId::new(*spawn)) {
            Some(CommandResult::BodySpawned { body, .. }) => live.push(body),
            other => return Err(format!("spawn: {other:?}")),
        }
    }
    let controlled = live[0];
    runtime.set_controlled(Some(controlled));
    let mut log: BTreeMap<BodyHandle, Delivered> = BTreeMap::new();
    let mut drained = Vec::new();
    let mut reactions = Vec::new();
    let mut removals = Vec::new();
    let end = start + 360;
    // Act once per tick: a frame may submit nothing while the port is backpressured.
    let mut acted = None;
    let mut drained_at = None;
    loop {
        let tick = next(&runtime);
        if tick >= end {
            break;
        }
        if acted != Some(tick) {
            acted = Some(tick);
            if tick.is_multiple_of(40) {
                for x in [16, 44] {
                    issue(
                        &mut runtime,
                        Command::LiftBodies {
                            target: Target::Column(IVec2::new(x, 30)),
                        },
                        tick,
                    )?;
                }
            }
            if tick == start + 150 {
                // Free six slots and fill them again in the next tick.
                for body in live.drain(1..7) {
                    removals.push(issue(&mut runtime, Command::RemoveBody { body }, tick)?);
                }
                for cell in &columns[1..7] {
                    let spawn = issue(&mut runtime, spawn_at(*cell, 24.0), tick + 1)?;
                    spawns.push(spawn);
                }
            }
        }
        runtime.frame_ticks(1);
        if tick.is_multiple_of(25) && drained_at != Some(tick) {
            drained_at = Some(tick);
            drained.clear();
            runtime.bodies_mut().drain_events(&mut drained);
            record(&mut log, &drained);
            // React: jump the controlled body when it landed.
            for event in &drained {
                if let BodyEvent::Landed {
                    body, tick: landed, ..
                } = event
                    && *body == controlled
                    && reactions.len() < 8
                {
                    let jump = issue(&mut runtime, Command::JumpBody { body: *body }, 0)?;
                    reactions.push((landed.value(), jump));
                }
            }
        }
    }
    settle(&mut runtime)?;
    // Track the respawned bodies, then remove every live body.
    let mut all = live.clone();
    for spawn in spawns.iter().skip(30) {
        if let Some(CommandResult::BodySpawned { body, .. }) =
            completed(&runtime, CommandId::new(*spawn))
        {
            all.push(body);
        }
    }
    check.equal("bodies live at the end", all.len(), 30);
    let at = next(&runtime) + 1;
    for body in &all {
        removals.push(issue(
            &mut runtime,
            Command::RemoveBody { body: *body },
            at,
        )?);
    }
    run_through(&mut runtime, at)?;
    drained.clear();
    runtime.bodies_mut().drain_events(&mut drained);
    record(&mut log, &drained);

    let removed = exact(check, "runtime", &log);
    check.equal("bodies reported removed", removed, 36);
    // The removals' own receipts agree with what the monitor delivered.
    for removal in &removals {
        if let Some(CommandResult::BodyRemoved { body, counters }) =
            completed(&runtime, CommandId::new(*removal))
        {
            check.equal(
                &format!("{body:?} removal receipt"),
                log.get(&body).and_then(|delivered| delivered.removed),
                Some(counters),
            );
        }
    }
    let totals = log.values().fold([0u32; 5], |mut sum, delivered| {
        for (total, count) in sum.iter_mut().zip(delivered.counts) {
            *total += count;
        }
        sum
    });
    check.that(totals.iter().all(|count| *count > 0), || {
        format!("not every kind of event happened: {totals:?}")
    });
    let summary = *runtime.bodies().summary();
    check.that(summary.events_deferred > 0, || {
        "the ring never filled".to_owned()
    });
    check.equal("ring high water", summary.ring_high_water, 8);
    check.equal("removals unreported", summary.removals_unreported, 0);
    check.that(
        summary.sample_age_ticks.max <= f64::from(max_in_flight),
        || {
            format!(
                "a sample applied {} ticks late",
                summary.sample_age_ticks.max
            )
        },
    );
    let ledger = *runtime.ledger().summary();
    check.that(ledger.cosmetic_dropped_on_device > 0, || {
        "four cosmetic events a tick never overflowed".to_owned()
    });
    check.equal("inconsistent ticks", ledger.inconsistent_ticks, 0);
    check.that(!reactions.is_empty(), || {
        "the consumer never reacted".to_owned()
    });
    let mut reaction_ticks = Vec::new();
    for (landed, jump) in &reactions {
        if let Some(applied) = applied_at(&runtime, *jump) {
            reaction_ticks.push(applied.value() - landed);
        }
    }
    check.equal("reactions applied", reaction_ticks.len(), reactions.len());
    check.metric("landings_crater_splash_contacts", format!("{totals:?}"));
    check.metric("events_delivered", summary.events);
    check.metric("events_deferred", summary.events_deferred);
    check.metric("samples", summary.samples);
    check.metric("stale_samples", summary.stale_samples);
    check.metric("retired_samples", summary.retired_samples);
    check.metric(
        "sample_age_mean_ticks",
        summary.sample_age_ticks.mean().unwrap_or(0.0),
    );
    check.metric("sample_age_max_ticks", summary.sample_age_ticks.max);
    check.metric(
        "cosmetic_dropped_on_device",
        ledger.cosmetic_dropped_on_device,
    );
    check.metric("cosmetic_overwritten_in_ring", ledger.cosmetic_overwritten);
    check.metric(
        "reaction_ticks_max",
        reaction_ticks.iter().copied().max().unwrap_or(0),
    );
    check.metric("unknown_outcomes", runtime.unknown_outcomes());

    replays(adapter, check)
}

/// Records a port-level run's outcomes, then replays them into fresh monitors in adversarial
/// orders: every replay must deliver what the in-order replay delivers.
fn replays(adapter: &str, check: &mut Check) -> Result<(), String> {
    let setup = Setup {
        bodies: 16,
        samples: 16,
        ..Setup::default()
    };
    let mut scene = Scene::open(adapter, setup, sand_and_water)?;
    let mut stream: Vec<Vec<Outcome>> = Vec::new();
    let spawns: Vec<Command> = (0..12)
        .map(|k| spawn_at(IVec2::new(10 + (k % 6) * 9, 24 + (k / 6) * 14), 14.0))
        .collect();
    scene.step(&spawns)?;
    stream.push(scene.outcomes.clone());
    for tick in 0..160u32 {
        let commands = match tick {
            40 | 100 => vec![
                Command::LiftBodies {
                    target: Target::Column(IVec2::new(20, 30)),
                },
                Command::LiftBodies {
                    target: Target::Column(IVec2::new(50, 30)),
                },
            ],
            70 => scene
                .bodies
                .iter()
                .take(4)
                .map(|body| Command::RemoveBody { body: *body })
                .collect(),
            71 => (0..4)
                .map(|k| spawn_at(IVec2::new(14 + 12 * k, 40), 16.0))
                .collect(),
            _ => Vec::new(),
        };
        scene.step(&commands)?;
        stream.push(scene.outcomes.clone());
    }
    let remove: Vec<Command> = scene
        .bodies
        .iter()
        .map(|body| Command::RemoveBody { body: *body })
        .collect();
    scene.step(&remove)?;
    stream.push(scene.outcomes.clone());

    let replay = |order: Order| -> BTreeMap<BodyHandle, Delivered> {
        let mut monitor = BodyMonitor::new(setup.bodies, order.ring);
        let mut log = BTreeMap::new();
        let mut events = Vec::new();
        let mut delayed: Vec<(usize, BodySample)> = Vec::new();
        let mut early: Vec<(BodyHandle, Tick, world_model::glam::DVec3)> = Vec::new();
        for (index, outcomes) in stream.iter().enumerate() {
            let mut samples: Vec<BodySample> = Vec::new();
            for outcome in outcomes {
                match outcome {
                    Outcome::Command(command) => match command.result {
                        CommandResult::BodySpawned { body, position } => {
                            if order.late_spawns {
                                early.push((body, command.tick, position));
                            } else {
                                monitor.spawned(body, command.tick, position);
                            }
                        }
                        CommandResult::BodyRemoved { body, counters } => {
                            monitor.removed(body, command.tick, counters)
                        }
                        _ => {}
                    },
                    Outcome::Body(sample) => samples.push(*sample),
                    _ => {}
                }
            }
            if order.reverse {
                samples.reverse();
            }
            for sample in samples {
                if order.delay > 0 && sample.tick % 3 == 0 {
                    delayed.push((index + order.delay, sample));
                    continue;
                }
                monitor.sample(sample, None);
                if order.duplicate {
                    monitor.sample(sample, None);
                }
            }
            let (due, waiting): (Vec<_>, Vec<_>) =
                delayed.drain(..).partition(|(at, _)| *at <= index);
            delayed = waiting;
            for (_, sample) in due {
                monitor.sample(sample, None);
            }
            // Spawns arrive two outcome batches late.
            if order.late_spawns && index % 2 == 1 {
                for (body, tick, position) in early.drain(..) {
                    monitor.spawned(body, tick, position);
                }
            }
            if index % order.drain_every == 0 {
                events.clear();
                monitor.drain_events(&mut events);
                record(&mut log, &events);
            }
        }
        for (_, sample) in delayed.drain(..) {
            monitor.sample(sample, None);
        }
        for (body, tick, position) in early.drain(..) {
            monitor.spawned(body, tick, position);
        }
        events.clear();
        monitor.drain_events(&mut events);
        record(&mut log, &events);
        log
    };
    let reference = replay(Order {
        ring: 4096,
        drain_every: 1,
        delay: 0,
        reverse: false,
        duplicate: false,
        late_spawns: false,
    });
    let removed = exact(check, "in order", &reference);
    check.equal("replayed bodies removed", removed, 16);
    for (label, order) in [
        (
            "delayed and reversed",
            Order {
                ring: 4,
                drain_every: 10,
                delay: 7,
                reverse: true,
                duplicate: false,
                late_spawns: false,
            },
        ),
        (
            "duplicated",
            Order {
                ring: 8,
                drain_every: 3,
                delay: 0,
                reverse: false,
                duplicate: true,
                late_spawns: false,
            },
        ),
        (
            "spawns after samples",
            Order {
                ring: 2,
                drain_every: 17,
                delay: 4,
                reverse: true,
                duplicate: true,
                late_spawns: true,
            },
        ),
    ] {
        let log = replay(order);
        exact(check, label, &log);
        check.equal(
            &format!("{label}: delivered as in order"),
            log,
            reference.clone(),
        );
    }
    scene.finish(check, "replay_source");
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct Order {
    ring: u32,
    drain_every: usize,
    delay: usize,
    reverse: bool,
    duplicate: bool,
    late_spawns: bool,
}
