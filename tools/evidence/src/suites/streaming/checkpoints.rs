//! Checkpoints through the real owners: saves of resident changes, periodic saves, a snapshot
//! draining while the window resizes, failed commits, bounded retention, delayed mapping, and
//! bodies suspended at the residency edge.

use std::time::Duration;

use world_model::{
    body::{BodyHandle, BodyPreset},
    command::{BodySpawn, Command, EditTool},
    glam::IVec2,
    rules::SimulationRules,
};
use world_runtime::{
    checkpoint::{SaveFailure, SaveReason, SaveState},
    ids::SessionEpoch,
    port::{CommandResult, LimitedResource, Refusal},
    residency::{ResidencyConfig, ResidencyPressure},
    session::SessionConfig,
    store::{ChunkPayload, WorldStore},
};
use world_store::faults::{Action, Barrier, StoreFaults};

use super::{
    super::Check,
    residency::stroke,
    world::{self, Live, Scratch, open, open_store, session_config, window},
};

fn close(check: &mut Check, live: Live) {
    let report = live.session.close(Duration::from_secs(30));
    check.that(report.saved, || format!("close did not save: {report:?}"));
}

/// Changes to chunks that never leave the window are saved by autosave alone, and a reopened
/// world holds them.
pub fn resident_only_autosave(adapter: &str, check: &mut Check) -> Result<(), String> {
    let scratch = Scratch::new("autosave")?;
    let path = scratch.path("world.redb");
    world::create(&path, window(0, 0, 1), SimulationRules::NONE)?;
    let store = open_store(&path, StoreFaults::new())?;
    let mut config = session_config();
    config.checkpoints.autosave_ticks = Some(40);
    let mut live = open(adapter, store.clone(), SessionEpoch::FIRST, config, |_| {})?;
    live.settle()?;
    let cells = [IVec2::new(10, 10), IVec2::new(-30, 50), IVec2::new(60, -60)];
    let mut expected = Vec::new();
    for cell in cells {
        stroke(&mut live, EditTool::Raise, cell, 1)?;
        expected.push((cell, live.column(cell)?.ok_or("not resident")?));
    }
    live.until("autosave", |session| {
        session.checkpoints().receipt().is_some_and(|receipt| {
            receipt.reason == SaveReason::Autosave && receipt.committed().is_some()
        })
    })?;
    check.equal(
        "no chunk left the window",
        live.session.residency().summary().snapshots_changed
            + live.session.residency().summary().snapshots_unchanged,
        9,
    );
    let latest = store.latest().map_err(|error| error.to_string())?;
    check.equal("saved keys", latest.chunks, 3);
    check.that(latest.tick.is_some(), || {
        "the autosave holds no tick".to_owned()
    });
    // End without the closing save: the autosave alone must hold the changes.
    let ended = live.session.abandon(Duration::from_secs(10));
    check.that(ended.workers_joined && !ended.saved, || {
        format!("{ended:?}")
    });
    drop(store);
    let store = open_store(&path, StoreFaults::new())?;
    let mut live = open(
        adapter,
        store,
        SessionEpoch::new(2),
        session_config(),
        |_| {},
    )?;
    check.that(live.session.start().previous_unclean, || {
        "a session that never closed was not recorded as unclean".to_owned()
    });
    live.settle()?;
    for (cell, column) in expected {
        check.equal(
            &format!("autosaved {cell}"),
            live.column(cell)?,
            Some(column),
        );
    }
    close(check, live);
    Ok(())
}

