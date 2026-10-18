//! Residency: which chunks are on the GPU, where every chunk's newest content is, and moving the
//! resident window without losing a change.
//!
//! The owner keeps metadata, never a resident chunk's cells. Per absolute key it knows a phase
//!
//! ```text
//! idle -> loading (request r) -> loaded (payload) -> uploading (slot) -> resident (slot)
//! resident -> evicting (snapshot requested, slot retired) -> idle
//! loading -> failed (corrupt, unsupported, unreadable: never generated, stays out)
//! ```
//!
//! and where its newest content is: a retained payload not yet durable, a durable revision this
//! session wrote, the published revision, or generated terrain. A load uses the newest of
//! these, so a chunk that left and comes back before its save finished is uploaded from the
//! retained payload, never from an older row on disk; only a key with no saved revision at all
//! is generated, and only after the store said so.
//!
//! A load result is applied only to the request it answers: one for a key that left the window,
//! or was requested again since, is discarded. An outgoing chunk is snapshotted on the device
//! and its slot retired in the same frame, so nothing changes between the two; the snapshot
//! says whether the chunk changed since it was uploaded or last snapshotted, and only a changed
//! one becomes a new revision, retained until it is durable. The window moves (a GPU resize)
//! once no chunk outside the new window holds a slot.
//!
//! Every queue is bounded by bytes or counts. Under pressure the owner stops expanding
//! residency (no new load, no eviction whose payload could not be kept), reports why, and
//! never drops a change to keep the window moving.

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use world_model::{cell::WorldCell, generation::ChunkTheme, spatial::ChunkKey};

use crate::{
    ids::ChunkRevision,
    ledger::{ResidencyOutcome, Series},
    port::{Pressure, Refusal, ResidentWindow, SlotRef, SnapshotTicket},
    runtime::{ResidencyError, Runtime},
    store::{ChunkPayload, StoreError},
    workers::{Done, LoadSource, Loaded, Workers, WriteRows},
};

/// Bounds of the owner's work and memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResidencyConfig {
    /// Loads handed to the workers and not yet collected.
    pub loads_in_flight: u32,
    /// Chunk payloads loaded and waiting for a slot.
    pub ready_bytes: u64,
    /// Payloads captured and not yet durable, plus the eviction snapshots that may add to them.
    pub retained_bytes: u64,
    /// Rows per write transaction.
    pub rows_per_write: u32,
    /// Uploads staged per frame.
    pub uploads_per_frame: u32,
    /// Outgoing chunks snapshotted per frame.
    pub evictions_per_frame: u32,
    /// Frames to wait before retrying a failed write or an unreadable row.
    pub retry_frames: u32,
}

impl Default for ResidencyConfig {
    fn default() -> Self {
        Self {
            loads_in_flight: 16,
            ready_bytes: 16 << 20,
            retained_bytes: 32 << 20,
            rows_per_write: 16,
            uploads_per_frame: 16,
            evictions_per_frame: 16,
            retry_frames: 30,
        }
    }
}

/// Where a key's newest content is.
#[derive(Debug, Clone)]
pub enum Holder {
    /// A payload captured this session and not yet durable.
    Retained(Arc<ChunkPayload>),
    /// Durable under its exact revision (written this session or published).
    Durable,
    /// Generated from the seed and unchanged: nothing is saved.
    Generated,
}

#[derive(Debug, Clone)]
enum Phase {
    Idle,
    Loading {
        request: u64,
    },
    Loaded {
        payload: Arc<ChunkPayload>,
        revision: ChunkRevision,
    },
    Uploading {
        slot: SlotRef,
        revision: ChunkRevision,
    },
    Resident {
        slot: SlotRef,
        revision: ChunkRevision,
    },
    /// A snapshot of the chunk is outstanding. `retired`: its slot was retired with it, and the
    /// chunk is out once the snapshot resolves; otherwise the retirement was refused, the chunk
    /// is still resident in `slot` and the eviction is tried again.
    Evicting {
        revision: ChunkRevision,
        slot: SlotRef,
        retired: bool,
    },
    Failed {
        error: StoreError,
        retry_at: u64,
    },
}

#[derive(Debug, Clone)]
struct Key {
    phase: Phase,
    /// The newest revision known and where it is; `None` until the key was loaded.
    newest: Option<(ChunkRevision, Holder)>,
    /// The revision the latest published checkpoint holds, when known.
    published: Option<ChunkRevision>,
    /// The theme the chunk was generated or saved with; a snapshot carries only cells.
    theme: ChunkTheme,
}

