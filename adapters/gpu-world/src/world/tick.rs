//! One tick: encoding its submission and decoding its readback.
//!
//! ```text
//! copies:   tick parameters, event header reset, command records, slot patches, staged chunks;
//!           clears of the list counts, the chunk wake requests, the diagnostics record and the
//!           activated chunks' tile records; the wake requests of activations and retirements
//! pass 1:   begin_tick -> one dispatch per command, in order -> (injected events)
//! pass 2:   scan_tiles -> candidate_args -> wake_tiles (indirect) -> process_args
//! pass 3:   propose -> resolve -> react -> commit (indirect)
//! copies:   results, events, diagnostics, totals records, timestamps -> tick readback;
//!           requested captures -> capture staging
//! ```
//!
//! Every refusal happens before the tick reserves anything it keeps. Dispatches run in order:
//! wgpu places a barrier between dispatches that use the same read-write storage buffer
//! (wgpu-hal 29.0.4 orders no `STORAGE_READ_WRITE` use), so a later command reads an earlier
//! one's result and each material pass reads the finished output of the one before. The three
//! compute passes each record a begin and an end timestamp when timestamps are enabled. The
//! readback is mapped right after submission and decoded by a later poll.

use std::time::Instant;

use world_model::{
    command::Command,
    rules::{PhaseThresholds, RuleTick, granular_runs},
};
use world_runtime::port::{
    Outcome, PayloadError, Pressure, Refusal, SubmissionTicket, TickBatch, TickReport,
};

