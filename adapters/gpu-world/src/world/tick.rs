//! One tick: encoding its submission and decoding its readback.
//!
//! ```text
//! copies:   tick parameters, body parameters, event header reset, command records, the observe
//!           list, slot patches, staged chunks; clears of the list counts, the chunk wake
//!           requests, the diagnostics and body records and the activated chunks' tile
//!           records; the wake requests of activations and retirements
//! pass 1:   begin_tick -> per command, in order: resolve_target (ray-targeted commands and ray
//!           queries) then its own kernel -> (injected events)
//! pass 2:   scan_tiles -> candidate_args -> wake_tiles (indirect) -> process_args
//! pass 3:   propose -> resolve -> react -> commit (indirect)
//! pass 4:   per substep: bins_clear -> body_begin -> bins_count -> bins_scan -> bins_scatter
//!           -> pairs_count -> pairs_position x4 -> pairs_velocity x4 -> pairs_residual
//!           -> claims_clear -> contact_prepare -> contact_apply -> contact_response
//!           -> claims_clear -> displace_prepare -> displace_apply -> body_end;
//!           then body_observe
//! copies:   results, events, diagnostics, totals records, the body record, samples,
//!           timestamps -> tick readback; requested captures -> capture staging
//! ```
//!
//! Every refusal happens before the tick reserves anything it keeps, and before any command
//! takes or frees a body slot. Dispatches run in order:
//! wgpu places a barrier between dispatches that use the same read-write storage buffer
//! (wgpu-hal 29.0.4 orders no `STORAGE_READ_WRITE` use), so a later command reads an earlier
//! one's result and each material pass reads the finished output of the one before. The three
//! compute passes each record a begin and an end timestamp when timestamps are enabled. The
//! readback is mapped right after submission and decoded by a later poll.

use std::time::Instant;

use world_model::{
    body::substeps_per_tick,
    cell::WorldCell,
    command::{Command, CommandKind},
    config,
    rules::{PhaseThresholds, RuleTick, granular_runs},
};
use world_runtime::port::{
    CommandResult, Outcome, PayloadError, Pressure, Refusal, SubmissionTicket, TickBatch,
    TickReport,
};

