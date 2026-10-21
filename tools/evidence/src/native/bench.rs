//! The native client measured on a frozen scenario, for `cargo evidence bench`: a new world made
//! to the scenario's settings, the observer on the scenario's camera path by logical tick, and
//! the scenario's commands issued through the client's own command path for the exact ticks
//! they belong to, so the simulated content does not depend on the frame rate. Every frame is
//! sampled in the bench's CSV layout, and the run's summary names the adapter, the window and the
//! presentation, as the baseline driver's did.
//!
//! A sample is one presented frame. The client's loop also runs iterations that present nothing
//! (both presentation slots busy, or the surface unavailable) and still advance the timeline;
//! they belong to the next presented frame's sample. `frame_ms` is the time between the starts of
//! consecutive presenting iterations, `main_cpu_us` the presenting iteration's CPU time (input,
//! overlay, intent, the session's frame, encoding, acquisition and submission), `ticks` the ticks
//! submitted since the previous presented frame, and `simulation_us` and `body_us` the device
//! time of the ticks observed complete in that span when the run measures timestamps (the
//! diagnostic run), zero otherwise. Columns whose baseline meaning has no native counterpart
//! stay zero: there is no CPU simulation, no CPU grid copy and no render-asset synchronization.
//!
//! A drive action possesses the body whose spawn outcome placed it nearest the observer's path
//! position at the drive's first tick: the outcome is the same in every run, where the bodies'
//! positions at that moment would depend on which samples happened to have arrived.
//!
//! Beyond the samples, the summary accounts for the whole measured window (`window`): the
//! process's and the main thread's CPU time and allocations (every loop iteration, presenting
//! or not, and every thread), the session's frame time by owner, transfer bytes by purpose,
//! waits, pressure and observation age; and per presented frame the swapchain acquisition's
//! wait and the session's frame time over the span.

use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    time::{Duration, Instant},
};

use morbid_app::{
    catalog,
    client::Running,
    driver::{Control, Driver, DriverFrame, PhaseKind},
    input::InputState,
    observer::Observer,
    play::Purpose,
};
use serde_json::{Value, json};
use world_model::{
    body::BodyHandle,
    command::{AmbientTemperature, Command, ElevationScale},
    glam::DVec3,
    rules::SimulationRules,
};
use world_runtime::{
    ids::Tick,
    port::{ResidentWindow, TransferCounters},
    receipts::AdmissionError,
    session::FrameTimes,
    store::PhysicalConfig,
};

use crate::{
    measurement::{self, CSV_HEADER, FrameSample},
    scenario::{Camera, Family, LoadedScenario, PresentMode, Scenario},
    suites::alloc,
};

/// Resident-set size is read every this many frames, as the baseline driver did.
const RSS_EVERY_FRAMES: u64 = 30;

/// The world a run measures: created in `worlds` under `name` with the scenario's seed, rules,
/// climate and scale, its window around the camera's start.
pub fn create_world(
    worlds: &std::path::Path,
    name: &str,
    scenario: &Scenario,
) -> Result<PathBuf, String> {
    let world = &scenario.world;
    let has = |family| world.families.contains(&family);
    let config = PhysicalConfig {
        rules: SimulationRules {
            liquid: has(Family::Liquid),
            granular: has(Family::Granular),
            thermal: has(Family::Thermal),
            fire: has(Family::Fire),
            biology: has(Family::Biology),
        },
        ambient_temperature: AmbientTemperature::celsius(world.ambient_temperature_c)
            .map_err(|error| format!("ambient temperature: {error:?}"))?,
        elevation_scale: ElevationScale::new(world.elevation_scale)
            .map_err(|error| format!("elevation scale: {error:?}"))?,
    };
    let start = observer_at(&scenario.camera, 0);
    let [radius_x, radius_y] = world.active_radius_chunks;
    let window = ResidentWindow::new(start.chunk(), radius_x, radius_y)
        .map_err(|error| format!("window: {error:?}"))?;
    catalog::create_with(worlds, name, world.seed, config, window)
}

