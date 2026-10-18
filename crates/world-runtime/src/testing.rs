//! Test doubles for the owners' rules: a model of the live world's residency mechanism and an
//! in-memory store with injected failures and holds.
//!
//! They answer like the real owners do (outcomes one poll after submission, change flags set
//! by edits and cleared by snapshots, refusals as pressure) so the residency and checkpoint
//! rules can be checked without a device. They prove nothing about durability or GPU
//! execution: `world-store`'s tests run the real store, and the `streaming` GPU suite runs both
//! real owners together.

use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use world_model::{
    cell::WorldCell,
    command::{AmbientTemperature, ElevationScale},
    resume::BodySet,
    rules::SimulationRules,
    spatial::{CHUNK_CELL_COUNT, ChunkKey},
};

use crate::{
    ids::{ChunkRevision, SessionEpoch, SessionId, Tick, WorldId},
    port::*,
    store::*,
};

pub fn session() -> SessionId {
    SessionId {
        world: WorldId::new(77),
        epoch: SessionEpoch::FIRST,
    }
}

pub fn config() -> PhysicalConfig {
    PhysicalConfig {
        rules: SimulationRules::ALL,
        ambient_temperature: AmbientTemperature::celsius(20.0).expect("supported"),
        elevation_scale: ElevationScale::DEFAULT,
    }
}

/// Cells whose every word is `value`.
pub fn cells(value: u64) -> Vec<WorldCell> {
    vec![WorldCell(value); CHUNK_CELL_COUNT]
}

#[derive(Debug, Clone)]
enum SlotState {
    Free,
    Reserved {
        key: ChunkKey,
    },
    Staged {
        key: ChunkKey,
        revision: ChunkRevision,
        cells: Vec<WorldCell>,
    },
    Resident {
        key: ChunkKey,
        revision: ChunkRevision,
        cells: Vec<WorldCell>,
        changed: bool,
    },
    /// Retired in a submitted tick; free at the next poll.
    Retiring,
}

#[derive(Debug, Clone)]
struct Slot {
    generation: u32,
    state: SlotState,
}

#[derive(Debug, Clone)]
enum Snap {
    Requested { slot: usize },
    Ready { cells: Vec<WorldCell> },
}

#[derive(Debug)]
pub struct Model {
    pub window: ResidentWindow,
    slots: Vec<Slot>,
    pub next_tick: Tick,
    last_tick: Option<Tick>,
    pending: Vec<Outcome>,
    retire_requested: Vec<usize>,
    snaps: HashMap<u32, Snap>,
    next_snap: u32,
    world: Option<(u32, bool)>,
    world_cells: HashMap<u32, Vec<WorldCell>>,
    world_bodies: Option<BodySet>,
    pub bodies: BodySet,
    pub config: PhysicalConfig,
    pub lost: bool,
    /// Refuse every retirement (to exercise the refused-retire path).
    pub refuse_retire: bool,
    pub ticks: u64,
    pub resizes: u64,
    pub restored: Option<BodySet>,
}

#[derive(Clone)]
pub struct ModelPort(pub Arc<Mutex<Model>>);

impl ModelPort {
    pub fn new(window: ResidentWindow) -> Self {
        Self(Arc::new(Mutex::new(Model {
            window,
            slots: vec![
                Slot {
                    generation: 0,
                    state: SlotState::Free,
                };
                window.chunk_count() as usize
            ],
            next_tick: Tick::ZERO,
            last_tick: None,
            pending: Vec::new(),
            retire_requested: Vec::new(),
            snaps: HashMap::new(),
            next_snap: 1,
            world: None,
            world_cells: HashMap::new(),
            world_bodies: None,
            bodies: BodySet::default(),
            config: config(),
            lost: false,
            refuse_retire: false,
            ticks: 0,
            resizes: 0,
            restored: None,
        })))
    }

    pub fn model(&self) -> std::sync::MutexGuard<'_, Model> {
        self.0.lock().expect("model")
    }
}

impl Model {
    /// The resident cells of `key`, if resident.
    pub fn resident(&self, key: ChunkKey) -> Option<(ChunkRevision, Vec<WorldCell>)> {
        self.slots.iter().find_map(|slot| match &slot.state {
            SlotState::Resident {
                key: k,
                revision,
                cells,
                ..
            } if *k == key => Some((*revision, cells.clone())),
            _ => None,
        })
    }

