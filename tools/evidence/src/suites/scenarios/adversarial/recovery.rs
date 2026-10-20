//! Parts 3 and 4 of step 6: faults, device loss and recovery (05's part), then process
//! interruption before and after publication, an orderly close and a fresh reopen (07's).

use std::{path::Path, process::Command as Process, time::Duration};

use world_model::{
    command::{AmbientTemperature, Command, EditTool},
    glam::IVec2,
    spatial::ChunkKey,
};
use world_runtime::{
    checkpoint::{SaveFailure, SaveState},
    ids::SessionEpoch,
    port::{AbandonReason, Refusal, ResidentWindow},
    store::{CheckpointManifest, WorldStore},
};
use world_store::faults::{Action, Barrier, StoreFaults};

use super::{
    super::{
        super::{Check, allow_software},
        Probes, ScenarioResult, Scratch, Story, chunk_of, compare, home, open_store, reopen_file,
        show_step, step, trace_config, world,
    },
    ambient, edit,
};

/// The environment variable that makes a `resident-world` process the interrupted child.
pub(in super::super) const INTERRUPT: &str = "MORBID_RESIDENT_WORLD_INTERRUPT";
/// What the interrupted child changes before its save.
const CHILD_CELL: IVec2 = IVec2::new(-40, 100);
const CHILD_AMBIENT_C: f32 = -5.0;

