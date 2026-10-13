//! One tick: encoding its submission and decoding its readback.
//!
//! ```text
//! copies: tick parameters, event header reset, command records, slot patches, staged chunks
//! compute pass: begin_tick -> one dispatch per command, in order -> (injected events)
//! copies: results, events, timestamps -> tick readback; requested captures -> capture staging
//! ```
//!
//! Every refusal happens before the tick reserves anything it keeps. Commands of a tick run as
//! separate dispatches in command order: wgpu places a barrier between dispatches that use the
//! same read-write storage buffer (wgpu-hal 29.0.4 orders no `STORAGE_READ_WRITE` use), so a
//! later command reads an earlier one's result. The readback is mapped right after submission
//! and decoded by a later poll.

use std::time::Instant;

use world_runtime::port::{
    Outcome, PayloadError, Pressure, Refusal, SubmissionTicket, TickBatch, TickReport,
};

use super::{CaptureState, GpuWorld, TimestampSupport};
use crate::{
    commands::{self, Dispatch},
    layout::{ChunkSlotRecord, CommandRecord, EventHeader, GpuRecord, ResultRecord, TickParams},
    plan::{self, BufferRole, CHUNK_BYTES, LINEAR_WORKGROUP_SIZE, WINDOW_ENTRY_BYTES},
    readback::{self, DecodeError},
    staging::StagingRef,
};

