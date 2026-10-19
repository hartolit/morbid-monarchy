//! The headless session: a saved world opened from its store, run, edited and closed in order
//! through the production owners (`world-store`, `world-runtime`'s session, `gpu-world`), the
//! way a client composes them, without presentation. `cargo evidence session` runs it in a
//! `gpu-suite` process; running it again on the same store reopens what the close saved.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use gpu_world::world::{GpuWorld, GpuWorldConfig};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use world_model::{
    command::{Command, MaterialEdit},
    glam::IVec2,
    spatial::ChunkKey,
};
use world_runtime::{
    ids::CommandId,
    port::{CommandResult, ResidentWindow},
    receipts::ReceiptState,
    session::{SessionConfig, SessionPlan, WorldSession},
    store::{CheckpointId, WorldStore},
};
use world_store::store::{OpenOptionsStore, RedbWorldStore};

use super::{Check, allow_software};
use crate::cli::{Autosave, SessionRequest};

/// One part of a session run: opening, running, closing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStep {
    pub name: String,
    pub passed: bool,
    pub summary: String,
    pub failures: Vec<String>,
    pub notes: Vec<String>,
    pub metrics: Map<String, Value>,
}

/// What a session process reports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResult {
    pub store: String,
    pub adapter: String,
    pub steps: Vec<SessionStep>,
}

impl SessionResult {
    /// Every step ran and passed (a session that stopped early lacks its later steps).
    pub fn passed(&self) -> bool {
        self.steps.len() == 3 && self.steps.iter().all(|step| step.passed)
    }
}

fn step(name: &str, summary: String, check: Check) -> SessionStep {
    let (failures, notes, metrics) = check.into_parts();
    SessionStep {
        name: name.to_owned(),
        passed: failures.is_empty(),
        summary,
        failures,
        notes,
        metrics,
    }
}

/// The longest a close may wait for the last ticks, writes and the final save.
const CLOSE_LIMIT: Duration = Duration::from_secs(120);

pub fn run(request: &SessionRequest) -> SessionResult {
    let mut result = SessionResult {
        store: request.store.display().to_string(),
        adapter: request.adapter.clone(),
        steps: Vec::new(),
    };
    let mut check = Check::default();
    let session = match open(request, &mut check) {
        Ok((session, summary)) => {
            result.steps.push(step("open", summary, check));
            session
        }
        Err(error) => {
            check.fail(error);
            result
                .steps
                .push(step("open", "the world did not open".to_owned(), check));
            return result;
        }
    };
    let mut check = Check::default();
    let (session, summary) = run_ticks(session, request, &mut check);
    result.steps.push(step("run", summary, check));
    let Some(session) = session else {
        return result;
    };
    let mut check = Check::default();
    let report = session.close(CLOSE_LIMIT);
    check.that(report.saved && !report.timed_out, || {
        format!(
            "the close did not save every change: timed out {}, {} captured revisions unwritten, save {:?}",
            report.timed_out, report.unwritten, report.save
        )
    });
    check.that(report.workers_joined, || {
        "the store's workers did not finish".to_owned()
    });
    check.metric("checkpoint", report.latest.id.value());
    check.metric(
        "tick",
        report
            .latest
            .tick
            .map_or(Value::Null, |tick| tick.value().into()),
    );
    check.metric("abandoned_commands", report.abandoned_commands);
    check.metric("unwritten_revisions", report.unwritten);
    check.metric("waited_ms", report.waited.as_secs_f64() * 1000.0);
    let summary = if report.saved {
        format!(
            "checkpoint {} at tick {} holds every change up to the last completed tick",
            report.latest.id.value(),
            tick_text(report.latest.tick.map(|tick| tick.value()))
        )
    } else {
        format!(
            "not saved: the latest complete checkpoint is still {} at tick {}",
            report.latest.id.value(),
            tick_text(report.latest.tick.map(|tick| tick.value()))
        )
    };
    result.steps.push(step("close", summary, check));
    result
}

fn tick_text(tick: Option<u64>) -> String {
    tick.map_or_else(|| "none (never run)".to_owned(), |tick| tick.to_string())
}