    /// An edit of a resident chunk, as a kernel's: changes its cells and marks it changed.
    pub fn edit(&mut self, key: ChunkKey, value: u64) -> bool {
        for slot in &mut self.slots {
            if let SlotState::Resident {
                key: k,
                cells,
                changed,
                ..
            } = &mut slot.state
                && *k == key
            {
                cells.fill(WorldCell(value));
                *changed = true;
                return true;
            }
        }
        false
    }

    fn copy_snapshots(&mut self, tick: Option<Tick>) {
        let ids: Vec<u32> = self.snaps.keys().copied().collect();
        for id in ids {
            if let Some(Snap::Requested { slot }) = self.snaps.get(&id).cloned() {
                let (key, revision, cells, changed) = match &mut self.slots[slot].state {
                    SlotState::Resident {
                        key,
                        revision,
                        cells,
                        changed,
                    } => {
                        let was = *changed;
                        *changed = false;
                        (*key, *revision, cells.clone(), was)
                    }
                    _ => continue,
                };
                // An unchanged snapshot carries no payload and frees its entry at once.
                if changed {
                    self.snaps.insert(id, Snap::Ready { cells });
                } else {
                    self.snaps.remove(&id);
                }
                self.pending.push(Outcome::Snapshot {
                    ticket: SnapshotTicket::new(session(), 0, id, id),
                    key,
                    revision,
                    tick,
                    changed,
                });
            }
        }
        if let Some((id, false)) = self.world {
            self.world = Some((id, true));
            let mut chunks = 0;
            for (index, slot) in self.slots.iter_mut().enumerate() {
                if let SlotState::Resident {
                    key,
                    revision,
                    cells,
                    changed,
                } = &mut slot.state
                {
                    chunks += 1;
                    let was = *changed;
                    *changed = false;
                    if was {
                        self.world_cells.insert(index as u32, cells.clone());
                    }
                    self.pending.push(Outcome::Snapshot {
                        ticket: SnapshotTicket::new(session(), id, index as u32, id),
                        key: *key,
                        revision: *revision,
                        tick,
                        changed: was,
                    });
                }
            }
            self.world_bodies = Some(self.bodies.clone());
            self.pending.push(Outcome::BodySnapshot {
                ticket: WorldSnapshotTicket::new(session(), id),
                tick,
                bodies: self.bodies.live() as u32,
                chunks,
                config: self.config,
            });
        }
    }

    fn lose(&mut self) {
        self.lost = true;
        for (id, snap) in self.snaps.drain() {
            if matches!(snap, Snap::Requested { .. }) {
                self.pending.push(Outcome::Abandoned {
                    obligation: Obligation::Snapshot(SnapshotTicket::new(session(), 0, id, id)),
                    reason: AbandonReason::DeviceLost,
                });
            }
        }
        if let Some((id, false)) = self.world.take() {
            self.pending.push(Outcome::Abandoned {
                obligation: Obligation::WorldSnapshot {
                    ticket: WorldSnapshotTicket::new(session(), id),
                    chunks: 0,
                },
                reason: AbandonReason::DeviceLost,
            });
        }
    }

    /// Loses the device: every outstanding snapshot is abandoned at the next poll.
    pub fn lose_device(&mut self) {
        self.lose();
    }
}

impl PhysicalWorld for ModelPort {
    fn session(&self) -> SessionId {
        session()
    }

    fn capabilities(&self) -> PortCapabilities {
        let model = self.model();
        PortCapabilities {
            commands_per_tick: 64,
            max_in_flight: 3,
            outcomes_per_poll: 4096,
            chunk_upload_slots: 8,
            capture_slots: 8,
            snapshot_entries: 8,
            window: model.window,
            supported: SupportedCommands::default(),
            body_capacity: 64,
            body_samples_per_tick: 8,
        }
    }

    fn next_tick(&self) -> Tick {
        self.model().next_tick
    }