impl GpuWorld {
    /// Encodes and submits one tick. Every refusal happens before anything is reserved for
    /// good; from the first copy on, the tick is accepted.
    pub(super) fn encode_tick(
        &mut self,
        batch: &TickBatch<'_>,
    ) -> Result<SubmissionTicket, Refusal> {
        self.admission()?;
        self.check_session(batch.session)?;
        if self.tick_exhausted {
            return Err(Refusal::TickExhausted);
        }
        if batch.tick != self.next_tick {
            return Err(Refusal::TickOutOfOrder {
                expected: self.next_tick,
                presented: batch.tick,
            });
        }
        let count = batch.commands.len();
        if count > self.transfer.commands_per_tick as usize {
            return Err(Refusal::InvalidPayload(PayloadError::TooManyCommands {
                count,
                limit: self.transfer.commands_per_tick,
            }));
        }
        let window = self.slots.window();
        let scratch = &mut self.scratch;
        scratch.records.clear();
        scratch.expected.clear();
        scratch.dispatches.clear();
        for (sequence, admitted) in batch.commands.iter().enumerate() {
            let (record, expected, dispatch) =
                commands::encode(sequence as u32, admitted, &window)?;
            scratch.records.push(record);
            scratch.expected.push(expected);
            scratch.dispatches.push(dispatch);
        }
        if !self.context.has_submission_capacity() {
            return Err(self.refuse(Pressure::InFlightSubmissions));
        }
        let Some(upload) = self.tick_uploads.reserve() else {
            return Err(self.refuse(Pressure::TickUploadStaging));
        };
        let Some(readback) = self.tick_readbacks.reserve() else {
            self.tick_uploads.release(upload);
            return Err(self.refuse(Pressure::TickReadbackStaging));
        };
        // Handles to the reserved buffers (reference-counted clones, not allocations). A fresh
        // reservation always names a buffer; the check keeps that invariant out of the
        // accepted path below.
        let (Some(upload_buffer), Some(readback_buffer)) = (
            self.tick_uploads.buffer(upload).cloned(),
            self.tick_readbacks.buffer(readback).cloned(),
        ) else {
            self.tick_uploads.release(upload);
            self.tick_readbacks.release(readback);
            return Err(Refusal::Faulted);
        };

        // Accepted from here on.
        let tick = batch.tick;
        let serial = self.context.next_serial();
        let flood = match (&self.faults, &self.pipelines.flood_events) {
            (Some(faults), Some(_)) => faults.take_flood(),
            _ => 0,
        };
        self.scratch.patches.clear();
        self.scratch.staged.clear();
        self.slots.encode(
            serial,
            tick,
            &mut self.scratch.staged,
            &mut self.scratch.patches,
        );

        let layout = self.plan.tick_upload;
        let [tick_lo, tick_hi] = tick.to_words();
        let params = TickParams {
            tick_lo,
            tick_hi,
            epoch: self.session.epoch.value(),
            command_count: count as u32,
            window_width: window.width(),
            window_height: window.height(),
            slot_capacity: self.slots.capacity(),
            event_capacity: self.transfer.events_per_tick,
            injected_events: flood,
            ..TickParams::default()
        };
        let header = EventHeader {
            capacity: self.transfer.events_per_tick,
            ..EventHeader::default()
        };
        let commands_bytes = count as u64 * CommandRecord::BYTES;
        let patch_count = self.scratch.patches.len() as u64;
        {
            let bytes = &mut self.scratch.bytes;
            bytes.clear();
            bytes.resize((layout.commands + commands_bytes) as usize, 0);
            params.write_bytes(&mut bytes[layout.params as usize..]);
            header.write_bytes(&mut bytes[layout.event_header as usize..]);
            let stride = CommandRecord::BYTES as usize;
            for (record, out) in self
                .scratch
                .records
                .iter()
                .zip(bytes[layout.commands as usize..].chunks_exact_mut(stride))
            {
                record.write_bytes(out);
            }
            self.tick_uploads.write(upload, 0, bytes);
            if patch_count > 0 {
                bytes.clear();
                bytes.resize((patch_count * ChunkSlotRecord::BYTES) as usize, 0);
                let stride = ChunkSlotRecord::BYTES as usize;
                for (patch, out) in self
                    .scratch
                    .patches
                    .iter()
                    .zip(bytes.chunks_exact_mut(stride))
                {
                    patch.record.write_bytes(out);
                }
                self.tick_uploads.write(upload, layout.slot_records, bytes);
                bytes.clear();
                for patch in &self.scratch.patches {
                    bytes.extend_from_slice(&patch.window_entry.to_le_bytes());
                }
                self.tick_uploads
                    .write(upload, layout.window_entries, bytes);
            }
        }
        self.tick_uploads.unmap_for_copy(upload);
        for staged in &self.scratch.staged {
            self.chunk_uploads.unmap_for_copy(staged.upload);
        }

        let device = self.context.device();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("tick"),
        });
        let (upload_buffer, readback_buffer) = (&upload_buffer, &readback_buffer);
        encoder.copy_buffer_to_buffer(
            upload_buffer,
            layout.params,
            &self.fixed.tick_params,
            0,
            TickParams::BYTES,
        );
        encoder.copy_buffer_to_buffer(
            upload_buffer,
            layout.event_header,
            &self.fixed.events,
            0,
            EventHeader::BYTES,
        );
        if commands_bytes > 0 {
            encoder.copy_buffer_to_buffer(
                upload_buffer,
                layout.commands,
                &self.fixed.commands,
                0,
                commands_bytes,
            );
        }
        for (index, patch) in self.scratch.patches.iter().enumerate() {
            let index = index as u64;
            encoder.copy_buffer_to_buffer(
                upload_buffer,
                layout.slot_records + index * ChunkSlotRecord::BYTES,
                &self.window.slot_records,
                u64::from(patch.slot) * ChunkSlotRecord::BYTES,
                ChunkSlotRecord::BYTES,
            );
            encoder.copy_buffer_to_buffer(
                upload_buffer,
                layout.window_entries + index * WINDOW_ENTRY_BYTES,
                &self.window.window_map,
                u64::from(patch.window_index) * WINDOW_ENTRY_BYTES,
                WINDOW_ENTRY_BYTES,
            );
        }
        for staged in &self.scratch.staged {
            if let Some(source) = self.chunk_uploads.buffer(staged.upload) {
                encoder.copy_buffer_to_buffer(
                    source,
                    0,
                    &self.window.cells,
                    u64::from(staged.slot.index()) * CHUNK_BYTES,
                    CHUNK_BYTES,
                );
            }
        }
        {
            let timestamp_writes = self.fixed.timestamps.as_ref().map(|(query_set, _)| {
                wgpu::ComputePassTimestampWrites {
                    query_set,
                    beginning_of_pass_write_index: Some(0),
                    end_of_pass_write_index: Some(1),
                }
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("tick commands"),
                timestamp_writes,
            });
            pass.set_bind_group(0, &self.bind_group, &[0]);
            if count > 0 {
                pass.set_pipeline(&self.pipelines.begin_tick);
                pass.dispatch_workgroups((count as u32).div_ceil(LINEAR_WORKGROUP_SIZE), 1, 1);
            }
            let stride = self.plan.dispatch_param_stride;
            for (index, dispatch) in self.scratch.dispatches.iter().enumerate() {
                let offset = [(index as u64 * stride) as u32];
                match *dispatch {
                    Dispatch::None => {}
                    Dispatch::Edit { groups } => {
                        pass.set_pipeline(&self.pipelines.apply_edit);
                        pass.set_bind_group(0, &self.bind_group, &offset);
                        pass.dispatch_workgroups(groups, groups, 1);
                    }
                    Dispatch::QueryColumn => {
                        pass.set_pipeline(&self.pipelines.query_column);
                        pass.set_bind_group(0, &self.bind_group, &offset);
                        pass.dispatch_workgroups(1, 1, 1);
                    }
                }
            }
            if let Some(flood_pipeline) = self.pipelines.flood_events.as_ref().filter(|_| flood > 0)
            {
                pass.set_pipeline(flood_pipeline);
                pass.set_bind_group(0, &self.bind_group, &[0]);
                pass.dispatch_workgroups(flood.div_ceil(LINEAR_WORKGROUP_SIZE), 1, 1);
            }
        }
        let readback_layout = self.plan.tick_readback;
        let results_bytes = count as u64 * ResultRecord::BYTES;
        if results_bytes > 0 {
            encoder.copy_buffer_to_buffer(
                &self.fixed.results,
                0,
                readback_buffer,
                readback_layout.results,
                results_bytes,
            );
        }
        let events_bytes = self.plan.buffer(BufferRole::Events).size;
        encoder.copy_buffer_to_buffer(
            &self.fixed.events,
            0,
            readback_buffer,
            readback_layout.events,
            events_bytes,
        );
        let mut timestamp_bytes = 0;
        if let Some((query_set, resolve)) = &self.fixed.timestamps {
            encoder.resolve_query_set(query_set, 0..2, resolve, 0);
            encoder.copy_buffer_to_buffer(
                resolve,
                0,
                readback_buffer,
                readback_layout.timestamps,
                plan::TIMESTAMP_PAIR_BYTES,
            );
            timestamp_bytes = plan::TIMESTAMP_PAIR_BYTES;
        }
        let mut captured = 0u64;
        for capture in &mut self.captures {
            if capture.state != CaptureState::Requested {
                continue;
            }
            if let Some(target) = self.capture_staging.buffer(capture.staging) {
                encoder.copy_buffer_to_buffer(
                    &self.window.cells,
                    u64::from(capture.ticket.slot().index()) * CHUNK_BYTES,
                    target,
                    0,
                    CHUNK_BYTES,
                );
                capture.state = CaptureState::Submitted { tick };
                captured += 1;
            }
        }

        let submitted = self.context.submit([encoder.finish()]);
        debug_assert_eq!(submitted, serial);
        self.note_submission();
        self.tick_uploads.submitted(upload, serial);
        self.tick_readbacks.submitted(readback, serial);
        for staged in &self.scratch.staged {
            self.chunk_uploads.submitted(staged.upload, serial);
        }
        for capture in &self.captures {
            if capture.state == (CaptureState::Submitted { tick }) {
                self.capture_staging.submitted(capture.staging, serial);
            }
        }

        let upload_bytes = TickParams::BYTES
            + EventHeader::BYTES
            + commands_bytes
            + patch_count * (ChunkSlotRecord::BYTES + WINDOW_ENTRY_BYTES);
        let readback_bytes = results_bytes + events_bytes + timestamp_bytes;
        let record = &mut self.ticks[readback.index as usize];
        record.active = true;
        record.tick = tick;
        record.readback = readback;
        record.submitted_at = Instant::now();
        record.expected.clear();
        record.expected.extend_from_slice(&self.scratch.expected);
        record.upload_bytes = upload_bytes as u32;
        record.readback_bytes = readback_bytes as u32;

        let counters = &mut self.counters;
        counters.upload.tick_parameters += TickParams::BYTES + EventHeader::BYTES;
        counters.upload.commands += commands_bytes;
        counters.upload.metadata += patch_count * (ChunkSlotRecord::BYTES + WINDOW_ENTRY_BYTES);
        counters.readback.results += results_bytes;
        counters.readback.events += events_bytes;
        counters.readback.timestamps += timestamp_bytes;
        counters.readback.captures += captured * CHUNK_BYTES;
        counters.ticks_submitted += 1;
        self.last_submitted = Some(tick);
        match tick.next() {
            Some(next) => self.next_tick = next,
            None => self.tick_exhausted = true,
        }
        self.refresh_counters();
        Ok(SubmissionTicket {
            session: self.session,
            tick,
            serial,
        })
    }

    /// Decodes a delivered tick readback.
    pub(super) fn decode_tick(&mut self, reservation: StagingRef, outcomes: &mut Vec<Outcome>) {
        let index = reservation.index as usize;
        let record = &self.ticks[index];
        if !record.active || record.readback != reservation {
            self.counters.stale_deliveries += 1;
            self.tick_readbacks.release(reservation);
            return;
        }
        let tick = record.tick;
        let layout = self.plan.tick_readback;
        let capacity = self.transfer.events_per_tick;
        let period = match self.timestamps {
            TimestampSupport::Enabled { period_ns } => Some(period_ns),
            _ => None,
        };
        let results_end = record.expected.len() as u64 * ResultRecord::BYTES;
        let decoded = self
            .tick_readbacks
            .read(reservation, 0..layout.size, |bytes| {
                let start = outcomes.len();
                let results =
                    &bytes[layout.results as usize..(layout.results + results_end) as usize];
                let events = &bytes[layout.events as usize..layout.timestamps as usize];
                let result = readback::decode_results(results, &record.expected, tick, outcomes)
                    .and_then(|()| readback::decode_events(events, capacity, tick, outcomes));
                if result.is_err() {
                    outcomes.truncate(start);
                }
                let gpu_time = period.and_then(|period| {
                    readback::decode_timestamps(&bytes[layout.timestamps as usize..], period)
                });
                result.map(|_| gpu_time)
            });
        let record = &mut self.ticks[index];
        record.active = false;
        let commands = record.expected.len() as u32;
        let report = TickReport {
            tick,
            commands,
            gpu_time: None,
            latency: record.submitted_at.elapsed(),
            upload_bytes: record.upload_bytes,
            readback_bytes: record.readback_bytes,
        };
        self.tick_readbacks.release(reservation);
        match decoded {
            Some(Ok(gpu_time)) => {
                self.counters.ticks_observed += 1;
                outcomes.push(Outcome::TickCompleted(TickReport { gpu_time, ..report }));
            }
            Some(Err(error)) => {
                // Restore the record so the fault abandons this tick with the others.
                self.ticks[index].active = true;
                self.fault(error, outcomes);
            }
            None => {
                self.ticks[index].active = true;
                self.fault(
                    DecodeError::Status {
                        position: u32::MAX,
                        status: u32::MAX,
                    },
                    outcomes,
                );
            }
        }
    }
}
