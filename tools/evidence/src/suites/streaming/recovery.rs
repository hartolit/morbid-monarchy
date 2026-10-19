//! Closing, losing and recovering a live world through the real owners: an orderly close with
//! work in flight and with one that cannot finish in time, device loss before and after a save's
//! payloads reached the CPU, a process interrupted inside a save, and the headless session
//! consumer saving and reopening a store.

use std::{
    process::Command as Process,
    time::{Duration, Instant},
};

use world_model::{
    command::{Command, EditTool},
    glam::IVec2,
    rules::SimulationRules,
    spatial::ChunkKey,
};
use world_runtime::{
    checkpoint::{SaveFailure, SaveState},
    ids::{SessionEpoch, Tick},
    port::{AbandonReason, Refusal},
    store::WorldStore,
};
use world_store::faults::{Action, Barrier, StoreFaults};

use crate::cli::{Autosave, SessionEdit, SessionRequest};

use super::{
    super::{Check, allow_software, session},
    residency::stroke,
    world::{self, Live, Scratch, open, open_store, session_config, window},
};

/// An orderly close with a command queued for a later tick, ticks in flight and a save held at
/// its barrier: it waits for the held save, saves the last completed tick, reports the command
/// abandoned, and the reopened world holds everything. A close whose save cannot finish within
/// its time limit says so and leaves the previous checkpoint; the next session reclaims what it
/// left unpublished.
pub fn orderly_shutdown(adapter: &str, check: &mut Check) -> Result<(), String> {
    let scratch = Scratch::new("shutdown")?;
    let path = scratch.path("world.redb");
    world::create(&path, window(0, 0, 1), SimulationRules::NONE)?;
    let faults = StoreFaults::new();
    let store = open_store(&path, faults.clone())?;
    let mut live = open(
        adapter,
        store.clone(),
        SessionEpoch::FIRST,
        session_config(),
        |_| {},
    )?;
    live.settle()?;
    let west = IVec2::new(-30, 10);
    let centre = IVec2::new(30, 30);
    stroke(&mut live, EditTool::Raise, west, 2)?;
    stroke(&mut live, EditTool::Raise, centre, 2)?;
    let (west_column, centre_column) = (live.column(west)?, live.column(centre)?);
    // A command for a tick the session will never reach.
    let later = live
        .session
        .runtime()
        .last_submitted_tick()
        .map_or(0, Tick::value)
        + 10_000;
    live.session
        .runtime_mut()
        .submit_at(
            world_runtime::ids::CommandId::new(9_999),
            Command::QueryColumn { cell: centre },
            Tick::new(later),
        )
        .map_err(|error| format!("{error:?}"))?;
    faults.arm(
        Barrier::BeforeChunkCommit,
        Action::Hold {
            limit: Duration::from_secs(60),
        },
        1,
    );
    live.move_to(window(1, 0, 1))?;
    live.until("the eviction's save is held", |_| {
        faults.holding() == Some(Barrier::BeforeChunkCommit)
    })?;
    let releaser = {
        let faults = faults.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            faults.release();
        })
    };
    let last = live.session.runtime().last_submitted_tick();
    let report = live.session.close(Duration::from_secs(30));
    releaser.join().map_err(|_| "releaser panicked")?;
    check.that(
        report.saved && !report.timed_out && report.workers_joined,
        || format!("{report:?}"),
    );
    check.equal("abandoned commands", report.abandoned_commands, 1);
    check.that(report.waited >= Duration::from_millis(300), || {
        format!(
            "the close did not wait for the held save: {:?}",
            report.waited
        )
    });
    check.equal("closing checkpoint tick", report.latest.tick, last);
    check.metric("close_waited_ms", report.waited.as_secs_f64() * 1000.0);
    drop(store);
    let store = open_store(&path, faults.clone())?;
    let mut live = open(
        adapter,
        store.clone(),
        SessionEpoch::new(2),
        session_config(),
        |_| {},
    )?;
    check.that(!live.session.start().previous_unclean, || {
        "an orderly close left the store unclean".to_owned()
    });
    live.move_to(window(0, 0, 1))?;
    check.equal("west after the close", live.column(west)?, west_column);
    check.equal(
        "centre after the close",
        live.column(centre)?,
        centre_column,
    );

    // A close that cannot finish: its save is held past the time limit.
    let previous = store.latest().map_err(|error| error.to_string())?;
    stroke(&mut live, EditTool::Lower, centre, 2)?;
    faults.arm(
        Barrier::BeforeBodiesCommit,
        Action::Hold {
            limit: Duration::from_secs(60),
        },
        1,
    );
    let report = live.session.close(Duration::from_secs(2));
    check.that(report.timed_out && !report.saved, || format!("{report:?}"));
    check.equal(
        "the latest checkpoint after the failed close",
        report.latest.id,
        previous.id,
    );
    faults.release();
    // The held writer finishes and lets the file go.
    let started = Instant::now();
    drop(store);
    let store = loop {
        match open_store(&path, StoreFaults::new()) {
            Ok(store) => break store,
            Err(_) if started.elapsed() < Duration::from_secs(20) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error),
        }
    };
    let mut live = open(
        adapter,
        store,
        SessionEpoch::new(3),
        session_config(),
        |_| {},
    )?;
    check.that(live.session.start().previous_unclean, || {
        "a close that did not save left the store marked clean".to_owned()
    });
    check.metric(
        "reclaimed_after_failed_close",
        live.session.start().reclaimed.chunk_rows + live.session.start().reclaimed.body_sets,
    );
    live.settle()?;
    check.equal(
        "the unsaved change is gone, as reported",
        live.column(centre)?,
        centre_column,
    );
    let report = live.session.close(Duration::from_secs(30));
    check.that(report.saved, || format!("{report:?}"));
    Ok(())
}