    fn submit_tick(&mut self, batch: &TickBatch<'_>) -> Result<SubmissionTicket, Refusal> {
        let mut model = self.model();
        if model.lost {
            return Err(Refusal::DeviceLost);
        }
        if batch.tick != model.next_tick {
            return Err(Refusal::TickOutOfOrder {
                expected: model.next_tick,
                presented: batch.tick,
            });
        }
        // Retirements and activations at the start of the tick, snapshots at its end.
        for index in std::mem::take(&mut model.retire_requested) {
            model.slots[index].state = SlotState::Retiring;
        }
        let mut activated = Vec::new();
        for (index, slot) in model.slots.iter_mut().enumerate() {
            if let SlotState::Staged {
                key,
                revision,
                cells,
            } = &slot.state
            {
                activated.push(Outcome::UploadActivated {
                    ticket: UploadTicket {
                        slot: SlotRef::new(session(), index as u32, slot.generation),
                        key: *key,
                        revision: *revision,
                    },
                    tick: batch.tick,
                });
                slot.state = SlotState::Resident {
                    key: *key,
                    revision: *revision,
                    cells: cells.clone(),
                    changed: false,
                };
            }
        }
        model.pending.extend(activated);
        model.copy_snapshots(Some(batch.tick));
        model.pending.push(Outcome::TickCompleted(TickReport {
            tick: batch.tick,
            commands: 0,
            gpu_time: None,
            phases: None,
            latency: Duration::ZERO,
            upload_bytes: 0,
            readback_bytes: 0,
            cells: CellReport::default(),
        }));
        model.last_tick = Some(batch.tick);
        model.next_tick = Tick::new(batch.tick.value() + 1);
        model.ticks += 1;
        Ok(SubmissionTicket {
            session: session(),
            tick: batch.tick,
            serial: batch.tick.value(),
        })
    }

    fn reserve_slot(&mut self, _: SessionId, key: ChunkKey) -> Result<SlotRef, Refusal> {
        let mut model = self.model();
        if model.lost {
            return Err(Refusal::DeviceLost);
        }
        if !model.window.contains(key) {
            return Err(Refusal::InvalidPayload(PayloadError::KeyOutsideWindow(key)));
        }
        let taken = model.slots.iter().any(|slot| match &slot.state {
            SlotState::Reserved { key: k }
            | SlotState::Staged { key: k, .. }
            | SlotState::Resident { key: k, .. } => *k == key,
            _ => false,
        });
        if taken {
            return Err(Refusal::InvalidPayload(PayloadError::KeyAlreadyResident(
                key,
            )));
        }
        let index = model
            .slots
            .iter()
            .position(|slot| matches!(slot.state, SlotState::Free))
            .ok_or(Refusal::Pressure(Pressure::Slots))?;
        let slot = &mut model.slots[index];
        slot.generation += 1;
        slot.state = SlotState::Reserved { key };
        Ok(SlotRef::new(session(), index as u32, slot.generation))
    }

    fn upload_chunk(
        &mut self,
        slot: SlotRef,
        revision: ChunkRevision,
        cells: &[WorldCell],
    ) -> Result<UploadTicket, Refusal> {
        let mut model = self.model();
        let entry = &mut model.slots[slot.index() as usize];
        if entry.generation != slot.generation() {
            return Err(Refusal::StaleSlot(slot));
        }
        let SlotState::Reserved { key } = entry.state else {
            return Err(Refusal::InvalidPayload(PayloadError::SlotState(slot)));
        };
        entry.state = SlotState::Staged {
            key,
            revision,
            cells: cells.to_vec(),
        };
        Ok(UploadTicket {
            slot,
            key,
            revision,
        })
    }

    fn retire_slot(&mut self, slot: SlotRef) -> Result<(), Refusal> {
        let mut model = self.model();
        if model.refuse_retire {
            return Err(Refusal::Pressure(Pressure::SlotChanges));
        }
        let index = slot.index() as usize;
        let entry = &mut model.slots[index];
        if entry.generation != slot.generation() {
            return Err(Refusal::StaleSlot(slot));
        }
        match entry.state {
            SlotState::Reserved { .. } => entry.state = SlotState::Free,
            SlotState::Resident { .. } => {
                entry.generation += 1;
                // Leaves the window at the next tick's start; the cells stay for a snapshot
                // copied at that tick's end.
                model.retire_requested.push(index);
                return Ok(());
            }
            _ => return Err(Refusal::InvalidPayload(PayloadError::SlotState(slot))),
        }
        entry.generation += 1;
        Ok(())
    }

    fn capture_chunk(&mut self, slot: SlotRef) -> Result<CaptureTicket, Refusal> {
        Err(Refusal::StaleSlot(slot))
    }