/// The observer the scenario places at timeline tick `tick`: on the closed path at the camera's
/// height and angles, flying and holding the resident window around itself.
fn observer_at(camera: &Camera, tick: u64) -> Observer {
    let cell = path_cell(camera, tick);
    let mut observer = Observer::above(world_model::spatial::ChunkKey::new(0, 0, 0));
    observer.position = DVec3::new(cell[0], f64::from(camera.height), -cell[1]);
    observer.yaw = camera.yaw_degrees.to_radians();
    observer.pitch = camera.pitch_degrees.to_radians();
    observer.noclip = true;
    observer.attached = true;
    observer
}

/// Position along the closed camera path after `tick` ticks (the baseline driver's rule).
fn path_cell(camera: &Camera, tick: u64) -> [f64; 2] {
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

/// Running totals read at the end of every frame; samples hold their differences.
#[derive(Debug, Clone, Copy, Default)]
struct Totals {
    ticks_submitted: u64,
    device_ticks: u64,
    device_ms: f64,
    body_ms: f64,
    upload_bytes: u64,
    loads_generated: u64,
    loads_stored: u64,
    writes_committed: u64,
    write_failures: u64,
}

impl Totals {
    fn read(running: &Running) -> Self {
        let session = running.session();
        let runtime = session.runtime();
        let counters = runtime.counters();
        let ledger = runtime.ledger().summary();
        let residency = session.residency().summary();
        Self {
            ticks_submitted: counters.ticks_submitted,
            device_ticks: ledger.gpu_time_ms.count,
            device_ms: ledger.gpu_time_ms.sum,
            body_ms: ledger.body_time_ms.sum,
            upload_bytes: counters.upload.total(),
            loads_generated: residency.loads_generated,
            loads_stored: residency.loads_stored,
            writes_committed: residency.writes_committed,
            write_failures: residency.write_failures,
        }
    }
}

/// The process's CPU time (user and system, every thread, exited ones included), from
/// `/proc/self/stat` in clock ticks of 10 ms (Linux's fixed `USER_HZ` of 100).
fn process_cpu() -> Option<Duration> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let ticks: u64 = fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?;
    Some(Duration::from_millis(ticks * 10))
}

/// The calling thread's time on a CPU, in nanoseconds, from `/proc/thread-self/schedstat`.
fn thread_cpu() -> Option<Duration> {
    let schedstat = std::fs::read_to_string("/proc/thread-self/schedstat").ok()?;
    let nanos: u64 = schedstat.split_whitespace().next()?.parse().ok()?;
    Some(Duration::from_nanos(nanos))
}

fn add_times(sum: &mut FrameTimes, times: FrameTimes) {
    sum.runtime += times.runtime;
    sum.residency += times.residency;
    sum.checkpoints += times.checkpoints;
    sum.transfers += times.transfers;
}

/// Where the process stood when the measured window began, or ended.
#[derive(Debug, Clone, Copy)]
struct Mark {
    at: Instant,
    process_cpu: Option<Duration>,
    thread_cpu: Option<Duration>,
    allocations: u64,
    thread_allocations: u64,
    iterations: u64,
    presented: u64,
    counters: TransferCounters,
    discarded_ticks: u64,
    session: FrameTimes,
}

/// Where the timeline stands.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Stage {
    /// Waiting for the world to open.
    Opening,
    /// Waiting for the initial window to become resident.
    Loading,
    /// Timeline tick 0 is the runtime's tick `base`.
    Running {
        base: u64,
    },
    Finished,
}

