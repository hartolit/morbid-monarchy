//! Body identity and the fixed clock: generation-bearing handles, bounded allocation and
//! retirement; substeps that divide the tick, the speed cap and the travel bound; and the same
//! bodies at the same tick whatever the presentation rate.

use std::time::Duration;

use gpu_world::world::GpuWorld;
use world_model::{
    body::{
        self, BodyHandle, BodyPreset, MAX_BODY_SPEED, MAX_SUBSTEP_TRAVEL, SphereDescription,
        substeps_per_tick,
    },
    command::{BodySpawn, Command, Direction, Drive, Ray, Target},
    glam::{DVec3, IVec2, Vec3},
    material::TerrainMat,
    observation::BodySample,
    units::GRAVITY_Y,
};
use world_runtime::{
    bodies::BodyMonitor,
    ids::{CommandId, Tick},
    port::{CommandResult, Outcome},
    runtime::{Runtime, RuntimeConfig},
};

use super::{
    super::Check,
    scene::{Scene, Setup, chunk_cells, scale},
};

fn metal() -> SphereDescription {
    BodyPreset::Metal.description()
}

fn stone(_: IVec2, column: &mut world_model::cell::WorldCell) {
    column.set_terrain_mat(TerrainMat::TERRAIN_STONE);
    column.set_elevation(40);
}

fn spawn_at(cell: IVec2, drop: f32) -> Command {
    Command::SpawnBody(BodySpawn::new(BodyPreset::Metal, cell, drop).expect("valid"))
}

/// Handles: a full table refuses, removal makes a handle stale at once, a slot is reused under
/// its next generation, a missed or non-resident spawn frees its slot, and a sample of a stale
/// handle is reported gone and ignored by the monitor.
pub fn body_handles(adapter: &str, check: &mut Check) -> Result<(), String> {
    let setup = Setup {
        bodies: 4,
        samples: 8,
        ..Setup::default()
    };
    let mut scene = Scene::open(adapter, setup, stone)?;
    let spawns: Vec<Command> = (0..5)
        .map(|k| spawn_at(IVec2::new(10 + 12 * k, 30), 1.0))
        .collect();
    let results = scene.step(&spawns)?;
    for (index, result) in results.iter().take(4).enumerate() {
        check.that(
            matches!(result, CommandResult::BodySpawned { body, .. } if *body == BodyHandle::new(index as u32, 0)),
            || format!("spawn {index}: {result:?}"),
        );
    }
    check.equal(
        "fifth spawn into a table of four",
        results[4],
        CommandResult::BodyCapacity { capacity: 4 },
    );
    scene.run(30)?;
    let old = BodyHandle::new(1, 0);
    let landed = scene.sample(old)?.counters.landings;
    let east = Drive::controls(Direction::new(1.0, 0.0).expect("unit"));
    let results = scene.step(&[
        Command::RemoveBody { body: old },
        Command::DriveBody {
            body: old,
            drive: east,
        },
        Command::JumpBody { body: old },
        Command::RemoveBody { body: old },
    ])?;
    check.that(
        matches!(results[0], CommandResult::BodyRemoved { body, counters } if body == old && counters.landings == landed),
        || format!("removal: {:?}", results[0]),
    );
    for (index, label) in [(1, "drive"), (2, "jump"), (3, "second removal")] {
        check.equal(
            &format!("{label} of the removed handle"),
            results[index],
            CommandResult::StaleBody { body: old },
        );
    }
    // The freed slot is reused under the next generation; the old handle stays stale and a
    // sample of it is reported gone.
    let reused = scene.step(&[spawn_at(IVec2::new(20, 50), 1.0)])?;
    let new = BodyHandle::new(1, 1);
    check.that(
        matches!(reused[0], CommandResult::BodySpawned { body, .. } if body == new),
        || format!("reuse: {:?}", reused[0]),
    );
    scene.extra_observe.push(old);
    scene.step(&[])?;
    let gone = scene
        .outcomes
        .iter()
        .any(|outcome| matches!(outcome, Outcome::BodyGone { body, .. } if *body == old));
    check.that(gone, || "the stale handle was not reported gone".to_owned());
    check.that(scene.sample(new).is_ok(), || {
        "the new body was not sampled".to_owned()
    });
    scene.extra_observe.clear();
    // A spawn whose ray misses and one into a column outside the window free their slots.
    let up = Ray::new(DVec3::new(20.0, 50.0, -20.0), Vec3::Y, 100.0).expect("ray");
    let missed = BodySpawn::with(metal(), Target::Ray(up), 1.0).expect("valid");
    let remove = Command::RemoveBody {
        body: BodyHandle::new(3, 0),
    };
    let results = scene.step(&[remove, Command::SpawnBody(missed)])?;
    check.that(matches!(results[1], CommandResult::TargetMissed(_)), || {
        format!("a spawn whose ray points up: {:?}", results[1])
    });
    let results = scene.step(&[spawn_at(IVec2::new(5_000, 5_000), 1.0)])?;
    check.that(
        matches!(results[0], CommandResult::TargetNotResident { .. }),
        || format!("a spawn outside the window: {:?}", results[0]),
    );
    let results = scene.step(&[spawn_at(IVec2::new(40, 40), 1.0)])?;
    check.that(
        matches!(results[0], CommandResult::BodySpawned { body, .. } if body.index() == 3 && body.generation() == 3),
        || format!("the slot freed by a removal and two failed spawns: {:?}", results[0]),
    );
    // The monitor applies each counter change once and ignores the retired generation.
    let mut monitor = BodyMonitor::new(4, 64);
    let tick = Tick::new(100);
    let late = BodySample {
        body: old,
        tick: 99,
        ..scene.sample(new)?
    };
    monitor.removed(old, tick, Default::default());
    monitor.sample(late, Some(tick));
    check.equal("retired samples", monitor.summary().retired_samples, 1);
    scene.finish(check, "handles");
    Ok(())
}

