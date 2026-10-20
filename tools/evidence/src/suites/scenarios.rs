//! Connected scenarios: one story through the production runtime and live world, step by step.
//!
//! `resident-world` (verification evidence, "Connected scenario") was delivered stage by stage
//! (04–07) and is complete since 07. Each step reports on its own; a step reported as not
//! implemented fails the scenario. Step 6, the adversarial repeat, is in `adversarial`.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use world_model::{
    body::{BodyHandle, BodyPreset},
    cell::WorldCell,
    command::{BodySpawn, Command, Direction, Drive, EditTool, MaterialEdit, Ray, Target},
    glam::{DVec3, IVec2, Vec3},
    material::{FluidMat, GranularMat, TerrainMat},
    rules::SimulationRules,
    spatial::{CHUNK_CELL_COUNT, CHUNK_SIZE, ChunkKey},
};
use world_runtime::{
    bodies::BodyEvent,
    checkpoint::SaveState,
    ids::{ChunkRevision, CommandId, SessionEpoch, Tick},
    port::{CommandResult, ResidentWindow},
    receipts::ReceiptState,
    runtime::Runtime,
    session::{CloseReport, SessionConfig, WorldSession},
    store::{CheckpointManifest, ChunkPayload, ChunkRow, Publication, WorldStore},
    workers::WorkerConfig,
};
use world_store::{
    faults::{Action, Barrier, StoreFaults},
    store::RedbWorldStore,
};

use super::{
    Check,
    bodies::scene,
    streaming::world::{self, Live, Scratch, open_store, session_config},
};

mod adversarial;

/// One step of a scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub name: String,
    /// The stage that delivers the step.
    pub owner: String,
    pub summary: String,
    pub implemented: bool,
    pub passed: bool,
    pub failures: Vec<String>,
    pub notes: Vec<String>,
    pub metrics: Map<String, Value>,
}

/// What a scenario process reports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioResult {
    pub scenario: String,
    pub adapter: String,
    pub steps: Vec<StepResult>,
}

impl ScenarioResult {
    pub fn passed(&self) -> bool {
        self.steps.iter().all(|step| step.passed)
    }
}

/// The steps of `resident-world`, in order, with the stage that delivers each: the six steps of
/// the acceptance matrix's connected execution trace.
pub const RESIDENT_WORLD_STEPS: [(&str, &str, &str); 6] = [
    (
        "known-terrain",
        "04",
        "known chunks with a sand bank on rock and a water basin become resident; columns at chunk corners, negative cells and the window edge read back as uploaded",
    ),
    (
        "sphere-into-water",
        "04",
        "a controlled sphere driven across the sand bank into the water ruts the sand and displaces water; every tick balances and the totals change by exactly the declared sinks",
    ),
    (
        "ray-edit-and-consumer",
        "04",
        "a brush stroke at a GPU-resolved ray is acknowledged and observed; the CPU consumer reacts to the sphere's observed immersion and its reaction applies",
    ),
    (
        "move-and-return",
        "05",
        "the window moves until the modified regions leave it, they are captured before reuse, and focus returns before a delayed save finishes: the newest state wins",
    ),
    (
        "checkpoint-and-reopen",
        "05",
        "a complete checkpoint at one completed tick, with bodies and configuration; the application, runtime and store are destroyed, reopened and compared",
    ),
    (
        "adversarial-recovery",
        "05, 07",
        "the trace again under load: dense bodies at a chunk and tile seam with brush edits and phase changes while the event consumer, readbacks and spawn outcomes are delayed, stale handles and a full body table; a rebase across negative coordinates with a held write and reversed deliveries; held deliveries under an overlapping resize, reordered loads, failed writes and a failed and retried save; device loss, process interruption before and after publication, orderly close and a fresh reopen, each recovering the last complete checkpoint with its bodies and configuration",
    ),
];

pub fn run(scenario: &str, adapter: &str) -> Option<ScenarioResult> {
    run_viewed(scenario, adapter, None, None)
}

/// [`run`], presenting every session of the scenario in the native client's window when a
/// viewer is given, and writing one frame per step into `captures`.
pub fn run_viewed(
    scenario: &str,
    adapter: &str,
    viewer: Option<world::SharedViewer>,
    captures: Option<std::path::PathBuf>,
) -> Option<ScenarioResult> {
    match scenario {
        "resident-world" => Some(match std::env::var(adversarial::INTERRUPT) {
            Ok(spec) => adversarial::interruption_child(adapter, &spec),
            Err(_) => resident_world_viewed(adapter, viewer, captures),
        }),
        _ => None,
    }
}

/// Captions the viewer for a step and, after it, writes the viewer's frame.
fn show_step(
    story: Option<&mut Story>,
    name: &str,
    captures: Option<&std::path::Path>,
    check: &mut Check,
) {
    let Some(story) = story else {
        return;
    };
    let Some(directory) = captures else {
        return;
    };
    let path = directory.join(format!("resident-world-{name}.png"));
    match story.live.capture(&path) {
        Ok(()) => check.note(format!("frame written to {}", path.display())),
        Err(error) => check.note(format!("no frame: {error}")),
    }
}

fn step(name: &str, check: Check, implemented: bool) -> StepResult {
    let (owner, summary) = RESIDENT_WORLD_STEPS
        .iter()
        .find(|(step, ..)| *step == name)
        .map(|(_, owner, summary)| ((*owner).to_owned(), (*summary).to_owned()))
        .unwrap_or_default();
    let (failures, notes, metrics) = check.into_parts();
    StepResult {
        name: name.to_owned(),
        owner,
        summary,
        implemented,
        passed: implemented && failures.is_empty(),
        failures,
        notes,
        metrics,
    }
}