pub struct BenchDriver {
    loaded: LoadedScenario,
    worlds: PathBuf,
    name: String,
    samples_path: PathBuf,
    summary_path: PathBuf,
    timestamps: bool,
    requested_adapter: String,
    process_started: Instant,
    stage: Stage,
    first_frame_seconds: Option<f64>,
    residency_seconds: Option<f64>,
    /// Timeline ticks whose commands have been issued.
    issued_through: Option<u64>,
    /// Commands refused for pressure, retried in order before any later one.
    held: VecDeque<(u64, Command)>,
    /// Commands admitted after their tick had already run (pressure delayed them).
    late_commands: u64,
    spawns_issued: u64,
    driven: Option<BodyHandle>,
    samples: Vec<FrameSample>,
    /// The last presented frame's sample, until the next presented frame gives its duration.
    pending: Option<(Instant, FrameSample)>,
    /// When the current loop iteration started.
    iteration_started: Option<Instant>,
    /// The timeline tick the current sample's span started at.
    span_start_tick: Option<u64>,
    /// Frames presented when the current sample's span started.
    presented: u64,
    iterations: u64,
    presented_iterations: u64,
    totals: Totals,
    device_tick_ms: Vec<f64>,
    body_tick_ms: Vec<f64>,
    /// The session's frame times over every iteration so far, and over the current span.
    session_total: FrameTimes,
    session_span: Duration,
    /// Per presented frame of the measured window: the acquisition's wait and the session's
    /// frame time over the span, in milliseconds.
    acquire_ms: Vec<f64>,
    session_ms: Vec<f64>,
    /// The measured window's start, taken when the presented frame before it ended, and its
    /// end, taken when the timeline completed.
    window_start: Option<Mark>,
    window_end: Option<Mark>,
    /// How old observations were and how much pressure the consumers saw, at the end.
    observation: Value,
    adapter: Value,
    window_physical: Option<[u32; 2]>,
    window_mismatch: Option<String>,
    /// The present mode the surface was configured with.
    surface_mode: Option<String>,
    failures: Vec<String>,
    status: &'static str,
}

impl BenchDriver {
    pub fn new(
        loaded: LoadedScenario,
        worlds: PathBuf,
        samples_path: PathBuf,
        summary_path: PathBuf,
        timestamps: bool,
        requested_adapter: String,
    ) -> Self {
        let name = loaded.scenario.case.clone();
        Self {
            loaded,
            worlds,
            name,
            samples_path,
            summary_path,
            timestamps,
            requested_adapter,
            process_started: Instant::now(),
            stage: Stage::Opening,
            first_frame_seconds: None,
            residency_seconds: None,
            issued_through: None,
            held: VecDeque::new(),
            late_commands: 0,
            spawns_issued: 0,
            driven: None,
            samples: Vec::new(),
            pending: None,
            iteration_started: None,
            span_start_tick: None,
            presented: 0,
            iterations: 0,
            presented_iterations: 0,
            totals: Totals::default(),
            device_tick_ms: Vec::new(),
            body_tick_ms: Vec::new(),
            session_total: FrameTimes::default(),
            session_span: Duration::ZERO,
            acquire_ms: Vec::new(),
            session_ms: Vec::new(),
            window_start: None,
            window_end: None,
            observation: Value::Null,
            adapter: Value::Null,
            window_physical: None,
            window_mismatch: None,
            surface_mode: None,
            failures: Vec::new(),
            status: "incomplete",
        }
    }

    /// Whether the scenario presents without waiting for the display.
    pub fn vsync(scenario: &Scenario) -> bool {
        scenario.presentation.present_mode == PresentMode::Vsync
    }

    fn scenario(&self) -> &Scenario {
        &self.loaded.scenario
    }

    fn final_tick(&self) -> u64 {
        self.scenario().run.warmup_ticks + self.scenario().run.measured_ticks
    }

    fn finish(&mut self, status: &'static str) {
        self.stage = Stage::Finished;
        self.status = status;
        self.write_outputs();
    }

    /// The runtime's next tick.
    fn next_tick(running: &Running) -> u64 {
        running
            .session()
            .runtime()
            .last_submitted_tick()
            .map_or(0, |tick| tick.value() + 1)
    }