/// The scheme the solver integrates a free body with (`integrate_body_forces`), in `f64`: the
/// height lost after `steps` substeps of `dt` from rest.
fn free_fall(steps: u32, dt: f64) -> (f64, f64) {
    let (mut v, mut y) = (0.0f64, 0.0f64);
    let drag = f64::from(body::AIR_DRAG_RATE);
    for _ in 0..steps {
        v += f64::from(GRAVITY_Y) * dt;
        v *= (-drag * dt).exp();
        y += v * dt;
    }
    (y, v)
}

/// The tick splits into the fewest substeps of at most 1/120 s at every supported rate; one
/// simulated second of free fall follows the scheme the solver states and the continuous
/// solution within the step's error, whatever the rate; speeds are capped at the supported
/// limit (and the cap counted) and no substep travels farther than the bound.
pub fn fixed_tick_substeps(adapter: &str, check: &mut Check) -> Result<(), String> {
    let drag = f64::from(body::AIR_DRAG_RATE);
    let g = f64::from(GRAVITY_Y);
    let continuous = |t: f64| {
        let terminal = g / drag;
        (
            terminal * t - terminal / drag * (1.0 - (-drag * t).exp()),
            terminal * (1.0 - (-drag * t).exp()),
        )
    };
    for rate in [10.0, 30.0, 60.0, 144.0, 240.0] {
        let (exact_drop, _) = continuous(1.0 + 1.0 / rate);
        let setup = Setup {
            tick_rate_hz: rate,
            ..Setup::default()
        };
        let mut scene = Scene::open(adapter, setup, stone)?;
        let body = scene.spawn(metal(), 32.0, -32.0, 200.0)?;
        let start = scene.position(body)?.y;
        // A body integrates in the tick that spawns it, so one second more of ticks makes
        // `rate + 1` ticks of fall.
        let ticks = rate as u32 + 1;
        scene.run(ticks - 1)?;
        let substeps = scene.last.map(|last| last.substeps).unwrap_or(0);
        check.equal(
            &format!("{rate} Hz: substeps"),
            substeps,
            substeps_per_tick(rate),
        );
        let sample = scene.sample(body)?;
        let dropped = f64::from(sample.position.y as f32 - start);
        let (scheme_drop, scheme_speed) =
            free_fall(ticks * substeps, 1.0 / (rate * f64::from(substeps)));
        check.that(
            (dropped - scheme_drop).abs() < 1e-3 * scheme_drop.abs(),
            || format!("{rate} Hz: fell {dropped}, the scheme {scheme_drop}"),
        );
        check.that(
            (f64::from(sample.velocity.y) - scheme_speed).abs() < 1e-3 * scheme_speed.abs(),
            || {
                format!(
                    "{rate} Hz: speed {}, the scheme {scheme_speed}",
                    sample.velocity.y
                )
            },
        );
        // Semi-implicit Euler at 1/120 s or less stays within 1% of the continuous fall.
        check.that(
            (dropped - exact_drop).abs() < 0.01 * exact_drop.abs(),
            || format!("{rate} Hz: fell {dropped}, continuously {exact_drop}"),
        );
        check.metric(&format!("drop_{rate}hz"), dropped);
    }

    // Attracted toward the ground below it, a body falls with gravity and attract together,
    // toward a terminal speed of (350 + 32) / 0.6 ≈ 637, past the supported limit: the solver
    // caps it at 600 and counts each capped substep.
    let mut scene = Scene::open(adapter, Setup::default(), stone)?;
    let below = IVec2::new(32, 32);
    let results = scene.step(&[spawn_at(below, 3_000.0)])?;
    let CommandResult::BodySpawned { body, .. } = results[0] else {
        return Err(format!("high spawn: {:?}", results[0]));
    };
    let target = Target::Column(below);
    let mut fastest = 0.0f32;
    let mut travel = 0.0f32;
    for _ in 0..600 {
        let before = scene.position(body)?;
        scene.step(&[Command::AttractBodies { target }])?;
        let sample = scene.sample(body)?;
        fastest = fastest.max(sample.speed());
        travel = travel.max(before.distance(sample.position.as_vec3()));
        if sample.position.y < 200.0 || sample.suspended {
            break;
        }
    }
    let limited = scene.body_totals.speed_limited;
    check.that(fastest <= MAX_BODY_SPEED * 1.0001, || {
        format!("speed reached {fastest}")
    });
    check.that(limited > 0, || "the cap never acted".to_owned());
    // Two substeps per tick at 60 Hz: a tick moves at most twice the substep bound.
    check.that(travel <= 2.0 * MAX_SUBSTEP_TRAVEL * 1.001, || {
        format!("a tick moved {travel}")
    });
    check.metric("fastest", f64::from(fastest));
    check.metric("speed_limited_substeps", u64::from(limited));
    check.metric("longest_tick_travel", f64::from(travel));
    Ok(())
}

