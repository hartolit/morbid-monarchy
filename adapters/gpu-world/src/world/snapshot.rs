//! Snapshots on the session's timeline (`snapshots.rs` holds their states), body restoration,
//! and submissions that carry only transfers.
//!
//! Copies are appended to the end of whichever submission comes next: a tick (the snapshot
//! holds the end of that tick) or a transfer submission (it holds the end of the last submitted
//! tick). Streaming copies of changed snapshots ride along in the same way, as capture staging
//! buffers become free. Body restoration copies go first in the session's first submission.

use world_model::{
    body::BodyHandle,
    cell::WorldCell,
    resume::{BodySet, SlotState},
    spatial::CHUNK_CELL_COUNT,
};
use world_runtime::{
    ids::{SessionId, Tick},
    port::{
        AbandonReason, Obligation, Outcome, PayloadError, Pressure, Refusal, SlotRef,
        SnapshotTicket, WorldSnapshotTicket,
    },
};

use super::GpuWorld;
use crate::{
    bodies::{self, BodyRecordError, BodyTable},
    layout::{BodyConfig, BodyState, GpuRecord},
    plan::CHUNK_BYTES,
    readback::DecodeError,
    snapshots::{
        BodyStream, BodyStreamState, ChunkStream, EntryState, Sheet, Source, WorldChunk,
        WorldImage, WorldState,
    },
    staging::StagingRef,
};

/// Device bytes of one body's records in a world image.
const BODY_RECORD_BYTES: u64 = BodyState::BYTES + BodyConfig::BYTES;

impl GpuWorld {
    /// The boundary a submission without a tick copies at: the last submitted tick, or the
    /// tick before the session's first when it has submitted none (the state it opened with).
    pub(super) fn boundary_tick(&self) -> Option<Tick> {
        self.last_submitted
            .or_else(|| self.first_tick.value().checked_sub(1).map(Tick::new))
    }

    pub(super) fn request_chunk_snapshot(
        &mut self,
        slot: SlotRef,
    ) -> Result<SnapshotTicket, Refusal> {
        self.admission()?;
        self.check_session(slot.session())?;
        let (key, revision) = self.slots.resident_source(slot)?;
        let source = Source {
            slot: slot.index(),
            key,
            revision,
        };
        match self.snapshots.reserve(self.session, source) {
            Some(ticket) => Ok(ticket),
            None => Err(self.refuse(Pressure::SnapshotImage)),
        }
    }

    pub(super) fn request_world_snapshot(
        &mut self,
        session: SessionId,
    ) -> Result<WorldSnapshotTicket, Refusal> {
        self.admission()?;
        self.check_session(session)?;
        if self.snapshots.world.is_some() {
            return Err(self.refuse(Pressure::SnapshotImage));
        }
        let cell_bytes = u64::from(self.slots.capacity()) * CHUNK_BYTES;
        let bytes = cell_bytes + u64::from(self.transfer.bodies) * BODY_RECORD_BYTES;
        let buffer = self
            .context
            .create_buffer(
                "world snapshot",
                bytes,
                wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
                false,
            )
            .map_err(|_| {
                Refusal::Unsupported(world_runtime::port::CapacityLimit {
                    resource: world_runtime::port::LimitedResource::Allocation,
                    requested: bytes,
                    available: 0,
                })
            })?;
        let id = self.snapshots.next_world;
        self.snapshots.next_world = id.checked_add(1).unwrap_or(1);
        self.snapshots.world = Some(WorldImage {
            id,
            config: None,
            buffer,
            bytes,
            cell_bytes,
            state: WorldState::Requested,
            chunks: Vec::new(),
            bodies: BodyStream {
                slots: Vec::new(),
                extent: 0,
                bytes: Vec::new(),
                next: 0,
                pieces: Vec::new(),
                received: 0,
                state: BodyStreamState::Pending,
            },
        });
        self.refresh_counters();
        Ok(WorldSnapshotTicket::new(self.session, id))
    }