/// Part 3, 05's: map deliveries are held while a save and a growth of the window overlap,
/// eviction writes fail and are retried, loads may answer out of order, and the window comes
/// back; a save whose publication fails is retried into a complete checkpoint. A later change
/// and its save are then lost with the device before the save's payloads reach the CPU, and a
/// new store and session recover exactly that checkpoint, with its bodies and configuration,
/// refusing the lost session's slots.
pub(super) fn faults_and_loss(
    adapter: &str,
    mut story: Story,
    probes: &Probes,
    check: &mut Check,
) -> Result<Story, String> {
    let bank = IVec2::new(-90, -40);
    let at = story.next();
    let stroke = story.submit(edit(EditTool::Raise, bank, 2, 3)?, at)?;
    story.await_result(stroke)?;
    let bank_column = story.column(bank)?;
    let summary_before = *story.session().residency().summary();

    // A save whose deliveries are held, and the window growing away while it drains.
    story.live.faults.hold_deliveries(true);
    story.faults.fail_writes(2);
    story.live.session.save();
    let grown = ResidentWindow::new(ChunkKey::new(4, 0, 0), 3, 3).map_err(|e| format!("{e:?}"))?;
    story.live.session.set_window(grown);
    story.live.frames(30);
    let receipt = story.session().checkpoints().receipt().cloned();
    check.that(
        receipt
            .as_ref()
            .is_some_and(|receipt| matches!(receipt.state, SaveState::Capturing { .. })),
        || format!("the save got ahead of held deliveries: {receipt:?}"),
    );
    story.live.faults.hold_deliveries(false);
    story.until("the grown window and the save", |session| {
        session.residency().window() == grown
            && session.residency().settled()
            && !session.residency().busy()
            && session
                .checkpoints()
                .receipt()
                .is_some_and(|receipt| receipt.finished())
    })?;
    let summary = *story.session().residency().summary();
    check.that(
        story
            .session()
            .checkpoints()
            .receipt()
            .is_some_and(|receipt| receipt.committed().is_some()),
        || format!("{:?}", story.session().checkpoints().receipt()),
    );
    check.that(
        summary.write_failures > summary_before.write_failures,
        || "no eviction write failed".to_owned(),
    );
    let (_, _, injected) = story.faults.io_counts();
    check.metric("injected_io_errors", injected);
    check.metric(
        "write_failures",
        summary.write_failures - summary_before.write_failures,
    );
    check.metric(
        "peak_requested_bytes",
        story.session().runtime().counters().peak_requested_bytes,
    );

    story.live.move_to(home())?;
    let columns = story.columns(&probes.cells)?;
    compare(check, "after the faults", probes, &columns);
    check.equal(
        "the bank edit after the faults",
        story.column(bank)?,
        bank_column,
    );
    let summary = *story.session().residency().summary();
    check.metric("loads_out_of_order", summary.loads_out_of_order);
    check.metric("stale_loads", summary.stale_loads);
    check.that(summary.loads_out_of_order > 0, || {
        format!("no load was answered out of order: {summary:?}")
    });

    // The publication's sync fails: an ambiguous commit, reported and retried.
    story.faults.fail_syncs(1);
    story.live.session.save();
    let mut saw_failure = false;
    story.until("the retried save", |session| {
        let receipt = session.checkpoints().receipt();
        saw_failure |= receipt.is_some_and(|receipt| {
            matches!(
                receipt.state,
                SaveState::Failed {
                    failure: SaveFailure::Store(_),
                    retrying: true
                }
            )
        });
        receipt.is_some_and(|receipt| receipt.finished())
    })?;
    check.that(saw_failure, || {
        "the failed publication was never reported".to_owned()
    });
    let complete = story
        .session()
        .checkpoints()
        .receipt()
        .and_then(|receipt| receipt.committed())
        .ok_or_else(|| {
            format!(
                "the retried save did not commit: {:?}",
                story.session().checkpoints().receipt()
            )
        })?;
    let complete_manifest = story.store.latest().map_err(|error| error.to_string())?;
    check.equal(
        "the retried save is the latest",
        complete_manifest.id,
        complete,
    );

    // A change after the checkpoint, and its save lost with the device before the readback.
    let lost_cell = IVec2::new(20, -90);
    let lost_before = story.column(lost_cell)?;
    let at = story.next();
    let stroke = story.submit(edit(EditTool::Raise, lost_cell, 2, 3)?, at)?;
    story.await_result(stroke)?;
    let stale_slot = story
        .session()
        .residency()
        .slot(chunk_of(lost_cell))
        .ok_or("no slot for the changed chunk")?;
    story.live.faults.hold_deliveries(true);
    story.live.session.save();
    story.live.frames(10);
    story.live.faults.lose_device();
    story.until("the loss", |session| session.loss().is_some())?;
    let loss = story.session().loss().ok_or("no loss report")?;
    check.equal("loss reason", loss.reason, AbandonReason::DeviceLost);
    check.equal(
        "the latest complete checkpoint",
        loss.last_complete,
        complete,
    );
    let receipt = story.session().checkpoints().receipt().cloned();
    check.that(
        receipt.as_ref().is_some_and(|receipt| {
            matches!(
                receipt.state,
                SaveState::Failed {
                    failure: SaveFailure::Abandoned(AbandonReason::DeviceLost),
                    retrying: false
                }
            )
        }),
        || format!("the uncaptured save: {receipt:?}"),
    );
    check.metric("ticks_lost", loss.ticks_lost);
    let complete_bodies = story
        .store
        .read_bodies(complete)
        .map_err(|error| error.to_string())?;

    // Everything of the lost session goes; the file is opened again.
    let Story {
        live,
        store,
        path,
        _scratch,
        ..
    } = story;
    let viewer = live.viewer.clone();
    live.detach();
    let (shared, loss) = live.session.finish_after_loss(Duration::from_secs(20));
    check.that(!loss.finished_captured_save, || format!("{loss:?}"));
    drop(shared);
    drop(store);
    let store = reopen_file(&path)?;
    let faults = store.faults().clone();
    let live = world::open_viewed(
        adapter,
        std::sync::Arc::clone(&store),
        SessionEpoch::new(4),
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
        _scratch,
    };
    let start = story.session().start();
    check.that(start.previous_unclean, || {
        "the lost session's store was not recorded unclean".to_owned()
    });
    check.metric(
        "reclaimed_rows",
        start.reclaimed.chunk_rows + start.reclaimed.body_sets,
    );
    check.equal(
        "recovered checkpoint",
        story.store.latest().map_err(|error| error.to_string())?.id,
        complete,
    );
    // Saved again before any tick, the recovered world is the checkpoint: its bodies and its
    // configuration.
    let again = story.save_first_boundary()?;
    check_restored(
        check,
        "recovered",
        &story,
        &again,
        &complete_manifest,
        &complete_bodies,
    )?;
    story.live.settle()?;
    let columns = story.columns(&probes.cells)?;
    compare(check, "recovered", probes, &columns);
    check.equal("the bank edit recovered", story.column(bank)?, bank_column);
    check.equal(
        "the change after the checkpoint is gone",
        story.column(lost_cell)?,
        lost_before,
    );
    check.that(
        matches!(
            story.runtime().retire_slot(stale_slot),
            Err(Refusal::StaleSession { .. })
        ),
        || "the lost session's slot was accepted by the new one".to_owned(),
    );
    check.equal(
        "recovered bodies",
        story.session().runtime().bodies().bodies().count(),
        complete_bodies.live(),
    );
    Ok(story)
}

/// A checkpoint the reopened session saved before its first tick holds what `expected` held.
fn check_restored(
    check: &mut Check,
    when: &str,
    story: &Story,
    saved: &CheckpointManifest,
    expected: &CheckpointManifest,
    expected_bodies: &world_model::resume::BodySet,
) -> Result<(), String> {
    check.equal(&format!("{when}: tick"), saved.tick, expected.tick);
    check.equal(
        &format!("{when}: configuration"),
        saved.config,
        expected.config,
    );
    check.equal(&format!("{when}: window"), saved.window, expected.window);
    check.equal(
        &format!("{when}: bodies"),
        &story
            .store
            .read_bodies(saved.id)
            .map_err(|error| error.to_string())?,
        expected_bodies,
    );
    Ok(())
}