use super::{Broadphase, CaptureState, Execution, GpuWorld, TimestampSupport};
use crate::{
    commands::{self, Dispatch},
    layout::{
        BodyDiagnosticsRecord, BodyParams, BodySampleRecord, ChunkSlotRecord, CommandRecord,
        DiagnosticsRecord, EventHeader, GpuRecord, ResultRecord, TickParams, TotalsRecord,
        activity, codes,
    },
    plan::{
        self, BODY_DISPATCH_ITERATIONS, BufferRole, CHUNK_BYTES, LINEAR_WORKGROUP_SIZE,
        WINDOW_ENTRY_BYTES,
    },
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
        if batch.observe.len() > self.transfer.body_samples_per_tick as usize {
            return Err(Refusal::InvalidPayload(PayloadError::TooManySamples {
                count: batch.observe.len(),
                limit: self.transfer.body_samples_per_tick,
            }));
        }
        if !(batch.tick_rate_hz.is_finite() && config::TICK_RATE_HZ.contains(&batch.tick_rate_hz)) {
            return Err(Refusal::InvalidPayload(PayloadError::TickRate));
        }
        let window = self.slots.window();
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

        // Accepted from here on: the commands may take and free body slots.
        let scratch = &mut self.scratch;
        scratch.records.clear();
        scratch.expected.clear();
        scratch.dispatches.clear();
        for (sequence, admitted) in batch.commands.iter().enumerate() {
            let (record, expected, dispatch) =
                commands::encode(sequence as u32, admitted, &window, &mut self.bodies);
            scratch.records.push(record);
            scratch.expected.push(expected);
            scratch.dispatches.push(dispatch);
        }
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
                Command::SetElevationScale(scale) => self.elevation_scale = scale,
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
        let body_extent = self.bodies.extent();
        let observe_count = batch.observe.len() as u32;
        let substeps = if body_extent > 0 {
            substeps_per_tick(batch.tick_rate_hz)
        } else {
            0
        };
        let tick_seconds = 1.0 / batch.tick_rate_hz;
        let bodies_layout = self.plan.bodies;
        let scale = self.elevation_scale.value();
        let max_surface = (WorldCell::MAX_ELEVATION
            + u32::from(WorldCell::MAX_GRANULAR_VOL)
            + u32::from(WorldCell::MAX_FLUID_VOL)) as f32
            * scale;
        let body_params = BodyParams {
            body_count: body_extent,
            capacity: bodies_layout.capacity,
            substeps,
            substep_seconds: (tick_seconds / f64::from(substeps.max(1))) as f32,
            tick_seconds: tick_seconds as f32,
            elevation_scale: scale,
            observe_count,
            broadphase: match self.broadphase {
                Broadphase::Bins => codes::BROADPHASE_BINS,
                Broadphase::AllPairs => codes::BROADPHASE_ALL_PAIRS,
            },
            hash_mask: bodies_layout.hash_slots - 1,
            state_base: bodies_layout.state,
            config_base: bodies_layout.config,
            work_base: bodies_layout.work,
            samples_base: bodies_layout.sample_records,
            observe_base: bodies_layout.observe,
            counts_base: bodies_layout.counts,
            starts_base: bodies_layout.starts,
            fill_base: bodies_layout.fill,
            entries_base: bodies_layout.entries,
            bounds_base: bodies_layout.bounds,
            diagnostics_base: bodies_layout.diagnostics,
            max_surface,
            ..BodyParams::default()
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
            bytes.clear();
            bytes.resize(BodyParams::BYTES as usize, 0);
            body_params.write_bytes(bytes);
            for body in batch.observe {
                bytes.extend_from_slice(&body.index().to_le_bytes());
                bytes.extend_from_slice(&body.generation().to_le_bytes());
            }
            self.tick_uploads.write(upload, layout.body_params, bytes);
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
        encoder.copy_buffer_to_buffer(
            upload_buffer,
            layout.body_params,
            &self.fixed.body_params,
            0,
            BodyParams::BYTES,
        );
        let observe_bytes = u64::from(observe_count) * 8;
        if observe_bytes > 0 {
            encoder.copy_buffer_to_buffer(
                upload_buffer,
                layout.observe,
                &self.fixed.bodies,
                u64::from(bodies_layout.observe) * 4,
                observe_bytes,
            );
        }
        let body_diagnostics_offset = u64::from(bodies_layout.diagnostics) * 4;
        encoder.clear_buffer(
            &self.fixed.diagnostics,
            body_diagnostics_offset,
            Some(BodyDiagnosticsRecord::BYTES),
        );
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
        let mut cleared = u64::from(activity::HEADER_WORDS) * 4
            + requests_bytes
            + DiagnosticsRecord::BYTES
            + BodyDiagnosticsRecord::BYTES;
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
            let body_groups = body_extent.div_ceil(LINEAR_WORKGROUP_SIZE);
            let body_commands = &self.pipelines.body_commands;
            for (index, dispatch) in self.scratch.dispatches.iter().enumerate() {
                let offset = [(index as u64 * stride) as u32];
                if dispatch.resolves_ray() {
                    pass.set_pipeline(&body_commands.resolve_target);
                    pass.set_bind_group(0, &self.bind_groups.body_commands, &offset);
                    pass.dispatch_workgroups(1, 1, 1);
                }
                let body_kernel = match *dispatch {
                    Dispatch::Spawn { .. } => Some((&body_commands.spawn_body, 1)),
                    Dispatch::Remove => Some((&body_commands.remove_body, 1)),
                    Dispatch::Drive => Some((&body_commands.drive_body, 1)),
                    Dispatch::Jump => Some((&body_commands.jump_body, 1)),
                    Dispatch::Pull { attract: true, .. } => {
                        Some((&body_commands.attract_bodies, body_groups.max(1)))
                    }
                    Dispatch::Pull { attract: false, .. } => {
                        Some((&body_commands.lift_bodies, body_groups.max(1)))
                    }
                    _ => None,
                };
                if let Some((pipeline, groups)) = body_kernel {
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, &self.bind_groups.body_commands, &offset);
                    pass.dispatch_workgroups(groups, 1, 1);
                    continue;
                }
                match *dispatch {
                    Dispatch::None
                    | Dispatch::QueryRay
                    | Dispatch::Spawn { .. }
                    | Dispatch::Remove
                    | Dispatch::Drive
                    | Dispatch::Jump
                    | Dispatch::Pull { .. } => {}
                    Dispatch::Edit { groups, .. } => {
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
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("tick bodies"),
                timestamp_writes: timestamp_writes(6),
            });
            self.encode_bodies(&mut pass, body_extent, substeps, observe_count);
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
        let body_record_bytes = BodyDiagnosticsRecord::BYTES;
        encoder.copy_buffer_to_buffer(
            &self.fixed.diagnostics,
            body_diagnostics_offset,
            readback_buffer,
            readback_layout.body_diagnostics,
            body_record_bytes,
        );
        diagnostics_bytes += body_record_bytes;
        let samples_bytes = u64::from(observe_count) * BodySampleRecord::BYTES;
        if samples_bytes > 0 {
            encoder.copy_buffer_to_buffer(
                &self.fixed.bodies,
                u64::from(bodies_layout.sample_records) * 4,
                readback_buffer,
                readback_layout.samples,
                samples_bytes,
            );
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
            + BodyParams::BYTES
            + observe_bytes
            + commands_bytes
            + patch_count * (ChunkSlotRecord::BYTES + WINDOW_ENTRY_BYTES)
            + request_bytes;
        let readback_bytes =
            results_bytes + events_bytes + diagnostics_bytes + samples_bytes + timestamp_bytes;
        let record = &mut self.ticks[readback.index as usize];
        record.active = true;
        record.tick = tick;
        record.readback = readback;
        record.submitted_at = Instant::now();
        record.expected.clear();
        record.expected.extend_from_slice(&self.scratch.expected);
        record.observed.clear();
        record.observed.extend_from_slice(batch.observe);
        record.substeps = substeps;
        record.upload_bytes = upload_bytes as u32;
        record.readback_bytes = readback_bytes as u32;

        let counters = &mut self.counters;
        counters.upload.tick_parameters +=
            TickParams::BYTES + EventHeader::BYTES + 4 + BodyParams::BYTES;
        counters.upload.commands += commands_bytes + observe_bytes;
        counters.readback.samples += samples_bytes;
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
        let substeps = record.substeps;
        let mut decoded_from = 0;
        let decoded = self
            .tick_readbacks
            .read(reservation, 0..layout.size, |bytes| {
                let start = outcomes.len();
                decoded_from = start;
                let results =
                    &bytes[layout.results as usize..(layout.results + results_end) as usize];
                let events = &bytes[layout.events as usize..layout.diagnostics as usize];
                let totals = &bytes[layout.totals as usize..layout.body_diagnostics as usize];
                let samples = &bytes[layout.samples as usize..layout.timestamps as usize];
                let result =
                    readback::decode_results(results, totals, &record.expected, tick, outcomes)
                        .and_then(|()| readback::decode_events(events, capacity, tick, outcomes))
                        .and_then(|_| {
                            readback::decode_samples(samples, &record.observed, tick, outcomes)
                        });
                if result.is_err() {
                    outcomes.truncate(start);
                }
                let cells = readback::decode_diagnostics(
                    &bytes[layout.diagnostics as usize..layout.totals as usize],
                );
                let bodies = readback::decode_body_diagnostics(
                    &bytes[layout.body_diagnostics as usize..layout.samples as usize],
                    substeps,
                );
                let times = period.and_then(|period| {
                    readback::decode_timestamps(&bytes[layout.timestamps as usize..], period)
                });
                result.map(|_| (cells, bodies, times))
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
            Some(Ok((cells, bodies, times))) => {
                self.counters.ticks_observed += 1;
                self.release_failed_spawns(index, decoded_from, outcomes);
                let tick = report.tick;
                outcomes.push(Outcome::TickCompleted(TickReport {
                    gpu_time: times.map(|(total, _)| total),
                    phases: times.map(|(_, phases)| phases),
                    cells,
                    ..report
                }));
                outcomes.push(Outcome::BodiesCompleted {
                    tick,
                    report: bodies,
                });
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

    /// Frees the slots of spawns whose target the device could not find: the device left
    /// them dead, and now the handle is stale here too.
    fn release_failed_spawns(&mut self, record: usize, from: usize, outcomes: &[Outcome]) {
        let expected = &self.ticks[record].expected;
        for (expected, outcome) in expected.iter().zip(&outcomes[from..]) {
            if expected.kind != CommandKind::SpawnBody {
                continue;
            }
            let failed = matches!(
                outcome,
                Outcome::Command(command) if matches!(
                    command.result,
                    CommandResult::TargetMissed(_) | CommandResult::TargetNotResident { .. }
                )
            );
            if let (true, Some(body)) = (failed, expected.body) {
                let _ = self.bodies.release(body);
            }
        }
    }

    /// The body substeps and the samples (`shaders/body/*.wgsl`), inside the tick's fourth
    /// compute pass. A tick with no body slot in use dispatches no substep.
    fn encode_bodies(
        &self,
        pass: &mut wgpu::ComputePass<'_>,
        extent: u32,
        substeps: u32,
        observe_count: u32,
    ) {
        let kernels = &self.pipelines.body;
        let group = &self.bind_groups.bodies;
        let stride = self.plan.body_dispatch_stride;
        let linear = extent.div_ceil(LINEAR_WORKGROUP_SIZE);
        let hash = self.plan.bodies.hash_slots.div_ceil(LINEAR_WORKGROUP_SIZE);
        let mut run = |pipeline: &wgpu::ComputePipeline, substep: u32, iteration: u32, groups| {
            let entry = substep * BODY_DISPATCH_ITERATIONS + iteration;
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group, &[(u64::from(entry) * stride) as u32]);
            pass.dispatch_workgroups(groups, 1, 1);
        };
        for substep in 0..substeps {
            run(&kernels.bins_clear, substep, 0, hash);
            run(&kernels.body_begin, substep, 0, linear);
            run(&kernels.bins_count, substep, 0, linear);
            run(&kernels.bins_scan, substep, 0, 1);
            run(&kernels.bins_scatter, substep, 0, linear);
            // The pair kernels and the contact kernels run one workgroup per body.
            run(&kernels.pairs_count, substep, 0, extent);
            for iteration in 0..self.pair_iterations.position {
                run(&kernels.pairs_position, substep, iteration, extent);
            }
            for iteration in 0..self.pair_iterations.velocity {
                run(&kernels.pairs_velocity, substep, iteration, extent);
            }
            run(&kernels.pairs_residual, substep, 0, extent);
            run(&kernels.claims_clear, substep, 0, extent);
            run(&kernels.contact_prepare, substep, 0, extent);
            run(&kernels.contact_apply, substep, 0, extent);
            run(&kernels.contact_response, substep, 0, extent);
            run(&kernels.claims_clear, substep, 1, extent);
            run(&kernels.displace_prepare, substep, 0, extent);
            run(&kernels.displace_apply, substep, 0, extent);
            run(&kernels.body_end, substep, 0, linear);
        }
        if observe_count > 0 {
            run(
                &kernels.body_observe,
                0,
                0,
                observe_count.div_ceil(LINEAR_WORKGROUP_SIZE),
            );
        }
    }
}
