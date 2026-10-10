//! Scripted driver for the Bevy baseline application.
//!
//! Runs one frozen scenario once through `morbid_app::build_app` in scripted input mode, then
//! writes per-frame samples and a summary. `cargo evidence bench` builds and launches it with a
//! fresh world directory per run; run it by hand only to reproduce one run:
//!
//! ```text
//! legacy-baseline --scenario FILE --world-dir NEW_DIR --samples FILE --summary FILE \
//!     --asset-dir crates/morbid-app/assets [--gpu-timestamps]
//! ```
//!
//! The timeline starts once every active chunk is resident. Physical actions are applied per
//! completed simulation tick (several in one frame when the clock catches up), so the simulated
//! content does not depend on the frame rate. The camera follows its path by logical tick.

use std::{
    collections::BTreeMap, fmt::Write as _, fs, path::PathBuf, process::ExitCode,
    sync::atomic::Ordering, time::Instant,
};

use bevy::{
    app::AppExit,
    diagnostic::DiagnosticsStore,
    ecs::system::SystemParam,
    prelude::*,
    render::{diagnostic::RenderDiagnosticsPlugin, renderer::RenderAdapterInfo},
};
use monarch_engine::{
    core::{
        counters::EngineCounters,
        entities::{observer::KinematicObserver, spherical::DynamicRigidSphere},
        world::{PRELOAD_EXT_RADIUS, PRELOAD_TRIGGER},
    },
    prelude::*,
};
use monarchy_evidence::{
    measurement::{CSV_HEADER, FrameSample, distribution},
    scenario::{
        self, Action, BodyPreset, BrushTool, Family, LoadedScenario, PresentMode, Scenario,
    },
};
use morbid_app::{
    AppOptions, InputMode, WindowRequest,
    baseline::{
        BrushSettings, DevOverlay, GridBrush, PersistenceCounters, RenderCounters, SphereSpawn,
        apply_brush, attract_sphere, drive_sphere, jump_sphere, lift_sphere, retained_payload,
        spawn_sphere,
    },
    build_app,
};
use spatial_lib::prelude::manager::ChunkManager;

/// Driver summary layout version; the bench runner refuses other versions.
const SUMMARY_SCHEMA: u32 = 1;
/// Resident-set size is read every this many frames.
const RSS_EVERY_FRAMES: u64 = 30;
/// Awake cells are counted (a full scan of the wake array) every this many frames.
const AWAKE_EVERY_FRAMES: u64 = 60;

struct Arguments {
    scenario: PathBuf,
    world_dir: PathBuf,
    samples: PathBuf,
    summary: PathBuf,
    asset_dir: PathBuf,
    gpu_timestamps: bool,
}