    /// Chooses the body a drive starting at `from_tick` possesses, once every spawn issued so
    /// far has its outcome.
    fn choose_driven(&mut self, running: &mut Running, from_tick: u64) {
        let play = running.play();
        let outcomes = play.applied(Purpose::Spawn) + play.rejected(Purpose::Spawn);
        if outcomes < self.spawns_issued {
            self.failures.push(format!(
                "the drive starting at tick {from_tick} was due before every spawn's outcome had arrived"
            ));
            return;
        }
        let eye = observer_at(&self.scenario().camera, from_tick).position;
        let nearest = running
            .session()
            .runtime()
            .bodies()
            .bodies()
            .filter_map(|tracked| {
                tracked
                    .spawned
                    .map(|(_, position)| (tracked.body, position))
            })
            .min_by(|a, b| {
                a.1.distance_squared(eye)
                    .total_cmp(&b.1.distance_squared(eye))
            });
        match nearest {
            Some((body, _)) => {
                self.driven = Some(body);
                // The driven body is sampled every tick, as a possessed one is.
                running
                    .session_mut()
                    .runtime_mut()
                    .set_controlled(Some(body));
            }
            None => self.failures.push(format!(
                "no body existed when the drive started at tick {from_tick}"
            )),
        }
    }

    /// Issues the commands of every timeline tick through `through` not yet issued, for their
    /// exact ticks; a command refused for pressure waits, with every later one, for the next
    /// frame.
    fn issue(&mut self, running: &mut Running, base: u64, through: u64) {
        while let Some(&(tick, command)) = self.held.front() {
            if !self.admit(running, base, tick, command) {
                return;
            }
            self.held.pop_front();
        }
        let from = self.issued_through.map_or(0, |tick| tick + 1);
        let scenario = self.loaded.scenario.clone();
        for tick in from..=through {
            // Spawns first, then the actions in file order: the order the commands take effect.
            let mut commands = Vec::new();
            for group in scenario.bodies.iter().filter(|group| group.at_tick == tick) {
                match group.spawns() {
                    Ok(spawns) => commands.extend(spawns.into_iter().map(Command::SpawnBody)),
                    Err(error) => self.failures.push(format!("spawns at {tick}: {error:?}")),
                }
            }
            for action in &scenario.actions {
                if action.drives_at(tick) && self.driven.is_none() {
                    self.choose_driven(running, tick);
                }
                match action.commands_at(tick, self.driven) {
                    Ok(issued) => commands.extend(issued),
                    Err(error) => self.failures.push(format!("action at {tick}: {error:?}")),
                }
            }
            for command in commands {
                if matches!(command, Command::SpawnBody(_)) {
                    self.spawns_issued += 1;
                }
                if !self.held.is_empty() || !self.admit(running, base, tick, command) {
                    self.held.push_back((tick, command));
                }
            }
            self.issued_through = Some(tick);
        }
    }

    /// Admits one command for timeline tick `tick`; false when pressure refused it.
    fn admit(&mut self, running: &mut Running, base: u64, tick: u64, command: Command) -> bool {
        let late = Self::next_tick(running) > base + tick;
        let (session, play) = running.session_and_play();
        match play.submit_scripted(session, command, Tick::new(base + tick)) {
            Ok(_) => {
                if late {
                    self.late_commands += 1;
                }
                true
            }
            Err(AdmissionError::Pressure(_)) => false,
            Err(error) => {
                self.failures
                    .push(format!("tick {tick}: {command:?} refused: {error:?}"));
                true
            }
        }
    }

    fn record_adapter(&mut self, running: &mut Running) {
        let record = running.gpu().report().adapter;
        self.adapter = json!({
            "id": record.id,
            "name": record.name,
            "vendor": format!("0x{:04x}", record.vendor),
            "device": format!("0x{:04x}", record.device),
            "device_type": record.device_type,
            "backend": record.backend,
            "driver": record.driver,
            "driver_info": record.driver_info,
        });
    }

    fn mark(&self, running: &Running) -> Mark {
        let runtime = running.session().runtime();
        Mark {
            at: Instant::now(),
            process_cpu: process_cpu(),
            thread_cpu: thread_cpu(),
            allocations: alloc::snapshot().0,
            thread_allocations: alloc::thread_allocations(),
            iterations: self.iterations,
            presented: self.presented_iterations,
            counters: runtime.counters(),
            discarded_ticks: runtime.discarded_ticks(),
            session: self.session_total,
        }
    }

