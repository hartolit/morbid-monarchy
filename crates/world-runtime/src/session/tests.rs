//! The session's owners against the model port and the in-memory store (`crate::testing`):
//! their rules, not durability or GPU execution.

use std::{sync::Arc, time::Duration};

use world_model::{
    body::{BodyHandle, BodyPreset},
    command::AmbientTemperature,
    glam::{IVec2, Vec2, Vec3},
    material::FluidMat,
    observation::BodyCounters,
    resume::{BodyResume, BodySet, BodySlot, GroundPlane, SlotState},
    spatial::ChunkKey,
};

use super::*;
use crate::{
    checkpoint::SaveState,
    ids::ChunkRevision,
    residency::ResidencyPressure,
    store::ChunkPayload,
    testing::{MemoryStore, ModelPort, cells, seed},
};

fn window(x: i32) -> ResidentWindow {
    ResidentWindow::new(ChunkKey::new(x, 0, 0), 1, 1).expect("window")
}

fn open_with(
    config: SessionConfig,
    prepare: impl FnOnce(&MemoryStore),
) -> (WorldSession, ModelPort, Arc<MemoryStore>) {
    let store = MemoryStore::new(window(0));
    prepare(&store);
    let port = ModelPort::new(window(0));
    let plan = SessionPlan::prepare(&*store).expect("plan");
    let session = WorldSession::open(
        Box::new(port.clone()),
        Arc::clone(&store) as Arc<dyn WorldStore>,
        plan,
        config,
    )
    .expect("open");
    (session, port, store)
}

fn quiet() -> SessionConfig {
    SessionConfig {
        checkpoints: CheckpointConfig {
            autosave_ticks: None,
            retry_frames: 2,
        },
        residency: ResidencyConfig {
            retry_frames: 2,
            ..ResidencyConfig::default()
        },
        ..SessionConfig::default()
    }
}

/// Runs frames of one tick until `done` holds.
fn until(session: &mut WorldSession, what: &str, mut done: impl FnMut(&WorldSession) -> bool) {
    for _ in 0..2000 {
        session.frame_ticks(1);
        if done(session) {
            return;
        }
        std::thread::sleep(Duration::from_micros(200));
    }
    panic!("{what}: not done; {:?}", session.residency().summary());
}

fn key(x: i32, y: i32) -> ChunkKey {
    ChunkKey::new(x, y, 0)
}

#[test]
fn only_absent_keys_are_generated_and_unreadable_ones_stay_out() {
    let (mut session, port, store) = open_with(quiet(), |store| {
        seed(store, key(0, 0), 3, 33);
        store.state().corrupt.push(key(1, 0));
    });
    until(&mut session, "settle", |session| {
        session.residency().settled()
    });
    let model = port.model();
    let (revision, stored) = model.resident(key(0, 0)).expect("resident");
    assert_eq!((revision, stored), (ChunkRevision::new(3), cells(33)));
    assert!(
        model.resident(key(1, 0)).is_none(),
        "corrupt rows are never generated over"
    );
    assert_eq!(
        model.resident(key(-1, 1)).map(|(r, _)| r),
        Some(ChunkRevision::default())
    );
    drop(model);
    let failed: Vec<ChunkKey> = session.residency().failed().map(|(key, _)| key).collect();
    assert_eq!(failed, vec![key(1, 0)]);
    let summary = *session.residency().summary();
    assert_eq!((summary.loads_stored, summary.loads_generated), (1, 7));
    assert_eq!(store.state().chunks.len(), 1, "nothing was written");
}

#[test]
fn a_chunk_that_returns_before_its_save_finishes_comes_back_from_the_retained_payload() {
    let (mut session, port, store) = open_with(quiet(), |_| {});
    until(&mut session, "settle", |session| {
        session.residency().settled()
    });
    assert!(port.model().edit(key(-1, 0), 77));
    store.state().hold_writes = true;
    session.set_window(window(2));
    until(&mut session, "move away", |session| {
        session.residency().window() == window(2) && session.residency().settled()
    });
    assert!(port.model().resident(key(-1, 0)).is_none());
    assert_eq!(
        session.residency().newest(key(-1, 0)),
        Some((ChunkRevision::new(1), false))
    );
    session.set_window(window(0));
    until(&mut session, "move back", |session| {
        session.residency().window() == window(0) && session.residency().settled()
    });
    assert_eq!(
        port.model().resident(key(-1, 0)),
        Some((ChunkRevision::new(1), cells(77))),
        "the newest retained revision wins over the absent row on disk"
    );
    assert!(session.residency().summary().loads_from_retained >= 1);
    assert!(store.state().chunks.is_empty(), "the save is still held");
    store.release();
    until(&mut session, "write", |session| !session.residency().busy());
    let state = store.state();
    assert_eq!(state.chunks.get(&(key(-1, 0), 1)), Some(&cells(77)));
    assert_eq!(state.chunks.len(), 1, "only the changed chunk was saved");
}