/// Periodic saves follow one another with increasing identities and ticks, each one complete,
/// and the world reopens at the latest.
pub fn periodic_saves(adapter: &str, check: &mut Check) -> Result<(), String> {
    let scratch = Scratch::new("periodic")?;
    let path = scratch.path("world.redb");
    world::create(&path, window(0, 0, 1), SimulationRules::NONE)?;
    let store = open_store(&path, StoreFaults::new())?;
    let mut config = session_config();
    config.checkpoints.autosave_ticks = Some(25);
    let mut live = open(adapter, store.clone(), SessionEpoch::FIRST, config, |_| {})?;
    live.settle()?;
    let cell = IVec2::new(-20, -20);
    let mut seen = Vec::new();
    for round in 0..3u32 {
        stroke(&mut live, EditTool::Raise, cell, 1)?;
        let before = live.session.checkpoints().committed();
        live.until("the next autosave", |session| {
            session.checkpoints().committed() > before
        })?;
        let latest = store.latest().map_err(|error| error.to_string())?;
        check.that(store.read_bodies(latest.id).is_ok(), || {
            format!("round {round}: no body set")
        });
        seen.push((latest.id.value(), latest.tick.map(|tick| tick.value())));
    }
    check.that(
        seen.windows(2)
            .all(|pair| pair[1].0 > pair[0].0 && pair[1].1 > pair[0].1),
        || format!("checkpoints did not advance: {seen:?}"),
    );
    let last = live.column(cell)?;
    close(check, live);
    drop(store);
    let store = open_store(&path, StoreFaults::new())?;
    let mut live = open(
        adapter,
        store,
        SessionEpoch::new(2),
        session_config(),
        |_| {},
    )?;
    live.settle()?;
    check.equal("the latest save reopens", live.column(cell)?, last);
    close(check, live);
    Ok(())
}

/// A world snapshot drains while the window grows: both finish, the checkpoint holds its tick's
/// state, and nothing resident is lost. A growth beyond the device budget is refused before
/// anything is evicted, and the world stays as it was.
pub fn snapshot_resize_overlap(adapter: &str, check: &mut Check) -> Result<(), String> {
    let scratch = Scratch::new("overlap")?;
    let path = scratch.path("world.redb");
    world::create(&path, window(0, 0, 1), SimulationRules::NONE)?;
    let store = open_store(&path, StoreFaults::new())?;
    // A budget that holds the growth from radius 1 to 2 with its overlap (the old allocations,
    // a world snapshot of radius 1 draining, the new window), and not the one from 2 to 4.
    let plan = |radius| {
        gpu_world::plan::plan(
            window(0, 0, radius),
            gpu_world::plan::TransferConfig {
                bodies: 64,
                ..Default::default()
            },
            false,
            &unlimited(),
            &gpu_world::plan::Budgets::default(),
        )
        .map_err(|error| format!("{error:?}"))
    };
    let (one, two, four) = (plan(1)?, plan(2)?, plan(4)?);
    let budget = one.overlap_with(&two).max(two.reserved_bytes());
    check.that(two.overlap_with(&four) > budget, || {
        "the budget would admit radius 4".to_owned()
    });
    let mut live = open(
        adapter,
        store.clone(),
        SessionEpoch::FIRST,
        session_config(),
        |config| {
            config.budgets.device_bytes = Some(budget);
        },
    )?;
    live.settle()?;
    let cells = [IVec2::new(5, 5), IVec2::new(-60, 70)];
    let mut expected = Vec::new();
    for cell in cells {
        stroke(&mut live, EditTool::Raise, cell, 2)?;
        expected.push((cell, live.column(cell)?.ok_or("not resident")?));
    }
    live.session.save();
    live.session.set_window(window(0, 0, 2));
    live.until("save and growth", |session| {
        session.residency().window() == window(0, 0, 2)
            && session.residency().settled()
            && session
                .checkpoints()
                .receipt()
                .is_some_and(|receipt| receipt.finished())
    })?;
    check.that(
        live.session
            .checkpoints()
            .receipt()
            .is_some_and(|receipt| receipt.committed().is_some()),
        || format!("{:?}", live.session.checkpoints().receipt()),
    );
    let counters = live.session.runtime().counters();
    check.metric("peak_requested_bytes", counters.peak_requested_bytes);
    check.metric("budget_bytes", budget);
    check.that(counters.peak_requested_bytes <= budget, || {
        format!(
            "{} above the budget {budget}",
            counters.peak_requested_bytes
        )
    });
    for (cell, column) in &expected {
        check.equal(
            &format!("{cell} after the growth"),
            live.column(*cell)?,
            Some(*column),
        );
    }

    // Beyond the budget: refused up front, nothing evicted, the radius-2 world intact.
    let evictions = live.session.residency().summary().snapshots_changed
        + live.session.residency().summary().snapshots_unchanged;
    live.session.set_window(window(0, 0, 4));
    live.frames(10);
    let refused = live.session.residency().refused_move();
    check.that(
        matches!(
            refused,
            Some(world_runtime::residency::RefusedMove {
                refusal: Refusal::Unsupported(limit),
                ..
            }) if limit.resource == LimitedResource::DeviceBytes
        ),
        || format!("the growth was not refused for the budget: {refused:?}"),
    );
    check.equal(
        "window after the refusal",
        live.session.residency().window(),
        window(0, 0, 2),
    );
    check.equal(
        "nothing was evicted for the refused growth",
        live.session.residency().summary().snapshots_changed
            + live.session.residency().summary().snapshots_unchanged,
        evictions,
    );
    for (cell, column) in &expected {
        check.equal(
            &format!("{cell} after the refusal"),
            live.column(*cell)?,
            Some(*column),
        );
    }
    close(check, live);
    drop(store);
    let store = open_store(&path, StoreFaults::new())?;
    let mut live = open(
        adapter,
        store,
        SessionEpoch::new(2),
        session_config(),
        |_| {},
    )?;
    live.settle()?;
    for (cell, column) in &expected {
        check.equal(
            &format!("{cell} reopened"),
            live.column(*cell)?,
            Some(*column),
        );
    }
    close(check, live);
    Ok(())
}