    /// Accounts for the loop iteration that just ran; a presenting iteration ends a sample.
    fn sample(&mut self, running: &mut Running, base: u64) {
        let Some(started) = self.iteration_started else {
            return;
        };
        self.iterations += 1;
        let times = running.session().frame_times();
        add_times(&mut self.session_total, times);
        self.session_span += times.total();
        let surface = running.surface();
        self.surface_mode = Some(format!("{:?}", surface.present_mode));
        if let Some(size) = surface.size {
            let requested = self.scenario().presentation.resolution;
            if size != requested && self.window_mismatch.is_none() {
                self.window_mismatch = Some(format!(
                    "window is {}x{} physical pixels at frame {}, requested {}x{}",
                    size[0],
                    size[1],
                    self.samples.len(),
                    requested[0],
                    requested[1]
                ));
            }
            self.window_physical = Some(size);
        }
        if surface.presented_frames == self.presented {
            return;
        }
        self.presented = surface.presented_frames;
        self.presented_iterations += 1;
        let main_cpu_us = started.elapsed().as_secs_f64() * 1e6;
        let session_span = std::mem::take(&mut self.session_span);
        let now = Totals::read(running);
        let before = self.totals;
        self.totals = now;
        let tick = Self::next_tick(running).saturating_sub(base);
        let span_start = self.span_start_tick.take().unwrap_or(tick);
        let frame = self.samples.len() as u64 + u64::from(self.pending.is_some());
        let session = running.session();
        let residency = session.residency().summary();
        let device_ticks = now.device_ticks - before.device_ticks;
        let device_ms = now.device_ms - before.device_ms;
        let body_ms = now.body_ms - before.body_ms;
        let measured = span_start >= self.scenario().run.warmup_ticks;
        if measured && device_ticks > 0 {
            self.device_tick_ms.push(device_ms / device_ticks as f64);
            self.body_tick_ms.push(body_ms / device_ticks as f64);
        }
        if measured {
            self.acquire_ms
                .push(surface.last_acquire.as_secs_f64() * 1000.0);
            self.session_ms.push(session_span.as_secs_f64() * 1000.0);
        }
        let sample = FrameSample {
            frame,
            measured,
            frame_ms: 0.0,
            main_cpu_us,
            ticks: now.ticks_submitted - before.ticks_submitted,
            tick,
            simulation_us: device_ms * 1000.0,
            body_us: body_ms * 1000.0,
            bodies: u64::from(session.runtime().bodies().summary().tracked),
            grid_upload_bytes: now.upload_bytes - before.upload_bytes,
            active_chunks: u64::from(residency.resident),
            pending_loads: u64::from(residency.loading),
            loads_generated: now.loads_generated - before.loads_generated,
            loads_from_store: now.loads_stored - before.loads_stored,
            chunks_written: now.writes_committed - before.writes_committed,
            write_failures: now.write_failures - before.write_failures,
            retained_bytes: residency.retained_bytes,
            rss_bytes: frame
                .is_multiple_of(RSS_EVERY_FRAMES)
                .then(read_rss)
                .flatten(),
            ..FrameSample::default()
        };
        // The previous presented frame lasted until this one started.
        if let Some((previous_start, mut previous)) = self.pending.take() {
            previous.frame_ms = started.duration_since(previous_start).as_secs_f64() * 1000.0;
            self.samples.push(previous);
        }
        self.pending = Some((started, sample));
        // The next span starts inside the measured window: it begins now.
        if self.window_start.is_none() && tick >= self.scenario().run.warmup_ticks {
            self.window_start = Some(self.mark(running));
        }
    }