#[test]
fn loads_answered_after_the_key_left_are_discarded() {
    let (mut session, port, store) = open_with(quiet(), |store| {
        seed(store, key(0, 0), 2, 20);
    });
    store.state().hold_reads = true;
    session.frame_ticks(1);
    session.set_window(window(9));
    session.frame_ticks(1);
    session.set_window(window(0));
    session.frame_ticks(1);
    store.release();
    until(&mut session, "settle", |session| {
        session.residency().settled()
    });
    assert!(
        session.residency().summary().stale_loads > 0,
        "{:?}",
        session.residency().summary()
    );
    assert_eq!(
        port.model().resident(key(0, 0)),
        Some((ChunkRevision::new(2), cells(20)))
    );
}

#[test]
fn failed_writes_keep_their_payloads_and_full_retention_stops_the_window() {
    let config = SessionConfig {
        residency: ResidencyConfig {
            retained_bytes: 2 * ChunkPayload::BYTES,
            retry_frames: 2,
            ..ResidencyConfig::default()
        },
        ..quiet()
    };
    let (mut session, port, store) = open_with(config, |_| {});
    until(&mut session, "settle", |session| {
        session.residency().settled()
    });
    for y in -1..=1 {
        assert!(port.model().edit(key(-1, y), (100 + y) as u64));
    }
    store.state().fail_writes = u32::MAX;
    session.set_window(window(1));
    for _ in 0..50 {
        session.frame_ticks(1);
    }
    let summary = *session.residency().summary();
    assert_eq!(summary.pressure, Some(ResidencyPressure::Retained));
    assert_eq!(session.residency().window(), window(0), "the window waits");
    assert!(summary.write_failures > 0);
    assert_eq!(summary.retained_payloads, 2);
    assert!(
        port.model().resident(key(-1, 1)).is_some() || port.model().resident(key(-1, -1)).is_some(),
        "a chunk that could not be kept was not evicted"
    );
    store.state().fail_writes = 0;
    until(&mut session, "move", |session| {
        session.residency().window() == window(1) && !session.residency().busy()
    });
    let state = store.state();
    for y in -1..=1 {
        assert_eq!(
            state.chunks.get(&(key(-1, y), 1)),
            Some(&cells((100 + y) as u64)),
            "every retained payload was written after the failures"
        );
    }
}

#[test]
fn a_refused_retirement_keeps_the_chunk_resident_and_records_its_snapshot() {
    let (mut session, port, store) = open_with(quiet(), |_| {});
    until(&mut session, "settle", |session| {
        session.residency().settled()
    });
    // The outgoing chunks are evicted in row order: (-1, -1) is tried first.
    port.model().edit(key(-1, -1), 5);
    port.model().refuse_retire = true;
    session.set_window(window(1));
    for _ in 0..20 {
        session.frame_ticks(1);
    }
    assert!(
        port.model().resident(key(-1, -1)).is_some(),
        "still resident"
    );
    assert_eq!(session.residency().window(), window(0));
    assert_eq!(
        session.residency().newest(key(-1, -1)).map(|(r, _)| r),
        Some(ChunkRevision::new(1)),
        "the snapshot taken for the refused eviction is a revision"
    );
    port.model().edit(key(-1, -1), 6);
    port.model().refuse_retire = false;
    until(&mut session, "move", |session| {
        session.residency().window() == window(1) && !session.residency().busy()
    });
    let state = store.state();
    assert_eq!(state.chunks.get(&(key(-1, -1), 1)), Some(&cells(5)));
    assert_eq!(
        state.chunks.get(&(key(-1, -1), 2)),
        Some(&cells(6)),
        "a change after the refused eviction's snapshot is the next revision"
    );
    assert_eq!(state.chunks.len(), 2, "unchanged snapshots wrote nothing");
}

fn body() -> BodySet {
    BodySet {
        slots: vec![BodySlot {
            generation: 2,
            state: SlotState::Live,
        }],
        bodies: vec![BodyResume {
            handle: BodyHandle::new(0, 2),
            description: BodyPreset::Wood.description(),
            anchor: IVec2::new(0, 0),
            local: Vec3::new(10.0, 30.0, 10.0),
            velocity: Vec3::ZERO,
            grounded: true,
            suspended: false,
            plane_valid: false,
            terrain_contact: false,
            ground_normal: Vec3::Y,
            submerged: 0.0,
            previous_submerged: 0.0,
            pending_wake: 0.0,
            plane: GroundPlane {
                origin: Vec2::ZERO,
                height: 0.0,
                slope: Vec2::ZERO,
            },
            counters: BodyCounters::default(),
            touching: 0,
            liquid: FluidMat::EMPTY,
            terrain_normal: 0,
        }],
    }
}