/// A device lost after a save's world snapshot was copied but before its payloads reached the
/// CPU: the save fails as abandoned, the loss report names the latest complete checkpoint and
/// what came after it, and a new session (a later epoch, refusing the old one's slots) recovers
/// exactly that checkpoint. Lost after every payload reached the CPU, the save is still
/// published and the recovered world holds it.
pub fn loss_and_recovery(adapter: &str, check: &mut Check) -> Result<(), String> {
    let scratch = Scratch::new("loss")?;
    let path = scratch.path("world.redb");
    world::create(&path, window(0, 0, 1), SimulationRules::NONE)?;
    let faults = StoreFaults::new();
    let store = open_store(&path, faults.clone())?;
    let a = IVec2::new(10, 10);
    let b = IVec2::new(-40, -40);
    let c = IVec2::new(40, -20);

    // Before readback.
    let mut live = open(
        adapter,
        store.clone(),
        SessionEpoch::FIRST,
        session_config(),
        |_| {},
    )?;
    live.settle()?;
    let original_b = live.column(b)?;
    stroke(&mut live, EditTool::Raise, a, 2)?;
    let a_column = live.column(a)?;
    let saved = live.save()?;
    stroke(&mut live, EditTool::Raise, b, 2)?;
    let stale_slot = live
        .session
        .residency()
        .slot(ChunkKey::new(0, 0, 0))
        .ok_or("no slot")?;
    live.faults.hold_deliveries(true);
    live.session.save();
    live.frames(10);
    live.faults.lose_device();
    live.until("the loss", |session| session.loss().is_some())?;
    let loss = live.session.loss().ok_or("no loss report")?;
    check.equal("loss reason", loss.reason, AbandonReason::DeviceLost);
    check.equal("the latest complete checkpoint", loss.last_complete, saved);
    check.that(loss.ticks_lost > 0 && loss.resident_chunks == 9, || {
        format!("{loss:?}")
    });
    let receipt = live.session.checkpoints().receipt().cloned();
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
    let (_, loss) = live.session.finish_after_loss(Duration::from_secs(10));
    check.that(!loss.finished_captured_save, || format!("{loss:?}"));
    check.metric("ticks_lost", loss.ticks_lost);

    let mut live = open(
        adapter,
        store.clone(),
        SessionEpoch::new(2),
        session_config(),
        |_| {},
    )?;
    live.settle()?;
    check.equal("recovered a", live.column(a)?, a_column);
    check.equal(
        "b, changed after the checkpoint, is lost",
        live.column(b)?,
        original_b,
    );
    check.that(
        matches!(
            live.session.runtime_mut().retire_slot(stale_slot),
            Err(Refusal::StaleSession { .. })
        ),
        || "the lost session's slot was accepted by the new one".to_owned(),
    );

    // After capture: every payload is on the CPU when the device goes.
    stroke(&mut live, EditTool::Raise, c, 2)?;
    let c_column = live.column(c)?;
    faults.arm(
        Barrier::BeforeBodiesCommit,
        Action::Hold {
            limit: Duration::from_secs(60),
        },
        1,
    );
    live.session.save();
    live.until("captured", |session| session.checkpoints().captured())?;
    live.faults.lose_device();
    live.until("the loss", |session| session.loss().is_some())?;
    faults.release();
    let (_, loss) = live.session.finish_after_loss(Duration::from_secs(20));
    check.that(loss.finished_captured_save, || format!("{loss:?}"));
    let latest = store.latest().map_err(|error| error.to_string())?;
    check.equal(
        "the captured save was published",
        latest.id,
        loss.last_complete,
    );
    let mut live = open(
        adapter,
        store.clone(),
        SessionEpoch::new(3),
        session_config(),
        |_| {},
    )?;
    live.settle()?;
    check.equal("recovered c", live.column(c)?, c_column);
    let report = live.session.close(Duration::from_secs(30));
    check.that(report.saved, || format!("{report:?}"));
    Ok(())
}