fn main() -> ExitCode {
    let started = Instant::now();
    let arguments = match parse_arguments() {
        Ok(arguments) => arguments,
        Err(message) => {
            eprintln!("legacy-baseline: {message}");
            return ExitCode::from(2);
        }
    };
    let loaded = match scenario::load(&arguments.scenario) {
        Ok(loaded) => loaded,
        Err(message) => {
            eprintln!("legacy-baseline: {message}");
            return ExitCode::from(2);
        }
    };
    // Every run starts from an empty store so all chunks are generated identically.
    if arguments.world_dir.exists() {
        eprintln!(
            "legacy-baseline: world directory {} already exists; each run needs a new one",
            arguments.world_dir.display()
        );
        return ExitCode::from(2);
    }
    if let Err(error) = fs::create_dir_all(&arguments.world_dir) {
        eprintln!(
            "legacy-baseline: create {}: {error}",
            arguments.world_dir.display()
        );
        return ExitCode::FAILURE;
    }

    let summary_path = arguments.summary.clone();
    let mut app = compose(&loaded.scenario, &arguments);
    app.insert_resource(Driver::new(loaded, arguments, started));
    app.run();

    // The summary exists only if the timeline finished or timed out; closing the window early
    // leaves none, which the runner reports as a failed run.
    let completed = fs::read_to_string(&summary_path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .is_some_and(|summary| summary["status"] == "completed");
    if completed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn parse_arguments() -> Result<Arguments, String> {
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    let mut gpu_timestamps = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--gpu-timestamps" => gpu_timestamps = true,
            "--scenario" | "--world-dir" | "--samples" | "--summary" | "--asset-dir" => {
                let value = args.next().ok_or_else(|| format!("{arg} needs a value"))?;
                values.insert(arg, value);
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    let mut take = |name: &str| {
        values
            .remove(name)
            .map(PathBuf::from)
            .ok_or_else(|| format!("missing {name}"))
    };
    Ok(Arguments {
        scenario: take("--scenario")?,
        world_dir: take("--world-dir")?,
        samples: take("--samples")?,
        summary: take("--summary")?,
        asset_dir: take("--asset-dir")?,
        gpu_timestamps,
    })
}

/// Builds the baseline application and replaces its default world settings with the scenario's.
fn compose(scenario: &Scenario, arguments: &Arguments) -> App {
    let mut app = build_app(AppOptions {
        world_dir: arguments.world_dir.clone(),
        asset_dir: Some(arguments.asset_dir.clone()),
        window: Some(WindowRequest {
            physical_size: scenario.presentation.resolution,
            vsync: scenario.presentation.present_mode == PresentMode::Vsync,
        }),
        input: InputMode::Scripted,
    });

    let world = &scenario.world;
    let [radius_x, radius_y] = world.active_radius_chunks;
    let width = (2 * radius_x as i32 + 1) * CHUNK_SIZE as i32;
    let height = (2 * radius_y as i32 + 1) * CHUNK_SIZE as i32;
    // The engine plugin inserted its defaults; these replace them before the first frame, the
    // same sizes the resize path would produce.
    app.insert_resource(WorldManager {
        inner: ChunkManager::new(
            radius_x,
            radius_y,
            PRELOAD_EXT_RADIUS,
            PRELOAD_TRIGGER,
            CHUNK_SIZE,
        ),
    })
    .insert_resource(ActiveWorldGrid::new(width, height, IVec2::ZERO))
    .insert_resource(SimulationConfig {
        run_liquid: world.families.contains(&Family::Liquid),
        run_granular: world.families.contains(&Family::Granular),
        run_thermal: world.families.contains(&Family::Thermal),
        run_fire: world.families.contains(&Family::Fire),
        run_biology: world.families.contains(&Family::Biology),
        tick_rate_hz: world.tick_rate_hz,
        max_ticks_per_frame: world.max_ticks_per_frame,
        ambient_temperature_c: world.ambient_temperature_c,
    })
    .insert_resource(GlobalPhysicsConfig {
        elevation_scale: world.elevation_scale,
        ..GlobalPhysicsConfig::default()
    })
    .insert_resource(DevOverlay {
        visible: scenario.presentation.overlay,
    })
    .add_systems(First, begin_frame)
    .add_systems(PostStartup, place_observer)
    .add_systems(PreUpdate, (capture_adapter, advance_timeline))
    .add_systems(Last, record_frame);
    if arguments.gpu_timestamps {
        app.add_plugins(RenderDiagnosticsPlugin);
    }
    app
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Loading,
    Running { start_tick: u64, start_frame: u64 },
    Finished,
}

#[derive(Default, Clone, Copy)]
struct CounterSnapshot {
    engine: EngineCounters,
    render: RenderCounters,
    load_requests: u64,
    loads_completed: u64,
    unload_events: u64,
    save_batches: u64,
    loads_generated: u64,
    loads_from_store: u64,
    chunks_written: u64,
    bytes_encoded: u64,
    write_failures: u64,
}

#[derive(Resource)]
struct Driver {
    scenario: Scenario,
    scenario_sha256: String,
    arguments: Arguments,
    process_started: Instant,
    stage: Stage,
    /// Frames presented since the process started.
    frame: u64,
    frame_started: Option<Instant>,
    first_frame_seconds: Option<f64>,
    residency_seconds: Option<f64>,
    /// Last logical tick whose actions were applied.
    applied_tick: Option<u64>,
    /// World tick at which the timeline started.
    timeline_start_tick: u64,
    /// Timeline tick when the current frame started, before it simulated.
    frame_start_tick: u64,
    driven_body: Option<Entity>,
    previous: CounterSnapshot,
    /// The frame recorded in `Last`, completed with its duration at the next `First`.
    pending: Option<FrameSample>,
    samples: Vec<FrameSample>,
    gpu_passes: BTreeMap<String, Vec<f64>>,
    gpu_seen: BTreeMap<String, Instant>,
    window_physical: Option<[u32; 2]>,
    window_mismatch: Option<String>,
    ticks_at_end: Option<u64>,
    /// The adapter Bevy selected, as reported by the renderer.
    adapter: serde_json::Value,
}

impl Driver {
    fn new(loaded: LoadedScenario, arguments: Arguments, process_started: Instant) -> Self {
        Self {
            scenario: loaded.scenario,
            scenario_sha256: loaded.sha256,
            arguments,
            process_started,
            stage: Stage::Loading,
            frame: 0,
            frame_started: None,
            first_frame_seconds: None,
            residency_seconds: None,
            applied_tick: None,
            timeline_start_tick: 0,
            frame_start_tick: 0,
            driven_body: None,
            previous: CounterSnapshot::default(),
            pending: None,
            samples: Vec::new(),
            gpu_passes: BTreeMap::new(),
            gpu_seen: BTreeMap::new(),
            window_physical: None,
            window_mismatch: None,
            ticks_at_end: None,
            adapter: serde_json::Value::Null,
        }
    }

    /// Timeline tick at which the measured window closes.
    fn final_tick(&self) -> u64 {
        self.scenario.run.warmup_ticks + self.scenario.run.measured_ticks
    }

    fn tick_seconds(&self) -> f32 {
        (1.0 / self.scenario.world.tick_rate_hz) as f32
    }
}

/// Completes the previous frame's sample with its duration and ends the run when the window is
/// full.
fn begin_frame(mut driver: ResMut<Driver>, mut exit: MessageWriter<AppExit>) {
    let now = Instant::now();
    if let (Some(started), Some(mut sample)) = (driver.frame_started, driver.pending.take()) {
        sample.frame_ms = now.duration_since(started).as_secs_f64() * 1000.0;
        driver.samples.push(sample);
    }
    driver.frame_started = Some(now);
    driver.frame += 1;
    if driver.first_frame_seconds.is_none() {
        driver.first_frame_seconds = Some(driver.process_started.elapsed().as_secs_f64());
    }

    let full = driver
        .samples
        .last()
        .is_some_and(|sample| sample.tick >= driver.final_tick());
    if full && driver.stage != Stage::Finished {
        driver.stage = Stage::Finished;
        write_outputs(&driver, "completed");
        exit.write(AppExit::Success);
    }
}

fn place_observer(
    driver: Res<Driver>,
    mut observers: Query<(&mut Transform, &mut KinematicObserver)>,
) {
    let camera = &driver.scenario.camera;
    for (mut transform, mut observer) in &mut observers {
        transform.translation = camera_position(camera, camera.start_cell.map(f64::from));
        observer.yaw = camera.yaw_degrees.to_radians();
        observer.pitch = camera.pitch_degrees.to_radians();
        observer.velocity = Vec3::ZERO;
        observer.is_noclip = true;
        observer.is_grid_attached = true;
    }
}

fn camera_position(camera: &scenario::Camera, cell: [f64; 2]) -> Vec3 {
    Vec3::new(cell[0] as f32, camera.height, -(cell[1] as f32))
}

/// Position along the closed camera path after `tick` ticks.
fn path_cell(camera: &scenario::Camera, tick: u64) -> [f64; 2] {
    if camera.path.len() < 2 {
        return camera.start_cell.map(f64::from);
    }
    let points: Vec<[f64; 2]> = camera.path.iter().map(|p| p.map(f64::from)).collect();
    let segments: Vec<([f64; 2], [f64; 2], f64)> = (0..points.len())
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % points.len()]);
            (a, b, ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt())
        })
        .collect();
    let perimeter: f64 = segments.iter().map(|segment| segment.2).sum();
    if perimeter <= 0.0 {
        return points[0];
    }
    let mut distance = (tick as f64 * camera.cells_per_tick) % perimeter;
    for (a, b, length) in segments {
        if distance <= length && length > 0.0 {
            let t = distance / length;
            return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        }
        distance -= length;
    }
    points[0]
}