fn committed(session: &WorldSession) -> bool {
    session
        .checkpoints()
        .receipt()
        .is_some_and(|receipt| receipt.committed().is_some())
}

#[test]
fn a_checkpoint_holds_resident_evicted_body_and_configuration_state_at_one_tick() {
    let (mut session, port, store) = open_with(quiet(), |_| {});
    until(&mut session, "settle", |session| {
        session.residency().settled()
    });
    port.model().edit(key(-1, 0), 22);
    session.set_window(window(1));
    until(&mut session, "move", |session| {
        session.residency().window() == window(1) && session.residency().settled()
    });
    port.model().edit(key(0, 0), 11);
    port.model().bodies = body();
    port.model().config.ambient_temperature = AmbientTemperature::celsius(-5.0).expect("ok");
    session.save();
    until(&mut session, "save", committed);
    let state = store.state();
    let latest = *state.manifests.last().expect("manifest");
    assert_eq!(latest.id.value(), 2);
    let tick = port.model().next_tick.value() - 1;
    assert!(latest.tick.is_some_and(|saved| saved.value() <= tick));
    assert_eq!(latest.window, window(1));
    assert_eq!(latest.config.ambient_temperature.degrees(), -5.0);
    assert_eq!(latest.bodies, 1);
    assert_eq!(state.bodies.get(&2), Some(&body()));
    assert_eq!(state.heads.get(&key(0, 0)), Some(&ChunkRevision::new(1)));
    assert_eq!(state.heads.get(&key(-1, 0)), Some(&ChunkRevision::new(1)));
    assert_eq!(
        state.heads.len(),
        2,
        "unchanged generated chunks move no head"
    );
    assert_eq!(state.chunks.get(&(key(0, 0), 1)), Some(&cells(11)));
    drop(state);

    // A second checkpoint moves only what changed since.
    port.model().edit(key(0, 0), 12);
    session.save();
    until(&mut session, "second save", |session| {
        session.checkpoints().latest().id.value() == 3
    });
    let state = store.state();
    assert_eq!(state.heads.get(&key(0, 0)), Some(&ChunkRevision::new(2)));
    assert_eq!(state.heads.get(&key(-1, 0)), Some(&ChunkRevision::new(1)));
}

#[test]
fn publication_waits_for_every_payload_and_retries_failed_store_operations() {
    let (mut session, port, store) = open_with(quiet(), |_| {});
    until(&mut session, "settle", |session| {
        session.residency().settled()
    });
    port.model().edit(key(1, 1), 9);
    store.state().hold_writes = true;
    session.save();
    for _ in 0..20 {
        session.frame_ticks(1);
    }
    assert!(matches!(
        session
            .checkpoints()
            .receipt()
            .map(|receipt| &receipt.state),
        Some(SaveState::Captured { .. })
    ));
    assert_eq!(
        store.state().manifests.len(),
        1,
        "nothing published before the payload is durable"
    );
    store.release();
    until(&mut session, "save", committed);

    port.model().edit(key(1, 1), 10);
    store.state().fail_publishes = 2;
    session.save();
    let mut saw_failure = false;
    until(&mut session, "retried save", |session| {
        let receipt = session.checkpoints().receipt().expect("receipt");
        saw_failure |= matches!(receipt.state, SaveState::Failed { retrying: true, .. });
        receipt.committed().is_some() && receipt.attempts == 2
    });
    assert!(saw_failure);
    assert_eq!(
        store.state().heads.get(&key(1, 1)),
        Some(&ChunkRevision::new(2))
    );
}

#[test]
fn an_orderly_close_saves_the_last_completed_tick() {
    let (mut session, port, store) = open_with(quiet(), |_| {});
    until(&mut session, "settle", |session| {
        session.residency().settled()
    });
    port.model().edit(key(0, 1), 4);
    session.frame_ticks(1);
    // The frame's time is accounted for by owner, the runtime's share first.
    let times = session.frame_times();
    assert!(times.runtime > Duration::ZERO, "{times:?}");
    assert_eq!(
        times.total(),
        times.runtime + times.residency + times.checkpoints + times.transfers
    );
    let last = port.model().next_tick.value() - 1;
    let report = session.close(Duration::from_secs(10));
    assert!(report.saved, "{report:?}");
    assert!(!report.timed_out && report.workers_joined);
    assert_eq!(report.latest.tick, Some(Tick::new(last)));
    assert_eq!(
        store.state().heads.get(&key(0, 1)),
        Some(&ChunkRevision::new(1))
    );
}