    fn take_capture(
        &mut self,
        _: CaptureTicket,
        _: &mut [WorldCell],
    ) -> Result<CapturedChunk, Refusal> {
        Err(Refusal::InvalidPayload(PayloadError::CaptureNotReady))
    }

    fn snapshot_chunk(&mut self, slot: SlotRef) -> Result<SnapshotTicket, Refusal> {
        let mut model = self.model();
        if model.lost {
            return Err(Refusal::DeviceLost);
        }
        let index = slot.index() as usize;
        if model.slots[index].generation != slot.generation()
            || !matches!(model.slots[index].state, SlotState::Resident { .. })
        {
            return Err(Refusal::StaleSlot(slot));
        }
        if model.snaps.len() >= 8 {
            return Err(Refusal::Pressure(Pressure::SnapshotImage));
        }
        let id = model.next_snap;
        model.next_snap += 1;
        model.snaps.insert(id, Snap::Requested { slot: index });
        Ok(SnapshotTicket::new(session(), 0, id, id))
    }

    fn snapshot_world(&mut self, _: SessionId) -> Result<WorldSnapshotTicket, Refusal> {
        let mut model = self.model();
        if model.lost {
            return Err(Refusal::DeviceLost);
        }
        if model.world.is_some() {
            return Err(Refusal::Pressure(Pressure::SnapshotImage));
        }
        let id = 1000 + model.next_snap;
        model.next_snap += 1;
        model.world = Some((id, false));
        Ok(WorldSnapshotTicket::new(session(), id))
    }

    fn take_snapshot(
        &mut self,
        ticket: SnapshotTicket,
        into: &mut [WorldCell],
    ) -> Result<(), Refusal> {
        let mut model = self.model();
        if ticket.image() == 0 {
            match model.snaps.remove(&ticket.entry()) {
                Some(Snap::Ready { cells }) => {
                    into.copy_from_slice(&cells);
                    Ok(())
                }
                _ => Err(Refusal::InvalidPayload(PayloadError::SnapshotNotReady)),
            }
        } else {
            let cells = model
                .world_cells
                .remove(&ticket.entry())
                .ok_or(Refusal::InvalidPayload(PayloadError::SnapshotNotReady))?;
            into.copy_from_slice(&cells);
            Ok(())
        }
    }

    fn take_body_snapshot(
        &mut self,
        _: WorldSnapshotTicket,
        into: &mut BodySet,
    ) -> Result<(), Refusal> {
        let mut model = self.model();
        *into = model
            .world_bodies
            .take()
            .ok_or(Refusal::InvalidPayload(PayloadError::SnapshotNotReady))?;
        model.world = None;
        Ok(())
    }

    fn submit_transfers(&mut self, _: SessionId) -> Result<bool, Refusal> {
        let mut model = self.model();
        if model.lost {
            return Err(Refusal::DeviceLost);
        }
        let pending = model
            .snaps
            .values()
            .any(|snap| matches!(snap, Snap::Requested { .. }))
            || matches!(model.world, Some((_, false)));
        if pending {
            let tick = model.last_tick;
            model.copy_snapshots(tick);
        }
        Ok(pending)
    }

    fn restore_bodies(&mut self, _: SessionId, bodies: &BodySet) -> Result<(), Refusal> {
        let mut model = self.model();
        model.bodies = bodies.clone();
        model.restored = Some(bodies.clone());
        Ok(())
    }

    fn preflight_resize(&self, _: &ResizeRequest) -> Result<(), Refusal> {
        Ok(())
    }