/// What spawning a sphere needs from the world.
#[derive(SystemParam)]
struct SpawnTargets<'w, 's> {
    commands: Commands<'w, 's>,
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
}

/// The observer camera and the bodies, which never overlap.
#[derive(SystemParam)]
struct Actors<'w, 's> {
    observers: Query<'w, 's, &'static mut Transform, With<KinematicObserver>>,
    bodies: Query<
        'w,
        's,
        (Entity, &'static Transform, &'static mut DynamicRigidSphere),
        Without<KinematicObserver>,
    >,
}

/// Simulation settings an action may change.
#[derive(SystemParam)]
struct Settings<'w> {
    physics: Res<'w, GlobalPhysicsConfig>,
    simulation: ResMut<'w, SimulationConfig>,
}

/// Applies the timeline for every tick completed since the previous frame.
fn advance_timeline(
    mut driver: ResMut<Driver>,
    mut spawn: SpawnTargets,
    mut actors: Actors,
    mut grid: ResMut<ActiveWorldGrid>,
    store: Res<WorldStore>,
    mut settings: Settings,
    mut exit: MessageWriter<AppExit>,
) {
    let world_tick = u64::from(grid.tick);
    match driver.stage {
        Stage::Finished => return,
        Stage::Loading => {
            let [rx, ry] = driver.scenario.world.active_radius_chunks;
            let expected = u64::from((2 * rx + 1) * (2 * ry + 1));
            if store.inner.active.len() as u64 >= expected {
                driver.residency_seconds = Some(driver.process_started.elapsed().as_secs_f64());
                let start_frame = driver.frame;
                driver.stage = Stage::Running {
                    start_tick: world_tick,
                    start_frame,
                };
            } else {
                if driver.process_started.elapsed().as_secs_f64()
                    > driver.scenario.run.residency_timeout_seconds
                {
                    driver.stage = Stage::Finished;
                    write_outputs(&driver, "residency-timeout");
                    exit.write(AppExit::error());
                }
                return;
            }
        }
        Stage::Running { .. } => {}
    }
    let Stage::Running { start_tick, .. } = driver.stage else {
        return;
    };
    let tick = world_tick.wrapping_sub(start_tick);
    let first = driver.applied_tick.map_or(0, |applied| applied + 1);
    let tick_seconds = driver.tick_seconds();
    let scale = settings.physics.elevation_scale;
    let observer_position = actors
        .observers
        .iter()
        .next()
        .map(|transform| transform.translation);

    // Borrow the plan and the driven-body slot separately so actions are not cloned per tick.
    let Driver {
        scenario,
        driven_body,
        ..
    } = &mut *driver;
    for t in first..=tick {
        for group in scenario.bodies.iter().filter(|group| group.at_tick == t) {
            for index in 0..group.count {
                let x = group.origin_cell[0] as f32
                    + (index % group.columns) as f32 * group.spacing_cells;
                let y = group.origin_cell[1] as f32
                    + (index / group.columns) as f32 * group.spacing_cells;
                spawn_sphere(
                    &mut spawn.commands,
                    &mut spawn.meshes,
                    &mut spawn.materials,
                    &grid,
                    scale,
                    SphereSpawn {
                        wood: group.preset == BodyPreset::Wood,
                        hit: Vec3::new(x, 0.0, -y),
                        drop_height: group.drop_height,
                    },
                );
            }
        }
        let context = TickContext {
            tick: t,
            tick_seconds,
            scale,
            observer_position,
        };
        for action in &scenario.actions {
            if let Action::Climate {
                ambient_temperature_c,
                at_tick,
            } = *action
            {
                // `simulate_world` notices the change and wakes the window, as for the slider.
                if at_tick == t {
                    settings.simulation.ambient_temperature_c = ambient_temperature_c;
                }
                continue;
            }
            apply_action(action, &context, &mut grid, &mut actors, driven_body);
        }
    }
    driver.applied_tick = Some(tick);
    driver.timeline_start_tick = start_tick;
    driver.frame_start_tick = tick;

    let camera = &driver.scenario.camera;
    let position = camera_position(camera, path_cell(camera, tick));
    for mut transform in &mut actors.observers {
        transform.translation = position;
    }
}