    /// Appends the body restoration copies of the session's first submission.
    pub(super) fn encode_restore(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        submitted: &mut Vec<StagingRef>,
    ) {
        for (staging, offset, length) in std::mem::take(&mut self.body_restore) {
            if let Some(source) = self.chunk_uploads.buffer(staging) {
                encoder.copy_buffer_to_buffer(source, 0, &self.fixed.bodies, offset, length);
                self.counters.upload.chunks += length;
                submitted.push(staging);
            }
        }
    }

    /// Appends requested snapshot copies and streaming copies to a submission being encoded.
    /// The capture staging buffers it reserved are listed in `submitted`.
    pub(super) fn encode_snapshots(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        tick: Option<Tick>,
        submitted: &mut Vec<StagingRef>,
    ) {
        let world_requested = self
            .snapshots
            .world
            .as_ref()
            .is_some_and(|world| world.state == WorldState::Requested);
        let chunks_requested = self
            .snapshots
            .entries
            .iter()
            .any(|entry| matches!(entry.state, EntryState::Requested(_)));
        let layout = self.plan.activity;
        let slots = self.slots.capacity();
        if (world_requested || chunks_requested)
            && let Some(sheet) = self.capture_staging.reserve()
            && let Some(sheet_buffer) = self.capture_staging.buffer(sheet).cloned()
        {
            let activity = &self.window.activity;
            // The sheet holds the world's change words first, then the chunk snapshots'. The
            // chunk snapshots are copied first: a slot both name is then unchanged for the
            // world, which is true relative to the chunk snapshot taken at the same boundary.
            let chunk_base = if world_requested { slots } else { 0 };
            let mut position = chunk_base;
            for entry_index in 0..self.snapshots.entries.len() {
                let EntryState::Requested(source) = self.snapshots.entries[entry_index].state
                else {
                    continue;
                };
                encoder.copy_buffer_to_buffer(
                    &self.window.cells,
                    u64::from(source.slot) * CHUNK_BYTES,
                    &self.snapshots.image,
                    entry_index as u64 * CHUNK_BYTES,
                    CHUNK_BYTES,
                );
                let word = layout.change_word(source.slot);
                encoder.copy_buffer_to_buffer(
                    activity,
                    word,
                    &sheet_buffer,
                    u64::from(position) * 4,
                    4,
                );
                encoder.clear_buffer(activity, word, Some(4));
                self.snapshots.entries[entry_index].state = EntryState::Copied {
                    source,
                    tick,
                    sheet,
                    position,
                };
                position += 1;
                self.counters.snapshot_copy_bytes += CHUNK_BYTES;
            }
            let mut world_id = None;
            if world_requested {
                let extent = self.bodies.extent();
                let resident = self.slots.snapshot_sources();
                let body_slots = self.bodies.export();
                let bodies_layout = self.plan.bodies;
                let config = world_runtime::store::PhysicalConfig {
                    rules: self.rules,
                    ambient_temperature: self.ambient_temperature,
                    elevation_scale: self.elevation_scale,
                };
                if let Some(world) = self.snapshots.world.as_mut() {
                    world.config = Some(config);
                    encoder.copy_buffer_to_buffer(
                        &self.window.cells,
                        0,
                        &world.buffer,
                        0,
                        world.cell_bytes,
                    );
                    let changes = layout.change_word(0);
                    encoder.copy_buffer_to_buffer(
                        activity,
                        changes,
                        &sheet_buffer,
                        0,
                        u64::from(slots) * 4,
                    );
                    encoder.clear_buffer(activity, changes, Some(u64::from(slots) * 4));
                    let states = u64::from(extent) * BodyState::BYTES;
                    let configs = u64::from(extent) * BodyConfig::BYTES;
                    if extent > 0 {
                        encoder.copy_buffer_to_buffer(
                            &self.fixed.bodies,
                            u64::from(bodies_layout.state) * 4,
                            &world.buffer,
                            world.cell_bytes,
                            states,
                        );
                        encoder.copy_buffer_to_buffer(
                            &self.fixed.bodies,
                            u64::from(bodies_layout.config) * 4,
                            &world.buffer,
                            world.cell_bytes + states,
                            configs,
                        );
                    }
                    world.chunks = resident
                        .into_iter()
                        .map(|source| WorldChunk {
                            source,
                            stream: ChunkStream::Copied,
                        })
                        .collect();
                    world.bodies = BodyStream {
                        slots: body_slots,
                        extent,
                        bytes: vec![0; (states + configs) as usize],
                        next: 0,
                        pieces: Vec::new(),
                        received: 0,
                        state: BodyStreamState::Pending,
                    };
                    world.state = WorldState::Copied { tick, sheet };
                    world_id = Some(world.id);
                    self.counters.snapshot_copy_bytes += world.cell_bytes + states + configs;
                }
            }
            self.counters.readback.snapshots += u64::from(position) * 4;
            self.snapshots.sheets.push(Sheet {
                staging: sheet,
                world: world_id,
            });
            submitted.push(sheet);
        }
        // Without a free buffer for the sheet, the requested snapshots wait for the next
        // submission; nothing is overwritten.
        self.encode_streams(encoder, submitted);
    }