/// Runs this executable as the interrupted child: it opens the world, changes it, and ends
/// inside its save at `point`.
fn interrupt(
    adapter: &str,
    path: &Path,
    point: &str,
    epoch: u32,
    result: &Path,
) -> Result<std::process::Output, String> {
    let mut child = Process::new(std::env::current_exe().map_err(|error| error.to_string())?);
    child.args([
        "--scenario",
        "resident-world",
        "--adapter",
        adapter,
        "--result",
        result.to_str().ok_or("a result path that is not UTF-8")?,
    ]);
    child.env(INTERRUPT, format!("{}|{point}|{epoch}", path.display()));
    if allow_software() {
        child.arg("--allow-software");
    }
    child.output().map_err(|error| error.to_string())
}

/// The interrupted child's part: raise a column, remove the body in the highest slot, change
/// the air temperature, and save with the process ending at the barrier the parent named.
pub(in super::super) fn interruption_child(adapter: &str, spec: &str) -> ScenarioResult {
    let mut check = Check::default();
    match run_child(adapter, spec) {
        Ok(()) => check.fail("the save finished: the process was never interrupted"),
        Err(error) => check.fail(error),
    }
    ScenarioResult {
        scenario: "resident-world".to_owned(),
        adapter: adapter.to_owned(),
        steps: vec![step("interrupted-child", check, true)],
    }
}

fn run_child(adapter: &str, spec: &str) -> Result<(), String> {
    let mut parts = spec.split('|');
    let (Some(path), Some(point), Some(epoch)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(format!("bad child spec {spec}"));
    };
    let epoch: u32 = epoch.parse().map_err(|_| format!("bad epoch {epoch}"))?;
    let barrier = match point {
        "before-publish" => Barrier::BeforePublishCommit,
        "after-publish" => Barrier::AfterPublishCommit,
        other => return Err(format!("unknown interruption point {other}")),
    };
    let faults = StoreFaults::new();
    let store = open_store(Path::new(path), faults.clone())?;
    let mut live = world::open(
        adapter,
        store,
        SessionEpoch::new(epoch),
        trace_config(),
        |_| {},
    )?;
    live.settle()?;
    live.run(edit(EditTool::Raise, CHILD_CELL, 2, 3)?)?;
    let highest = live
        .session
        .runtime()
        .bodies()
        .bodies()
        .map(|tracked| tracked.body)
        .max_by_key(|body| body.index())
        .ok_or("no body to remove")?;
    live.run(Command::RemoveBody { body: highest })?;
    live.run(ambient(CHILD_AMBIENT_C)?)?;
    faults.arm(barrier, Action::Abort, 1);
    live.save()?;
    Ok(())
}

/// Opens the file at `path` again under `epoch`: a new store object and a new session, with
/// nothing but the file connecting them to the last.
fn reopen_story(
    adapter: &str,
    path: &Path,
    epoch: u32,
    viewer: Option<world::SharedViewer>,
    scratch: Scratch,
) -> Result<Story, String> {
    let store = reopen_file(path)?;
    let faults = store.faults().clone();
    let live = world::open_viewed(
        adapter,
        std::sync::Arc::clone(&store),
        SessionEpoch::new(epoch),
        trace_config(),
        |_| {},
        viewer,
    )?;
    Ok(Story {
        live,
        store,
        faults,
        path: path.to_path_buf(),
        id: 0,
        stroke: None,
        _scratch: scratch,
    })
}

/// Closes the story's session in order and lets its store go, keeping what the next session
/// needs: the viewer, the scratch directory and the close's report.
fn close_story(
    story: Story,
) -> (
    world_runtime::session::CloseReport,
    world_model::resume::BodySet,
    Option<world::SharedViewer>,
    Scratch,
) {
    let Story {
        live,
        store,
        _scratch,
        ..
    } = story;
    let viewer = live.viewer.clone();
    live.detach();
    let report = live.session.close(Duration::from_secs(60));
    let bodies = store.read_bodies(report.latest.id).unwrap_or_default();
    drop(store);
    (report, bodies, viewer, _scratch)
}