    fn resize(&mut self, request: &ResizeRequest) -> Result<ResizeReport, Refusal> {
        let mut model = self.model();
        if model.slots.iter().any(|slot| {
            matches!(
                slot.state,
                SlotState::Staged { .. } | SlotState::Reserved { .. } | SlotState::Retiring
            )
        }) || !model.retire_requested.is_empty()
            || model
                .snaps
                .values()
                .any(|snap| matches!(snap, Snap::Requested { .. }))
        {
            return Err(Refusal::Pressure(Pressure::SlotChanges));
        }
        let outside = model.slots.iter().any(|slot| {
            matches!(&slot.state, SlotState::Resident { key, .. } if !request.window.contains(*key))
        });
        if outside {
            return Err(Refusal::InvalidPayload(
                PayloadError::ResidentOutsideNewWindow { count: 1 },
            ));
        }
        let kept: Vec<SlotState> = model
            .slots
            .iter()
            .filter(|slot| matches!(slot.state, SlotState::Resident { .. }))
            .map(|slot| slot.state.clone())
            .collect();
        let floor = model
            .slots
            .iter()
            .map(|slot| slot.generation)
            .max()
            .unwrap_or(0)
            + 1;
        let mut slots = vec![
            Slot {
                generation: floor,
                state: SlotState::Free,
            };
            request.window.chunk_count() as usize
        ];
        let mut retained = Vec::new();
        for (index, state) in kept.into_iter().enumerate() {
            if let SlotState::Resident { key, .. } = &state {
                retained.push((*key, SlotRef::new(session(), index as u32, floor)));
            }
            slots[index].state = state;
        }
        model.slots = slots;
        model.window = request.window;
        model.resizes += 1;
        Ok(ResizeReport {
            retained,
            migrated_bytes: 0,
            requested_bytes: 0,
            peak_overlap_bytes: 0,
        })
    }

    fn poll(&mut self, outcomes: &mut Vec<Outcome>) -> PollReport {
        let mut model = self.model();
        for slot in &mut model.slots {
            if matches!(slot.state, SlotState::Retiring) {
                slot.state = SlotState::Free;
            }
        }
        let delivered = model.pending.len() as u32;
        outcomes.append(&mut model.pending);
        PollReport {
            health: if model.lost {
                PortHealth::Lost
            } else {
                PortHealth::Running
            },
            in_flight: 0,
            delivered,
        }
    }

    fn stop(&mut self, _: Duration, _: &mut Vec<Outcome>) -> DrainReport {
        DrainReport {
            health: PortHealth::Stopped,
            delivered: 0,
            abandoned: 0,
            waited: Duration::ZERO,
            timed_out: false,
        }
    }

    fn counters(&self) -> TransferCounters {
        let model = self.model();
        TransferCounters {
            ticks_submitted: model.ticks,
            ticks_observed: model.ticks,
            ..TransferCounters::default()
        }
    }
}

/// What the in-memory store holds.
#[derive(Debug, Default)]
pub struct MemoryState {
    pub chunks: HashMap<(ChunkKey, u64), Vec<WorldCell>>,
    pub heads: HashMap<ChunkKey, ChunkRevision>,
    pub bodies: HashMap<u64, BodySet>,
    pub manifests: Vec<CheckpointManifest>,
    pub corrupt: Vec<ChunkKey>,
    pub fail_writes: u32,
    pub fail_publishes: u32,
    pub hold_writes: bool,
    pub hold_reads: bool,
    pub reads: u64,
    pub writes: u64,
}

pub struct MemoryStore {
    header: WorldHeader,
    pub state: Mutex<MemoryState>,
    released: Condvar,
}

impl MemoryStore {
    pub fn new(window: ResidentWindow) -> Arc<Self> {
        let store = Self {
            header: WorldHeader {
                world: WorldId::new(77),
                generator: Generator {
                    revision: world_model::identity::GENERATOR_REVISION,
                    seed: 5,
                },
                rule_seed: 0,
                origin: WorldOrigin::Created,
            },
            state: Mutex::new(MemoryState::default()),
            released: Condvar::new(),
        };
        store
            .state
            .lock()
            .expect("state")
            .manifests
            .push(CheckpointManifest {
                id: CheckpointId::FIRST,
                tick: None,
                config: config(),
                window,
                chunks: 0,
                bodies: 0,
                epoch: SessionEpoch::FIRST,
            });
        store
            .state
            .lock()
            .expect("state")
            .bodies
            .insert(1, BodySet::default());
        Arc::new(store)
    }

    pub fn state(&self) -> std::sync::MutexGuard<'_, MemoryState> {
        self.state.lock().expect("state")
    }

    /// Lets held writes and reads continue.
    pub fn release(&self) {
        let mut state = self.state();
        state.hold_writes = false;
        state.hold_reads = false;
        drop(state);
        self.released.notify_all();
    }
}

impl WorldStore for MemoryStore {
    fn header(&self) -> &WorldHeader {
        &self.header
    }

    fn latest(&self) -> Result<CheckpointManifest, StoreError> {
        self.state()
            .manifests
            .last()
            .copied()
            .ok_or(StoreError::Incomplete)
    }

