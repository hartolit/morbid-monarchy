//! The loss bound of periodic saves (ADR 0004) through the real owners, at the client's default
//! cadence and paced at the tick rate as the client runs, with the store on the workspace's disk:
//!
//! ```text
//! device lost while a save is copying       the save is abandoned; the loss report's ticks lost
//!                                           stay within the interval and the save's allowance
//! process ends before a publication commits the reopened world is the previous checkpoint, no
//!                                           further behind that process's last tick than that
//! storage fails for a while                 saves retry, the exposure grows past the bound and the
//!                                           receipt says why; once writes succeed the next save
//!                                           brings it back within the bound
//! ```
//!
//! Every fifth tick raises a marker cell of its own by one unit, with every rule off, so after
//! each of these the reopened world must hold exactly the markers of the ticks its checkpoint
//! holds, and every other cell as generated.

use std::{
    collections::BTreeMap,
    path::Path,
    process::Command as Process,
    time::{Duration, Instant},
};

use world_model::{
    cell::WorldCell,
    command::{Command, EditTool, MaterialEdit},
    generation::WorldGenerator,
    glam::IVec2,
    rules::SimulationRules,
    spatial::{CHUNK_SIZE, ChunkKey},
};
use world_runtime::{
    checkpoint::{DEFAULT_AUTOSAVE_TICKS, SaveReason, SaveState},
    ids::{CommandId, SessionEpoch, Tick},
    receipts::ReceiptState,
    session::{SessionConfig, WorldSession},
    store::{ChunkPayload, WorldStore},
};
use world_store::faults::{Action, Barrier, StoreFaults};

use super::{
    super::{Check, allow_software},
    world::{self, Live, Scratch, open, open_store, session_config, window},
};

/// Ticks a save may take from its request to its publication here. The cadence measurements
/// (`checkpoint-cadence-*`) published within 10; the margin is for a loaded machine.
const PUBLICATION_ALLOWANCE: u64 = 30;
/// The most a crash or a lost device may lose while storage keeps up.
const BOUND: u64 = DEFAULT_AUTOSAVE_TICKS + PUBLICATION_ALLOWANCE;
const MARKER_EVERY: u64 = 5;
const CHILD: &str = "MORBID_STREAMING_BOUNDED_CHILD";

/// The cell marker `index` raises: every other cell of the radius-1 window at the origin, row by
/// row from its lowest corner, so no two markers share a cell.
fn marker_cell(index: u64) -> IVec2 {
    IVec2::new(-64 + 2 * (index % 96) as i32, -64 + 2 * (index / 96) as i32)
}

/// Markers by index, with the tick each applied in (or was due in, until it was submitted).
type Markers = BTreeMap<u64, Tick>;

/// A session paced at 60 ticks/s that raises a marker every fifth tick.
struct Paced {
    live: Live,
    /// The next marker's index; each run starts at its own base.
    next_marker: u64,
    /// Admitted markers and their commands, until their tick is known.
    pending: Vec<(u64, CommandId)>,
    markers: Markers,
    next_id: u64,
    deadline: Instant,
    /// The most ticks after the latest checkpoint seen at any frame.
    exposure_max: u64,
}

impl Paced {
    fn new(live: Live, first_marker: u64) -> Self {
        Self {
            live,
            next_marker: first_marker,
            pending: Vec::new(),
            markers: Markers::new(),
            next_id: 1,
            deadline: Instant::now(),
            exposure_max: 0,
        }
    }

    fn last(&self) -> Option<u64> {
        self.live
            .session
            .runtime()
            .last_submitted_tick()
            .map(Tick::value)
    }

    fn exposure(&self) -> u64 {
        exposure(&self.live.session)
    }