/// The scenario's terrain, by absolute cell: a sand bank west of x = 40 (rock at 44 under six
/// units of sand), a slope down to x = 64, and east of it a rock basin at 16 holding 24 units of
/// water. Rows differ only by a one-unit ripple every 16 cells of y, so cells keep their identity
/// in the checks.
pub fn known_terrain(cell: IVec2) -> WorldCell {
    let mut column = scene::dirt();
    column.set_terrain_mat(TerrainMat::TERRAIN_STONE);
    let ripple = u16::from(cell.y.rem_euclid(16) == 0);
    if cell.x < 40 {
        column.set_elevation(44 + ripple);
        column.set_granular_mat(GranularMat::GRANULAR_SAND);
        column.set_granular_vol(6);
    } else if cell.x < 64 {
        column.set_elevation((44 - (cell.x - 40)) as u16 + ripple);
        column.set_granular_mat(GranularMat::GRANULAR_SAND);
        column.set_granular_vol(4);
    } else {
        column.set_elevation(16 + ripple);
        column.set_fluid_mat(FluidMat::FLUID_WATER);
        column.set_fluid_vol(24);
    }
    column
}

fn chunk(key: ChunkKey) -> Vec<WorldCell> {
    let origin = key.origin_cell();
    (0..CHUNK_CELL_COUNT)
        .map(|index| {
            known_terrain(
                origin + IVec2::new((index % CHUNK_SIZE) as i32, (index / CHUNK_SIZE) as i32),
            )
        })
        .collect()
}

struct Story {
    live: Live,
    store: Arc<RedbWorldStore>,
    faults: StoreFaults,
    path: std::path::PathBuf,
    id: u64,
    /// The centre of step 3's stroke, once it is known.
    stroke: Option<IVec2>,
    // Last: the directory outlives every store and session above.
    _scratch: Scratch,
}

impl Story {
    fn runtime(&mut self) -> &mut Runtime {
        self.live.session.runtime_mut()
    }

    fn session(&self) -> &WorldSession {
        &self.live.session
    }

    fn submit(&mut self, command: Command, tick: u64) -> Result<CommandId, String> {
        self.id += 1;
        let id = CommandId::new(self.id);
        self.runtime()
            .submit_at(id, command, Tick::new(tick))
            .map_err(|error| format!("submit {command:?}: {error:?}"))?;
        Ok(id)
    }

    fn next(&self) -> u64 {
        self.session()
            .runtime()
            .last_submitted_tick()
            .map_or(0, |tick| tick.value() + 1)
    }

    /// Runs frames of one tick until one more tick is submitted (a frame submits nothing while
    /// three ticks are in flight).
    fn frame(&mut self) {
        let before = self.next();
        let started = std::time::Instant::now();
        while self.next() == before && started.elapsed() < super::driver::DEVICE_PATIENCE {
            self.live.frame_ticks(1);
            if self.next() == before {
                std::thread::sleep(Duration::from_micros(50));
            }
        }
    }

    fn settle(&mut self) -> Result<(), String> {
        let started = std::time::Instant::now();
        loop {
            let counters = self.session().runtime().counters();
            if counters.ticks_observed == counters.ticks_submitted {
                return Ok(());
            }
            if started.elapsed() > super::driver::DEVICE_PATIENCE {
                return Err("submitted ticks were never observed".to_owned());
            }
            self.live.frame_ticks(0);
            std::thread::sleep(Duration::from_micros(100));
        }
    }

    fn result(&self, id: CommandId) -> Option<(Tick, CommandResult)> {
        match self.session().runtime().receipt(id)?.state {
            ReceiptState::Completed { tick, result } => Some((tick, result)),
            _ => None,
        }
    }

    /// Runs frames until `id` has completed.
    fn await_result(&mut self, id: CommandId) -> Result<(Tick, CommandResult), String> {
        for _ in 0..600 {
            if let Some(done) = self.result(id) {
                return Ok(done);
            }
            self.frame();
        }
        self.settle()?;
        self.result(id)
            .ok_or_else(|| format!("{id:?} never completed"))
    }

    fn column(&mut self, cell: IVec2) -> Result<Option<WorldCell>, String> {
        let at = self.next();
        let id = self.submit(Command::QueryColumn { cell }, at)?;
        match self.await_result(id)?.1 {
            CommandResult::Column { column, .. } => Ok(Some(column)),
            CommandResult::ColumnNotResident { .. } => Ok(None),
            other => Err(format!("query {cell}: {other:?}")),
        }
    }

    /// The columns at `cells`, in order.
    fn columns(&mut self, cells: &[IVec2]) -> Result<Vec<Option<WorldCell>>, String> {
        cells.iter().map(|cell| self.column(*cell)).collect()
    }

    fn sample(&self, body: BodyHandle) -> Option<world_model::observation::BodySample> {
        self.session()
            .runtime()
            .bodies()
            .body(body)
            .and_then(|tracked| tracked.last)
    }

    /// Runs frames of one tick until `done` holds.
    fn until(
        &mut self,
        what: &str,
        done: impl FnMut(&WorldSession) -> bool,
    ) -> Result<u32, String> {
        self.live.until(what, done)
    }