    /// Streams changed snapshot cells and world body pieces into free capture staging buffers.
    fn encode_streams(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        submitted: &mut Vec<StagingRef>,
    ) {
        for entry_index in 0..self.snapshots.entries.len() {
            let EntryState::Changed { source, tick } = self.snapshots.entries[entry_index].state
            else {
                continue;
            };
            let Some(staging) = self.capture_staging.reserve() else {
                return;
            };
            let Some(target) = self.capture_staging.buffer(staging) else {
                self.capture_staging.release(staging);
                continue;
            };
            encoder.copy_buffer_to_buffer(
                &self.snapshots.image,
                entry_index as u64 * CHUNK_BYTES,
                target,
                0,
                CHUNK_BYTES,
            );
            self.snapshots.entries[entry_index].state = EntryState::Streaming {
                source,
                tick,
                staging,
            };
            self.counters.readback.snapshots += CHUNK_BYTES;
            submitted.push(staging);
        }
        let Some(world) = self.snapshots.world.as_mut() else {
            return;
        };
        if !matches!(world.state, WorldState::Decoded { .. }) {
            return;
        }
        for chunk in &mut world.chunks {
            if chunk.stream != ChunkStream::Changed {
                continue;
            }
            let Some(staging) = self.capture_staging.reserve() else {
                return;
            };
            let Some(target) = self.capture_staging.buffer(staging) else {
                self.capture_staging.release(staging);
                continue;
            };
            encoder.copy_buffer_to_buffer(
                &world.buffer,
                u64::from(chunk.source.slot) * CHUNK_BYTES,
                target,
                0,
                CHUNK_BYTES,
            );
            chunk.stream = ChunkStream::Streaming(staging);
            self.counters.readback.snapshots += CHUNK_BYTES;
            submitted.push(staging);
        }
        let total = world.bodies.bytes.len() as u64;
        while world.bodies.next < total {
            let Some(staging) = self.capture_staging.reserve() else {
                return;
            };
            let Some(target) = self.capture_staging.buffer(staging) else {
                self.capture_staging.release(staging);
                continue;
            };
            let offset = world.bodies.next;
            let length = (total - offset).min(CHUNK_BYTES);
            encoder.copy_buffer_to_buffer(
                &world.buffer,
                world.cell_bytes + offset,
                target,
                0,
                length,
            );
            world.bodies.pieces.push((staging, offset, length));
            world.bodies.next += length;
            self.counters.readback.snapshots += length;
            submitted.push(staging);
        }
    }