use super::{CaptureState, Execution, GpuWorld, TimestampSupport};
use crate::{
    commands::{self, Dispatch},
    layout::{
        ChunkSlotRecord, CommandRecord, DiagnosticsRecord, EventHeader, GpuRecord, ResultRecord,
        TickParams, TotalsRecord, activity, codes,
    },
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
        let unmatched = self
            .faults
            .as_ref()
            .is_some_and(|faults| faults.unmatched_transfers())
            && self.pipelines.react_unmatched.is_some();
        // Configuration commands take effect for this tick's material phases, the last one of
        // a kind winning; a change wakes every resident cell.
        let mut wake_all = std::mem::take(&mut self.wake_all_pending);
        for admitted in batch.commands {
            match admitted.command {
                Command::SetAmbientTemperature(temperature) => {
                    wake_all |= temperature != self.ambient_temperature;
                    self.ambient_temperature = temperature;
                }
                Command::SetSimulationRules(rules) => {
                    wake_all |= rules != self.rules;
                    self.rules = rules;
                }
                _ => {}
            }
        }
        self.scratch.patches.clear();
        self.scratch.staged.clear();
        self.slots.encode(
            serial,
            tick,
            &mut self.scratch.staged,
            &mut self.scratch.patches,
        );

        let layout = self.plan.tick_upload;
        let activity_layout = self.plan.activity;
        let [tick_lo, tick_hi] = tick.to_words();
        let rule_tick = RuleTick::new(self.rule_seed, tick.value());
        let mut rule_bits = self.rules.bits();
        if !granular_runs(tick.value()) {
            rule_bits &= !world_model::rules::SimulationRules::GRANULAR_BIT;
        }
        let phases = PhaseThresholds::at(self.ambient_temperature.degrees());
        let mut activity_flags = 0;
        if wake_all {
            activity_flags |= activity::ACTIVITY_WAKE_ALL;
        }
        if self.execution == Execution::Dense {
            activity_flags |= activity::ACTIVITY_DENSE;
        }
        let min_key = window.min_key();
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
            origin_key_x: min_key.key.x,
            origin_key_y: min_key.key.y,
            rules: rule_bits,
            activity_flags,
            phase_flags: (u32::from(phases.freeze.is_some()) * activity::PHASE_FREEZE)
                | (u32::from(phases.melt.is_some()) * activity::PHASE_MELT),
            freeze_threshold: phases.freeze.unwrap_or(0),
            melt_threshold: phases.melt.unwrap_or(0),
            rule_tick: rule_tick.tick,
            rule_key: rule_tick.key,
            parity: tick_lo & 1,
            tile_count: self.plan.tiles,
            requests_base: activity_layout.requests,
            candidates_base: activity_layout.candidates,
            process_base: activity_layout.process,
            records_base: activity_layout.records,
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
            self.tick_uploads
                .write(upload, layout.one, &1u32.to_le_bytes());
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
        // The list counts, this tick's chunk wake requests and its accounting start empty.
        let activity_buffer = &self.window.activity;
        let requests_offset = u64::from(activity_layout.requests) * 4;
        let requests_bytes = u64::from(window.chunk_count()) * 4;
        encoder.clear_buffer(
            activity_buffer,
            0,
            Some(u64::from(activity::HEADER_WORDS) * 4),
        );
        encoder.clear_buffer(activity_buffer, requests_offset, Some(requests_bytes));
        encoder.clear_buffer(&self.fixed.diagnostics, 0, Some(DiagnosticsRecord::BYTES));
        let mut cleared =
            u64::from(activity::HEADER_WORDS) * 4 + requests_bytes + DiagnosticsRecord::BYTES;
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
            // An activated chunk starts with empty tile records; both an activation and a
            // retirement wake the chunk's reach this tick.
            if patch.window_entry != codes::SLOT_NONE {
                let (offset, size) = activity_layout.chunk_records(patch.window_index);
                encoder.clear_buffer(activity_buffer, offset, Some(size));
                cleared += size;
            }
            encoder.copy_buffer_to_buffer(
                upload_buffer,
                layout.one,
                activity_buffer,
                requests_offset + u64::from(patch.window_index) * 4,
                4,
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
        let timestamp_writes = |begin: u32| {
            self.fixed
                .timestamps
                .as_ref()
                .map(|(query_set, _)| wgpu::ComputePassTimestampWrites {
                    query_set,
                    beginning_of_pass_write_index: Some(begin),
                    end_of_pass_write_index: Some(begin + 1),
                })
        };
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("tick commands"),
                timestamp_writes: timestamp_writes(0),
            });
            let bind_group = &self.bind_groups.world;
            pass.set_bind_group(0, bind_group, &[0]);
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
                        pass.set_bind_group(0, bind_group, &offset);
                        pass.dispatch_workgroups(groups, groups, 1);
                    }
                    Dispatch::QueryColumn => {
                        pass.set_pipeline(&self.pipelines.query_column);
                        pass.set_bind_group(0, bind_group, &offset);
                        pass.dispatch_workgroups(1, 1, 1);
                    }
                    Dispatch::Totals { chunks } => {
                        let [x, y, z] = Dispatch::totals_groups(chunks);
                        pass.set_pipeline(&self.pipelines.measure_totals);
                        pass.set_bind_group(0, bind_group, &offset);
                        pass.dispatch_workgroups(x, y, z);
                    }
                }
            }
            if let Some(flood_pipeline) = self.pipelines.flood_events.as_ref().filter(|_| flood > 0)
            {
                pass.set_pipeline(flood_pipeline);
                pass.set_bind_group(0, bind_group, &[0]);
                pass.dispatch_workgroups(flood.div_ceil(LINEAR_WORKGROUP_SIZE), 1, 1);
            }
        }
        let args = &self.fixed.dispatch_args;
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("tick activity"),
                timestamp_writes: timestamp_writes(2),
            });
            pass.set_bind_group(0, &self.bind_groups.sim, &[]);
            pass.set_pipeline(&self.pipelines.scan_tiles);
            pass.dispatch_workgroups(window.chunk_count(), 1, 1);
            pass.set_bind_group(1, &self.bind_groups.args, &[]);
            pass.set_pipeline(&self.pipelines.candidate_args);
            pass.dispatch_workgroups(1, 1, 1);
            pass.set_pipeline(&self.pipelines.wake_tiles);
            pass.dispatch_workgroups_indirect(args, u64::from(activity::ARGS_CANDIDATES) * 4);
            pass.set_pipeline(&self.pipelines.process_args);
            pass.dispatch_workgroups(1, 1, 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("tick material"),
                timestamp_writes: timestamp_writes(4),
            });
            pass.set_bind_group(0, &self.bind_groups.sim, &[]);
            let react = match &self.pipelines.react_unmatched {
                Some(faulty) if unmatched => faulty,
                _ => &self.pipelines.react,
            };
            let offset = u64::from(activity::ARGS_PROCESS) * 4;
            for pipeline in [
                &self.pipelines.propose,
                &self.pipelines.resolve,
                react,
                &self.pipelines.commit,
            ] {
                pass.set_pipeline(pipeline);
                pass.dispatch_workgroups_indirect(args, offset);
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
        encoder.copy_buffer_to_buffer(
            &self.fixed.diagnostics,
            0,
            readback_buffer,
            readback_layout.diagnostics,
            DiagnosticsRecord::BYTES,
        );
        let mut diagnostics_bytes = DiagnosticsRecord::BYTES;
        for (position, dispatch) in self.scratch.dispatches.iter().enumerate() {
            if matches!(dispatch, Dispatch::Totals { .. }) {
                let at = position as u64 * TotalsRecord::BYTES;
                encoder.copy_buffer_to_buffer(
                    &self.fixed.diagnostics,
                    DiagnosticsRecord::BYTES + at,
                    readback_buffer,
                    readback_layout.totals + at,
                    TotalsRecord::BYTES,
                );
                diagnostics_bytes += TotalsRecord::BYTES;
            }
        }
        let mut timestamp_bytes = 0;
        if let Some((query_set, resolve)) = &self.fixed.timestamps {
            encoder.resolve_query_set(query_set, 0..plan::TIMESTAMP_QUERIES, resolve, 0);
            encoder.copy_buffer_to_buffer(
                resolve,
                0,
                readback_buffer,
                readback_layout.timestamps,
                plan::TIMESTAMP_BYTES,
            );
            timestamp_bytes = plan::TIMESTAMP_BYTES;
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

        let request_bytes = patch_count * 4;
        let upload_bytes = TickParams::BYTES
            + EventHeader::BYTES
            + 4
            + commands_bytes
            + patch_count * (ChunkSlotRecord::BYTES + WINDOW_ENTRY_BYTES)
            + request_bytes;
        let readback_bytes = results_bytes + events_bytes + diagnostics_bytes + timestamp_bytes;
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
        counters.upload.tick_parameters += TickParams::BYTES + EventHeader::BYTES + 4;
        counters.upload.commands += commands_bytes;
        counters.upload.metadata +=
            patch_count * (ChunkSlotRecord::BYTES + WINDOW_ENTRY_BYTES) + request_bytes;
        counters.readback.results += results_bytes;
        counters.readback.events += events_bytes;
        counters.readback.diagnostics += diagnostics_bytes;
        counters.readback.timestamps += timestamp_bytes;
        counters.readback.captures += captured * CHUNK_BYTES;
        counters.device_clear_bytes += cleared;
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
                let events = &bytes[layout.events as usize..layout.diagnostics as usize];
                let totals = &bytes[layout.totals as usize..layout.timestamps as usize];
                let result =
                    readback::decode_results(results, totals, &record.expected, tick, outcomes)
                        .and_then(|()| readback::decode_events(events, capacity, tick, outcomes));
                if result.is_err() {
                    outcomes.truncate(start);
                }
                let cells = readback::decode_diagnostics(
                    &bytes[layout.diagnostics as usize..layout.totals as usize],
                );
                let times = period.and_then(|period| {
                    readback::decode_timestamps(&bytes[layout.timestamps as usize..], period)
                });
                result.map(|_| (cells, times))
            });
        let record = &mut self.ticks[index];
        record.active = false;
        let commands = record.expected.len() as u32;
        let report = TickReport {
            tick,
            commands,
            gpu_time: None,
            phases: None,
            latency: record.submitted_at.elapsed(),
            upload_bytes: record.upload_bytes,
            readback_bytes: record.readback_bytes,
            cells: Default::default(),
        };
        self.tick_readbacks.release(reservation);
        match decoded {
            Some(Ok((cells, times))) => {
                self.counters.ticks_observed += 1;
                outcomes.push(Outcome::TickCompleted(TickReport {
                    gpu_time: times.map(|(total, _)| total),
                    phases: times.map(|(_, phases)| phases),
                    cells,
                    ..report
                }));
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