/// Part 4: an orderly close, then a child process changing the world and dying inside its save
/// before the publication commits and after it; each reopen finds the checkpoint the boundary
/// names, with its bodies and configuration. A last orderly close and a fresh reopen end it.
pub(super) fn interruptions(
    adapter: &str,
    mut story: Story,
    probes: &Probes,
    check: &mut Check,
    captures: Option<&Path>,
) -> Result<(), String> {
    let untouched = story.column(CHILD_CELL)?;
    let path = story.path.clone();
    let (report, bodies, mut viewer, mut scratch) = close_story(story);
    check.that(
        report.saved && report.workers_joined && !report.timed_out,
        || format!("the orderly close: {report:?}"),
    );
    let mut expected = (report.latest, bodies);
    let mut epoch = 5;
    for (point, published) in [("before-publish", false), ("after-publish", true)] {
        let output = interrupt(
            adapter,
            &path,
            point,
            epoch,
            &path.with_file_name(format!("child-{point}.json")),
        )?;
        check.that(!output.status.success(), || {
            format!(
                "{point}: the child was not interrupted: {}",
                String::from_utf8_lossy(&output.stdout)
            )
        });
        let mut story = reopen_story(adapter, &path, epoch + 1, viewer, scratch)?;
        let start = story.session().start();
        check.that(start.previous_unclean, || {
            format!("{point}: the interrupted session was not recorded unclean")
        });
        check.metric(
            &format!("{point}_reclaimed_rows"),
            start.reclaimed.chunk_rows + start.reclaimed.body_sets,
        );
        let latest = story.store.latest().map_err(|error| error.to_string())?;
        let latest_bodies = story
            .store
            .read_bodies(latest.id)
            .map_err(|error| error.to_string())?;
        let (previous, previous_bodies) = &expected;
        if published {
            check.equal(
                &format!("{point}: the child's checkpoint is the latest"),
                Some(latest.id),
                previous.id.next(),
            );
            check.equal(
                &format!("{point}: the child's air temperature"),
                latest.config.ambient_temperature,
                AmbientTemperature::celsius(CHILD_AMBIENT_C)
                    .map_err(|error| format!("{error:?}"))?,
            );
            let removed = previous_bodies
                .bodies
                .iter()
                .map(|record| record.handle)
                .max_by_key(|handle| handle.index());
            check.equal(
                &format!("{point}: bodies after the child's removal"),
                latest_bodies.live() + 1,
                previous_bodies.live(),
            );
            check.that(
                latest_bodies
                    .bodies
                    .iter()
                    .all(|record| Some(record.handle) != removed),
                || format!("{point}: the body the child removed is back"),
            );
        } else {
            check.equal(
                &format!("{point}: the checkpoint before the interruption"),
                latest.id,
                previous.id,
            );
            check.equal(&format!("{point}: bodies"), &latest_bodies, previous_bodies);
        }
        // Saved again before any tick, the reopened world is that checkpoint.
        let saved = story.save_first_boundary()?;
        check_restored(check, point, &story, &saved, &latest, &latest_bodies)?;
        story.live.settle()?;
        let raised = story.column(CHILD_CELL)? != untouched;
        check.that(raised == published, || {
            format!(
                "{point}: the child's raised column is {} after the interruption",
                if published { "missing" } else { "present" }
            )
        });
        let columns = story.columns(&probes.cells)?;
        compare(check, point, probes, &columns);
        check.equal(
            &format!("{point}: bodies reopened"),
            story.session().runtime().bodies().bodies().count(),
            latest_bodies.live(),
        );
        let (report, bodies, kept_viewer, kept_scratch) = close_story(story);
        check.that(report.saved && !report.timed_out, || {
            format!("{point}: the close: {report:?}")
        });
        expected = (report.latest, bodies);
        viewer = kept_viewer;
        scratch = kept_scratch;
        epoch += 2;
    }

    // A fresh reopen of the last orderly close.
    let mut story = reopen_story(adapter, &path, epoch, viewer, scratch)?;
    check.that(!story.session().start().previous_unclean, || {
        "the last orderly close left the store unclean".to_owned()
    });
    let (closed, closed_bodies) = &expected;
    check.equal(
        "the fresh reopen's checkpoint",
        story.store.latest().map_err(|error| error.to_string())?.id,
        closed.id,
    );
    let saved = story.save_first_boundary()?;
    check_restored(check, "fresh reopen", &story, &saved, closed, closed_bodies)?;
    story.live.settle()?;
    let columns = story.columns(&probes.cells)?;
    compare(check, "fresh reopen", probes, &columns);
    check.that(story.column(CHILD_CELL)? != untouched, || {
        "the published child's change is gone after the fresh reopen".to_owned()
    });
    check.equal(
        "bodies after the fresh reopen",
        story.session().runtime().bodies().bodies().count(),
        closed_bodies.live(),
    );
    show_step(Some(&mut story), "adversarial-recovery", captures, check);
    let (report, ..) = close_story(story);
    check.that(report.saved, || format!("the last close: {report:?}"));
    Ok(())
}