fn runtime_world(adapter: &str, setup: Setup) -> Result<Runtime, String> {
    let world =
        GpuWorld::open(setup.config(adapter)).map_err(|error| format!("open: {error:?}"))?;
    Runtime::new(Box::new(world), RuntimeConfig::default())
        .map_err(|error| format!("runtime: {error:?}"))
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

/// Fixed-tick invariance: the same tick-scheduled spawns, drives and jumps, presented at 30,
/// 60 and 144 Hz and at 60 Hz with 400 ms stalls, leave every body in the same state at the same
/// tick, bit for bit on one adapter (the solver sums in fixed point and orders every reduction),
/// and the stalled run discards ticks rather than integrating a longer step.
pub fn body_presentation_rates(adapter: &str, check: &mut Check) -> Result<(), String> {
    const START: u64 = 40;
    const TARGET: u64 = 200;
    let pacings: [(&str, &[f64]); 4] = [
        ("30 Hz", &[1.0 / 30.0]),
        ("60 Hz", &[1.0 / 60.0]),
        ("144 Hz", &[1.0 / 144.0]),
        (
            "60 Hz with stalls",
            &[1.0 / 60.0, 1.0 / 60.0, 1.0 / 60.0, 0.4],
        ),
    ];
    let setup = Setup::default();
    let mut worlds: Vec<(&str, Vec<BodySample>, u64)> = Vec::new();
    for (name, frames) in pacings {
        let mut runtime = runtime_world(adapter, setup)?;
        let view = setup.window().view();
        for y in view.min.key.y..=view.max.key.y {
            for x in view.min.key.x..=view.max.key.x {
                let key = world_model::spatial::ChunkKey::new(x, y, 0);
                super::super::transfers::runtime_upload(
                    &mut runtime,
                    key,
                    &chunk_cells(key, &|cell, column| {
                        column.set_elevation(40 + (cell.x.rem_euclid(9) == 0) as u16);
                        column.set_granular_mat(world_model::material::GranularMat::GRANULAR_SAND);
                        column.set_granular_vol(3);
                    }),
                )?;
            }
        }
        while runtime
            .last_submitted_tick()
            .is_none_or(|tick| tick.value() + 1 < START)
        {
            runtime.frame_ticks(1);
            idle(&mut runtime)?;
        }
        let mut id = 0u64;
        let mut schedule = |runtime: &mut Runtime, command: Command, tick: u64| {
            id += 1;
            runtime
                .submit_at(CommandId::new(id), command, Tick::new(tick))
                .map(|_| ())
                .map_err(|error| format!("submit: {error:?}"))
        };
        for k in 0..6 {
            schedule(
                &mut runtime,
                spawn_at(IVec2::new(16 + 6 * k, 30 + (k % 2) * 5), 6.0),
                START,
            )?;
        }
        let handles: Vec<BodyHandle> = (0..6).map(|k| BodyHandle::new(k, 0)).collect();
        for tick in START + 20..TARGET - 20 {
            for (k, body) in handles.iter().enumerate() {
                let direction =
                    Direction::new(1.0, if k % 2 == 0 { 0.3 } else { -0.3 }).expect("unit");
                schedule(
                    &mut runtime,
                    Command::DriveBody {
                        body: *body,
                        drive: Drive::controls(direction),
                    },
                    tick,
                )?;
            }
            if tick % 37 == 0 {
                schedule(&mut runtime, Command::JumpBody { body: handles[0] }, tick)?;
            }
        }
        let mut frame = 0;
        while runtime
            .last_submitted_tick()
            .is_none_or(|tick| tick.value() < TARGET - 4)
        {
            runtime.frame(Duration::from_secs_f64(frames[frame % frames.len()]));
            frame += 1;
            idle(&mut runtime)?;
        }
        while runtime
            .last_submitted_tick()
            .is_some_and(|tick| tick.value() < TARGET)
        {
            runtime.frame_ticks(1);
        }
        check.equal(
            &format!("{name}: last tick"),
            runtime.last_submitted_tick(),
            Some(Tick::new(TARGET)),
        );
        // The runtime samples every tracked body each tick (six fit the selection).
        idle(&mut runtime)?;
        let mut final_samples = Vec::new();
        for body in &handles {
            let tracked = runtime
                .bodies()
                .body(*body)
                .ok_or_else(|| format!("{body:?} untracked"))?;
            let last = tracked.last.ok_or("no sample")?;
            check.equal(
                &format!("{name}: {body:?} sampled at the target"),
                last.tick,
                TARGET,
            );
            final_samples.push(last);
        }
        let summary = *runtime.ledger().summary();
        check.equal(
            &format!("{name}: inconsistent ticks"),
            summary.inconsistent_ticks,
            0,
        );
        check.metric(
            &format!("{}_discarded_ticks", name.replace(' ', "_")),
            runtime.discarded_ticks(),
        );
        worlds.push((name, final_samples, runtime.discarded_ticks()));
    }
    let (_, reference, _) = &worlds[0];
    for (name, samples, _) in &worlds[1..] {
        let differing = samples
            .iter()
            .zip(reference)
            .filter(|(a, b)| {
                a.position != b.position || a.velocity != b.velocity || a.counters != b.counters
            })
            .count();
        check.equal(
            &format!("bodies of {name} differing from 30 Hz"),
            differing,
            0,
        );
    }
    check.that(worlds[3].2 > 0, || {
        "the stalled run discarded nothing".to_owned()
    });
    let travelled = reference
        .iter()
        .map(|s| s.position.x)
        .fold(f64::MIN, f64::max);
    check.metric("east_most", travelled);
    let _ = scale();
    Ok(())
}