    /// One frame of one tick at the tick rate, with a marker admitted for the next tick when it
    /// is a fifth one.
    fn frame(&mut self) -> Result<(), String> {
        let next = self.last().map_or(0, |last| last + 1);
        let due = next.is_multiple_of(MARKER_EVERY)
            && self.markers.values().all(|tick| tick.value() != next);
        if due {
            let edit = MaterialEdit::new(EditTool::Raise, marker_cell(self.next_marker), 0, 1)
                .map_err(|error| error.to_string())?;
            let id = CommandId::new(self.next_id);
            self.live
                .session
                .runtime_mut()
                .submit_at(id, Command::EditMaterial(edit), Tick::new(next))
                .map_err(|error| format!("marker {}: {error:?}", self.next_marker))?;
            self.next_id += 1;
            self.markers.insert(self.next_marker, Tick::new(next));
            self.pending.push((self.next_marker, id));
            self.next_marker += 1;
        }
        self.deadline += Duration::from_secs_f64(1.0 / 60.0);
        self.live.frame_ticks(1);
        // A marker applies in the tick it was submitted in, which pressure can delay.
        let runtime = self.live.session.runtime();
        let markers = &mut self.markers;
        self.pending.retain(|(index, id)| {
            match runtime.receipt(*id).map(|receipt| receipt.state) {
                Some(ReceiptState::Submitted { tick } | ReceiptState::Completed { tick, .. }) => {
                    markers.insert(*index, tick);
                    false
                }
                _ => true,
            }
        });
        self.exposure_max = self.exposure_max.max(self.exposure());
        let now = Instant::now();
        if self.deadline > now {
            std::thread::sleep(self.deadline - now);
        } else {
            self.deadline = now;
        }
        Ok(())
    }

    /// Runs frames until `done` holds, at most `frames` of them.
    fn until(
        &mut self,
        what: &str,
        frames: u32,
        mut done: impl FnMut(&Self) -> bool,
    ) -> Result<(), String> {
        for _ in 0..frames {
            self.frame()?;
            if done(self) {
                return Ok(());
            }
        }
        Err(format!(
            "{what}: not after {frames} frames; receipt {:?}",
            self.live.session.checkpoints().receipt()
        ))
    }
}

/// Ticks after the latest published checkpoint: what a crash now would lose.
fn exposure(session: &WorldSession) -> u64 {
    let Some(last) = session.runtime().last_submitted_tick() else {
        return 0;
    };
    session
        .checkpoints()
        .latest()
        .tick
        .map_or(last.value() + 1, |saved| last.since(saved))
}

/// The client's cadence, over the streaming cases' configuration.
fn config() -> SessionConfig {
    let mut config = session_config();
    config.checkpoints.autosave_ticks = Some(DEFAULT_AUTOSAVE_TICKS);
    config
}

/// Whether `session` is in the middle of an autosave that has not reached the CPU yet.
fn autosave_copying(session: &WorldSession) -> bool {
    session.checkpoints().receipt().is_some_and(|receipt| {
        receipt.reason == SaveReason::Autosave
            && matches!(
                receipt.state,
                SaveState::Requested | SaveState::Capturing { .. }
            )
    })
}

/// Adds the markers of a run to the world's history, keeping only those the checkpoint the
/// world continues from holds: later ones were lost, and their ticks are run again.
fn continue_history(history: &mut Markers, run: &Markers, store: &dyn WorldStore) {
    let saved = store
        .latest()
        .ok()
        .and_then(|latest| latest.tick)
        .map_or(-1, |tick| tick.value() as i64);
    history.extend(run.iter().map(|(index, tick)| (*index, *tick)));
    history.retain(|_, tick| tick.value() as i64 <= saved);
}

/// Checks a store's latest checkpoint against the history: exactly its markers are raised and
/// every other cell of the window is as generated.
fn check_markers(
    check: &mut Check,
    what: &str,
    store: &dyn WorldStore,
    history: &Markers,
) -> Result<(), String> {
    let generator = WorldGenerator::new(world::SEED);
    let size = CHUNK_SIZE as i32;
    let mut expected: BTreeMap<(i32, i32), Vec<WorldCell>> = BTreeMap::new();
    for key in window(0, 0, 1).view().iter() {
        expected.insert(
            (key.key.x, key.key.y),
            generator.generate_chunk(key).cells.to_vec(),
        );
    }
    for index in history.keys() {
        let cell = marker_cell(*index);
        let (x, y) = (cell.x.div_euclid(size), cell.y.div_euclid(size));
        let local = (cell.y.rem_euclid(size) * size + cell.x.rem_euclid(size)) as usize;
        let column = &mut expected
            .get_mut(&(x, y))
            .ok_or("marker outside the window")?[local];
        column.set_elevation(column.elevation() + 1);
    }
    let mut payload = ChunkPayload::zeroed();
    let mut differing = 0;
    for ((x, y), cells) in &expected {
        let key = ChunkKey::new(*x, *y, 0);
        let found = store
            .read_chunk(key, None, &mut payload)
            .map_err(|error| format!("{what}: read {key:?}: {error}"))?;
        let saved = match found {
            Some(_) => payload.cells().to_vec(),
            None => generator.generate_chunk(key).cells.to_vec(),
        };
        differing += saved.iter().zip(cells).filter(|(a, b)| a != b).count();
    }
    check.equal(
        &format!("{what}: cells differing from the markers the checkpoint holds"),
        differing,
        0,
    );
    check.metric(&format!("{what}_markers_held"), history.len());
    Ok(())
}