fn open(request: &SessionRequest, check: &mut Check) -> Result<(WorldSession, String), String> {
    let store = RedbWorldStore::open(&request.store, OpenOptionsStore::default()).map_err(|error| {
        format!(
            "open {}: {error} (create a world with `cargo run -p world-store -- create`, or import a legacy save with `import-legacy`)",
            request.store.display()
        )
    })?;
    let store = Arc::new(store);
    let plan = SessionPlan::prepare(&*store)
        .map_err(|error| format!("read the latest checkpoint: {error}"))?;
    let epoch = plan
        .manifest
        .epoch
        .next()
        .ok_or("the store's session epochs are exhausted")?;
    let mut gpu = GpuWorldConfig::resume(&plan, &request.adapter, epoch)
        .ok_or("the latest checkpoint has no next tick")?;
    gpu.allow_software = allow_software();
    let header = plan.header;
    let manifest = plan.manifest;
    let start = plan.start;
    let world = GpuWorld::open(gpu).map_err(|error| format!("open the live world: {error:?}"))?;
    let mut config = SessionConfig::default();
    match request.autosave {
        Autosave::Default => {}
        Autosave::Off => config.checkpoints.autosave_ticks = None,
        Autosave::Every(ticks) => config.checkpoints.autosave_ticks = Some(ticks),
    }
    let session = WorldSession::open(Box::new(world), store as Arc<dyn WorldStore>, plan, config)
        .map_err(|error| format!("open the session: {error:?}"))?;
    check.metric("world", header.world.value());
    check.metric("origin", format!("{:?}", header.origin));
    check.metric("generator_seed", header.generator.seed);
    check.metric("checkpoint", manifest.id.value());
    check.metric(
        "tick",
        manifest
            .tick
            .map_or(Value::Null, |tick| tick.value().into()),
    );
    check.metric("epoch", epoch.value());
    check.metric(
        "window",
        format!(
            "{:?} radius {}x{}",
            manifest.window.center(),
            manifest.window.radius_x(),
            manifest.window.radius_y()
        ),
    );
    check.metric("rules", manifest.config.rules.bits());
    check.metric(
        "ambient_temperature_c",
        f64::from(manifest.config.ambient_temperature.degrees()),
    );
    check.metric(
        "elevation_scale",
        f64::from(manifest.config.elevation_scale.value()),
    );
    check.metric("autosave_ticks", config_autosave(request.autosave));
    if start.previous_unclean {
        check.note(format!(
            "the previous session did not close; {} unpublished chunk rows and {} body sets it left were reclaimed",
            start.reclaimed.chunk_rows, start.reclaimed.body_sets
        ));
    }
    check.metric("previous_unclean", start.previous_unclean);
    let summary = format!(
        "checkpoint {} at tick {} as session epoch {}",
        manifest.id.value(),
        tick_text(manifest.tick.map(|tick| tick.value())),
        epoch.value()
    );
    Ok((session, summary))
}

fn config_autosave(autosave: Autosave) -> Value {
    match autosave {
        Autosave::Default => SessionConfig::default()
            .checkpoints
            .autosave_ticks
            .map_or(Value::Null, Value::from),
        Autosave::Off => Value::Null,
        Autosave::Every(ticks) => ticks.into(),
    }
}