fn unlimited() -> gpu_world::plan::PlanLimits {
    gpu_world::plan::PlanLimits {
        max_buffer_size: u64::MAX,
        max_storage_binding: u64::MAX,
        max_uniform_binding: u64::MAX,
        min_uniform_offset_alignment: 256,
        max_workgroups_per_dimension: u32::MAX,
        max_invocations_per_workgroup: u32::MAX,
        max_workgroup_size_x: u32::MAX,
        max_workgroup_size_y: u32::MAX,
        max_storage_buffers_per_stage: u32::MAX,
    }
}

/// Real I/O errors under redb fail eviction writes and a publication; every payload stays
/// retained, the receipt shows the failure, the retries succeed and the reopened world holds
/// every change.
pub fn failed_commits_retained(adapter: &str, check: &mut Check) -> Result<(), String> {
    let scratch = Scratch::new("failed")?;
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
    let west = IVec2::new(-40, 0);
    let centre = IVec2::new(30, 30);
    stroke(&mut live, EditTool::Raise, west, 2)?;
    stroke(&mut live, EditTool::Raise, centre, 2)?;
    let west_column = live.column(west)?;
    let centre_column = live.column(centre)?;
    // Evictions' writes fail inside redb's commit, repeatedly.
    faults.fail_writes(3);
    live.move_to(window(1, 0, 1))?;
    live.until("the writes failed", |session| {
        session.residency().summary().write_failures >= 1
    })?;
    check.that(
        live.session.residency().summary().retained_payloads >= 1,
        || "a failed write dropped its payload".to_owned(),
    );
    live.until("the writes recovered", |session| {
        !session.residency().busy()
    })?;
    // The publication's sync fails: an ambiguous commit, retried.
    faults.fail_syncs(1);
    live.session.save();
    let mut saw_failure = false;
    live.until("the save", |session| {
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
        receipt.is_some_and(|receipt| receipt.committed().is_some())
    })?;
    check.that(saw_failure, || {
        "the failed publication was never reported".to_owned()
    });
    let summary = *live.session.residency().summary();
    check.metric("write_failures", summary.write_failures);
    check.metric("retained_high_water_bytes", summary.retained_high_water);
    let (_, _, injected) = faults.io_counts();
    check.that(injected >= 2, || {
        format!("only {injected} faults were injected")
    });
    close(check, live);
    drop(store);
    let store = open_store(&path, StoreFaults::new())?;
    let mut live = open(
        adapter,
        store,
        SessionEpoch::new(2),
        session_config(),
        |_| {},
    )?;
    live.move_to(window(0, 0, 1))?;
    check.equal("the evicted change", live.column(west)?, west_column);
    check.equal("the resident change", live.column(centre)?, centre_column);
    close(check, live);
    Ok(())
}