    /// Saves the world before its first tick and waits for the save to commit: in a reopened
    /// session, a checkpoint of exactly what the session restored.
    fn save_first_boundary(&mut self) -> Result<CheckpointManifest, String> {
        self.live.session.save();
        let started = std::time::Instant::now();
        while !self
            .session()
            .checkpoints()
            .receipt()
            .is_some_and(|receipt| receipt.finished())
        {
            if started.elapsed() > super::driver::DEVICE_PATIENCE {
                return Err("the first-boundary save never finished".to_owned());
            }
            self.live.frame_ticks(0);
            std::thread::sleep(Duration::from_micros(200));
        }
        let receipt = self.session().checkpoints().receipt().cloned();
        let committed = receipt
            .as_ref()
            .and_then(|receipt| receipt.committed())
            .ok_or_else(|| format!("the first-boundary save did not commit: {receipt:?}"))?;
        let latest = self.store.latest().map_err(|error| error.to_string())?;
        if latest.id != committed {
            return Err(format!(
                "the first-boundary save {committed:?} is not the latest: {latest:?}"
            ));
        }
        Ok(latest)
    }

    /// Closes this session in order, lets the store go, and opens the file again with a new
    /// store object and a new session under `epoch`: nothing but the file connects them.
    fn reopen(self, adapter: &str, epoch: SessionEpoch) -> Result<(Story, CloseReport), String> {
        let Story {
            live,
            store,
            faults,
            path,
            _scratch,
            ..
        } = self;
        let viewer = live.viewer.clone();
        live.detach();
        let report = live.session.close(Duration::from_secs(60));
        drop(store);
        let store = open_store(&path, faults.clone())?;
        let live = world::open_viewed(
            adapter,
            Arc::clone(&store),
            epoch,
            trace_config(),
            |_| {},
            viewer,
        )?;
        Ok((
            Story {
                live,
                store,
                faults,
                path,
                id: 0,
                stroke: None,
                _scratch,
            },
            report,
        ))
    }
}

pub fn resident_world(adapter: &str) -> ScenarioResult {
    resident_world_viewed(adapter, None, None)
}

fn caption(story: &Option<Story>, name: &str) {
    if let Some(story) = story {
        let summary = RESIDENT_WORLD_STEPS
            .iter()
            .find(|(step, ..)| *step == name)
            .map_or("", |(_, _, summary)| *summary);
        story
            .live
            .set_caption(&format!("resident-world: {name}\n{summary}"));
    }
}

pub fn resident_world_viewed(
    adapter: &str,
    viewer: Option<world::SharedViewer>,
    captures: Option<std::path::PathBuf>,
) -> ScenarioResult {
    let captures = captures.as_deref();
    let mut steps = Vec::new();
    let mut story = None;
    let mut check = Check::default();
    if let Some(viewer) = &viewer {
        viewer
            .borrow_mut()
            .set_caption("resident-world: known-terrain");
    }
    match known_terrain_step(adapter, &mut check, viewer.clone()) {
        Ok(opened) => story = Some(opened),
        Err(error) => check.fail(format!("stopped early: {error}")),
    }
    show_step(story.as_mut(), "known-terrain", captures, &mut check);
    steps.push(step("known-terrain", check, true));

    let mut sphere = None;
    let mut check = Check::default();
    caption(&story, "sphere-into-water");
    match story.as_mut() {
        Some(story) => match sphere_step(story, &mut check) {
            Ok(body) => sphere = Some(body),
            Err(error) => check.fail(format!("stopped early: {error}")),
        },
        None => check.fail("the terrain step did not open the world"),
    }
    show_step(story.as_mut(), "sphere-into-water", captures, &mut check);
    steps.push(step("sphere-into-water", check, true));

    let mut check = Check::default();
    let mut edited = false;
    caption(&story, "ray-edit-and-consumer");
    match (story.as_mut(), sphere) {
        (Some(story), Some(body)) => match edit_and_consumer_step(story, body, &mut check) {
            Ok(()) => edited = true,
            Err(error) => check.fail(format!("stopped early: {error}")),
        },
        _ => check.fail("the sphere step did not complete"),
    }
    show_step(
        story.as_mut(),
        "ray-edit-and-consumer",
        captures,
        &mut check,
    );
    steps.push(step("ray-edit-and-consumer", check, true));

    let mut check = Check::default();
    let mut probes = None;
    caption(&story, "move-and-return");
    match (story.as_mut(), sphere, edited) {
        (Some(story), Some(body), true) => match move_and_return_step(story, body, &mut check) {
            Ok(found) => probes = Some(found),
            Err(error) => check.fail(format!("stopped early: {error}")),
        },
        _ => check.fail("an earlier step did not complete"),
    }
    show_step(story.as_mut(), "move-and-return", captures, &mut check);
    steps.push(step("move-and-return", check, true));

    let mut check = Check::default();
    caption(&story, "checkpoint-and-reopen");
    match (story.take(), sphere, probes.as_ref()) {
        (Some(current), Some(body), Some(probes)) => {
            match checkpoint_and_reopen_step(adapter, current, body, probes, &mut check) {
                Ok(reopened) => story = Some(reopened),
                Err(error) => check.fail(format!("stopped early: {error}")),
            }
        }
        _ => check.fail("an earlier step did not complete"),
    }
    show_step(
        story.as_mut(),
        "checkpoint-and-reopen",
        captures,
        &mut check,
    );
    steps.push(step("checkpoint-and-reopen", check, true));

    let mut check = Check::default();
    caption(&story, "adversarial-recovery");
    match (story.take(), sphere, probes) {
        (Some(current), Some(body), Some(probes)) => {
            if let Err(error) = adversarial::adversarial_recovery_step(
                adapter, current, body, probes, &mut check, captures,
            ) {
                check.fail(format!("stopped early: {error}"));
            }
        }
        _ => check.fail("an earlier step did not complete"),
    }
    steps.push(step("adversarial-recovery", check, true));
    ScenarioResult {
        scenario: "resident-world".to_owned(),
        adapter: adapter.to_owned(),
        steps,
    }
}