    /// The measured window's whole-process account, from its start to `end`.
    fn window(&self, end: &Mark) -> Value {
        let Some(start) = self.window_start else {
            return Value::Null;
        };
        let seconds = end.at.duration_since(start.at).as_secs_f64();
        let frames = (end.presented - start.presented).max(1) as f64;
        let per_frame_ms = |later: Option<Duration>, earlier: Option<Duration>| {
            later
                .zip(earlier)
                .map(|(later, earlier)| (later - earlier).as_secs_f64() * 1000.0 / frames)
        };
        let ms = |duration: Duration| duration.as_secs_f64() * 1000.0 / frames;
        let (before, after) = (start.counters, end.counters);
        let per_second = |later: u64, earlier: u64| (later - earlier) as f64 / seconds.max(1e-9);
        json!({
            "seconds": seconds,
            "presented_frames": end.presented - start.presented,
            "loop_iterations": end.iterations - start.iterations,
            "process_cpu_ms_per_frame": per_frame_ms(end.process_cpu, start.process_cpu),
            "process_cpu_cores": end.process_cpu.zip(start.process_cpu)
                .map(|(later, earlier)| (later - earlier).as_secs_f64() / seconds.max(1e-9)),
            "main_thread_cpu_ms_per_frame": per_frame_ms(end.thread_cpu, start.thread_cpu),
            "allocations_counted": alloc::installed(),
            "allocations_per_frame": (end.allocations - start.allocations) as f64 / frames,
            "main_thread_allocations_per_frame":
                (end.thread_allocations - start.thread_allocations) as f64 / frames,
            "session_ms_per_frame": {
                "runtime": ms(end.session.runtime - start.session.runtime),
                "residency": ms(end.session.residency - start.session.residency),
                "checkpoints": ms(end.session.checkpoints - start.session.checkpoints),
                "transfers": ms(end.session.transfers - start.session.transfers),
            },
            "ticks_submitted": after.ticks_submitted - before.ticks_submitted,
            "ticks_discarded": end.discarded_ticks - start.discarded_ticks,
            "upload_bytes_per_second": {
                "commands": per_second(after.upload.commands, before.upload.commands),
                "tick_parameters": per_second(after.upload.tick_parameters, before.upload.tick_parameters),
                "chunks": per_second(after.upload.chunks, before.upload.chunks),
                "metadata": per_second(after.upload.metadata, before.upload.metadata),
            },
            "readback_bytes_per_second": {
                "results": per_second(after.readback.results, before.readback.results),
                "events": per_second(after.readback.events, before.readback.events),
                "samples": per_second(after.readback.samples, before.readback.samples),
                "diagnostics": per_second(after.readback.diagnostics, before.readback.diagnostics),
                "timestamps": per_second(after.readback.timestamps, before.readback.timestamps),
                "captures": per_second(after.readback.captures, before.readback.captures),
                "snapshots": per_second(after.readback.snapshots, before.readback.snapshots),
            },
            "device_copy_bytes": after.device_copy_bytes - before.device_copy_bytes,
            "snapshot_copy_bytes": after.snapshot_copy_bytes - before.snapshot_copy_bytes,
            "transfer_submissions": after.transfer_submissions - before.transfer_submissions,
            "frame_waits": after.frame_waits - before.frame_waits,
            "maintenance_waits": after.maintenance_waits - before.maintenance_waits,
            "pressure_refusals": after.pressure_refusals.iter().zip(before.pressure_refusals.iter())
                .map(|(later, earlier)| later - earlier).collect::<Vec<u64>>(),
            "stale_deliveries": after.stale_deliveries - before.stale_deliveries,
            "buffers_created": after.buffers_created - before.buffers_created,
            "pipelines_created": after.pipelines_created - before.pipelines_created,
            "in_flight_high_water": after.in_flight_high_water,
            "requested_bytes": after.requested_bytes,
            "peak_requested_bytes": after.peak_requested_bytes,
        })
    }

