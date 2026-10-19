//! Checkpoints: one consistent saved world at a completed tick, published atomically.
//!
//! A save, whether requested, periodic or part of closing, goes through the same owner:
//!
//! ```text
//! requested -> capturing (a world snapshot holds the end of tick T)
//!           -> captured (every chunk's revision at T is known, and the bodies)
//!           -> committed (its revisions and body set durable, then published) | failed
//! ```
//!
//! The world snapshot copies every resident chunk, its change word and every body on the
//! device at the end of one submission, so the checkpoint is the world after one completed tick
//! even though the copies stream out over later frames. A chunk that is not resident at T has
//! not changed since it left (unloaded regions are frozen); its newest revision is what the
//! residency owner holds, and a chunk whose eviction snapshot is still outstanding is waited for.
//! Its payloads become durable through the residency owner's writes; the checkpoint writes its
//! body set and publishes only when every revision it names is durable. A failed write or
//! publication is retried, and every payload it needs stays retained until it succeeds.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use world_model::{resume::BodySet, spatial::ChunkKey};

use crate::{
    ids::{ChunkRevision, SessionEpoch, Tick},
    ledger::ResidencyOutcome,
    port::{AbandonReason, Refusal, ResidentWindow, WorldSnapshotTicket},
    residency::Residency,
    runtime::{ResidencyError, Runtime},
    store::{CheckpointId, CheckpointManifest, PhysicalConfig, StoreError},
    workers::{Done, PublishJob, Workers},
};

/// Why a save was requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveReason {
    Requested,
    Autosave,
    /// The final checkpoint of an orderly close.
    Close,
}

/// A save request's identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SaveId(u64);

impl SaveId {
    pub fn value(self) -> u64 {
        self.0
    }
}

/// Why a save failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveFailure {
    /// The live world gave up a snapshot (device loss, stop): what it held is gone.
    Abandoned(AbandonReason),
    /// The live world refused the snapshot.
    Refused(Refusal),
    /// The store failed a write or the publication. The payloads are retained and the save is
    /// retried.
    Store(StoreError),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SaveState {
    Requested,
    /// The world snapshot is copied or streaming; `tick` once it is known.
    Capturing {
        tick: Option<Tick>,
    },
    /// Every revision at `tick` is known; waiting for durability and publication.
    Captured {
        tick: Option<Tick>,
    },
    Committed {
        checkpoint: CheckpointId,
        tick: Option<Tick>,
    },
    /// `retrying`: the payloads are retained and the save tries again; otherwise it is over.
    Failed {
        failure: SaveFailure,
        retrying: bool,
    },
}

/// What a save reached, kept until the next save replaces it.
#[derive(Debug, Clone, PartialEq)]
pub struct SaveReceipt {
    pub id: SaveId,
    pub reason: SaveReason,
    pub state: SaveState,
    /// Store operations that failed and were retried.
    pub attempts: u32,
    /// Chunk revisions the checkpoint names, and live bodies, once captured.
    pub chunks: usize,
    pub bodies: u32,
}

impl SaveReceipt {
    pub fn committed(&self) -> Option<CheckpointId> {
        match self.state {
            SaveState::Committed { checkpoint, .. } => Some(checkpoint),
            _ => None,
        }
    }

    pub fn finished(&self) -> bool {
        matches!(
            self.state,
            SaveState::Committed { .. }
                | SaveState::Failed {
                    retrying: false,
                    ..
                }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Collecting,
    WritingBodies,
    WaitingDurable,
    Publishing,
    Retry { at: u64, publish: bool },
}

struct InProgress {
    id: SaveId,
    ticket: WorldSnapshotTicket,
    tick: Option<Tick>,
    /// The revision the checkpoint holds for each key that has changed since its published one.
    heads: HashMap<ChunkKey, ChunkRevision>,
    expected_chunks: Option<u32>,
    reported_chunks: u32,
    evictions: HashSet<ChunkKey>,
    bodies: Option<Arc<BodySet>>,
    config: Option<PhysicalConfig>,
    window: ResidentWindow,
    epoch: SessionEpoch,
    checkpoint: CheckpointId,
    step: Step,
}

/// Bounds of the owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointConfig {
    /// Ticks between periodic saves; `None` disables them.
    pub autosave_ticks: Option<u64>,
    /// Frames between retries of a failed store operation.
    pub retry_frames: u32,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            autosave_ticks: Some(60 * 60),
            retry_frames: 30,
        }
    }
}