/// The window the trace starts in and returns to.
fn home() -> ResidentWindow {
    ResidentWindow::new(ChunkKey::new(0, 0, 0), 2, 2).expect("window")
}

/// The scenario's sessions: four loaders, so loads may answer out of order, and no autosave.
fn trace_config() -> SessionConfig {
    SessionConfig {
        workers: WorkerConfig {
            loaders: 4,
            load_queue: 64,
            write_queue: 16,
        },
        ..session_config()
    }
}

/// Columns of the regions the earlier steps changed, and what they held.
struct Probes {
    cells: Vec<IVec2>,
    columns: Vec<Option<WorldCell>>,
}

/// Step 1: the known chunks are the world's saved state; the residency owner loads them from
/// the store and they read back exactly.
fn known_terrain_step(
    adapter: &str,
    check: &mut Check,
    viewer: Option<world::SharedViewer>,
) -> Result<Story, String> {
    let scratch = Scratch::new("resident-world")?;
    let path = scratch.path("world.redb");
    let window = home();
    // The terrain is read back before any rule runs; the rules start with step 2.
    world::create(&path, window, SimulationRules::NONE)?;
    let faults = StoreFaults::new();
    let store = open_store(&path, faults.clone())?;
    let keys: Vec<ChunkKey> = window.view().iter().collect();
    let payloads = keys
        .iter()
        .map(|key| {
            ChunkPayload::new(chunk(*key).into_boxed_slice(), Default::default())
                .map_err(|error| format!("{error:?}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let rows: Vec<ChunkRow<'_>> = keys
        .iter()
        .zip(&payloads)
        .map(|(key, payload)| ChunkRow {
            key: *key,
            revision: ChunkRevision::new(1),
            payload,
        })
        .collect();
    store
        .write_chunks(&rows)
        .map_err(|error| error.to_string())?;
    let first = store.latest().map_err(|error| error.to_string())?;
    let next = first.id.next().ok_or("checkpoints exhausted")?;
    store
        .write_bodies(next, &Default::default())
        .map_err(|error| error.to_string())?;
    let heads: Vec<(ChunkKey, ChunkRevision)> = keys
        .iter()
        .map(|key| (*key, ChunkRevision::new(1)))
        .collect();
    store
        .publish(&Publication {
            id: next,
            tick: None,
            config: first.config,
            window,
            epoch: SessionEpoch::FIRST,
            heads: &heads,
            bodies: 0,
        })
        .map_err(|error| error.to_string())?;
    let live = world::open_viewed(
        adapter,
        Arc::clone(&store),
        SessionEpoch::new(2),
        trace_config(),
        |_| {},
        viewer,
    )?;
    let mut story = Story {
        live,
        store,
        faults,
        path,
        id: 0,
        stroke: None,
        _scratch: scratch,
    };
    story.live.settle()?;
    let summary = *story.session().residency().summary();
    check.metric("chunks", keys.len());
    check.equal(
        "chunks loaded from the store",
        summary.loads_stored,
        keys.len() as u64,
    );
    check.equal("chunks generated", summary.loads_generated, 0);
    // Chunk corners on both sides of the origin, the window's outer corners, interior cells,
    // and one cell past the edge.
    let probes = [
        IVec2::new(-128, -128),
        IVec2::new(-1, -1),
        IVec2::new(0, 0),
        IVec2::new(63, 63),
        IVec2::new(64, 0),
        IVec2::new(-65, 17),
        IVec2::new(39, 32),
        IVec2::new(40, 32),
        IVec2::new(191, 191),
        IVec2::new(100, -100),
    ];
    let at = story.next() + 1;
    let mut ids = Vec::new();
    for cell in probes {
        ids.push((cell, story.submit(Command::QueryColumn { cell }, at)?));
    }
    let outside = story.submit(
        Command::QueryColumn {
            cell: IVec2::new(192, 0),
        },
        at,
    )?;
    for (cell, id) in ids {
        match story.await_result(id)?.1 {
            CommandResult::Column {
                cell: answered,
                column,
            } => {
                check.equal(&format!("column {cell} answered for"), answered, cell);
                check.equal(
                    &format!("column {cell} packed"),
                    column,
                    known_terrain(cell),
                );
            }
            other => check.fail(format!("column {cell}: {other:?}")),
        }
    }
    check.equal(
        "a column past the window edge",
        story.await_result(outside)?.1,
        CommandResult::ColumnNotResident {
            cell: IVec2::new(192, 0),
        },
    );
    Ok(story)
}

/// Step 2: with the rules on, a controlled sphere is driven east across the bank into the
/// basin.
fn sphere_step(story: &mut Story, check: &mut Check) -> Result<BodyHandle, String> {
    let at = story.next() + 1;
    let before = story.submit(Command::MeasureTotals, at)?;
    story.submit(
        Command::SetSimulationRules(SimulationRules::DETERMINISTIC),
        at,
    )?;
    let spawn =
        BodySpawn::new(BodyPreset::Metal, IVec2::new(8, 32), 2.0).map_err(|e| e.to_string())?;
    let spawned = story.submit(Command::SpawnBody(spawn), at)?;
    let body = match story.await_result(spawned)?.1 {
        CommandResult::BodySpawned { body, .. } => body,
        other => return Err(format!("spawn: {other:?}")),
    };
    story.runtime().set_controlled(Some(body));
    let east = Drive::controls(Direction::new(1.0, 0.0).map_err(|e| e.to_string())?);
    let started = story.next();
    let mut entered = None;
    let mut events = Vec::new();
    let mut log: BTreeMap<&'static str, u32> = BTreeMap::new();
    let mut scheduled = started;
    for _ in 0..900 {
        let next = story.next();
        // Keep a few ticks of drive commands queued ahead, as held controls do.
        while scheduled < next + 4 {
            story.submit(Command::DriveBody { body, drive: east }, scheduled)?;
            scheduled += 1;
        }
        story.frame();
        events.clear();
        story.runtime().bodies_mut().drain_events(&mut events);
        for event in &events {
            let kind = match event {
                BodyEvent::Spawned { .. } => "spawned",
                BodyEvent::Landed { .. } => "landed",
                BodyEvent::EnteredLiquid { .. } => "entered_liquid",
                BodyEvent::TouchedBody { .. } => "touched_body",
                BodyEvent::Cratered { .. } => "cratered",
                BodyEvent::Splashed { .. } => "splashed",
                BodyEvent::Removed { .. } => "removed",
            };
            *log.entry(kind).or_default() += 1;
            if let BodyEvent::EnteredLiquid { tick, .. } = event {
                entered.get_or_insert(*tick);
            }
        }
        if let Some(sample) = story.runtime().bodies().body(body).and_then(|t| t.last)
            && entered.is_some()
            && sample.position.x > 72.0
        {
            break;
        }
    }
    story.settle()?;
    let sample = story.sample(body).ok_or("no sample of the sphere")?;
    check.that(entered.is_some(), || {
        format!("the sphere never entered water: {sample:?}")
    });
    check.that(sample.position.x > 64.0, || {
        format!("the sphere did not reach the basin: {sample:?}")
    });
    check.that(
        sample.liquid == Some(FluidMat::FLUID_WATER) && sample.submerged_fraction > 0.0,
        || format!("the last sample is not in water: {sample:?}"),
    );
    for (kind, count) in &log {
        check.metric(&format!("events_{kind}"), *count);
    }
    check.that(log.get("landed").copied().unwrap_or(0) > 0, || {
        "no landing event".to_owned()
    });
    if let Some(tick) = entered {
        check.metric("entered_water_at_tick", tick.value() - started);
    }
    // The expected material units: liquid is conserved (no rule here boils, burns or takes up
    // water), and the solid column (rock and loose sand) loses exactly what the bodies compacted.
    let after = story.submit(Command::MeasureTotals, 0)?;
    let (before, after) = match (story.await_result(before)?.1, story.await_result(after)?.1) {
        (CommandResult::Totals(before), CommandResult::Totals(after)) => (before, after),
        other => return Err(format!("totals: {other:?}")),
    };
    let ledger = *story.runtime().ledger().summary();
    check.equal("inconsistent ticks", ledger.inconsistent_ticks, 0);
    check.equal(
        "water",
        after.fluid.iter().sum::<u64>(),
        before.fluid.iter().sum::<u64>(),
    );
    check.equal(
        "rock and sand, with the bodies' compaction",
        after.elevation + after.granular + ledger.bodies.compaction,
        before.elevation + before.granular,
    );
    check.metric("compacted_units", ledger.bodies.compaction);
    check.metric("ticks_observed", ledger.ticks_observed);
    check.metric(
        "sample_age_ticks_max",
        story.runtime().bodies().summary().sample_age_ticks.max,
    );
    check.that(
        story.runtime().bodies().summary().sample_age_ticks.max <= 2.0,
        || "a controlled-body sample was applied more than two ticks late".to_owned(),
    );
    // The body accounting the device reported: the sphere moved sand (its rut) and water.
    let moved = ledger.bodies;
    check.that(moved.solid_in > 0 && moved.ruts > 0, || {
        format!("the sphere left no rut: {moved:?}")
    });
    check.that(moved.fluid_in > 0, || {
        format!("the sphere displaced no water: {moved:?}")
    });
    check.metric("sand_moved_units", moved.solid_in);
    check.metric("water_displaced_units", moved.fluid_in);
    check.metric("ruts", moved.ruts);
    check.metric("wakes", moved.wakes);
    check.metric("splashes", moved.splashes);
    check.metric("craters", moved.craters);
    Ok(body)
}

/// Step 3: a stroke at a GPU-resolved ray, its acknowledgement and its observation, and a CPU
/// consumer reacting to the sphere's observed state.
fn edit_and_consumer_step(
    story: &mut Story,
    body: BodyHandle,
    check: &mut Check,
) -> Result<(), String> {
    // Aim at the sand bank from above, as a pointer would.
    let ray = Ray::new(
        DVec3::new(20.4, 120.0, -24.6),
        Vec3::new(0.0, -1.0, -0.05),
        400.0,
    )
    .map_err(|e| e.to_string())?;
    let edit =
        MaterialEdit::at(EditTool::Sand, Target::Ray(ray), 2, 8).map_err(|e| e.to_string())?;
    let at = story.next() + 1;
    let query = story.submit(Command::QueryRay { ray }, at)?;
    let stroke = story.submit(Command::EditMaterial(edit), at)?;
    let (query_tick, hit) = story.await_result(query)?;
    let (stroke_tick, edited) = story.await_result(stroke)?;
    let hit_cell = match hit {
        CommandResult::Ray {
            surface: world_model::observation::SurfaceHit::Hit { cell, .. },
            ..
        } => cell,
        other => return Err(format!("ray query: {other:?}")),
    };
    check.equal("query and stroke in one tick", query_tick, stroke_tick);
    let center = match edited {
        CommandResult::Edited {
            center, changed, ..
        } => {
            check.that(changed > 0, || "the stroke changed nothing".to_owned());
            check.metric("stroke_changed_cells", changed);
            center
        }
        other => return Err(format!("stroke: {other:?}")),
    };
    check.equal("stroke centre is the ray's hit", center, hit_cell);
    story.stroke = Some(center);
    // Eventual observation: a later column query sees the added sand.
    let observe = story.submit(Command::QueryColumn { cell: center }, 0)?;
    match story.await_result(observe)? {
        (tick, CommandResult::Column { column, .. }) => {
            check.that(tick > stroke_tick, || {
                "observed before the acknowledgement".to_owned()
            });
            check.that(
                column.granular_vol() > known_terrain(center).granular_vol(),
                || format!("the stroke's sand is not observed: {column:?}"),
            );
            check.metric(
                "observation_ticks_after_ack",
                tick.value() - stroke_tick.value(),
            );
        }
        other => check.fail(format!("observation: {other:?}")),
    }

    // The consumer: while the controlled sphere's observed summary says it is in water, it
    // holds the lift at the sphere's column, one command a tick, as a held tool does; it records
    // how old each observation was and when its first reaction applied.
    let mut first = None;
    let mut rose = false;
    let mut left_water = None;
    let mut lifts = 0u32;
    let mut last_reacted = None;
    for _ in 0..240 {
        story.frame();
        let Some(sample) = story.runtime().bodies().body(body).and_then(|t| t.last) else {
            continue;
        };
        if first.is_some() {
            rose |= sample.velocity.y > 0.0;
            if sample.submerged_fraction == 0.0 && rose {
                left_water = Some(sample.tick);
                break;
            }
        }
        if sample.submerged_fraction > 0.0 && last_reacted != Some(sample.tick) {
            last_reacted = Some(sample.tick);
            let cell = IVec2::new(
                sample.position.x.round() as i32,
                (-sample.position.z).round() as i32,
            );
            let observed_at = story
                .runtime()
                .last_submitted_tick()
                .map_or(0, |t| t.value());
            let id = story.submit(
                Command::LiftBodies {
                    target: Target::Column(cell),
                },
                0,
            )?;
            lifts += 1;
            first.get_or_insert((sample.tick, observed_at, id));
        }
    }
    let (sampled, observed_at, id) = first.ok_or("the consumer never saw the sphere in water")?;
    check.metric("observation_age_at_reaction_ticks", observed_at - sampled);
    check.metric("lift_commands", lifts);
    let (applied, result) = story.await_result(id)?;
    match result {
        CommandResult::Lifted { bodies, .. } => {
            check.that(bodies >= 1, || "the reaction lifted nothing".to_owned());
        }
        other => check.fail(format!("reaction: {other:?}")),
    }
    check.metric("reaction_ticks_from_sample", applied.value() - sampled);
    check.that(applied.value() - sampled <= 4, || {
        format!(
            "the reaction applied {} ticks after its observation",
            applied.value() - sampled
        )
    });
    check.that(rose, || {
        "the lifted sphere was never observed rising".to_owned()
    });
    match left_water {
        Some(tick) => check.metric("left_water_ticks_after_reaction", tick - applied.value()),
        None => check.fail("the lifted sphere was never observed out of the water"),
    }
    story.settle()?;
    let ledger = *story.runtime().ledger().summary();
    check.equal("inconsistent ticks", ledger.inconsistent_ticks, 0);
    check.equal("unknown outcomes", story.runtime().unknown_outcomes(), 0);
    check.metric(
        "input_latency_ticks_mean",
        ledger.input_latency_ticks.mean().unwrap_or(0.0),
    );
    check.metric(
        "observation_age_ticks_mean",
        ledger.observation_age_ticks.mean().unwrap_or(0.0),
    );
    Ok(())
}

fn chunk_of(cell: IVec2) -> ChunkKey {
    ChunkKey::new(
        cell.x.div_euclid(CHUNK_SIZE as i32),
        cell.y.div_euclid(CHUNK_SIZE as i32),
        0,
    )
}

/// The cell `cell` of a stored payload of its chunk.
fn stored_cell(payload: &ChunkPayload, cell: IVec2) -> WorldCell {
    let local = cell - chunk_of(cell).origin_cell();
    payload.cells()[local.y as usize * CHUNK_SIZE + local.x as usize]
}

fn compare(check: &mut Check, when: &str, probes: &Probes, columns: &[Option<WorldCell>]) {
    for ((cell, expected), actual) in probes.cells.iter().zip(&probes.columns).zip(columns) {
        check.equal(&format!("{cell} {when}"), actual, expected);
    }
}

/// Step 4: the rules go off so the regions the trace changed hold still; the window walks east
/// until they have left it, they are captured before their slots are reused, and it comes back
/// while their write is held: the returning chunks are the captured state, not the store's older
/// rows. The sphere is suspended in place while its terrain is away, and resumes, once, when it
/// is back.
fn move_and_return_step(
    story: &mut Story,
    body: BodyHandle,
    check: &mut Check,
) -> Result<Probes, String> {
    let at = story.next() + 1;
    let rules = story.submit(Command::SetSimulationRules(SimulationRules::NONE), at)?;
    story.await_result(rules)?;
    // The stroke, the rut along the bank and the slope, a bank chunk nothing touched, and the
    // cell beside the origin.
    let mut cells = vec![
        IVec2::new(12, 32),
        IVec2::new(24, 32),
        IVec2::new(36, 32),
        IVec2::new(48, 32),
        IVec2::new(-100, -100),
        IVec2::new(-1, -1),
    ];
    if let Some(stroke) = story.stroke {
        cells.insert(0, stroke);
    }
    let columns = story.columns(&cells)?;
    check.that(columns.iter().all(Option::is_some), || {
        format!("a probe is not resident: {columns:?}")
    });
    let probes = Probes { cells, columns };
    let edited = chunk_of(IVec2::new(24, 32));
    let untouched = chunk_of(IVec2::new(-100, -100));
    let before = story.session().runtime().counters();
    let summary_before = *story.session().residency().summary();

    story.faults.arm(
        Barrier::BeforeChunkCommit,
        Action::Hold {
            limit: Duration::from_secs(120),
        },
        1,
    );
    let mut walk_frames = 0;
    for x in [2, 4] {
        let window =
            ResidentWindow::new(ChunkKey::new(x, 0, 0), 2, 2).map_err(|e| format!("{e:?}"))?;
        walk_frames += story.live.move_to(window)?;
    }
    let faults = story.faults.clone();
    story.until("an outgoing write held", |_| {
        faults.holding() == Some(Barrier::BeforeChunkCommit)
    })?;
    check.equal(
        "the edited chunk's newest revision while away",
        story.session().residency().newest(edited),
        Some((ChunkRevision::new(2), false)),
    );
    check.equal(
        "an untouched chunk resolves to its stored revision",
        story.session().residency().newest(untouched),
        Some((ChunkRevision::new(1), true)),
    );
    check.that(!story.session().residency().is_resident(edited), || {
        "the edited chunk is still resident".to_owned()
    });
    // The sphere's chunk is away: frozen where it was, and still sampled.
    story.live.frames(10);
    let frozen = story.sample(body).ok_or("no sample of the sphere")?;
    story.live.frames(20);
    let later = story.sample(body).ok_or("no sample of the sphere")?;
    check.that(frozen.suspended && later.suspended, || {
        format!("the sphere was not suspended while away: {frozen:?}")
    });
    check.that(later.tick > frozen.tick, || {
        "the suspended sphere was not sampled".to_owned()
    });
    check.equal(
        "the suspended sphere is frozen",
        later.position,
        frozen.position,
    );

    // Back home while the write is still held.
    walk_frames += story.live.move_to(home())?;
    check.that(
        story.faults.holding() == Some(Barrier::BeforeChunkCommit),
        || "the write finished before the window came back".to_owned(),
    );
    let returned = story.columns(&probes.cells)?;
    compare(check, "after returning", &probes, &returned);
    let summary = *story.session().residency().summary();
    check.that(
        summary.loads_from_retained > summary_before.loads_from_retained,
        || "no returning chunk was taken from its retained payload".to_owned(),
    );
    story.live.frames(10);
    let resumed = story.sample(body).ok_or("no sample of the sphere")?;
    check.that(!resumed.suspended, || {
        format!("the sphere did not resume: {resumed:?}")
    });
    check.equal(
        "bodies after the return",
        story.session().runtime().bodies().bodies().count(),
        1,
    );

    // The held write finishes after the return; the revision becomes durable and the store's
    // row holds the captured cells.
    story.faults.release();
    story.until("the delayed writes", |session| !session.residency().busy())?;
    check.equal(
        "the edited chunk after its write",
        story.session().residency().newest(edited),
        Some((ChunkRevision::new(2), true)),
    );
    let mut payload = ChunkPayload::zeroed();
    check.equal(
        "the stored revision",
        story
            .store
            .read_chunk(edited, Some(ChunkRevision::new(2)), &mut payload)
            .map_err(|error| error.to_string())?,
        Some(ChunkRevision::new(2)),
    );
    for (cell, expected) in probes.cells.iter().zip(&probes.columns) {
        if chunk_of(*cell) == edited {
            check.equal(
                &format!("{cell} in the stored row"),
                Some(stored_cell(&payload, *cell)),
                *expected,
            );
        }
    }
    let after = story.session().runtime().counters();
    let summary = *story.session().residency().summary();
    check.equal("frame waits", after.frame_waits - before.frame_waits, 0);
    check.metric("walk_frames", walk_frames);
    check.metric("upload_bytes", after.upload.chunks - before.upload.chunks);
    check.metric(
        "snapshot_readback_bytes",
        after.readback.snapshots - before.readback.snapshots,
    );
    check.metric(
        "snapshots_changed",
        summary.snapshots_changed - summary_before.snapshots_changed,
    );
    check.metric(
        "snapshots_unchanged",
        summary.snapshots_unchanged - summary_before.snapshots_unchanged,
    );
    check.metric(
        "loads_generated",
        summary.loads_generated - summary_before.loads_generated,
    );
    check.metric(
        "loads_stored",
        summary.loads_stored - summary_before.loads_stored,
    );
    check.metric(
        "loads_from_retained",
        summary.loads_from_retained - summary_before.loads_from_retained,
    );
    check.metric("loads_out_of_order", summary.loads_out_of_order);
    check.metric("stale_loads", summary.stale_loads);
    check.metric("retained_high_water_bytes", summary.retained_high_water);
    check.metric("load_age_ms_max", summary.load_age_ms.max);
    check.metric("write_age_ms_max", summary.write_age_ms.max);
    Ok(probes)
}

/// Step 5: a requested checkpoint at one completed tick, with the sphere and a changed
/// configuration; then the session closes in order (saving its last tick), the store object is
/// dropped, and a new store and session open the file under the next epoch. Saved again at its
/// first boundary, before any tick, the reopened world is the checkpoint it opened: the same
/// body records, configuration and heads. The probes read back and every resident chunk holds
/// its published revision.
fn checkpoint_and_reopen_step(
    adapter: &str,
    mut story: Story,
    body: BodyHandle,
    probes: &Probes,
    check: &mut Check,
) -> Result<Story, String> {
    let ambient = world_model::command::AmbientTemperature::celsius(7.0)
        .map_err(|error| format!("{error:?}"))?;
    let at = story.next() + 1;
    let configured = story.submit(Command::SetAmbientTemperature(ambient), at)?;
    story.await_result(configured)?;
    let requested = story.live.save()?;
    let latest = story.store.latest().map_err(|error| error.to_string())?;
    let receipt = story.session().checkpoints().receipt().cloned();
    check.equal(
        "the requested checkpoint is the latest",
        latest.id,
        requested,
    );
    check.that(
        receipt.as_ref().is_some_and(|receipt| {
            matches!(receipt.state, SaveState::Committed { tick, .. } if tick == latest.tick)
        }),
        || format!("the receipt and the manifest disagree: {receipt:?} {latest:?}"),
    );
    check.equal(
        "saved ambient temperature",
        latest.config.ambient_temperature,
        ambient,
    );
    check.equal("saved rules", latest.config.rules, SimulationRules::NONE);
    check.equal("saved window", latest.window, home());
    let saved = story
        .store
        .read_bodies(latest.id)
        .map_err(|error| error.to_string())?;
    check.equal("saved bodies", saved.live(), 1);
    check.that(
        saved
            .bodies
            .first()
            .is_some_and(|record| record.handle == body),
        || {
            format!(
                "the saved body is not the sphere: {:?}",
                saved.bodies.first()
            )
        },
    );

    let (mut story, report) = story.reopen(adapter, SessionEpoch::new(3))?;
    check.that(
        report.saved && report.workers_joined && !report.timed_out,
        || format!("{report:?}"),
    );
    check.metric("close_waited_ms", report.waited.as_secs_f64() * 1000.0);
    let closed = report.latest;
    check.that(!story.session().start().previous_unclean, || {
        "the orderly close left the store unclean".to_owned()
    });
    let closed_bodies = story
        .store
        .read_bodies(closed.id)
        .map_err(|error| error.to_string())?;
    let heads: Vec<(ChunkKey, ChunkRevision)> = story
        .store
        .heads()
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|(key, head)| (key, head.revision))
        .collect();

    // Before any tick: save the restored world as it is.
    let again = story.save_first_boundary()?;
    check.equal(
        "the first-boundary save",
        again.id.value(),
        closed.id.value() + 1,
    );
    check.equal("reopened tick", again.tick, closed.tick);
    check.equal("reopened configuration", again.config, closed.config);
    check.equal(
        "reopened bodies",
        story
            .store
            .read_bodies(again.id)
            .map_err(|error| error.to_string())?,
        closed_bodies,
    );
    check.equal(
        "reopened heads",
        story
            .store
            .heads()
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|(key, head)| (key, head.revision))
            .collect::<Vec<_>>(),
        heads.clone(),
    );

    story.runtime().set_controlled(Some(body));
    story.live.settle()?;
    let columns = story.columns(&probes.cells)?;
    compare(check, "reopened", probes, &columns);
    for (key, revision) in &heads {
        if home().contains(*key) {
            check.equal(
                &format!("{key:?} published revision"),
                story.session().residency().published_revision(*key),
                Some(*revision),
            );
        }
    }
    check.equal(
        "bodies after reopening",
        story.session().runtime().bodies().bodies().count(),
        1,
    );
    story.live.frames(5);
    check.that(story.sample(body).is_some(), || {
        "the reopened sphere is not sampled under its handle".to_owned()
    });
    check.metric(
        "checkpoint_tick",
        closed.tick.map_or(0, |tick| tick.value()),
    );
    check.metric("published_heads", heads.len());
    Ok(story)
}

/// Opens the file again, waiting for a store object the lost session's workers may still hold.
fn reopen_file(path: &std::path::Path) -> Result<Arc<RedbWorldStore>, String> {
    let started = std::time::Instant::now();
    loop {
        match open_store(path, StoreFaults::new()) {
            Ok(store) => return Ok(store),
            Err(_) if started.elapsed() < Duration::from_secs(20) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error),
        }
    }
}