    fn write_outputs(&self) {
        let mut csv = String::from(CSV_HEADER);
        csv.push('\n');
        for sample in &self.samples {
            csv.push_str(&sample.to_csv());
            csv.push('\n');
        }
        if let Err(error) = std::fs::write(&self.samples_path, csv) {
            eprintln!("native-client: write samples: {error}");
        }
        let distribution = |values: &[f64]| {
            serde_json::to_value(measurement::distribution(values.iter().copied()))
                .unwrap_or_default()
        };
        let mut device_passes = BTreeMap::new();
        if !self.device_tick_ms.is_empty() {
            device_passes.insert("tick", distribution(&self.device_tick_ms));
            device_passes.insert("tick/bodies", distribution(&self.body_tick_ms));
        }
        let scenario = self.scenario();
        let summary = json!({
            "schema": 1,
            "application": "native client (morbid-app) under native-client bench",
            "status": self.status,
            "scenario": self.loaded.path,
            "scenario_sha256": self.loaded.sha256,
            "gpu_timestamps": self.timestamps,
            "adapter": self.adapter,
            "requested_adapter": self.requested_adapter,
            "window_requested": scenario.presentation.resolution,
            "window_physical": self.window_physical,
            "window_mismatch": self.window_mismatch,
            "present_mode": scenario.presentation.present_mode,
            "surface_present_mode": self.surface_mode,
            "first_frame_seconds": self.first_frame_seconds,
            "residency_seconds": self.residency_seconds,
            "samples": self.samples.len(),
            "warmup_ticks": scenario.run.warmup_ticks,
            "measured_ticks": scenario.run.measured_ticks,
            "final_tick": self.samples.last().map(|sample| sample.tick),
            "late_commands": self.late_commands,
            "loop_iterations": self.iterations,
            "presenting_iterations": self.presented_iterations,
            "driven_body": self.driven.map(|body| json!({
                "slot": body.index(),
                "generation": body.generation(),
            })),
            "gpu_pass_elapsed_ms": device_passes,
            "window": self.window_end.map_or(Value::Null, |end| self.window(&end)),
            "acquire_ms": distribution(&self.acquire_ms),
            "acquire_over_5ms": self.acquire_ms.iter().filter(|ms| **ms > 5.0).count(),
            "session_frame_ms": distribution(&self.session_ms),
            "observation": self.observation,
            "failures": self.failures,
        });
        let text = serde_json::to_string_pretty(&summary).unwrap_or_default();
        if let Err(error) = std::fs::write(&self.summary_path, text + "\n") {
            eprintln!("native-client: write summary: {error}");
        }
    }
}

/// Observation ages and the consumers' pressure over the run.
fn observation(running: &Running) -> Value {
    let runtime = running.session().runtime();
    let ledger = runtime.ledger().summary();
    let bodies = runtime.bodies().summary();
    json!({
        "input_latency_ticks": { "mean": ledger.input_latency_ticks.mean(), "max": ledger.input_latency_ticks.max },
        "observation_age_ticks": { "mean": ledger.observation_age_ticks.mean(), "max": ledger.observation_age_ticks.max },
        "body_sample_age_ticks": { "mean": bodies.sample_age_ticks.mean(), "max": bodies.sample_age_ticks.max },
        "body_events_deferred": bodies.events_deferred,
        "body_event_ring_high_water": bodies.ring_high_water,
        "cosmetic_dropped_on_device": ledger.cosmetic_dropped_on_device,
        "cosmetic_overwritten": ledger.cosmetic_overwritten,
        "inconsistent_ticks": ledger.inconsistent_ticks,
        "commands_completed": ledger.commands_completed,
        "commands_abandoned": ledger.commands_abandoned,
    })
}

fn read_rss() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    kib.checked_mul(1024)
}

impl Driver for BenchDriver {
    fn timestamps(&self) -> bool {
        self.timestamps
    }