pub struct Checkpoints {
    config: CheckpointConfig,
    latest: CheckpointManifest,
    next_save: u64,
    requested: Option<(SaveId, SaveReason)>,
    current: Option<InProgress>,
    receipt: Option<SaveReceipt>,
    frame: u64,
    committed: u64,
    failures: u64,
}

impl Checkpoints {
    /// The owner of a session that opened `latest`.
    pub fn new(latest: CheckpointManifest, config: CheckpointConfig) -> Self {
        Self {
            config,
            latest,
            next_save: 1,
            requested: None,
            current: None,
            receipt: None,
            frame: 0,
            committed: 0,
            failures: 0,
        }
    }

    /// Ticks between periodic saves, or `None` when they are off.
    pub fn autosave_ticks(&self) -> Option<u64> {
        self.config.autosave_ticks
    }

    /// Changes the periodic save interval from the next frame on; `None` turns it off.
    pub fn set_autosave(&mut self, ticks: Option<u64>) {
        self.config.autosave_ticks = ticks.filter(|ticks| *ticks > 0);
    }

    /// The latest published checkpoint: what a reopened world, or a recovery, starts from.
    pub fn latest(&self) -> &CheckpointManifest {
        &self.latest
    }

    /// The latest save's receipt.
    pub fn receipt(&self) -> Option<&SaveReceipt> {
        self.receipt.as_ref()
    }

    pub fn committed(&self) -> u64 {
        self.committed
    }

    pub fn store_failures(&self) -> u64 {
        self.failures
    }

    /// Whether a save is waiting to start or in progress.
    pub fn busy(&self) -> bool {
        self.requested.is_some() || self.current.is_some()
    }

    /// Whether the save in progress has every payload on the CPU: it no longer needs the live
    /// world and can finish after a device loss.
    pub fn captured(&self) -> bool {
        self.current
            .as_ref()
            .is_some_and(|current| current.step != Step::Collecting)
    }

    /// Asks for a save. A save already waiting or in progress answers for this one.
    pub fn request(&mut self, reason: SaveReason) -> SaveId {
        if let Some((id, _)) = self.requested {
            return id;
        }
        if let Some(current) = &self.current {
            return current.id;
        }
        let id = SaveId(self.next_save);
        self.next_save += 1;
        self.requested = Some((id, reason));
        self.receipt = Some(SaveReceipt {
            id,
            reason,
            state: SaveState::Requested,
            attempts: 0,
            chunks: 0,
            bodies: 0,
        });
        id
    }

    fn set_state(&mut self, state: SaveState) {
        if let Some(receipt) = &mut self.receipt {
            receipt.state = state;
        }
    }

    /// Runs one frame, after the residency owner's: starts a requested save, routes its world
    /// snapshot, and moves it toward publication.
    pub fn frame(
        &mut self,
        runtime: &mut Runtime,
        residency: &mut Residency,
        workers: &mut Workers,
        outcomes: &[ResidencyOutcome],
        done: &mut Vec<Done>,
    ) {
        self.frame += 1;
        self.autosave(runtime);
        self.start(runtime, residency);
        self.collect(runtime, residency, outcomes);
        let finished = std::mem::take(done);
        for job in finished {
            self.on_done(job, residency, done);
        }
        self.advance(residency, workers);
    }

    fn autosave(&mut self, runtime: &Runtime) {
        let Some(interval) = self.config.autosave_ticks else {
            return;
        };
        let Some(last) = runtime.last_submitted_tick() else {
            return;
        };
        let since = self
            .latest
            .tick
            .map_or(last.value() + 1, |tick| last.since(tick));
        if since >= interval && !self.busy() {
            self.request(SaveReason::Autosave);
        }
    }