const CHILD: &str = "MORBID_STREAMING_INTERRUPT";

/// A live session's process ends inside a save, at a defined barrier: before the publication
/// commits the reopened world is the previous checkpoint, after it the new one, never a mix, and
/// the next session reclaims what the interrupted one left.
pub fn process_interruption(adapter: &str, check: &mut Check) -> Result<(), String> {
    if let Ok(spec) = std::env::var(CHILD) {
        return interruption_child(adapter, &spec);
    }
    for (point, published) in [("before-publish", false), ("after-publish", true)] {
        let scratch = Scratch::new(&format!("interrupt-{point}"))?;
        let path = scratch.path("world.redb");
        world::create(&path, window(0, 0, 1), SimulationRules::NONE)?;
        let mut child = Process::new(std::env::current_exe().map_err(|error| error.to_string())?);
        child.args([
            "--suite",
            "streaming",
            "--case",
            "process-interruption",
            "--adapter",
            adapter,
            "--result",
            scratch.path("child.json").to_str().ok_or("path")?,
        ]);
        child.env(CHILD, format!("{}|{point}", path.display()));
        if allow_software() {
            child.arg("--allow-software");
        }
        let output = child.output().map_err(|error| error.to_string())?;
        check.that(!output.status.success(), || {
            format!(
                "{point}: the child was not interrupted: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        let store = open_store(&path, StoreFaults::new())?;
        let latest = store.latest().map_err(|error| error.to_string())?;
        check.equal(
            &format!("{point}: the checkpoint after the interruption"),
            latest.id.value(),
            if published { 2 } else { 1 },
        );
        let mut live = open(
            adapter,
            store,
            SessionEpoch::new(2),
            session_config(),
            |_| {},
        )?;
        check.that(live.session.start().previous_unclean, || {
            format!("{point}: not recorded unclean")
        });
        live.settle()?;
        let column = live.column(IVec2::new(20, 20))?.ok_or("not resident")?;
        let generated = world_model::generation::WorldGenerator::new(world::SEED)
            .generate_chunk(ChunkKey::new(0, 0, 0))
            .cells[20 * 64 + 20];
        check.that((column != generated) == published, || {
            format!(
                "{point}: the edit is {} after the interruption",
                if published { "missing" } else { "present" }
            )
        });
        let report = live.session.close(Duration::from_secs(30));
        check.that(report.saved, || format!("{point}: {report:?}"));
    }
    Ok(())
}

fn interruption_child(adapter: &str, spec: &str) -> Result<(), String> {
    let (path, point) = spec.split_once('|').ok_or("bad child spec")?;
    let faults = StoreFaults::new();
    let store = open_store(std::path::Path::new(path), faults.clone())?;
    let mut live: Live = open(
        adapter,
        store,
        SessionEpoch::FIRST,
        session_config(),
        |_| {},
    )?;
    live.settle()?;
    stroke(&mut live, EditTool::Raise, IVec2::new(20, 20), 2)?;
    let barrier = match point {
        "before-publish" => Barrier::BeforePublishCommit,
        _ => Barrier::AfterPublishCommit,
    };
    faults.arm(barrier, Action::Abort, 1);
    live.save()?;
    // Reached only if the barrier never was: the parent fails the case.
    Ok(())
}

/// The headless session consumer (`cargo evidence session`) over a real store: its close saves
/// its edit, a second run reopens that checkpoint under the next epoch after a clean close, and
/// the old writer's save given as its store is refused unchanged.
pub fn headless_session(adapter: &str, check: &mut Check) -> Result<(), String> {
    let scratch = Scratch::new("session")?;
    let path = scratch.path("world.redb");
    world::create(&path, window(0, 0, 1), SimulationRules::NONE)?;
    let cell = IVec2::new(20, 20);
    let request = SessionRequest {
        store: path.clone(),
        adapter: adapter.to_owned(),
        ticks: 30,
        window: None,
        edits: vec![SessionEdit {
            tool: EditTool::Raise,
            cell: (cell.x, cell.y),
        }],
        autosave: Autosave::Off,
    };
    let first = session::run(&request);
    check.that(first.passed(), || format!("the first run: {first:?}"));
    let metric = |result: &session::SessionResult, step: &str, key: &str| {
        result
            .steps
            .iter()
            .find(|found| found.name == step)
            .and_then(|found| found.metrics.get(key).cloned())
    };
    let second = session::run(&SessionRequest {
        edits: Vec::new(),
        ..request.clone()
    });
    check.that(second.passed(), || format!("the second run: {second:?}"));
    check.equal(
        "the second run opens the first run's close",
        metric(&second, "open", "checkpoint"),
        metric(&first, "close", "checkpoint"),
    );
    check.equal(
        "the second run's epoch",
        metric(&second, "open", "epoch"),
        Some(3.into()),
    );
    check.equal(
        "the first run closed cleanly",
        metric(&second, "open", "previous_unclean"),
        Some(false.into()),
    );
    let store = open_store(&path, StoreFaults::new())?;
    let mut live = open(
        adapter,
        store,
        SessionEpoch::new(4),
        session_config(),
        |_| {},
    )?;
    live.settle()?;
    let generated = world_model::generation::WorldGenerator::new(world::SEED)
        .generate_chunk(ChunkKey::new(0, 0, 0))
        .cells[20 * 64 + 20];
    let column = live.column(cell)?.ok_or("not resident")?;
    check.that(column != generated, || {
        "the session's edit was not saved".to_owned()
    });
    let report = live.session.close(Duration::from_secs(30));
    check.that(report.saved, || format!("{report:?}"));

    let legacy = scratch.path("save.redb");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../adapters/world-store/tests/fixtures/legacy-v0/legacy-chunks.redb"),
        &legacy,
    )
    .map_err(|error| format!("copy the legacy fixture: {error}"))?;
    let before = std::fs::read(&legacy).map_err(|error| error.to_string())?;
    let refused = session::run(&SessionRequest {
        store: legacy.clone(),
        edits: Vec::new(),
        ..request
    });
    check.that(
        !refused.passed()
            && refused.steps.len() == 1
            && refused.steps[0]
                .failures
                .iter()
                .any(|failure| failure.contains("not a saved world")),
        || format!("the legacy save as a store: {refused:?}"),
    );
    check.equal(
        "the legacy save is unchanged",
        std::fs::read(&legacy).map_err(|error| error.to_string())? == before,
        true,
    );
    Ok(())
}