/// With writes failing and room for two retained payloads, evictions stop and the window waits,
/// visibly; memory stays at the budget; once writes succeed the window moves and every change is
/// saved. A snapshot image of two entries limits evictions per frame the same way.
pub fn retention_budget(adapter: &str, check: &mut Check) -> Result<(), String> {
    let scratch = Scratch::new("budget")?;
    let path = scratch.path("world.redb");
    world::create(&path, window(0, 0, 1), SimulationRules::NONE)?;
    let faults = StoreFaults::new();
    let store = open_store(&path, faults.clone())?;
    let config = SessionConfig {
        residency: ResidencyConfig {
            retained_bytes: 2 * ChunkPayload::BYTES,
            ..session_config().residency
        },
        ..session_config()
    };
    let mut live = open(adapter, store.clone(), SessionEpoch::FIRST, config, |gpu| {
        gpu.transfer.snapshot_entries = 2;
    })?;
    live.settle()?;
    let west = [
        IVec2::new(-30, -30),
        IVec2::new(-30, 30),
        IVec2::new(-30, 100),
    ];
    let mut expected = Vec::new();
    for cell in west {
        stroke(&mut live, EditTool::Raise, cell, 1)?;
        expected.push((cell, live.column(cell)?));
    }
    faults.arm(Barrier::BeforeChunkCommit, Action::Fail, u32::MAX);
    live.session.set_window(window(1, 0, 1));
    let mut pressure = None;
    let mut high_water = 0;
    for _ in 0..80 {
        live.frames(1);
        let summary = *live.session.residency().summary();
        pressure = pressure.or(summary
            .pressure
            .filter(|p| *p == ResidencyPressure::Retained));
        high_water = high_water.max(summary.retained_bytes);
    }
    check.equal("pressure", pressure, Some(ResidencyPressure::Retained));
    check.equal(
        "the window waited",
        live.session.residency().window(),
        window(0, 0, 1),
    );
    check.that(high_water <= 2 * ChunkPayload::BYTES, || {
        format!("{high_water} bytes retained against a budget of two payloads")
    });
    check.metric("retained_high_water_bytes", high_water);
    // Clear the failures: the window moves and everything is saved.
    faults.disarm(Barrier::BeforeChunkCommit);
    live.until("move", |session| {
        session.residency().window() == window(1, 0, 1) && !session.residency().busy()
    })?;
    live.save()?;
    close(check, live);
    drop(store);
    let store = open_store(&path, StoreFaults::new())?;
    let mut live = open(
        adapter,
        store,
        SessionEpoch::new(2),
        session_config(),
        |_| {},
    )?;
    live.move_to(window(0, 0, 1))?;
    for (cell, column) in expected {
        check.equal(
            &format!("{cell} saved after the pressure"),
            live.column(cell)?,
            column,
        );
    }
    close(check, live);
    Ok(())
}

/// Map deliveries held while eviction and world snapshots are copied: the copies sit in their
/// device images, nothing is taken early, the outgoing chunks cannot be loaded back before their
/// snapshots resolve, and when deliveries resume everything completes with the right content.
pub fn delayed_mapping(adapter: &str, check: &mut Check) -> Result<(), String> {
    let scratch = Scratch::new("delayed")?;
    let path = scratch.path("world.redb");
    world::create(&path, window(0, 0, 1), SimulationRules::NONE)?;
    let store = open_store(&path, StoreFaults::new())?;
    let mut live = open(
        adapter,
        store,
        SessionEpoch::FIRST,
        session_config(),
        |_| {},
    )?;
    live.settle()?;
    let west = IVec2::new(-20, 20);
    let centre = IVec2::new(20, 20);
    stroke(&mut live, EditTool::Raise, west, 2)?;
    stroke(&mut live, EditTool::Raise, centre, 2)?;
    let (west_column, centre_column) = (live.column(west)?, live.column(centre)?);
    live.faults.hold_deliveries(true);
    live.session.save();
    live.session.set_window(window(1, 0, 1));
    live.frames(30);
    let capturing = live
        .session
        .checkpoints()
        .receipt()
        .is_some_and(|receipt| matches!(receipt.state, SaveState::Capturing { .. }));
    check.that(capturing, || {
        format!(
            "the save got ahead of held deliveries: {:?}",
            live.session.checkpoints().receipt()
        )
    });
    let held = *live.session.residency().summary();
    check.metric("evicting_while_held", held.evicting);
    check.that(held.evicting >= 3, || {
        format!("the west chunks' snapshots resolved while their deliveries were held: {held:?}")
    });
    // Back to the original window while their snapshots are held: the west chunks wait for
    // them rather than reload anything older.
    live.session.set_window(window(0, 0, 1));
    live.frames(30);
    check.that(
        !live
            .session
            .residency()
            .is_resident(world_model::spatial::ChunkKey::new(-1, 0, 0)),
        || "a chunk came back before its outgoing snapshot resolved".to_owned(),
    );
    live.faults.hold_deliveries(false);
    live.until("everything after the hold", |session| {
        session.residency().window() == window(0, 0, 1)
            && session.residency().settled()
            && !session.residency().busy()
            && session
                .checkpoints()
                .receipt()
                .is_some_and(|receipt| receipt.committed().is_some())
    })?;
    check.equal("west after the hold", live.column(west)?, west_column);
    check.equal("centre after the hold", live.column(centre)?, centre_column);
    close(check, live);
    Ok(())
}