    fn read_chunk(
        &self,
        key: ChunkKey,
        revision: Option<ChunkRevision>,
        into: &mut ChunkPayload,
    ) -> Result<Option<ChunkRevision>, StoreError> {
        let mut state = self.state();
        while state.hold_reads {
            state = self.released.wait(state).expect("state");
        }
        state.reads += 1;
        if state.corrupt.contains(&key) {
            return Err(StoreError::Corrupt {
                item: StoredItem::Head(key),
                detail: "test corruption",
            });
        }
        let revision = match revision {
            Some(revision) => revision,
            None => match state.heads.get(&key) {
                Some(head) => *head,
                None => return Ok(None),
            },
        };
        match state.chunks.get(&(key, revision.value())) {
            Some(cells) => {
                into.cells_mut().copy_from_slice(cells);
                Ok(Some(revision))
            }
            None => Ok(None),
        }
    }

    fn read_bodies(&self, checkpoint: CheckpointId) -> Result<BodySet, StoreError> {
        self.state()
            .bodies
            .get(&checkpoint.value())
            .cloned()
            .ok_or(StoreError::Incomplete)
    }

    fn write_chunks(&self, rows: &[ChunkRow<'_>]) -> Result<(), StoreError> {
        let mut state = self.state();
        while state.hold_writes {
            state = self.released.wait(state).expect("state");
        }
        if state.fail_writes > 0 {
            state.fail_writes -= 1;
            return Err(StoreError::Io {
                operation: "write",
                detail: "injected".to_owned(),
            });
        }
        for row in rows {
            state.writes += 1;
            state.chunks.insert(
                (row.key, row.revision.value()),
                row.payload.cells().to_vec(),
            );
        }
        Ok(())
    }

    fn write_bodies(&self, checkpoint: CheckpointId, bodies: &BodySet) -> Result<(), StoreError> {
        self.state()
            .bodies
            .insert(checkpoint.value(), bodies.clone());
        Ok(())
    }

    fn publish(&self, publication: &Publication<'_>) -> Result<CheckpointManifest, StoreError> {
        let mut state = self.state();
        if state.fail_publishes > 0 {
            state.fail_publishes -= 1;
            return Err(StoreError::Io {
                operation: "publish",
                detail: "injected".to_owned(),
            });
        }
        let expected = state
            .manifests
            .last()
            .map_or(CheckpointId::FIRST, |m| m.id.next().expect("ids"));
        if publication.id != expected {
            return Err(StoreError::Conflict(Conflict::CheckpointOrder {
                expected,
                presented: publication.id,
            }));
        }
        for (key, revision) in publication.heads {
            if !state.chunks.contains_key(&(*key, revision.value())) {
                return Err(StoreError::Conflict(Conflict::MissingRevision {
                    key: *key,
                    revision: *revision,
                }));
            }
            if let Some(head) = state.heads.get(key)
                && head >= revision
            {
                return Err(StoreError::Conflict(Conflict::HeadNotNewer {
                    key: *key,
                    head: *head,
                    presented: *revision,
                }));
            }
        }
        for (key, revision) in publication.heads {
            state.heads.insert(*key, *revision);
        }
        let manifest = CheckpointManifest {
            id: publication.id,
            tick: publication.tick,
            config: publication.config,
            window: publication.window,
            chunks: state.heads.len() as u64,
            bodies: publication.bodies,
            epoch: publication.epoch,
        };
        state.manifests.push(manifest);
        Ok(manifest)
    }

    fn reclaim(&self, _: &[(ChunkKey, ChunkRevision)]) -> Result<ReclaimReport, StoreError> {
        Ok(ReclaimReport::default())
    }

    fn begin_session(&self) -> Result<SessionStart, StoreError> {
        Ok(SessionStart {
            previous_unclean: false,
            reclaimed: ReclaimReport::default(),
        })
    }

    fn end_session(&self) -> Result<(), StoreError> {
        Ok(())
    }
}

/// Seeds `store` with a published revision of `key` holding `value` cells.
pub fn seed(store: &MemoryStore, key: ChunkKey, revision: u64, value: u64) {
    let mut state = store.state();
    state.chunks.insert((key, revision), cells(value));
    state.heads.insert(key, ChunkRevision::new(revision));
}