impl Key {
    fn new() -> Self {
        Self {
            phase: Phase::Idle,
            newest: None,
            published: None,
            theme: ChunkTheme::default(),
        }
    }
}

/// Why residency is not expanding, if it is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidencyPressure {
    /// Captured payloads waiting to become durable fill their budget (writes are failing or
    /// slow): no chunk is evicted, so the window cannot move.
    Retained,
    /// Loaded payloads fill their budget: no new load.
    Ready,
    /// A bounded resource of the live world.
    World(Pressure),
    /// The load or write queue is full.
    Workers,
}

/// What the owner reports, for displays and evidence.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ResidencySummary {
    pub resident: u32,
    pub loading: u32,
    pub ready: u32,
    pub uploading: u32,
    pub evicting: u32,
    pub failed: u32,
    pub retained_payloads: u32,
    pub retained_bytes: u64,
    pub retained_high_water: u64,
    pub ready_bytes: u64,
    pub writes_pending: u32,
    pub loads_generated: u64,
    pub loads_stored: u64,
    pub loads_from_retained: u64,
    pub stale_loads: u64,
    /// Load results applied after a result of a later request: the workers answered out of
    /// request order.
    pub loads_out_of_order: u64,
    pub read_failures: u64,
    pub write_failures: u64,
    pub writes_committed: u64,
    pub snapshots_changed: u64,
    pub snapshots_unchanged: u64,
    pub window_moves: u64,
    pub refused_moves: u64,
    /// Load latency (queue and run), in milliseconds.
    pub load_age_ms: Series,
    /// Write latency, in milliseconds.
    pub write_age_ms: Series,
    pub pressure: Option<ResidencyPressure>,
}

/// A refused move of the window: the current window stays, with every chunk in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefusedMove {
    pub window: ResidentWindow,
    pub refusal: Refusal,
}

/// A failure of a key the owner cannot load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyFailure {
    pub key: ChunkKey,
    pub error: StoreError,
}

pub struct Residency {
    config: ResidencyConfig,
    current: ResidentWindow,
    target: ResidentWindow,
    keys: HashMap<ChunkKey, Key>,
    next_request: u64,
    /// The latest request whose result was applied.
    applied_request: u64,
    /// Snapshot tickets of outgoing chunks, by ticket.
    evictions: HashMap<SnapshotTicket, ChunkKey>,
    /// Captured revisions waiting to be written, oldest first, and batches in flight.
    unwritten: VecDeque<(ChunkKey, ChunkRevision, Arc<ChunkPayload>)>,
    writing: HashMap<u64, Vec<(ChunkKey, ChunkRevision)>>,
    next_batch: u64,
    retry_writes_at: u64,
    free: Vec<ChunkPayload>,
    frame: u64,
    summary: ResidencySummary,
    refused: Option<RefusedMove>,
    failures: Vec<KeyFailure>,
    residency_outcomes: Vec<ResidencyOutcome>,
    /// Revisions whose durability a checkpoint waits for, by key; reported as they become
    /// durable.
    durable_events: Vec<(ChunkKey, ChunkRevision)>,
    /// Eviction snapshots resolved this frame: the key and the revision it holds now.
    resolved_evictions: Vec<(ChunkKey, ChunkRevision)>,
}

const PAYLOAD_BYTES: u64 = ChunkPayload::BYTES;

impl Residency {
    /// An owner for a world whose GPU window is `window`, with nothing resident yet.
    pub fn new(window: ResidentWindow, config: ResidencyConfig) -> Self {
        Self {
            config,
            current: window,
            target: window,
            keys: HashMap::new(),
            next_request: 1,
            applied_request: 0,
            evictions: HashMap::new(),
            unwritten: VecDeque::new(),
            writing: HashMap::new(),
            next_batch: 1,
            retry_writes_at: 0,
            free: Vec::new(),
            frame: 0,
            summary: ResidencySummary::default(),
            refused: None,
            failures: Vec::new(),
            residency_outcomes: Vec::new(),
            durable_events: Vec::new(),
            resolved_evictions: Vec::new(),
        }
    }

    pub fn window(&self) -> ResidentWindow {
        self.current
    }

    pub fn target(&self) -> ResidentWindow {
        self.target
    }

    /// Asks for the window to become `window`: chunks leaving it are evicted, chunks entering
    /// it loaded, and the GPU window moves once that is possible.
    pub fn set_target(&mut self, window: ResidentWindow) {
        self.target = window;
        self.refused = None;
    }