    fn start(&mut self, runtime: &mut Runtime, residency: &Residency) {
        let Some((id, reason)) = self.requested else {
            return;
        };
        let Some(checkpoint) = self.latest.id.next() else {
            return;
        };
        let ticket = match runtime.snapshot_world() {
            Ok(ticket) => ticket,
            Err(ResidencyError::OutcomesFull | ResidencyError::Refused(Refusal::Pressure(_))) => {
                return;
            }
            Err(ResidencyError::Refused(refusal)) => {
                self.requested = None;
                self.failures += 1;
                self.set_state(SaveState::Failed {
                    failure: SaveFailure::Refused(refusal),
                    retrying: false,
                });
                return;
            }
        };
        self.requested = None;
        // Chunks that are not resident at the snapshot's boundary hold their newest revision;
        // the snapshot reports every resident one, which replaces the entry below.
        let heads = residency.unpublished().collect();
        let evictions = residency.evicting_keys().collect();
        self.current = Some(InProgress {
            id,
            ticket,
            tick: None,
            heads,
            expected_chunks: None,
            reported_chunks: 0,
            evictions,
            bodies: None,
            config: None,
            window: residency.window(),
            epoch: runtime.session().epoch,
            checkpoint,
            step: Step::Collecting,
        });
        if let Some(receipt) = &mut self.receipt {
            receipt.reason = reason;
        }
        self.set_state(SaveState::Capturing { tick: None });
    }

    fn collect(
        &mut self,
        runtime: &mut Runtime,
        residency: &mut Residency,
        outcomes: &[ResidencyOutcome],
    ) {
        let mut resolved = Vec::new();
        residency.take_resolved_evictions(&mut resolved);
        let Some(current) = &mut self.current else {
            return;
        };
        for (key, revision) in resolved {
            if current.evictions.remove(&key) {
                current.heads.insert(key, revision);
            }
        }
        let mut failed = None;
        for outcome in outcomes {
            match *outcome {
                ResidencyOutcome::Snapshot {
                    ticket,
                    key,
                    tick,
                    changed,
                    ..
                } if ticket.world() == Some(current.ticket) => {
                    current.tick = tick;
                    current.reported_chunks += 1;
                    let payload = if changed {
                        let mut payload = residency.spare_payload();
                        match runtime.take_snapshot(ticket, payload.cells_mut()) {
                            Ok(()) => Some(payload),
                            Err(refusal) => {
                                residency.return_payload(payload);
                                failed = Some(SaveFailure::Refused(refusal));
                                continue;
                            }
                        }
                    } else {
                        None
                    };
                    if let Some(revision) = residency.world_snapshot_chunk(key, payload) {
                        current.heads.insert(key, revision);
                    }
                }
                ResidencyOutcome::BodySnapshot {
                    ticket,
                    tick,
                    chunks,
                    config,
                    ..
                } if ticket == current.ticket => {
                    let mut bodies = BodySet::default();
                    match runtime.take_body_snapshot(ticket, &mut bodies) {
                        Ok(()) => {
                            current.bodies = Some(Arc::new(bodies));
                            current.config = Some(config);
                            current.expected_chunks = Some(chunks);
                            current.tick = tick;
                        }
                        Err(refusal) => failed = Some(SaveFailure::Refused(refusal)),
                    }
                }
                ResidencyOutcome::BodySnapshot { .. } => {}
                ResidencyOutcome::SnapshotAbandoned { ticket, reason }
                    if ticket.world() == Some(current.ticket) =>
                {
                    failed = Some(SaveFailure::Abandoned(reason));
                }
                ResidencyOutcome::BodySnapshotAbandoned { ticket, reason }
                    if ticket == current.ticket =>
                {
                    failed = Some(SaveFailure::Abandoned(reason));
                }
                _ => {}
            }
        }
        if let Some(failure) = failed {
            self.failures += 1;
            self.current = None;
            self.set_state(SaveState::Failed {
                failure,
                retrying: false,
            });
        }
    }