/// Writes held for a moment, from another thread, while a close waits.
fn release_later(store: &Arc<MemoryStore>) -> std::thread::JoinHandle<()> {
    let store = Arc::clone(store);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        store.release();
    })
}

#[test]
fn a_close_after_an_earlier_save_saves_its_own_last_tick() {
    let (mut session, port, store) = open_with(quiet(), |_| {});
    until(&mut session, "settle", |session| {
        session.residency().settled()
    });
    port.model().edit(key(0, 1), 4);
    store.state().hold_writes = true;
    session.save();
    session.frame_ticks(1);
    assert!(session.checkpoints().busy());
    // A change after that save's boundary, which only the close's own save can hold.
    for _ in 0..4 {
        session.frame_ticks(1);
    }
    port.model().edit(key(0, 1), 5);
    session.frame_ticks(1);
    let last = port.model().next_tick.value() - 1;
    let releaser = release_later(&store);
    let report = session.close(Duration::from_secs(10));
    releaser.join().expect("releaser");
    assert!(report.saved, "{report:?}");
    assert_eq!(report.latest.tick, Some(Tick::new(last)));
    assert_eq!(
        report.latest.id.value(),
        3,
        "the earlier save, then the close's"
    );
    let state = store.state();
    assert_eq!(state.heads.get(&key(0, 1)), Some(&ChunkRevision::new(2)));
    assert_eq!(state.chunks.get(&(key(0, 1), 2)), Some(&cells(5)));
}

#[test]
fn a_close_during_an_autosave_of_its_last_tick_is_saved() {
    let mut config = quiet();
    config.checkpoints.autosave_ticks = Some(8);
    let (mut session, port, store) = open_with(config, |_| {});
    until(&mut session, "settle", |session| {
        session.residency().settled()
    });
    // Autosaves run while the window settles. Let the last of them finish, so the autosave
    // awaited below is one that starts after the edit: an earlier one still in flight holds an
    // older tick, and the close would rightly save again.
    until(&mut session, "no save in progress", |session| {
        !session.checkpoints().busy()
    });
    port.model().edit(key(-1, -1), 6);
    store.state().hold_writes = true;
    until(&mut session, "an autosave", |session| {
        session.checkpoints().busy()
            && session
                .checkpoints()
                .receipt()
                .is_some_and(|receipt| receipt.reason == SaveReason::Autosave)
    });
    let last = port.model().next_tick.value() - 1;
    let latest = session.checkpoints().latest().id.value();
    let releaser = release_later(&store);
    let report = session.close(Duration::from_secs(10));
    releaser.join().expect("releaser");
    assert!(report.saved, "{report:?}");
    assert_eq!(report.latest.tick, Some(Tick::new(last)));
    assert_eq!(
        report.latest.id.value(),
        latest + 1,
        "the autosave already holds the last tick"
    );
    assert_eq!(
        store.state().heads.get(&key(-1, -1)),
        Some(&ChunkRevision::new(1))
    );
}

#[test]
fn a_lost_device_reports_what_the_latest_checkpoint_does_not_hold() {
    let (mut session, port, store) = open_with(quiet(), |_| {});
    until(&mut session, "settle", |session| {
        session.residency().settled()
    });
    session.save();
    until(&mut session, "save", committed);
    let saved = session.checkpoints().latest().tick.expect("tick");
    port.model().edit(key(0, 0), 3);
    for _ in 0..10 {
        session.frame_ticks(1);
    }
    port.model().lose_device();
    session.frame_ticks(1);
    let loss = session.loss().expect("loss");
    assert_eq!(loss.last_complete.value(), 2);
    assert_eq!(loss.last_complete_tick, Some(saved));
    assert!(loss.ticks_lost >= 10);
    assert_eq!(loss.resident_chunks, 9);
    let (_, loss) = session.finish_after_loss(Duration::from_secs(5));
    assert!(!loss.finished_captured_save);
    assert_eq!(
        store.state().manifests.len(),
        2,
        "no checkpoint was invented"
    );
}

#[test]
fn a_save_captured_before_a_loss_is_still_published() {
    let (mut session, port, store) = open_with(quiet(), |_| {});
    until(&mut session, "settle", |session| {
        session.residency().settled()
    });
    port.model().edit(key(-1, -1), 8);
    store.state().hold_writes = true;
    session.save();
    until(&mut session, "captured", |session| {
        session.checkpoints().captured()
    });
    port.model().lose_device();
    session.frame_ticks(1);
    store.release();
    let (_, loss) = session.finish_after_loss(Duration::from_secs(5));
    assert!(loss.finished_captured_save, "{loss:?}");
    assert_eq!(loss.last_complete.value(), 2);
    assert_eq!(
        store.state().heads.get(&key(-1, -1)),
        Some(&ChunkRevision::new(1))
    );
}