    fn before_frame(&mut self, frame: DriverFrame<'_>) -> Control {
        self.iteration_started = Some(Instant::now());
        frame.hud.visible = self.scenario().presentation.overlay;
        if self.first_frame_seconds.is_none() {
            self.first_frame_seconds = Some(self.process_started.elapsed().as_secs_f64());
        }
        if self.stage == Stage::Finished {
            return Control::Quit;
        }
        if self
            .samples
            .last()
            .is_some_and(|sample| sample.tick >= self.final_tick())
        {
            if let Some(running) = frame.running.as_deref() {
                self.window_end = Some(self.mark(running));
                self.observation = observation(running);
            }
            let status = if self.failures.is_empty() {
                "completed"
            } else {
                "failed"
            };
            self.finish(status);
            return Control::Quit;
        }
        let Some(running) = frame.running else {
            match (self.stage, frame.phase) {
                (Stage::Opening, PhaseKind::Start) => {
                    let path = catalog::world_path(&self.worlds, &self.name);
                    return Control::Open {
                        name: self.name.clone(),
                        path,
                    };
                }
                (Stage::Opening, _) => {}
                _ => {
                    self.failures.push(format!(
                        "the world left the running phase ({:?})",
                        frame.phase
                    ));
                    self.finish("failed");
                    return Control::Quit;
                }
            }
            return Control::Continue;
        };
        match self.stage {
            Stage::Opening => {
                self.stage = Stage::Loading;
                self.record_adapter(running);
                let world = &self.loaded.scenario.world;
                if let Err(error) = running
                    .session_mut()
                    .runtime_mut()
                    .set_clock(world.tick_rate_hz, world.max_ticks_per_frame)
                {
                    self.failures.push(format!("clock: {error:?}"));
                    self.finish("failed");
                    return Control::Quit;
                }
                running.play_mut().observer = observer_at(&self.loaded.scenario.camera, 0);
                return Control::Continue;
            }
            Stage::Loading => {
                running.play_mut().observer = observer_at(&self.loaded.scenario.camera, 0);
                let residency = running.session().residency();
                let summary = residency.summary();
                let complete = residency.settled()
                    && summary.failed == 0
                    && summary.resident == residency.window().chunk_count();
                if !complete {
                    if self.process_started.elapsed().as_secs_f64()
                        > self.scenario().run.residency_timeout_seconds
                    {
                        self.finish("residency-timeout");
                        return Control::Quit;
                    }
                    return Control::Continue;
                }
                self.residency_seconds = Some(self.process_started.elapsed().as_secs_f64());
                self.stage = Stage::Running {
                    base: Self::next_tick(running),
                };
                self.totals = Totals::read(running);
                self.presented = running.surface().presented_frames;
            }
            Stage::Running { .. } | Stage::Finished => {}
        }
        let Stage::Running { base } = self.stage else {
            return Control::Continue;
        };
        let tick = Self::next_tick(running).saturating_sub(base);
        self.span_start_tick.get_or_insert(tick);
        running.play_mut().observer = observer_at(&self.loaded.scenario.camera, tick);
        // Every tick this frame may run: its commands are admitted for their exact ticks.
        let lookahead = u64::from(self.scenario().world.max_ticks_per_frame);
        self.issue(running, base, tick + lookahead - 1);
        Control::Continue
    }

    fn after_frame(&mut self, running: Option<&mut Running>, _input: &InputState) -> bool {
        if let (Some(running), Stage::Running { base }) = (running, self.stage) {
            self.sample(running, base);
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera(path: Vec<[i32; 2]>, cells_per_tick: f64) -> Camera {
        Camera {
            start_cell: path.first().copied().unwrap_or([0, 0]),
            height: 150.0,
            yaw_degrees: 30.0,
            pitch_degrees: -40.0,
            path,
            cells_per_tick,
        }
    }

    #[test]
    fn the_camera_follows_its_closed_path_by_tick() {
        let square = camera(vec![[0, 0], [10, 0], [10, 10], [0, 10]], 1.0);
        assert_eq!(path_cell(&square, 0), [0.0, 0.0]);
        assert_eq!(path_cell(&square, 5), [5.0, 0.0]);
        assert_eq!(path_cell(&square, 15), [10.0, 5.0]);
        // The path closes: 40 cells later the camera is back where it started.
        assert_eq!(path_cell(&square, 40), [0.0, 0.0]);
        assert_eq!(path_cell(&square, 45), [5.0, 0.0]);
        let still = camera(vec![[3, 4]], 1.0);
        assert_eq!(path_cell(&still, 100), [3.0, 4.0]);
    }

    #[test]
    fn the_observer_stands_over_the_path_in_render_space() {
        let square = camera(vec![[100, 200], [300, 200]], 0.5);
        let observer = observer_at(&square, 20);
        assert_eq!(observer.position, DVec3::new(110.0, 150.0, -200.0));
        assert!(observer.noclip && observer.attached);
        assert_eq!(
            observer.chunk(),
            world_model::spatial::ChunkKey::new(1, 3, 0)
        );
    }
}