/// Values every action reads for the tick being applied.
#[derive(Clone, Copy)]
struct TickContext {
    tick: u64,
    tick_seconds: f32,
    scale: f32,
    observer_position: Option<Vec3>,
}

/// Applies one action for the context's tick, if it is active then.
fn apply_action(
    action: &Action,
    context: &TickContext,
    grid: &mut ActiveWorldGrid,
    actors: &mut Actors,
    driven_body: &mut Option<Entity>,
) {
    let TickContext {
        tick: t,
        tick_seconds,
        scale,
        observer_position,
    } = *context;
    match *action {
        Action::Brush {
            tool,
            cell,
            radius,
            strength,
            from_tick,
            to_tick,
            every_ticks,
        } => {
            if (from_tick..=to_tick).contains(&t) && (t - from_tick) % every_ticks == 0 {
                let settings = BrushSettings { radius, strength };
                apply_brush(grid, grid_brush(tool), &settings, IVec2::from(cell));
            }
        }
        Action::Attract {
            cell,
            alternate_cell,
            alternate_every_ticks,
            from_tick,
            to_tick,
        } => {
            if (from_tick..=to_tick).contains(&t) {
                let alternate =
                    alternate_every_ticks > 0 && ((t - from_tick) / alternate_every_ticks) % 2 == 1;
                let target_cell = match (alternate, alternate_cell) {
                    (true, Some(alternate)) => alternate,
                    _ => cell,
                };
                let target = column_point(grid, target_cell, scale);
                for (_, transform, mut body) in &mut actors.bodies {
                    attract_sphere(transform, &mut body, target, tick_seconds);
                }
            }
        }
        Action::Lift {
            cell,
            from_tick,
            to_tick,
        } => {
            if (from_tick..=to_tick).contains(&t) {
                let target = column_point(grid, cell, scale);
                for (_, transform, mut body) in &mut actors.bodies {
                    lift_sphere(transform, &mut body, target, tick_seconds);
                }
            }
        }
        Action::Climate { .. } => {}
        Action::Drive {
            direction,
            from_tick,
            to_tick,
            jump_every_ticks,
        } => {
            if !(from_tick..=to_tick).contains(&t) {
                return;
            }
            if driven_body.is_none()
                && let Some(origin) = observer_position
            {
                *driven_body = actors
                    .bodies
                    .iter()
                    .min_by(|a, b| {
                        a.1.translation
                            .distance_squared(origin)
                            .total_cmp(&b.1.translation.distance_squared(origin))
                    })
                    .map(|(entity, _, _)| entity);
            }
            if let Some(entity) = *driven_body
                && let Ok((_, _, mut body)) = actors.bodies.get_mut(entity)
            {
                let move_dir = Vec3::new(direction[0], 0.0, -direction[1]).normalize();
                drive_sphere(&mut body, move_dir, tick_seconds);
                if jump_every_ticks > 0 && (t - from_tick) % jump_every_ticks == 0 {
                    jump_sphere(&mut body);
                }
            }
        }
    }
}