    fn on_done(&mut self, done: Done, residency: &mut Residency, keep: &mut Vec<Done>) {
        let Some(current) = &mut self.current else {
            keep.push(done);
            return;
        };
        match done {
            Done::BodiesWritten { checkpoint, result } if checkpoint == current.checkpoint => {
                match result {
                    Ok(()) => current.step = Step::WaitingDurable,
                    Err(error) => {
                        current.step = Step::Retry {
                            at: self.frame + u64::from(self.config.retry_frames),
                            publish: false,
                        };
                        self.failures += 1;
                        if let Some(receipt) = &mut self.receipt {
                            receipt.attempts += 1;
                            receipt.state = SaveState::Failed {
                                failure: SaveFailure::Store(error),
                                retrying: true,
                            };
                        }
                    }
                }
            }
            Done::Published { checkpoint, result } if checkpoint == current.checkpoint => {
                match result {
                    Ok(manifest) => {
                        let heads: Vec<(ChunkKey, ChunkRevision)> =
                            current.heads.iter().map(|(k, r)| (*k, *r)).collect();
                        residency.published(&heads);
                        self.latest = manifest;
                        self.committed += 1;
                        let tick = current.tick;
                        self.current = None;
                        self.set_state(SaveState::Committed { checkpoint, tick });
                    }
                    Err(error) => {
                        current.step = Step::Retry {
                            at: self.frame + u64::from(self.config.retry_frames),
                            publish: true,
                        };
                        self.failures += 1;
                        if let Some(receipt) = &mut self.receipt {
                            receipt.attempts += 1;
                            receipt.state = SaveState::Failed {
                                failure: SaveFailure::Store(error),
                                retrying: true,
                            };
                        }
                    }
                }
            }
            other => keep.push(other),
        }
    }

    fn advance(&mut self, residency: &Residency, workers: &mut Workers) {
        let frame = self.frame;
        let Some(current) = &mut self.current else {
            return;
        };
        if current.step == Step::Collecting {
            // The bodies arrive with the number of chunks the snapshot holds; it is captured
            // when every one of them reported and every outstanding eviction resolved.
            let complete = current.bodies.is_some()
                && current.config.is_some()
                && current
                    .expected_chunks
                    .is_some_and(|expected| current.reported_chunks >= expected)
                && current.evictions.is_empty();
            if !complete {
                return;
            }
            // Only revisions newer than the published ones move a head; a resident chunk that
            // did not change since it was loaded from its head, or a generated one that never
            // changed (revision 0), moves nothing.
            current.heads.retain(|key, revision| {
                revision.value() > 0
                    && residency
                        .published_revision(*key)
                        .is_none_or(|published| *revision > published)
            });
            let tick = current.tick;
            let chunks = current.heads.len();
            let bodies = current.bodies.as_ref().map_or(0, |set| set.live() as u32);
            if let Some(receipt) = &mut self.receipt {
                receipt.state = SaveState::Captured { tick };
                receipt.chunks = chunks;
                receipt.bodies = bodies;
            }
            current.step = Step::WritingBodies;
            if let Some(bodies) = &current.bodies
                && !workers.write_bodies(current.checkpoint, Arc::clone(bodies))
            {
                current.step = Step::Retry {
                    at: frame + 1,
                    publish: false,
                };
            }
            return;
        }
        if let Step::Retry { at, publish } = current.step {
            if frame < at {
                return;
            }
            if publish {
                current.step = Step::WaitingDurable;
            } else {
                current.step = Step::WritingBodies;
                if let Some(bodies) = &current.bodies
                    && !workers.write_bodies(current.checkpoint, Arc::clone(bodies))
                {
                    current.step = Step::Retry {
                        at: frame + 1,
                        publish: false,
                    };
                }
                return;
            }
        }
        if current.step == Step::WaitingDurable {
            let durable = current
                .heads
                .iter()
                .all(|(key, revision)| residency.is_durable(*key, *revision));
            if !durable {
                return;
            }
            let (Some(config), Some(bodies)) = (current.config, current.bodies.as_ref()) else {
                return;
            };
            let job = PublishJob {
                id: current.checkpoint,
                tick: current.tick,
                config,
                window: current.window,
                epoch: current.epoch,
                heads: current.heads.iter().map(|(k, r)| (*k, *r)).collect(),
                bodies: bodies.live() as u32,
            };
            current.step = match workers.publish(job) {
                Ok(()) => Step::Publishing,
                Err(_) => Step::Retry {
                    at: frame + 1,
                    publish: true,
                },
            };
        }
    }

    /// Ends a save that can no longer finish (the session is closing without it): its receipt
    /// says it failed. Its payloads stay where the residency owner keeps them.
    pub fn give_up(&mut self, failure: SaveFailure) {
        if self.current.take().is_some() || self.requested.take().is_some() {
            self.set_state(SaveState::Failed {
                failure,
                retrying: false,
            });
        }
    }

    /// Retries a failed store operation now (an orderly close does not wait for the backoff).
    pub fn retry_now(&mut self) {
        if let Some(current) = &mut self.current
            && let Step::Retry { publish, .. } = current.step
        {
            current.step = Step::Retry { at: 0, publish };
        }
    }
}