/// A body whose region leaves the window is suspended, not removed, and is saved suspended with
/// its handle; the reopened world holds it once, still suspended, and resumes it when its terrain
/// is resident again.
pub fn body_suspension_resume(adapter: &str, check: &mut Check) -> Result<(), String> {
    let scratch = Scratch::new("bodies")?;
    let path = scratch.path("world.redb");
    world::create(&path, window(0, 0, 1), SimulationRules::NONE)?;
    let store = open_store(&path, StoreFaults::new())?;
    let mut live = open(
        adapter,
        store.clone(),
        SessionEpoch::FIRST,
        session_config(),
        |_| {},
    )?;
    live.settle()?;
    let spawn = BodySpawn::new(BodyPreset::Metal, IVec2::new(-32, 32), 6.0)
        .map_err(|error| error.to_string())?;
    let body = match live.run(Command::SpawnBody(spawn))? {
        CommandResult::BodySpawned { body, .. } => body,
        other => return Err(format!("spawn: {other:?}")),
    };
    live.session.runtime_mut().set_controlled(Some(body));
    live.frames(90);
    let sample = |live: &Live, body: BodyHandle| {
        live.session
            .runtime()
            .bodies()
            .body(body)
            .and_then(|tracked| tracked.last)
    };
    let resting = sample(&live, body).ok_or("no sample")?;
    check.that(!resting.suspended && resting.grounded, || {
        format!("the body did not land: {resting:?}")
    });
    // Its chunk leaves the window: the body is frozen where it was.
    live.move_to(window(2, 0, 1))?;
    live.frames(10);
    let frozen = sample(&live, body).ok_or("no sample")?;
    live.frames(20);
    let later = sample(&live, body).ok_or("no sample")?;
    check.that(frozen.suspended && later.suspended, || {
        format!("not suspended: {frozen:?}")
    });
    check.that(later.tick > frozen.tick, || {
        "the body was not sampled again".to_owned()
    });
    check.equal("frozen in place", later.position, frozen.position);
    check.that(frozen.position.distance(resting.position) < 1.0, || {
        format!("moved while leaving: {resting:?} -> {frozen:?}")
    });
    live.save()?;
    let bodies = store
        .read_bodies(store.latest().map_err(|error| error.to_string())?.id)
        .map_err(|error| error.to_string())?;
    check.equal("saved bodies", bodies.live(), 1);
    check.that(
        bodies
            .bodies
            .first()
            .is_some_and(|saved| saved.handle == body && saved.suspended),
        || format!("saved: {:?}", bodies.bodies.first()),
    );
    close(check, live);
    drop(store);

    let store = open_store(&path, StoreFaults::new())?;
    let mut live = open(
        adapter,
        store,
        SessionEpoch::new(2),
        session_config(),
        |_| {},
    )?;
    live.session.runtime_mut().set_controlled(Some(body));
    live.settle()?;
    live.frames(5);
    let reopened = sample(&live, body).ok_or("the restored body was never sampled")?;
    check.that(reopened.suspended, || {
        format!("not suspended after reopening: {reopened:?}")
    });
    check.equal("restored in place", reopened.position, frozen.position);
    check.equal(
        "one body after reopening",
        live.session.runtime().bodies().bodies().count(),
        1,
    );
    live.move_to(window(0, 0, 1))?;
    live.frames(10);
    let resumed = sample(&live, body).ok_or("no sample")?;
    check.that(!resumed.suspended, || format!("not resumed: {resumed:?}"));
    let jump = live.run(Command::JumpBody { body })?;
    check.equal(
        "the resumed body answers its handle",
        jump,
        CommandResult::Jumped { body, jumped: true },
    );
    close(check, live);
    Ok(())
}