    pub fn summary(&self) -> &ResidencySummary {
        &self.summary
    }

    /// The last refused move, if the target was given up.
    pub fn refused_move(&self) -> Option<RefusedMove> {
        self.refused
    }

    /// Keys whose stored row could not be read, with why. They are not resident and were never
    /// generated over; an I/O failure is retried, a corrupt or unsupported row is not.
    pub fn failed(&self) -> impl Iterator<Item = (ChunkKey, &StoreError)> + '_ {
        self.keys
            .iter()
            .filter_map(|(key, entry)| match &entry.phase {
                Phase::Failed { error, .. } => Some((*key, error)),
                _ => None,
            })
    }

    /// Keys that could not be loaded, since the last call.
    pub fn take_failures(&mut self) -> Vec<KeyFailure> {
        std::mem::take(&mut self.failures)
    }

    /// Whether every chunk of the target window is resident and the GPU window is the target.
    pub fn settled(&self) -> bool {
        self.current == self.target
            && self.current.view().iter().all(|key| {
                matches!(
                    self.keys.get(&key).map(|entry| &entry.phase),
                    Some(Phase::Resident { .. } | Phase::Failed { .. })
                )
            })
            && self.evictions.is_empty()
    }

    /// The slot of a resident key.
    pub fn slot(&self, key: ChunkKey) -> Option<SlotRef> {
        match self.keys.get(&key)?.phase {
            Phase::Resident { slot, .. } => Some(slot),
            _ => None,
        }
    }

    /// The revision of `key` the latest published checkpoint holds, when known.
    pub fn published_revision(&self, key: ChunkKey) -> Option<ChunkRevision> {
        self.keys.get(&key)?.published
    }

    /// The newest revision of `key` the owner knows, and whether it is durable.
    pub fn newest(&self, key: ChunkKey) -> Option<(ChunkRevision, bool)> {
        let (revision, holder) = self.keys.get(&key)?.newest.as_ref()?;
        Some((*revision, !matches!(holder, Holder::Retained(_))))
    }

    /// Whether `key` is resident on the GPU now.
    pub fn is_resident(&self, key: ChunkKey) -> bool {
        matches!(
            self.keys.get(&key).map(|entry| &entry.phase),
            Some(Phase::Resident { .. } | Phase::Uploading { .. })
        )
    }

    /// Every key resident (or uploading) and its slot, for a world snapshot's accounting.
    pub fn resident_keys(&self) -> impl Iterator<Item = ChunkKey> + '_ {
        self.keys.iter().filter_map(|(key, entry)| {
            matches!(
                entry.phase,
                Phase::Resident { .. } | Phase::Uploading { .. }
            )
            .then_some(*key)
        })
    }

    /// Keys with an eviction snapshot outstanding: their state at a checkpoint is what the
    /// snapshot reports.
    pub fn evicting_keys(&self) -> impl Iterator<Item = ChunkKey> + '_ {
        self.evictions.values().copied()
    }

    /// Keys whose newest revision is not the published one, for a checkpoint: the revision and
    /// whether it is durable yet. Generated keys need nothing.
    pub fn unpublished(&self) -> impl Iterator<Item = (ChunkKey, ChunkRevision)> + '_ {
        self.keys.iter().filter_map(|(key, entry)| {
            let (revision, holder) = entry.newest.as_ref()?;
            if matches!(holder, Holder::Generated) || Some(*revision) == entry.published {
                return None;
            }
            Some((*key, *revision))
        })
    }

    /// Whether a revision the owner captured or loaded is durable: it is neither waiting to
    /// be written nor in a write in flight.
    pub fn is_durable(&self, key: ChunkKey, revision: ChunkRevision) -> bool {
        let identity = (key, revision);
        !self.unwritten.iter().any(|(k, r, _)| (*k, *r) == identity)
            && !self.writing.values().any(|rows| rows.contains(&identity))
    }

    /// Records a publication: the keys it moved now hold these revisions as published.
    pub fn published(&mut self, heads: &[(ChunkKey, ChunkRevision)]) {
        for (key, revision) in heads {
            if let Some(entry) = self.keys.get_mut(key) {
                entry.published = Some(
                    entry
                        .published
                        .map_or(*revision, |published| published.max(*revision)),
                );
            }
        }
        // Forget idle keys whose newest content is published: the store answers for them.
        self.keys.retain(|_, entry| {
            !matches!(entry.phase, Phase::Idle)
                || match &entry.newest {
                    Some((revision, Holder::Durable)) => entry.published != Some(*revision),
                    Some((_, Holder::Generated)) => false,
                    Some((_, Holder::Retained(_))) => true,
                    None => false,
                }
        });
    }

    /// Revisions that became durable since the last call.
    pub fn take_durable(&mut self, into: &mut Vec<(ChunkKey, ChunkRevision)>) {
        into.append(&mut self.durable_events);
    }

    /// Eviction snapshots resolved since the last call: each key and the revision it holds.
    pub fn take_resolved_evictions(&mut self, into: &mut Vec<(ChunkKey, ChunkRevision)>) {
        into.append(&mut self.resolved_evictions);
    }

    /// Captured payloads not yet durable, for a loss report.
    pub fn retained_unwritten(&self) -> usize {
        self.unwritten.len() + self.writing.values().map(Vec::len).sum::<usize>()
    }

    fn payload(&mut self) -> ChunkPayload {
        self.free.pop().unwrap_or_else(ChunkPayload::zeroed)
    }

    fn recycle(&mut self, payload: Arc<ChunkPayload>) {
        if let Some(payload) = Arc::into_inner(payload)
            && self.free.len() < 64
        {
            self.free.push(payload);
        }
    }

    fn retained_bytes(&self) -> u64 {
        let retained = self
            .keys
            .values()
            .filter(|entry| matches!(entry.newest, Some((_, Holder::Retained(_)))))
            .count() as u64;
        // Unwritten revisions superseded by a newer one are retained too.
        let superseded = self
            .unwritten
            .iter()
            .filter(|(key, revision, _)| {
                !matches!(
                    self.keys.get(key).and_then(|entry| entry.newest.as_ref()),
                    Some((newest, Holder::Retained(_))) if newest == revision
                )
            })
            .count() as u64;
        (retained + superseded) * PAYLOAD_BYTES
    }

    fn ready_bytes(&self) -> u64 {
        self.keys
            .values()
            .filter(|entry| matches!(entry.phase, Phase::Loaded { .. }))
            .count() as u64
            * PAYLOAD_BYTES
    }

    /// Runs one frame of residency after the runtime's frame: applies outcomes and finished
    /// jobs, then moves toward the target window.
    pub fn frame(&mut self, runtime: &mut Runtime, workers: &mut Workers, done: &mut Vec<Done>) {
        self.frame += 1;
        self.summary.pressure = None;
        self.residency_outcomes.clear();
        runtime.drain_residency(&mut self.residency_outcomes);
        let outcomes = std::mem::take(&mut self.residency_outcomes);
        for outcome in &outcomes {
            self.on_outcome(runtime, outcome);
        }
        self.residency_outcomes = outcomes;
        let finished = std::mem::take(done);
        for job in finished {
            self.on_done(job, done);
        }
        self.write(workers);
        self.evict(runtime);
        self.move_window(runtime);
        self.load(workers);
        self.upload(runtime);
        self.update_summary();
    }

    /// Routes the outcomes of other owners (world snapshots) back to the caller.
    pub fn other_outcomes(&self) -> impl Iterator<Item = &ResidencyOutcome> {
        self.residency_outcomes.iter().filter(|outcome| {
            matches!(
                outcome,
                ResidencyOutcome::Snapshot { ticket, .. } | ResidencyOutcome::SnapshotAbandoned { ticket, .. }
                    if ticket.world().is_some()
            ) || matches!(
                outcome,
                ResidencyOutcome::BodySnapshot { .. } | ResidencyOutcome::BodySnapshotAbandoned { .. }
            )
        })
    }

    fn on_outcome(&mut self, runtime: &mut Runtime, outcome: &ResidencyOutcome) {
        match *outcome {
            ResidencyOutcome::UploadActivated { ticket, .. } => {
                if let Some(entry) = self.keys.get_mut(&ticket.key)
                    && let Phase::Uploading { slot, revision } = entry.phase
                    && slot == ticket.slot
                {
                    entry.phase = Phase::Resident { slot, revision };
                }
            }
            ResidencyOutcome::Snapshot {
                ticket,
                key,
                changed,
                ..
            } if ticket.world().is_none() => {
                if self.evictions.remove(&ticket).is_none() {
                    return;
                }
                let Some(entry) = self.keys.get_mut(&key) else {
                    return;
                };
                let Phase::Evicting {
                    revision,
                    slot,
                    retired,
                    ..
                } = entry.phase
                else {
                    return;
                };
                let revision = if changed {
                    let mut payload = self.free.pop().unwrap_or_else(ChunkPayload::zeroed);
                    if runtime.take_snapshot(ticket, payload.cells_mut()).is_err() {
                        // Unreachable while the session runs: the outcome said it was ready.
                        // The chunk stays out; its change is reported lost with the session.
                        self.free.push(payload);
                        return;
                    }
                    self.summary.snapshots_changed += 1;
                    self.capture(key, payload)
                } else {
                    // Unchanged since it was uploaded or last snapshotted: it holds the newest
                    // revision (a world snapshot may have captured one since the upload).
                    self.summary.snapshots_unchanged += 1;
                    entry
                        .newest
                        .as_ref()
                        .map_or(revision, |(newest, _)| *newest)
                };
                if let Some(entry) = self.keys.get_mut(&key) {
                    entry.phase = if retired {
                        Phase::Idle
                    } else {
                        Phase::Resident { slot, revision }
                    };
                }
                if retired {
                    self.resolved_evictions.push((key, revision));
                }
            }
            ResidencyOutcome::SnapshotAbandoned { ticket, .. } if ticket.world().is_none() => {
                // Only a lost or stopped session abandons; the session reports the loss.
                self.evictions.remove(&ticket);
            }
            _ => {}
        }
    }

    /// Records a captured payload of `key` as its newest revision and queues its write.
    /// Returns the new revision. The live chunk (if still resident) now holds it too.
    pub fn capture(&mut self, key: ChunkKey, mut payload: ChunkPayload) -> ChunkRevision {
        let entry = self.keys.entry(key).or_insert_with(Key::new);
        let base = entry
            .newest
            .as_ref()
            .map_or(ChunkRevision::default(), |(revision, _)| *revision);
        let base = base.max(entry.published.unwrap_or_default());
        let revision = ChunkRevision::new(base.value() + 1);
        // A snapshot carries cells only; the chunk keeps the theme it was loaded with.
        payload.set_theme(entry.theme);
        let payload = Arc::new(payload);
        entry.newest = Some((revision, Holder::Retained(Arc::clone(&payload))));
        match &mut entry.phase {
            Phase::Resident { revision: live, .. }
            | Phase::Uploading { revision: live, .. }
            | Phase::Evicting { revision: live, .. } => *live = revision,
            _ => {}
        }
        self.unwritten.push_back((key, revision, payload));
        revision
    }

    /// The revision a resident chunk's live content holds when a snapshot says it did not
    /// change.
    pub fn resident_revision(&self, key: ChunkKey) -> Option<ChunkRevision> {
        match self.keys.get(&key)?.phase {
            Phase::Resident { revision, .. } | Phase::Uploading { revision, .. } => Some(revision),
            _ => None,
        }
    }

    fn on_done(&mut self, done: Done, keep: &mut Vec<Done>) {
        match done {
            Done::Loaded {
                key,
                request,
                result,
                buffer,
                age,
            } => {
                self.summary.load_age_ms.add(age.as_secs_f64() * 1000.0);
                let current = matches!(
                    self.keys.get(&key).map(|entry| &entry.phase),
                    Some(Phase::Loading { request: wanted }) if *wanted == request
                );
                if !current {
                    // Cancelled, or superseded by a newer request: never applied.
                    self.summary.stale_loads += 1;
                    if self.free.len() < 64 {
                        self.free.push(buffer);
                    }
                    return;
                }
                if request < self.applied_request {
                    self.summary.loads_out_of_order += 1;
                }
                self.applied_request = self.applied_request.max(request);
                let Some(entry) = self.keys.get_mut(&key) else {
                    return;
                };
                entry.theme = buffer.theme();
                match result {
                    Ok(Loaded::Stored(revision)) => {
                        self.summary.loads_stored += 1;
                        if entry.newest.is_none() {
                            entry.newest = Some((revision, Holder::Durable));
                            entry.published = Some(revision);
                        }
                        entry.phase = Phase::Loaded {
                            payload: Arc::new(buffer),
                            revision,
                        };
                    }
                    Ok(Loaded::Generated) => {
                        self.summary.loads_generated += 1;
                        if entry.newest.is_none() {
                            entry.newest = Some((ChunkRevision::default(), Holder::Generated));
                        }
                        entry.phase = Phase::Loaded {
                            payload: Arc::new(buffer),
                            revision: ChunkRevision::default(),
                        };
                    }
                    Err(error) => {
                        self.summary.read_failures += 1;
                        let retry_at = match error {
                            // A failing device may recover; damaged or foreign data does not.
                            StoreError::Io { .. } => {
                                self.frame + u64::from(self.config.retry_frames)
                            }
                            _ => u64::MAX,
                        };
                        self.failures.push(KeyFailure {
                            key,
                            error: error.clone(),
                        });
                        entry.phase = Phase::Failed { error, retry_at };
                        if self.free.len() < 64 {
                            self.free.push(buffer);
                        }
                    }
                }
            }
            Done::Written {
                batch,
                rows,
                result,
                age,
            } => {
                self.summary.write_age_ms.add(age.as_secs_f64() * 1000.0);
                self.writing.remove(&batch);
                match result {
                    Ok(()) => {
                        self.summary.writes_committed += rows.len() as u64;
                        for (key, revision, payload) in rows {
                            if let Some(entry) = self.keys.get_mut(&key)
                                && let Some((newest, holder)) = &mut entry.newest
                                && *newest == revision
                                && matches!(holder, Holder::Retained(_))
                            {
                                *holder = Holder::Durable;
                            }
                            self.durable_events.push((key, revision));
                            self.recycle(payload);
                        }
                    }
                    Err(_) => {
                        // Keep every payload and try again later, oldest first.
                        self.summary.write_failures += 1;
                        for row in rows.into_iter().rev() {
                            self.unwritten.push_front(row);
                        }
                        self.retry_writes_at = self.frame + u64::from(self.config.retry_frames);
                    }
                }
            }
            other => keep.push(other),
        }
    }

    fn write(&mut self, workers: &mut Workers) {
        if self.frame < self.retry_writes_at {
            return;
        }
        while !self.unwritten.is_empty() {
            let count = self
                .unwritten
                .len()
                .min(self.config.rows_per_write as usize);
            let rows: WriteRows = self.unwritten.drain(..count).collect();
            let identities = rows
                .iter()
                .map(|(key, revision, _)| (*key, *revision))
                .collect();
            let batch = self.next_batch;
            match workers.write_chunks(batch, rows) {
                Ok(()) => {
                    self.writing.insert(batch, identities);
                    self.next_batch += 1;
                }
                Err(crate::workers::QueueFull(rows)) => {
                    for row in rows.into_iter().rev() {
                        self.unwritten.push_front(row);
                    }
                    self.summary
                        .pressure
                        .get_or_insert(ResidencyPressure::Workers);
                    return;
                }
            }
        }
    }

    /// Snapshots and retires resident chunks outside the target window.
    fn evict(&mut self, runtime: &mut Runtime) {
        let target = self.target;
        let mut outgoing: Vec<(ChunkKey, SlotRef, ChunkRevision)> = self
            .keys
            .iter()
            .filter_map(|(key, entry)| match entry.phase {
                Phase::Resident { slot, revision } if !target.contains(*key) => {
                    Some((*key, slot, revision))
                }
                _ => None,
            })
            .collect();
        outgoing.sort_by_key(|(key, ..)| (key.key.y, key.key.x));
        for (key, slot, revision) in outgoing
            .into_iter()
            .take(self.config.evictions_per_frame as usize)
        {
            // An eviction may add a payload: keep room for it before asking.
            let reserved = (self.evictions.len() as u64 + 1) * PAYLOAD_BYTES;
            if self.retained_bytes() + reserved > self.config.retained_bytes {
                self.summary.pressure = Some(ResidencyPressure::Retained);
                return;
            }
            let ticket = match runtime.snapshot_chunk(slot) {
                Ok(ticket) => ticket,
                Err(ResidencyError::Refused(Refusal::Pressure(pressure))) => {
                    self.summary
                        .pressure
                        .get_or_insert(ResidencyPressure::World(pressure));
                    return;
                }
                Err(_) => return,
            };
            // Retired in the same frame, so the slot leaves the window at the start of the
            // tick whose end the snapshot copies: nothing changes between the two.
            let retired = runtime.retire_slot(slot);
            self.evictions.insert(ticket, key);
            if let Some(entry) = self.keys.get_mut(&key) {
                // A refused retirement leaves the chunk resident: the snapshot still records
                // its content as a revision, and the eviction is tried again after it.
                entry.phase = Phase::Evicting {
                    revision,
                    slot,
                    retired: retired.is_ok(),
                };
            }
            if let Err(refusal) = retired {
                if let Refusal::Pressure(pressure) = refusal {
                    self.summary
                        .pressure
                        .get_or_insert(ResidencyPressure::World(pressure));
                }
                return;
            }
        }
    }

    /// Moves the GPU window to the target once nothing outside the target holds a slot.
    fn move_window(&mut self, runtime: &mut Runtime) {
        if self.current == self.target {
            return;
        }
        let target = self.target;
        let blocked = self.keys.iter().any(|(key, entry)| {
            !target.contains(*key)
                && matches!(
                    entry.phase,
                    Phase::Resident { .. }
                        | Phase::Uploading { .. }
                        | Phase::Evicting { retired: false, .. }
                )
        });
        if blocked {
            // Before the first eviction, find out whether the move can succeed at all.
            if let Err(refusal) = runtime.preflight_resize(target)
                && !matches!(refusal, Refusal::Pressure(_))
            {
                self.give_up(target, refusal);
            }
            return;
        }
        match runtime.resize(target) {
            Ok(report) => {
                self.current = target;
                self.summary.window_moves += 1;
                for (key, slot) in report.retained {
                    if let Some(entry) = self.keys.get_mut(&key)
                        && let Phase::Resident { revision, .. } = entry.phase
                    {
                        entry.phase = Phase::Resident { slot, revision };
                    }
                }
            }
            Err(Refusal::Pressure(pressure)) => {
                self.summary
                    .pressure
                    .get_or_insert(ResidencyPressure::World(pressure));
            }
            Err(refusal) => self.give_up(target, refusal),
        }
    }

    fn give_up(&mut self, window: ResidentWindow, refusal: Refusal) {
        self.summary.refused_moves += 1;
        self.refused = Some(RefusedMove { window, refusal });
        self.target = self.current;
    }

    /// Requests loads for target keys that are not resident, newest content first.
    fn load(&mut self, workers: &mut Workers) {
        let mut wanted: Vec<ChunkKey> = self.target.view().iter().collect();
        // Nearest the centre first.
        let center = self.target.center();
        wanted.sort_by_key(|key| {
            let dx = i64::from(key.key.x) - i64::from(center.key.x);
            let dy = i64::from(key.key.y) - i64::from(center.key.y);
            dx * dx + dy * dy
        });
        // Loads of keys that left the target are cancelled: their results will be discarded.
        let target = self.target;
        for (key, entry) in self.keys.iter_mut() {
            if !target.contains(*key) {
                match entry.phase {
                    Phase::Loading { .. } => entry.phase = Phase::Idle,
                    Phase::Loaded { .. } => entry.phase = Phase::Idle,
                    _ => {}
                }
            }
        }
        for key in wanted {
            let entry = self.keys.entry(key).or_insert_with(Key::new);
            match &entry.phase {
                Phase::Idle => {}
                Phase::Failed { retry_at, .. } if self.frame >= *retry_at => {}
                _ => continue,
            }
            // The newest retained payload wins over anything on disk.
            if let Some((revision, Holder::Retained(payload))) = &entry.newest {
                entry.phase = Phase::Loaded {
                    payload: Arc::clone(payload),
                    revision: *revision,
                };
                self.summary.loads_from_retained += 1;
                continue;
            }
            if workers.loads_in_flight() >= self.config.loads_in_flight {
                self.summary
                    .pressure
                    .get_or_insert(ResidencyPressure::Workers);
                return;
            }
            let ready = self
                .keys
                .values()
                .filter(|entry| matches!(entry.phase, Phase::Loaded { .. } | Phase::Loading { .. }))
                .count() as u64
                * PAYLOAD_BYTES;
            if ready + PAYLOAD_BYTES > self.config.ready_bytes {
                self.summary
                    .pressure
                    .get_or_insert(ResidencyPressure::Ready);
                return;
            }
            let entry = self.keys.get(&key).expect("inserted above");
            let source = match &entry.newest {
                Some((revision, Holder::Durable)) => LoadSource::Revision(*revision),
                Some((_, Holder::Generated)) => LoadSource::Generate,
                _ => LoadSource::Published,
            };
            let request = self.next_request;
            self.next_request += 1;
            let buffer = self.payload();
            match workers.load(key, request, source, buffer) {
                Ok(()) => {
                    if let Some(entry) = self.keys.get_mut(&key) {
                        entry.phase = Phase::Loading { request };
                    }
                }
                Err(crate::workers::QueueFull(buffer)) => {
                    self.free.push(buffer);
                    self.summary
                        .pressure
                        .get_or_insert(ResidencyPressure::Workers);
                    return;
                }
            }
        }
    }

    /// Uploads loaded payloads whose key is inside the GPU window.
    fn upload(&mut self, runtime: &mut Runtime) {
        let current = self.current;
        let mut ready: Vec<(ChunkKey, Arc<ChunkPayload>, ChunkRevision)> = self
            .keys
            .iter()
            .filter_map(|(key, entry)| match &entry.phase {
                Phase::Loaded { payload, revision } if current.contains(*key) => {
                    Some((*key, Arc::clone(payload), *revision))
                }
                _ => None,
            })
            .collect();
        ready.sort_by_key(|(key, ..)| (key.key.y, key.key.x));
        for (key, payload, revision) in ready
            .into_iter()
            .take(self.config.uploads_per_frame as usize)
        {
            let slot = match runtime.reserve_slot(key) {
                Ok(slot) => slot,
                Err(Refusal::Pressure(pressure)) => {
                    self.summary
                        .pressure
                        .get_or_insert(ResidencyPressure::World(pressure));
                    return;
                }
                Err(_) => return,
            };
            match runtime.upload_chunk(slot, revision, payload.cells()) {
                Ok(_) => {
                    if let Some(entry) = self.keys.get_mut(&key) {
                        entry.phase = Phase::Uploading { slot, revision };
                    }
                    self.recycle(payload);
                }
                Err(error) => {
                    // Return the reservation so no slot is held without its upload.
                    let _ = runtime.retire_slot(slot);
                    if let ResidencyError::Refused(Refusal::Pressure(pressure)) = error {
                        self.summary
                            .pressure
                            .get_or_insert(ResidencyPressure::World(pressure));
                    }
                    return;
                }
            }
        }
    }

    fn update_summary(&mut self) {
        let mut summary = ResidencySummary {
            pressure: self.summary.pressure,
            ..self.summary
        };
        summary.resident = 0;
        summary.loading = 0;
        summary.ready = 0;
        summary.uploading = 0;
        summary.evicting = 0;
        summary.failed = 0;
        for entry in self.keys.values() {
            match entry.phase {
                Phase::Resident { .. } => summary.resident += 1,
                Phase::Loading { .. } => summary.loading += 1,
                Phase::Loaded { .. } => summary.ready += 1,
                Phase::Uploading { .. } => summary.uploading += 1,
                Phase::Evicting { .. } => summary.evicting += 1,
                Phase::Failed { .. } => summary.failed += 1,
                Phase::Idle => {}
            }
        }
        summary.retained_bytes = self.retained_bytes();
        summary.retained_high_water = summary.retained_high_water.max(summary.retained_bytes);
        summary.retained_payloads = (summary.retained_bytes / PAYLOAD_BYTES) as u32;
        summary.ready_bytes = self.ready_bytes();
        summary.writes_pending = self.retained_unwritten() as u32;
        self.summary = summary;
    }

    /// Whether anything is still on its way to being durable or resolved.
    pub fn busy(&self) -> bool {
        !self.unwritten.is_empty() || !self.writing.is_empty() || !self.evictions.is_empty()
    }

    /// Frames until a failed write is retried; zero when nothing waits.
    pub fn write_retry_in(&self) -> u64 {
        self.retry_writes_at.saturating_sub(self.frame)
    }

    /// Retries failed writes now (an orderly close does not wait for the backoff).
    pub fn retry_writes_now(&mut self) {
        self.retry_writes_at = 0;
    }

    /// Cells of a key's retained newest payload, for tests and evidence.
    pub fn retained_cells(&self, key: ChunkKey) -> Option<&[WorldCell]> {
        match &self.keys.get(&key)?.newest {
            Some((_, Holder::Retained(payload))) => Some(payload.cells()),
            _ => None,
        }
    }

    /// Marks a resident key's live content as captured at a world snapshot: `changed` with its
    /// payload makes a new revision; unchanged keeps the revision it holds. Returns the
    /// revision the key holds at the snapshot.
    pub fn world_snapshot_chunk(
        &mut self,
        key: ChunkKey,
        payload: Option<ChunkPayload>,
    ) -> Option<ChunkRevision> {
        match payload {
            Some(payload) => {
                self.summary.snapshots_changed += 1;
                Some(self.capture(key, payload))
            }
            None => {
                self.summary.snapshots_unchanged += 1;
                self.resident_revision(key)
            }
        }
    }

    /// A spare payload buffer for taking a snapshot into.
    pub fn spare_payload(&mut self) -> ChunkPayload {
        self.payload()
    }

    /// Returns a buffer that was not used.
    pub fn return_payload(&mut self, payload: ChunkPayload) {
        if self.free.len() < 64 {
            self.free.push(payload);
        }
    }
}