    /// Handles a mapped capture staging buffer that belongs to a snapshot. `false` when it is
    /// an inspection capture's (or stale).
    pub(super) fn snapshot_ready(
        &mut self,
        reservation: StagingRef,
        outcomes: &mut Vec<Outcome>,
    ) -> bool {
        let session = self.session;
        if let Some(position) = self
            .snapshots
            .sheets
            .iter()
            .position(|sheet| sheet.staging == reservation)
        {
            let sheet = self.snapshots.sheets.swap_remove(position);
            let words = self
                .capture_staging
                .read(reservation, 0..CHUNK_BYTES, |bytes| {
                    bytes
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|word| u32::from_le_bytes(*word))
                        .collect::<Vec<u32>>()
                })
                .unwrap_or_default();
            self.capture_staging.release(reservation);
            self.decode_sheet(sheet, &words, outcomes);
            return true;
        }
        for index in 0..self.snapshots.entries.len() {
            if let EntryState::Streaming {
                source,
                tick,
                staging,
            } = self.snapshots.entries[index].state
                && staging == reservation
            {
                self.snapshots.entries[index].state = EntryState::Ready {
                    source,
                    tick,
                    staging,
                };
                outcomes.push(Outcome::Snapshot {
                    ticket: self.snapshots.entry_ticket(session, index),
                    key: source.key,
                    revision: source.revision,
                    tick,
                    changed: true,
                });
                return true;
            }
        }
        let Some(world) = self.snapshots.world.as_mut() else {
            return false;
        };
        let WorldState::Decoded { tick } = world.state else {
            return false;
        };
        let id = world.id;
        for chunk in &mut world.chunks {
            if chunk.stream == ChunkStream::Streaming(reservation) {
                chunk.stream = ChunkStream::Ready(reservation);
                outcomes.push(Outcome::Snapshot {
                    ticket: SnapshotTicket::new(session, id, chunk.source.slot, id),
                    key: chunk.source.key,
                    revision: chunk.source.revision,
                    tick,
                    changed: true,
                });
                return true;
            }
        }
        if let Some(position) = world
            .bodies
            .pieces
            .iter()
            .position(|(staging, ..)| *staging == reservation)
        {
            let (_, offset, length) = world.bodies.pieces.swap_remove(position);
            let range = offset as usize..(offset + length) as usize;
            let copied = self
                .capture_staging
                .read(reservation, 0..length, |bytes| {
                    world.bodies.bytes[range].copy_from_slice(bytes);
                })
                .is_some();
            if copied {
                world.bodies.received += length;
            }
            self.capture_staging.release(reservation);
            if !copied {
                self.fault(
                    DecodeError::Status {
                        position: u32::MAX,
                        status: u32::MAX,
                    },
                    outcomes,
                );
                return true;
            }
            self.finish_bodies(outcomes);
            return true;
        }
        false
    }

    fn decode_sheet(&mut self, sheet: Sheet, words: &[u32], outcomes: &mut Vec<Outcome>) {
        let session = self.session;
        let flag = |position: u32| words.get(position as usize).copied().unwrap_or(1) != 0;
        for index in 0..self.snapshots.entries.len() {
            let EntryState::Copied {
                source,
                tick,
                sheet: entry_sheet,
                position,
            } = self.snapshots.entries[index].state
            else {
                continue;
            };
            if entry_sheet != sheet.staging {
                continue;
            }
            if flag(position) {
                self.snapshots.entries[index].state = EntryState::Changed { source, tick };
            } else {
                outcomes.push(Outcome::Snapshot {
                    ticket: self.snapshots.entry_ticket(session, index),
                    key: source.key,
                    revision: source.revision,
                    tick,
                    changed: false,
                });
                self.snapshots.entries[index].state = EntryState::Free;
            }
        }
        if let (Some(id), Some(world)) = (sheet.world, self.snapshots.world.as_mut())
            && world.id == id
            && let WorldState::Copied { tick, .. } = world.state
        {
            let id = world.id;
            for chunk in &mut world.chunks {
                if flag(chunk.source.slot) {
                    chunk.stream = ChunkStream::Changed;
                } else {
                    chunk.stream = ChunkStream::Unchanged;
                    outcomes.push(Outcome::Snapshot {
                        ticket: SnapshotTicket::new(session, id, chunk.source.slot, id),
                        key: chunk.source.key,
                        revision: chunk.source.revision,
                        tick,
                        changed: false,
                    });
                }
            }
            world.state = WorldState::Decoded { tick };
            self.finish_bodies(outcomes);
        }
        self.release_drained_world();
    }

    /// Decodes the world snapshot's bodies once every piece arrived.
    fn finish_bodies(&mut self, outcomes: &mut Vec<Outcome>) {
        let session = self.session;
        let capacity = self.transfer.bodies;
        let Some(world) = self.snapshots.world.as_mut() else {
            return;
        };
        let WorldState::Decoded { tick } = world.state else {
            return;
        };
        let stream = &mut world.bodies;
        let total = stream.bytes.len() as u64;
        if stream.state != BodyStreamState::Pending
            || stream.next < total
            || stream.received < total
        {
            return;
        }
        let extent = stream.extent as usize;
        let mut set = BodySet {
            slots: stream.slots.clone(),
            bodies: Vec::new(),
        };
        let configs = extent * BodyState::BYTES as usize;
        let mut invalid = false;
        for (index, slot) in stream.slots.iter().enumerate() {
            if slot.state != SlotState::Live || index >= extent {
                continue;
            }
            let state = BodyState::read_bytes(&stream.bytes[index * BodyState::BYTES as usize..]);
            let config = BodyConfig::read_bytes(
                &stream.bytes[configs + index * BodyConfig::BYTES as usize..],
            );
            let handle = BodyHandle::new(index as u32, slot.generation);
            match bodies::resume_from_device(handle, &state, &config) {
                Ok(body) => set.bodies.push(body),
                Err(BodyRecordError::NotLive) => {
                    // A spawn of the copied tick whose target missed: the device left the slot
                    // dead and the CPU frees it when that tick's outcomes arrive. Record it as
                    // that release will leave it.
                    set.slots[index] = match slot.generation.checked_add(1) {
                        Some(generation) => world_model::resume::BodySlot {
                            generation,
                            state: SlotState::Free,
                        },
                        None => world_model::resume::BodySlot {
                            generation: slot.generation,
                            state: SlotState::Retired,
                        },
                    };
                }
                Err(BodyRecordError::Description) => invalid = true,
            }
        }
        if invalid || set.validate(capacity).is_err() {
            // A live body's device record is outside the envelope: the readback cannot be
            // trusted, so the session faults rather than save it.
            self.fault(
                DecodeError::Status {
                    position: u32::MAX,
                    status: u32::MAX,
                },
                outcomes,
            );
            return;
        }
        let Some(world) = self.snapshots.world.as_mut() else {
            return;
        };
        let Some(config) = world.config else {
            return;
        };
        let bodies = set.live() as u32;
        world.bodies.state = BodyStreamState::Ready(set);
        world.bodies.bytes = Vec::new();
        outcomes.push(Outcome::BodySnapshot {
            ticket: world.ticket(session),
            tick,
            bodies,
            chunks: world.chunks.len() as u32,
            config,
        });
    }

    /// Frees the world image once everything in it was reported and taken.
    fn release_drained_world(&mut self) {
        if self
            .snapshots
            .world
            .as_ref()
            .is_some_and(WorldImage::drained)
            && let Some(world) = self.snapshots.world.take()
        {
            self.context.release_buffer(world.buffer);
            self.refresh_counters();
        }
    }

    pub(super) fn take_snapshot_cells(
        &mut self,
        ticket: SnapshotTicket,
        into: &mut [WorldCell],
    ) -> Result<(), Refusal> {
        self.check_session(ticket.session())?;
        if matches!(
            self.lifecycle,
            super::Lifecycle::Lost | super::Lifecycle::Faulted
        ) {
            return Err(Refusal::DeviceLost);
        }
        if into.len() != CHUNK_CELL_COUNT {
            return Err(Refusal::InvalidPayload(PayloadError::CellCount {
                expected: CHUNK_CELL_COUNT,
                presented: into.len(),
            }));
        }
        let not_ready = Refusal::InvalidPayload(PayloadError::SnapshotNotReady);
        let staging = if ticket.image() == 0 {
            let index = self.snapshots.entry_index(ticket).ok_or(not_ready)?;
            let EntryState::Ready { staging, .. } = self.snapshots.entries[index].state else {
                return Err(not_ready);
            };
            self.snapshots.entries[index].state = EntryState::Free;
            staging
        } else {
            let world = self
                .snapshots
                .world
                .as_mut()
                .filter(|world| world.id == ticket.image())
                .ok_or(not_ready)?;
            let chunk = world
                .chunks
                .iter_mut()
                .find(|chunk| chunk.source.slot == ticket.entry())
                .ok_or(not_ready)?;
            let ChunkStream::Ready(staging) = chunk.stream else {
                return Err(not_ready);
            };
            chunk.stream = ChunkStream::Taken;
            staging
        };
        let copied = self.capture_staging.read(staging, 0..CHUNK_BYTES, |bytes| {
            for (cell, word) in into.iter_mut().zip(bytes.as_chunks::<8>().0) {
                *cell = WorldCell(u64::from_le_bytes(*word));
            }
        });
        self.capture_staging.release(staging);
        self.release_drained_world();
        copied.ok_or(Refusal::Faulted)
    }

    pub(super) fn take_world_bodies(
        &mut self,
        ticket: WorldSnapshotTicket,
        into: &mut BodySet,
    ) -> Result<(), Refusal> {
        self.check_session(ticket.session())?;
        let not_ready = Refusal::InvalidPayload(PayloadError::SnapshotNotReady);
        let world = self
            .snapshots
            .world
            .as_mut()
            .filter(|world| world.id == ticket.id())
            .ok_or(not_ready)?;
        match std::mem::replace(&mut world.bodies.state, BodyStreamState::Taken) {
            BodyStreamState::Ready(set) => {
                *into = set;
                self.release_drained_world();
                Ok(())
            }
            other => {
                world.bodies.state = other;
                Err(not_ready)
            }
        }
    }

    /// Reports every snapshot obligation abandoned. Ready payloads are kept when `keep_ready`
    /// (an orderly stop: they are already in CPU-visible memory).
    pub(super) fn abandon_snapshots(
        &mut self,
        reason: AbandonReason,
        keep_ready: bool,
        outcomes: &mut Vec<Outcome>,
    ) {
        let session = self.session;
        for index in 0..self.snapshots.entries.len() {
            let state = self.snapshots.entries[index].state;
            let keep = keep_ready && matches!(state, EntryState::Ready { .. });
            if state == EntryState::Free || keep {
                continue;
            }
            outcomes.push(Outcome::Abandoned {
                obligation: Obligation::Snapshot(self.snapshots.entry_ticket(session, index)),
                reason,
            });
            self.snapshots.entries[index].state = EntryState::Free;
        }
        self.snapshots.sheets.clear();
        let Some(mut world) = self.snapshots.world.take() else {
            return;
        };
        let chunks = world.chunks.len() as u32;
        let id = world.id;
        let mut kept = false;
        for chunk in &mut world.chunks {
            if chunk.stream.done() {
                continue;
            }
            if keep_ready && matches!(chunk.stream, ChunkStream::Ready(_)) {
                kept = true;
                continue;
            }
            outcomes.push(Outcome::Abandoned {
                obligation: Obligation::Snapshot(SnapshotTicket::new(
                    session,
                    id,
                    chunk.source.slot,
                    id,
                )),
                reason,
            });
            chunk.stream = ChunkStream::Unchanged;
        }
        match &world.bodies.state {
            BodyStreamState::Taken => {}
            BodyStreamState::Ready(_) if keep_ready => kept = true,
            _ => {
                outcomes.push(Outcome::Abandoned {
                    obligation: Obligation::WorldSnapshot {
                        ticket: world.ticket(session),
                        chunks,
                    },
                    reason,
                });
                world.bodies.state = BodyStreamState::Taken;
            }
        }
        if kept {
            world.bodies.pieces.clear();
            self.snapshots.world = Some(world);
        } else {
            self.context.release_buffer(world.buffer);
        }
    }

    /// Abandons snapshots that can no longer be copied or streamed because the session stopped
    /// submitting: requested ones and changed ones waiting for a stream.
    pub(super) fn abandon_unsubmitted_snapshots(&mut self, outcomes: &mut Vec<Outcome>) -> u32 {
        let session = self.session;
        let mut abandoned = 0;
        for index in 0..self.snapshots.entries.len() {
            if matches!(
                self.snapshots.entries[index].state,
                EntryState::Requested(_) | EntryState::Changed { .. }
            ) {
                outcomes.push(Outcome::Abandoned {
                    obligation: Obligation::Snapshot(self.snapshots.entry_ticket(session, index)),
                    reason: AbandonReason::Stopped,
                });
                self.snapshots.entries[index].state = EntryState::Free;
                abandoned += 1;
            }
        }
        let unfinished = self.snapshots.world.as_ref().is_some_and(|world| {
            world.state == WorldState::Requested
                || world
                    .chunks
                    .iter()
                    .any(|chunk| chunk.stream == ChunkStream::Changed)
                || world.bodies.next < world.bodies.bytes.len() as u64
        });
        if unfinished {
            let before = outcomes.len();
            self.abandon_snapshots(AbandonReason::Stopped, true, outcomes);
            abandoned += (outcomes.len() - before) as u32;
        }
        abandoned
    }

    /// Restores a checkpoint's bodies before the session's first submission.
    pub(super) fn restore(&mut self, session: SessionId, set: &BodySet) -> Result<(), Refusal> {
        self.admission()?;
        self.check_session(session)?;
        if self.submitted_any || !self.body_restore.is_empty() {
            return Err(Refusal::InvalidPayload(PayloadError::RestoreAfterStart));
        }
        set.validate(self.transfer.bodies)
            .map_err(|error| Refusal::InvalidPayload(PayloadError::BodySet(error)))?;
        let table = BodyTable::restored(self.transfer.bodies, &set.slots).ok_or(
            Refusal::InvalidPayload(PayloadError::BodySet(
                world_model::resume::BodySetError::Capacity {
                    slots: set.slots.len(),
                    capacity: self.transfer.bodies,
                },
            )),
        )?;
        let extent = table.extent() as usize;
        let mut states = vec![0u8; extent * BodyState::BYTES as usize];
        let mut configs = vec![0u8; extent * BodyConfig::BYTES as usize];
        for body in &set.bodies {
            let index = body.handle.index() as usize;
            let (state, config) = bodies::device_from_resume(body);
            state.write_bytes(&mut states[index * BodyState::BYTES as usize..]);
            config.write_bytes(&mut configs[index * BodyConfig::BYTES as usize..]);
        }
        let layout = self.plan.bodies;
        let mut pieces: Vec<(StagingRef, u64, u64)> = Vec::new();
        for (bytes, base) in [
            (&states, u64::from(layout.state) * 4),
            (&configs, u64::from(layout.config) * 4),
        ] {
            for (index, piece) in bytes.chunks(CHUNK_BYTES as usize).enumerate() {
                let Some(staging) = self.chunk_uploads.reserve() else {
                    for (staging, ..) in pieces {
                        self.chunk_uploads.release(staging);
                    }
                    return Err(self.refuse(Pressure::ChunkUploadStaging));
                };
                self.chunk_uploads.write(staging, 0, piece);
                self.chunk_uploads.unmap_for_copy(staging);
                pieces.push((
                    staging,
                    base + index as u64 * CHUNK_BYTES,
                    piece.len() as u64,
                ));
            }
        }
        self.bodies = table;
        self.body_restore = pieces;
        Ok(())
    }

    /// Submits pending snapshot, streaming and restoration copies without a tick.
    pub(super) fn submit_transfer(&mut self, session: SessionId) -> Result<bool, Refusal> {
        self.admission()?;
        self.check_session(session)?;
        if !self.snapshots.has_pending_copies() && self.body_restore.is_empty() {
            return Ok(false);
        }
        if !self.context.has_submission_capacity() {
            return Err(self.refuse(Pressure::InFlightSubmissions));
        }
        let mut encoder =
            self.context
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("transfers"),
                });
        let mut uploads = Vec::new();
        self.encode_restore(&mut encoder, &mut uploads);
        let mut readbacks = Vec::new();
        let tick = self.boundary_tick();
        self.encode_snapshots(&mut encoder, tick, &mut readbacks);
        if uploads.is_empty() && readbacks.is_empty() {
            // Nothing could be encoded (no free staging): nothing is submitted.
            return Ok(false);
        }
        let serial = self.context.submit([encoder.finish()]);
        self.note_submission();
        self.submitted_any = true;
        for staging in uploads {
            self.chunk_uploads.submitted(staging, serial);
        }
        for staging in readbacks {
            self.capture_staging.submitted(staging, serial);
        }
        self.counters.transfer_submissions += 1;
        self.refresh_counters();
        Ok(true)
    }
}