/// Runs the session by wall time, as a client does, until `ticks` ticks are submitted and every
/// edit has its result. Returns the session unless the device was lost.
fn run_ticks(
    mut session: WorldSession,
    request: &SessionRequest,
    check: &mut Check,
) -> (Option<WorldSession>, String) {
    if let Some(((x, y), radius)) = request.window {
        match ResidentWindow::new(ChunkKey::new(x, y, 0), radius, radius) {
            Ok(window) => session.set_window(window),
            Err(error) => check.fail(format!("window: {error:?}")),
        }
    }
    let limit = Duration::from_secs(120 + u64::from(request.ticks) / 20);
    let started = Instant::now();
    let mut last = started;
    let mut frames = 0u64;
    let mut edits: Vec<(CommandId, String)> = Vec::new();
    let mut submitted_edits = false;
    let mut results = Vec::new();
    let mut checkpoints = vec![session.checkpoints().latest().id];
    let first = session.runtime().counters().ticks_submitted;
    loop {
        let now = Instant::now();
        session.frame(now - last);
        last = now;
        frames += 1;
        if let Some(loss) = session.loss() {
            check.fail(format!(
                "the device was lost: the latest complete checkpoint is {} at tick {}; {} ticks after it are lost",
                loss.last_complete.value(),
                tick_text(loss.last_complete_tick.map(|tick| tick.value())),
                loss.ticks_lost
            ));
            let (_, loss) = session.finish_after_loss(CLOSE_LIMIT);
            check.metric("finished_captured_save", loss.finished_captured_save);
            return (None, "the device was lost".to_owned());
        }
        let latest = session.checkpoints().latest().id;
        if checkpoints.last() != Some(&latest) {
            checkpoints.push(latest);
        }
        if !submitted_edits && session.residency().settled() {
            submitted_edits = true;
            for (index, edit) in request.edits.iter().enumerate() {
                let cell = IVec2::new(edit.cell.0, edit.cell.1);
                let label = format!("{} at {cell}", edit.name());
                let id = CommandId::new(index as u64 + 1);
                let submitted = MaterialEdit::new(edit.tool, cell, 2, 3)
                    .map_err(|error| format!("{error:?}"))
                    .and_then(|material| {
                        session
                            .runtime_mut()
                            .submit(id, Command::EditMaterial(material))
                            .map_err(|error| format!("{error:?}"))
                    });
                match submitted {
                    Ok(_) => edits.push((id, label)),
                    Err(error) => check.fail(format!("{label}: refused: {error}")),
                }
            }
        }
        edits.retain(
            |(id, label)| match session.runtime().receipt(*id).map(|r| r.state) {
                Some(ReceiptState::Completed { tick, result }) => {
                    results.push((label.clone(), tick.value(), result));
                    false
                }
                Some(ReceiptState::Pending | ReceiptState::Submitted { .. }) | None => true,
                Some(other) => {
                    check.fail(format!("{label}: {other:?}"));
                    false
                }
            },
        );
        let ticks = session.runtime().counters().ticks_submitted - first;
        if ticks >= u64::from(request.ticks) && submitted_edits && edits.is_empty() {
            break;
        }
        if started.elapsed() > limit {
            check.fail(format!(
                "not done after {:.0} s: {ticks} of {} ticks, residency {:?}",
                limit.as_secs_f64(),
                request.ticks,
                session.residency().summary()
            ));
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    for (label, tick, result) in &results {
        match result {
            CommandResult::Edited { changed, .. } => {
                check.note(format!("{label}: {changed} cells changed at tick {tick}"));
            }
            other => check.fail(format!("{label}: {other:?}")),
        }
    }
    let failed: Vec<String> = session
        .residency()
        .failed()
        .map(|(key, error)| format!("{key:?}: {error}"))
        .collect();
    for failure in &failed {
        check.fail(format!(
            "a chunk could not be loaded and stays out: {failure}"
        ));
    }
    let counters = session.runtime().counters();
    let summary = *session.residency().summary();
    let ticks = counters.ticks_submitted - first;
    check.metric("ticks", ticks);
    check.metric("frames", frames);
    check.metric("seconds", started.elapsed().as_secs_f64());
    check.metric("frame_waits", counters.frame_waits);
    check.metric("resident_chunks", summary.resident);
    check.metric("loads_stored", summary.loads_stored);
    check.metric("loads_generated", summary.loads_generated);
    check.metric("read_failures", summary.read_failures);
    check.metric("writes_committed", summary.writes_committed);
    check.metric("write_failures", summary.write_failures);
    check.metric(
        "checkpoints_during_run",
        checkpoints
            .iter()
            .skip(1)
            .map(|id: &CheckpointId| id.value())
            .collect::<Vec<_>>(),
    );
    check.metric(
        "last_save",
        format!("{:?}", session.checkpoints().receipt()),
    );
    (
        Some(session),
        format!(
            "{ticks} ticks, {} edits, {} checkpoints during the run",
            results.len(),
            checkpoints.len() - 1
        ),
    )
}