fn grid_brush(tool: BrushTool) -> GridBrush {
    match tool {
        BrushTool::Water => GridBrush::Water,
        BrushTool::Lava => GridBrush::Lava,
        BrushTool::Oil => GridBrush::Oil,
        BrushTool::Ice => GridBrush::Ice,
        BrushTool::Fire => GridBrush::Fire,
        BrushTool::Sand => GridBrush::Sand,
        BrushTool::Raise => GridBrush::RaiseTerrain,
        BrushTool::Lower => GridBrush::LowerTerrain,
    }
}

/// The top of the column at `cell`, where the pointer tools would hit it.
fn column_point(grid: &ActiveWorldGrid, cell: [i32; 2], scale: f32) -> Vec3 {
    let position = IVec2::from(cell);
    let lower = grid.spatial.window_origin;
    let upper = lower + IVec2::new(grid.spatial.width, grid.spatial.height);
    let inside = position.cmpge(lower).all() && position.cmplt(upper).all();
    let top = if inside {
        grid.get_cell(position).liquid_head() as f32 * scale
    } else {
        0.0
    };
    Vec3::new(cell[0] as f32, top, -(cell[1] as f32))
}

/// Records the frame's work once the timeline is running.
fn record_frame(world: &mut World) {
    let main_cpu_us = world
        .resource::<Driver>()
        .frame_started
        .map_or(0.0, |started| started.elapsed().as_secs_f64() * 1e6);
    let Stage::Running { start_frame, .. } = world.resource::<Driver>().stage else {
        return;
    };

    let snapshot = snapshot(world);
    let payload = retained_payload(world);
    let bodies = world
        .query_filtered::<(), With<DynamicRigidSphere>>()
        .iter(world)
        .count() as u64;
    let window = world
        .query::<&Window>()
        .iter(world)
        .next()
        .map(|window| [window.physical_width(), window.physical_height()]);
    let tick = u64::from(world.resource::<ActiveWorldGrid>().tick);
    let frame_in_timeline = world.resource::<Driver>().frame - start_frame;
    let awake_cells = frame_in_timeline
        .is_multiple_of(AWAKE_EVERY_FRAMES)
        .then(|| {
            world
                .resource::<ActiveWorldGrid>()
                .next_wake_buffer
                .iter()
                .filter(|wake| wake.load(Ordering::Relaxed) != 0)
                .count() as u64
        });
    let rss_bytes = frame_in_timeline
        .is_multiple_of(RSS_EVERY_FRAMES)
        .then(read_rss)
        .flatten();
    let gpu_values: Vec<(String, std::time::Instant, f64)> = world
        .get_resource::<DiagnosticsStore>()
        .map(|store| {
            store
                .iter()
                .filter(|diagnostic| {
                    let path = diagnostic.path().as_str();
                    path.starts_with("render/") && path.ends_with("/elapsed_gpu")
                })
                .filter_map(|diagnostic| {
                    let measurement = diagnostic.measurement()?;
                    Some((
                        diagnostic.path().as_str().to_owned(),
                        measurement.time,
                        measurement.value,
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    let mut driver = world.resource_mut::<Driver>();
    // A frame belongs to the measured window when it starts at or after the warm-up ticks.
    let measured = driver.frame_start_tick >= driver.scenario.run.warmup_ticks;
    let timeline_tick = tick.wrapping_sub(driver.timeline_start_tick);
    let previous = driver.previous;
    let delta = |now: u64, before: u64| now.saturating_sub(before);
    let engine = snapshot.engine;
    let before = previous.engine;
    let sample = FrameSample {
        frame: frame_in_timeline,
        measured,
        frame_ms: 0.0,
        main_cpu_us,
        ticks: delta(engine.ticks, before.ticks),
        tick: timeline_tick,
        simulation_us: delta(engine.simulation_nanos, before.simulation_nanos) as f64 / 1000.0,
        discarded_simulation_us: delta(
            engine.discarded_simulation_nanos,
            before.discarded_simulation_nanos,
        ) as f64
            / 1000.0,
        full_copy_bytes: delta(engine.full_copy_bytes, before.full_copy_bytes),
        wake_clear_bytes: delta(engine.wake_clear_bytes, before.wake_clear_bytes),
        body_us: delta(engine.body_nanos, before.body_nanos) as f64 / 1000.0,
        body_substeps: delta(engine.body_substeps, before.body_substeps),
        pair_tests: delta(engine.body_pair_tests, before.body_pair_tests),
        pair_contacts: delta(engine.body_pair_contacts, before.body_pair_contacts),
        bodies,
        render_sync_us: delta(snapshot.render.sync_nanos, previous.render.sync_nanos) as f64
            / 1000.0,
        grid_uploads: delta(snapshot.render.grid_uploads, previous.render.grid_uploads),
        grid_upload_bytes: delta(
            snapshot.render.grid_upload_bytes,
            previous.render.grid_upload_bytes,
        ),
        material_writes: delta(
            snapshot.render.material_writes,
            previous.render.material_writes,
        ),
        active_chunks: payload.active_chunks,
        cached_chunks: payload.cached_chunks,
        pending_loads: payload.pending_loads,
        save_queue_chunks: payload.save_queue_chunks,
        load_tasks: payload.load_tasks,
        save_tasks: payload.save_tasks,
        load_requests: delta(snapshot.load_requests, previous.load_requests),
        loads_completed: delta(snapshot.loads_completed, previous.loads_completed),
        loads_generated: delta(snapshot.loads_generated, previous.loads_generated),
        loads_from_store: delta(snapshot.loads_from_store, previous.loads_from_store),
        unload_events: delta(snapshot.unload_events, previous.unload_events),
        save_batches: delta(snapshot.save_batches, previous.save_batches),
        chunks_written: delta(snapshot.chunks_written, previous.chunks_written),
        bytes_encoded: delta(snapshot.bytes_encoded, previous.bytes_encoded),
        write_failures: delta(snapshot.write_failures, previous.write_failures),
        retained_bytes: payload.total_bytes(),
        rss_bytes,
        awake_cells,
    };
    driver.previous = snapshot;
    driver.pending = Some(sample);
    driver.ticks_at_end = Some(timeline_tick);

    if let Some(size) = window {
        let requested = driver.scenario.presentation.resolution;
        if size != requested && driver.window_mismatch.is_none() {
            driver.window_mismatch = Some(format!(
                "window is {}x{} physical pixels at frame {frame_in_timeline}, requested {}x{}",
                size[0], size[1], requested[0], requested[1]
            ));
        }
        driver.window_physical = Some(size);
    }
    if measured {
        for (path, time, value) in gpu_values {
            if driver.gpu_seen.get(&path) != Some(&time) {
                driver.gpu_seen.insert(path.clone(), time);
                driver.gpu_passes.entry(path).or_default().push(value);
            }
        }
    }
}

fn snapshot(world: &World) -> CounterSnapshot {
    let persistence = world.resource::<PersistenceCounters>();
    let tasks = &persistence.tasks;
    CounterSnapshot {
        engine: world
            .get_resource::<EngineCounters>()
            .copied()
            .unwrap_or_default(),
        render: world
            .get_resource::<RenderCounters>()
            .copied()
            .unwrap_or_default(),
        load_requests: persistence.load_requests,
        loads_completed: persistence.loads_completed,
        unload_events: persistence.unload_events,
        save_batches: persistence.save_batches,
        loads_generated: tasks.loads_generated.load(Ordering::Relaxed),
        loads_from_store: tasks.loads_from_store.load(Ordering::Relaxed),
        chunks_written: tasks.chunks_written.load(Ordering::Relaxed),
        bytes_encoded: tasks.bytes_encoded.load(Ordering::Relaxed),
        write_failures: tasks.write_failures.load(Ordering::Relaxed),
    }
}

fn read_rss() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    kib.checked_mul(1024)
}

fn write_outputs(driver: &Driver, status: &str) {
    let mut csv = String::from(CSV_HEADER);
    csv.push('\n');
    for sample in &driver.samples {
        let _ = writeln!(csv, "{}", sample.to_csv());
    }
    if let Err(error) = fs::write(&driver.arguments.samples, csv) {
        eprintln!("legacy-baseline: write samples: {error}");
    }

    let gpu_passes: BTreeMap<&str, serde_json::Value> = driver
        .gpu_passes
        .iter()
        .map(|(path, values)| {
            (
                path.as_str(),
                serde_json::to_value(distribution(values.iter().copied())).unwrap_or_default(),
            )
        })
        .collect();
    let summary = serde_json::json!({
        "schema": SUMMARY_SCHEMA,
        "status": status,
        "scenario": driver.arguments.scenario,
        "scenario_sha256": driver.scenario_sha256,
        "gpu_timestamps": driver.arguments.gpu_timestamps,
        "adapter": driver.adapter,
        "window_requested": driver.scenario.presentation.resolution,
        "window_physical": driver.window_physical,
        "window_mismatch": driver.window_mismatch,
        "present_mode": driver.scenario.presentation.present_mode,
        "first_frame_seconds": driver.first_frame_seconds,
        "residency_seconds": driver.residency_seconds,
        "samples": driver.samples.len(),
        "warmup_ticks": driver.scenario.run.warmup_ticks,
        "measured_ticks": driver.scenario.run.measured_ticks,
        "final_tick": driver.ticks_at_end,
        "gpu_pass_elapsed_ms": gpu_passes,
    });
    let text = serde_json::to_string_pretty(&summary).unwrap_or_default();
    if let Err(error) = fs::write(&driver.arguments.summary, text + "\n") {
        eprintln!("legacy-baseline: write summary: {error}");
    }
}

/// Records the adapter Bevy selected, once the renderer has reported it.
fn capture_adapter(mut driver: ResMut<Driver>, info: Option<Res<RenderAdapterInfo>>) {
    let Some(info) = info else { return };
    if !driver.adapter.is_null() {
        return;
    }
    driver.adapter = serde_json::json!({
        "name": info.name,
        "vendor": format!("0x{:04x}", info.vendor),
        "device": format!("0x{:04x}", info.device),
        "device_type": format!("{:?}", info.device_type),
        "backend": format!("{:?}", info.backend).to_lowercase(),
        "driver": info.driver,
        "driver_info": info.driver_info,
    });
}