/// A device lost while an autosave is still copying: the save is abandoned and the loss stays
/// within the bound; the recovered world holds the markers up to its checkpoint exactly.
fn device_loss(
    adapter: &str,
    check: &mut Check,
    path: &Path,
    history: &mut Markers,
) -> Result<(), String> {
    let store = open_store(path, StoreFaults::new())?;
    let mut live = open(
        adapter,
        store.clone(),
        SessionEpoch::FIRST,
        config(),
        |_| {},
    )?;
    live.settle()?;
    let mut run = Paced::new(live, 0);
    run.until("three autosaves", 600, |run| {
        run.live.session.checkpoints().committed() >= 3
    })?;
    run.until("an autosave copying", 600, |run| {
        autosave_copying(&run.live.session)
    })?;
    check.that(run.exposure_max <= BOUND, || {
        format!("{} ticks exposed before the loss", run.exposure_max)
    });
    check.metric("device_loss_exposure_max", run.exposure_max);
    run.live.faults.lose_device();
    let started = Instant::now();
    while run.live.session.loss().is_none() {
        run.live.frame_ticks(0);
        if started.elapsed() > Duration::from_secs(10) {
            return Err("the loss was never reported".to_owned());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let markers = run.markers.clone();
    let (_, loss) = run.live.session.finish_after_loss(Duration::from_secs(10));
    check.equal(
        "device loss: the interval reported",
        loss.autosave_ticks,
        Some(DEFAULT_AUTOSAVE_TICKS),
    );
    check.that(!loss.finished_captured_save, || {
        format!("the save in progress had reached the CPU: {loss:?}")
    });
    check.that(loss.ticks_lost > 0 && loss.ticks_lost <= BOUND, || {
        format!("{} ticks lost, bound {BOUND}: {loss:?}", loss.ticks_lost)
    });
    check.metric("device_loss_ticks_lost", loss.ticks_lost);
    drop(store);
    // Recovery: a later epoch from the latest complete checkpoint, which holds the markers up
    // to its tick and no others.
    let store = open_store(path, StoreFaults::new())?;
    check.equal(
        "device loss: the recovered checkpoint",
        store.latest().map_err(|error| error.to_string())?.tick,
        loss.last_complete_tick,
    );
    continue_history(history, &markers, &*store);
    check_markers(check, "device_loss", &*store, history)?;
    let mut live = open(adapter, store, SessionEpoch::new(2), config(), |_| {})?;
    live.settle()?;
    let report = live.session.close(Duration::from_secs(30));
    check.that(report.saved, || {
        format!("the recovered session's close: {report:?}")
    });
    Ok(())
}

/// The child half of the interruption: a paced session whose writer ends the process right
/// before a publication commits, once three autosaves have; it records its last submitted tick
/// and its markers in `progress` every frame.
fn interruption_child(adapter: &str, spec: &str) -> Result<(), String> {
    let (path, progress) = spec.split_once('|').ok_or("bad child spec")?;
    let faults = StoreFaults::new();
    let store = open_store(Path::new(path), faults.clone())?;
    let mut live = open(adapter, store, SessionEpoch::new(3), config(), |_| {})?;
    live.settle()?;
    let mut run = Paced::new(live, 1_000);
    let mut armed = false;
    for _ in 0..3_000 {
        run.frame()?;
        let markers: Vec<String> = run
            .markers
            .iter()
            .map(|(index, tick)| format!("{index}:{}", tick.value()))
            .collect();
        std::fs::write(
            progress,
            format!("{}\n{}", run.last().unwrap_or(0), markers.join(",")),
        )
        .map_err(|error| error.to_string())?;
        if !armed && run.live.session.checkpoints().committed() >= 3 {
            faults.arm(Barrier::BeforePublishCommit, Action::Abort, 1);
            armed = true;
        }
    }
    // Reached only if the abort never was: the parent fails the case.
    Ok(())
}

/// A process ending right before a publication commits: the reopened world is the previous
/// checkpoint, within the bound of the process's last tick, with exactly its markers.
fn interruption(
    adapter: &str,
    check: &mut Check,
    path: &Path,
    scratch: &Scratch,
    history: &mut Markers,
) -> Result<(), String> {
    let progress = scratch.path("progress.txt");
    let mut child = Process::new(std::env::current_exe().map_err(|error| error.to_string())?);
    child.args([
        "--suite",
        "streaming",
        "--case",
        "bounded-loss",
        "--adapter",
        adapter,
        "--result",
        scratch.path("child.json").to_str().ok_or("path")?,
    ]);
    child.env(CHILD, format!("{}|{}", path.display(), progress.display()));
    if allow_software() {
        child.arg("--allow-software");
    }
    let output = child.output().map_err(|error| error.to_string())?;
    check.that(!output.status.success(), || {
        format!(
            "the child was not interrupted: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    let text = std::fs::read_to_string(&progress).map_err(|error| error.to_string())?;
    let (last, entries) = text.split_once('\n').ok_or("no progress")?;
    let last: u64 = last.trim().parse().map_err(|_| "bad progress")?;
    let mut markers = Markers::new();
    for entry in entries.split(',').filter(|entry| !entry.is_empty()) {
        let (index, tick) = entry.split_once(':').ok_or("bad marker")?;
        markers.insert(
            index.parse().map_err(|_| "bad marker")?,
            Tick::new(tick.parse().map_err(|_| "bad marker")?),
        );
    }
    let store = open_store(path, StoreFaults::new())?;
    let saved = store
        .latest()
        .map_err(|error| error.to_string())?
        .tick
        .map_or(0, Tick::value);
    let lost = last.saturating_sub(saved);
    check.that(lost > 0 && lost <= BOUND, || {
        format!(
            "interruption: the process's last tick {last}, the checkpoint's {saved}: {lost} ticks lost"
        )
    });
    check.metric("interruption_ticks_lost", lost);
    continue_history(history, &markers, &*store);
    check_markers(check, "interruption", &*store, history)?;
    Ok(())
}

/// Storage failing for a while: saves retry, the exposure grows past the bound and the receipt
/// says why; once writes succeed, the next save brings the exposure back within the bound.
fn failing_storage(
    adapter: &str,
    check: &mut Check,
    path: &Path,
    history: &mut Markers,
) -> Result<(), String> {
    let faults = StoreFaults::new();
    let store = open_store(path, faults.clone())?;
    let mut live = open(
        adapter,
        store.clone(),
        SessionEpoch::new(4),
        config(),
        |_| {},
    )?;
    check.that(live.session.start().previous_unclean, || {
        "the interrupted session was not recorded unclean".to_owned()
    });
    live.settle()?;
    let mut run = Paced::new(live, 2_000);
    run.until("an autosave", 600, |run| {
        run.live.session.checkpoints().committed() >= 1
    })?;
    check.that(run.exposure_max <= BOUND, || {
        format!("{} ticks exposed before the failures", run.exposure_max)
    });
    faults.fail_writes(u64::MAX);
    let mut retrying = false;
    run.until("the exposure past the bound", 900, |run| {
        retrying |= run
            .live
            .session
            .checkpoints()
            .receipt()
            .is_some_and(|receipt| {
                matches!(receipt.state, SaveState::Failed { retrying: true, .. })
            });
        run.exposure() > BOUND + DEFAULT_AUTOSAVE_TICKS
    })?;
    check.that(retrying, || {
        format!(
            "no failed, retrying save reported: {:?}",
            run.live.session.checkpoints().receipt()
        )
    });
    check.metric("failing_storage_exposure", run.exposure());
    faults.fail_writes(0);
    let healed_at = run.last().unwrap_or(0);
    run.until("the exposure back within the bound", 900, |run| {
        run.exposure() <= BOUND
    })?;
    check.metric(
        "failing_storage_recovered_after_ticks",
        run.last().unwrap_or(0) - healed_at,
    );
    let markers = run.markers.clone();
    let report = run.live.session.close(Duration::from_secs(30));
    check.that(report.saved, || {
        format!("the close after the failures: {report:?}")
    });
    drop(store);
    let store = open_store(path, StoreFaults::new())?;
    continue_history(history, &markers, &*store);
    check_markers(check, "failing_storage", &*store, history)?;
    Ok(())
}

pub fn bounded_loss(adapter: &str, check: &mut Check) -> Result<(), String> {
    if let Ok(spec) = std::env::var(CHILD) {
        return interruption_child(adapter, &spec);
    }
    let scratch = Scratch::on_disk("bounded")?;
    let path = scratch.path("world.redb");
    world::create(&path, window(0, 0, 1), SimulationRules::NONE)?;
    let mut history = Markers::new();
    device_loss(adapter, check, &path, &mut history)?;
    interruption(adapter, check, &path, &scratch, &mut history)?;
    failing_storage(adapter, check, &path, &mut history)?;
    Ok(())
}
