//! The live world on one device, behind the runtime's [`PhysicalWorld`] port.
//!
//! One tick is one submission on the session's timeline (`world/tick.rs`). Submitting never
//! waits. Results come back through [`PhysicalWorld::poll`] (`world/poll.rs`), which polls the
//! device without blocking, observes completed submissions and delivered maps, and decodes each
//! tick's readback into outcomes after checking its identity. At most `max_in_flight` ticks are
//! outstanding; beyond that, and when any staging pool is empty, a request is refused as
//! pressure and nothing changes.
//!
//! A lost device, or a fault the session cannot explain (an uncaptured device error, a result
//! with the wrong identity), ends the session: every outstanding obligation is reported
//! abandoned and nothing further is admitted. A new session (a new epoch) starts from the last
//! durable state; it does not inherit anything from this one.
//!
//! Each tick runs the material phases after its commands (`docs/reference/gpu-contract.md`,
//! "Cell phases"). The session holds the simulation configuration (rules, ambient temperature,
//! rule seed) on the CPU and applies configuration commands to the tick they arrive in; a change
//! wakes every resident cell for that tick, as a resize does for the first tick after it.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use world_model::{
    cell::WorldCell,
    command::{AmbientTemperature, CommandKind},
    config::DEFAULT_AMBIENT_TEMPERATURE_C,
    rules::SimulationRules,
    spatial::{CHUNK_CELL_COUNT, ChunkKey},
};
use world_runtime::{
    ids::{ChunkRevision, SessionEpoch, SessionId, Tick, WorldId},
    port::{
        CapacityLimit, CaptureTicket, CapturedChunk, DrainReport, LimitedResource, Outcome,
        PayloadError, PhysicalWorld, PollReport, PortCapabilities, PortHealth, Pressure, Refusal,
        ResidentWindow, ResizeReport, ResizeRequest, SlotRef, SubmissionTicket, SupportedCommands,
        TickBatch, TransferCounters, UploadTicket, check_chunk_payload,
    },
};

use crate::{
    commands::{Dispatch, Expected},
    context::{
        AdapterRecord, AllocationCounters, ContextError, DeviceNeeds, GpuContext, LimitCeiling,
        WaitOutcome,
    },
    faults::{FaultController, FaultSwitches},
    layout::CommandRecord,
    plan::{
        self, Budgets, BufferRole, CHUNK_BYTES, PlanError, PlanLimits, ResourcePlan, TransferConfig,
    },
    resources::{BindGroups, FixedBuffers, PipelineError, Pipelines, Retired, WindowBuffers},
    slots::{Activated, Patch, SlotTable, StagedUpload},
    staging::{Completion, Direction, StagingPool, StagingRef},
};

/// Everything needed to open a session.
#[derive(Debug, Clone, PartialEq)]
pub struct GpuWorldConfig {
    pub world: WorldId,
    pub epoch: SessionEpoch,
    /// The identifier `inspect-adapter` prints; never chosen automatically.
    pub adapter: String,
    /// Accept a software adapter if `adapter` names one.
    pub allow_software: bool,
    pub limit_ceiling: LimitCeiling,
    pub window: ResidentWindow,
    pub transfer: TransferConfig,
    pub budgets: Budgets,
    /// Capture the command pass's device time when the adapter supports timestamps.
    pub timestamps: bool,
    /// Hand out a [`FaultController`]; only for conformance runs.
    pub fault_injection: bool,
    /// The simulation configuration the session starts with; commands change it.
    pub rules: SimulationRules,
    pub ambient_temperature: AmbientTemperature,
    /// Keys every pseudo-random choice of the rules (`world_model::rules::RuleTick`); the
    /// composition root derives it from the world's identity.
    pub rule_seed: u64,
    pub execution: Execution,
    /// The first tick the session runs: 0 for a new world, the next tick after a checkpoint
    /// for a resumed one.
    pub first_tick: Tick,
}

/// How the material phases traverse the window. Both give the same result by construction; the
/// `cells` suite compares them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Execution {
    /// Only tiles with an awake cell are visited, and only awake cells are committed.
    #[default]
    Sparse,
    /// Every resident tile is visited and every cell committed every tick: the maintained
    /// reference for the sparse scheduling, and the dense-work comparison case.
    Dense,
}

impl GpuWorldConfig {
    pub fn new(world: WorldId, adapter: impl Into<String>, window: ResidentWindow) -> Self {
        Self {
            world,
            epoch: SessionEpoch::FIRST,
            adapter: adapter.into(),
            allow_software: false,
            limit_ceiling: LimitCeiling::Adapter,
            window,
            transfer: TransferConfig::default(),
            budgets: Budgets::default(),
            timestamps: false,
            fault_injection: false,
            rules: SimulationRules::ALL,
            ambient_temperature: AmbientTemperature::celsius(DEFAULT_AMBIENT_TEMPERATURE_C)
                .expect("the default temperature is supported"),
            rule_seed: 0,
            execution: Execution::Sparse,
            first_tick: Tick::ZERO,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    Context(ContextError),
    Plan(PlanError),
    Allocation(crate::context::AllocationError),
    Pipeline(PipelineError),
}

/// Whether pass timestamps are measured, and why not when they are not.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimestampSupport {
    NotRequested,
    /// The adapter lacks `TIMESTAMP_QUERY`; no device time is reported.
    Unavailable,
    Enabled {
        period_ns: f32,
    },
}

/// What a session was opened with, for reports.
#[derive(Debug, Clone)]
pub struct SessionReport {
    pub session: SessionId,
    pub adapter: AdapterRecord,
    pub granted_limits: PlanLimits,
    pub adapter_limits: PlanLimits,
    pub features: Vec<String>,
    pub timestamps: TimestampSupport,
    pub plan: ResourcePlan,
    pub allocations: AllocationCounters,
    /// The loss the device reported through its callback, if any.
    pub device_loss: Option<crate::context::DeviceFault>,
}

/// A read-only view of the resident world for consumers that encode their own passes on the
/// same device: the renderer, layout probes.
pub struct WorldView<'a> {
    pub session: SessionId,
    pub device: &'a wgpu::Device,
    pub cells: &'a wgpu::Buffer,
    pub window_map: &'a wgpu::Buffer,
    pub slot_records: &'a wgpu::Buffer,
    /// Tile records and lists of the material phases, for diagnostic inspection.
    pub activity: &'a wgpu::Buffer,
    pub plan: &'a ResourcePlan,
    pub window: ResidentWindow,
    pub last_submitted_tick: Option<Tick>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    Running,
    Stopping,
    Stopped,
    Lost,
    Faulted,
}

/// A submitted tick awaiting its readback, stored under its readback buffer's index.
struct TickRecord {
    active: bool,
    tick: Tick,
    readback: StagingRef,
    submitted_at: Instant,
    expected: Vec<Expected>,
    upload_bytes: u32,
    readback_bytes: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureState {
    Requested,
    Submitted { tick: Tick },
    Ready { tick: Tick },
}

#[derive(Debug, Clone, Copy)]
struct Capture {
    ticket: CaptureTicket,
    staging: StagingRef,
    key: ChunkKey,
    revision: ChunkRevision,
    state: CaptureState,
}

/// Buffers reused by every tick, so steady operation allocates nothing.
#[derive(Default)]
struct Scratch {
    records: Vec<CommandRecord>,
    expected: Vec<Expected>,
    dispatches: Vec<Dispatch>,
    patches: Vec<Patch>,
    staged: Vec<StagedUpload>,
    activated: Vec<Activated>,
    completions: Vec<Completion>,
    bytes: Vec<u8>,
}

pub struct GpuWorld {
    session: SessionId,
    transfer: TransferConfig,
    budgets: Budgets,
    timestamps: TimestampSupport,
    context: GpuContext,
    plan: ResourcePlan,
    fixed: FixedBuffers,
    window: WindowBuffers,
    pipelines: Pipelines,
    bind_groups: BindGroups,
    tick_uploads: StagingPool,
    tick_readbacks: StagingPool,
    chunk_uploads: StagingPool,
    capture_staging: StagingPool,
    slots: SlotTable,
    ticks: Vec<TickRecord>,
    captures: Vec<Capture>,
    retired: Vec<Retired>,
    next_tick: Tick,
    tick_exhausted: bool,
    last_submitted: Option<Tick>,
    lifecycle: Lifecycle,
    loss_injected: bool,
    counters: TransferCounters,
    faults: Option<Arc<FaultSwitches>>,
    scratch: Scratch,
    rules: SimulationRules,
    ambient_temperature: AmbientTemperature,
    rule_seed: u64,
    execution: Execution,
    /// The next tick wakes every resident cell (after a resize).
    wake_all_pending: bool,
}

fn pressure(kind: Pressure) -> Refusal {
    Refusal::Pressure(kind)
}

impl GpuWorld {
    /// Opens the adapter, requests a device for the largest supported plan, checks and
    /// allocates the plan for `config.window`, and compiles the kernels.
    pub fn open(config: GpuWorldConfig) -> Result<Self, OpenError> {
        let session = SessionId {
            world: config.world,
            epoch: config.epoch,
        };
        let (max_buffer_size, max_storage_binding) =
            plan::largest_supported_needs(config.transfer).map_err(OpenError::Plan)?;
        let mut context = GpuContext::open(
            &config.adapter,
            config.allow_software,
            config.limit_ceiling,
            DeviceNeeds {
                max_buffer_size,
                max_storage_binding,
                timestamps: config.timestamps,
            },
            config.transfer.max_in_flight,
        )
        .map_err(OpenError::Context)?;
        let timestamps = match (config.timestamps, context.timestamp_period_ns()) {
            (false, _) => TimestampSupport::NotRequested,
            (true, Some(period_ns)) => TimestampSupport::Enabled { period_ns },
            (true, None) => TimestampSupport::Unavailable,
        };
        let limits = PlanLimits::from_wgpu(context.limits());
        let plan = plan::plan(
            config.window,
            config.transfer,
            matches!(timestamps, TimestampSupport::Enabled { .. }),
            &limits,
            &config.budgets,
        )
        .map_err(OpenError::Plan)?;
        let slots = SlotTable::new(
            session,
            config.window,
            config.transfer.metadata_patches_per_tick,
        );
        let pipelines =
            Pipelines::create(&context, config.fault_injection).map_err(OpenError::Pipeline)?;
        let (fixed, fixed_bytes) =
            FixedBuffers::create(&mut context, &plan).map_err(OpenError::Allocation)?;
        let (window, window_bytes) =
            WindowBuffers::create(&mut context, &plan, &slots).map_err(OpenError::Allocation)?;
        let pool = |context: &mut GpuContext, role: BufferRole, label, direction| {
            let buffer = plan.buffer(role);
            StagingPool::new(
                context,
                label,
                direction,
                buffer.count,
                buffer.size,
                buffer.usage,
            )
            .map_err(OpenError::Allocation)
        };
        let tick_uploads = pool(
            &mut context,
            BufferRole::TickUploadStaging,
            "tick upload",
            Direction::Upload,
        )?;
        let tick_readbacks = pool(
            &mut context,
            BufferRole::TickReadbackStaging,
            "tick readback",
            Direction::Readback,
        )?;
        let chunk_uploads = pool(
            &mut context,
            BufferRole::ChunkUploadStaging,
            "chunk upload",
            Direction::Upload,
        )?;
        let capture_staging = pool(
            &mut context,
            BufferRole::CaptureStaging,
            "capture",
            Direction::Readback,
        )?;
        let bind_groups = pipelines.bind_groups(&context, &fixed, &window);

        let commands = config.transfer.commands_per_tick as usize;
        let ticks = (0..tick_readbacks.capacity())
            .map(|_| TickRecord {
                active: false,
                tick: Tick::ZERO,
                readback: StagingRef {
                    index: 0,
                    generation: 0,
                },
                submitted_at: Instant::now(),
                expected: Vec::with_capacity(commands),
                upload_bytes: 0,
                readback_bytes: 0,
            })
            .collect();
        let mut counters = TransferCounters {
            pipelines_created: pipelines.created,
            ..TransferCounters::default()
        };
        counters.upload.metadata += fixed_bytes + window_bytes;
        let faults = config
            .fault_injection
            .then(|| Arc::new(FaultSwitches::default()));
        let scratch = Scratch {
            records: Vec::with_capacity(commands),
            expected: Vec::with_capacity(commands),
            dispatches: Vec::with_capacity(commands),
            patches: Vec::with_capacity(config.transfer.metadata_patches_per_tick as usize),
            staged: Vec::with_capacity(config.transfer.chunk_upload_slots as usize),
            activated: Vec::with_capacity(config.transfer.chunk_upload_slots as usize),
            completions: Vec::with_capacity(
                (tick_readbacks.capacity()
                    + chunk_uploads.capacity()
                    + capture_staging.capacity()
                    + tick_uploads.capacity()) as usize,
            ),
            bytes: Vec::with_capacity(CHUNK_BYTES.max(plan.tick_upload.size) as usize),
        };
        let mut world = Self {
            session,
            transfer: config.transfer,
            budgets: config.budgets,
            timestamps,
            context,
            plan,
            fixed,
            window,
            pipelines,
            bind_groups,
            tick_uploads,
            tick_readbacks,
            chunk_uploads,
            capture_staging,
            slots,
            ticks,
            captures: Vec::with_capacity(config.transfer.capture_slots as usize),
            retired: Vec::with_capacity(config.transfer.max_in_flight as usize + 1),
            next_tick: config.first_tick,
            tick_exhausted: false,
            last_submitted: None,
            lifecycle: Lifecycle::Running,
            loss_injected: false,
            counters,
            faults,
            scratch,
            rules: config.rules,
            ambient_temperature: config.ambient_temperature,
            rule_seed: config.rule_seed,
            execution: config.execution,
            wake_all_pending: false,
        };
        world.refresh_counters();
        Ok(world)
    }

    pub fn fault_controller(&self) -> Option<FaultController> {
        self.faults
            .as_ref()
            .map(|switches| FaultController(Arc::clone(switches)))
    }

    pub fn report(&self) -> SessionReport {
        SessionReport {
            session: self.session,
            adapter: self.context.adapter().clone(),
            granted_limits: PlanLimits::from_wgpu(self.context.limits()),
            adapter_limits: PlanLimits::from_wgpu(self.context.adapter_limits()),
            features: self
                .context
                .features()
                .iter_names()
                .map(|(name, _)| name.to_owned())
                .collect(),
            timestamps: self.timestamps,
            plan: self.plan.clone(),
            allocations: self.context.allocations(),
            device_loss: self.context.health().lost(),
        }
    }

    pub fn plan(&self) -> &ResourcePlan {
        &self.plan
    }

    /// The simulation configuration of the next tick (before its commands).
    pub fn rules(&self) -> SimulationRules {
        self.rules
    }

    pub fn ambient_temperature(&self) -> AmbientTemperature {
        self.ambient_temperature
    }

    pub fn execution(&self) -> Execution {
        self.execution
    }

    pub fn view(&self) -> WorldView<'_> {
        WorldView {
            session: self.session,
            device: self.context.device(),
            cells: &self.window.cells,
            window_map: &self.window.window_map,
            slot_records: &self.window.slot_records,
            activity: &self.window.activity,
            plan: &self.plan,
            window: self.slots.window(),
            last_submitted_tick: self.last_submitted,
        }
    }

    /// Creates a buffer on the session's device, counted with the session's own.
    pub fn create_buffer(
        &mut self,
        label: &'static str,
        size: u64,
        usage: wgpu::BufferUsages,
        mapped_at_creation: bool,
    ) -> Result<wgpu::Buffer, crate::context::AllocationError> {
        let buffer = self
            .context
            .create_buffer(label, size, usage, mapped_at_creation);
        self.refresh_counters();
        buffer
    }

    /// Encodes work of another consumer (a renderer, a probe) on the session's timeline, after
    /// every tick submitted so far.
    pub fn submit_external(
        &mut self,
        label: &'static str,
        encode: impl FnOnce(&WorldView<'_>, &mut wgpu::CommandEncoder),
    ) -> Result<u64, Refusal> {
        self.admission()?;
        if !self.context.has_submission_capacity() {
            return Err(self.refuse(Pressure::InFlightSubmissions));
        }
        let mut encoder = self
            .context
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
        encode(&self.view(), &mut encoder);
        let serial = self.context.submit([encoder.finish()]);
        self.note_submission();
        Ok(serial)
    }

    /// Blocks until every submission so far completes or `timeout` passes. A maintenance
    /// wait: counted separately, never on the frame path.
    pub fn wait_idle(&mut self, timeout: Duration) -> WaitOutcome {
        let last = self.context.next_serial().saturating_sub(1);
        self.maintenance_wait(last, timeout)
    }

    fn maintenance_wait(&mut self, serial: u64, timeout: Duration) -> WaitOutcome {
        let started = Instant::now();
        let outcome = self.context.wait(serial, timeout);
        self.counters.maintenance_waits += 1;
        self.counters.maintenance_wait_time += started.elapsed();
        outcome
    }

    fn admission(&self) -> Result<(), Refusal> {
        match self.lifecycle {
            Lifecycle::Running => {}
            Lifecycle::Stopping | Lifecycle::Stopped => return Err(Refusal::Stopped),
            Lifecycle::Lost => return Err(Refusal::DeviceLost),
            Lifecycle::Faulted => return Err(Refusal::Faulted),
        }
        if self.loss_injected || self.context.health().lost().is_some() {
            return Err(Refusal::DeviceLost);
        }
        if self.context.health().uncaptured_count() > 0 {
            return Err(Refusal::Faulted);
        }
        Ok(())
    }

    fn refuse(&mut self, kind: Pressure) -> Refusal {
        self.counters.pressure_refusals[kind.index()] += 1;
        pressure(kind)
    }

    fn check_session(&self, presented: SessionId) -> Result<(), Refusal> {
        if presented == self.session {
            Ok(())
        } else {
            Err(Refusal::StaleSession {
                current: self.session,
                presented,
            })
        }
    }

    fn note_submission(&mut self) {
        self.counters.in_flight_high_water = self
            .counters
            .in_flight_high_water
            .max(self.context.in_flight());
    }

    fn refresh_counters(&mut self) {
        let allocations = self.context.allocations();
        self.counters.buffers_created = allocations.buffers_created;
        self.counters.requested_bytes = allocations.requested_bytes;
        self.counters.peak_requested_bytes = allocations.peak_requested_bytes;
        self.counters.upload_staging_high_water =
            self.tick_uploads.high_water() + self.chunk_uploads.high_water();
        self.counters.readback_staging_high_water =
            self.tick_readbacks.high_water() + self.capture_staging.high_water();
        self.counters.stale_deliveries = self.tick_uploads.stale_deliveries()
            + self.tick_readbacks.stale_deliveries()
            + self.chunk_uploads.stale_deliveries()
            + self.capture_staging.stale_deliveries();
    }

    fn health(&self) -> PortHealth {
        match self.lifecycle {
            Lifecycle::Running => PortHealth::Running,
            Lifecycle::Stopping => PortHealth::Stopping,
            Lifecycle::Stopped => PortHealth::Stopped,
            Lifecycle::Lost => PortHealth::Lost,
            Lifecycle::Faulted => PortHealth::Faulted,
        }
    }
}

mod poll;
mod tick;

impl PhysicalWorld for GpuWorld {
    fn session(&self) -> SessionId {
        self.session
    }

    fn capabilities(&self) -> PortCapabilities {
        let ticks = self.tick_readbacks.capacity();
        let per_tick = self.transfer.commands_per_tick + self.transfer.events_per_tick + 2;
        PortCapabilities {
            commands_per_tick: self.transfer.commands_per_tick,
            max_in_flight: self.transfer.max_in_flight,
            outcomes_per_poll: ticks * per_tick
                + self.transfer.chunk_upload_slots * 2
                + self.transfer.capture_slots,
            chunk_upload_slots: self.transfer.chunk_upload_slots,
            capture_slots: self.transfer.capture_slots,
            window: self.slots.window(),
            supported: SupportedCommands::default()
                .with(CommandKind::EditMaterial)
                .with(CommandKind::QueryColumn)
                .with(CommandKind::SetAmbientTemperature)
                .with(CommandKind::SetSimulationRules)
                .with(CommandKind::MeasureTotals),
        }
    }

    fn next_tick(&self) -> Tick {
        self.next_tick
    }

    fn submit_tick(&mut self, batch: &TickBatch<'_>) -> Result<SubmissionTicket, Refusal> {
        self.encode_tick(batch)
    }

    fn reserve_slot(&mut self, session: SessionId, key: ChunkKey) -> Result<SlotRef, Refusal> {
        self.admission()?;
        self.check_session(session)?;
        self.slots.reserve(key).inspect_err(|refusal| {
            if let Refusal::Pressure(kind) = refusal {
                self.counters.pressure_refusals[kind.index()] += 1;
            }
        })
    }

    fn upload_chunk(
        &mut self,
        slot: SlotRef,
        revision: ChunkRevision,
        cells: &[WorldCell],
    ) -> Result<UploadTicket, Refusal> {
        self.admission()?;
        self.check_session(slot.session())?;
        check_chunk_payload(cells).map_err(Refusal::InvalidPayload)?;
        let key = self.slots.check_upload(slot)?;
        let Some(staging) = self.chunk_uploads.reserve() else {
            return Err(self.refuse(Pressure::ChunkUploadStaging));
        };
        let bytes = &mut self.scratch.bytes;
        bytes.clear();
        for cell in cells {
            bytes.extend_from_slice(&cell.0.to_le_bytes());
        }
        self.chunk_uploads.write(staging, 0, bytes);
        self.slots.stage(slot, revision, staging);
        self.counters.upload.chunks += CHUNK_BYTES;
        self.refresh_counters();
        Ok(UploadTicket {
            slot,
            key,
            revision,
        })
    }

    fn retire_slot(&mut self, slot: SlotRef) -> Result<(), Refusal> {
        self.admission()?;
        self.check_session(slot.session())?;
        self.slots.retire(slot)
    }

    fn capture_chunk(&mut self, slot: SlotRef) -> Result<CaptureTicket, Refusal> {
        self.admission()?;
        self.check_session(slot.session())?;
        let (key, revision) = self.slots.capture_source(slot)?;
        let Some(staging) = self.capture_staging.reserve() else {
            return Err(self.refuse(Pressure::CaptureStaging));
        };
        let ticket = CaptureTicket::new(self.session, staging.index, staging.generation, slot);
        self.captures.push(Capture {
            ticket,
            staging,
            key,
            revision,
            state: CaptureState::Requested,
        });
        self.refresh_counters();
        Ok(ticket)
    }

    fn take_capture(
        &mut self,
        ticket: CaptureTicket,
        into: &mut [WorldCell],
    ) -> Result<CapturedChunk, Refusal> {
        self.check_session(ticket.session())?;
        if matches!(self.lifecycle, Lifecycle::Lost | Lifecycle::Faulted) {
            return Err(Refusal::DeviceLost);
        }
        if into.len() != CHUNK_CELL_COUNT {
            return Err(Refusal::InvalidPayload(PayloadError::CellCount {
                expected: CHUNK_CELL_COUNT,
                presented: into.len(),
            }));
        }
        let position = self
            .captures
            .iter()
            .position(|capture| capture.ticket == ticket)
            .ok_or(Refusal::StaleSlot(ticket.slot()))?;
        let capture = self.captures[position];
        let CaptureState::Ready { tick } = capture.state else {
            return Err(Refusal::InvalidPayload(PayloadError::CaptureNotReady));
        };
        let copied = self
            .capture_staging
            .read(capture.staging, 0..CHUNK_BYTES, |bytes| {
                let (words, _) = bytes.as_chunks::<8>();
                for (cell, word) in into.iter_mut().zip(words) {
                    *cell = WorldCell(u64::from_le_bytes(*word));
                }
            });
        self.capture_staging.release(capture.staging);
        self.captures.swap_remove(position);
        match copied {
            Some(()) => Ok(CapturedChunk {
                slot: capture.ticket.slot(),
                key: capture.key,
                revision: capture.revision,
                tick,
            }),
            None => Err(Refusal::Faulted),
        }
    }

    fn resize(&mut self, request: &ResizeRequest) -> Result<ResizeReport, Refusal> {
        self.admission()?;
        self.check_session(request.session)?;
        let limits = PlanLimits::from_wgpu(self.context.limits());
        let next = plan::plan(
            request.window,
            self.transfer,
            self.plan.timestamps,
            &limits,
            &self.budgets,
        )
        .map_err(PlanError::refusal)?;
        let overlap = self.plan.overlap_with(&next);
        if let Some(budget) = self.budgets.device_bytes
            && overlap > budget
        {
            return Err(Refusal::Unsupported(CapacityLimit {
                resource: LimitedResource::DeviceBytes,
                requested: overlap,
                available: budget,
            }));
        }
        if self
            .captures
            .iter()
            .any(|capture| capture.state == CaptureState::Requested)
        {
            return Err(self.refuse(Pressure::SlotChanges));
        }
        let (slots, moves) = self.slots.resized(request.window)?;
        if !self.context.has_submission_capacity() {
            return Err(self.refuse(Pressure::InFlightSubmissions));
        }
        let (window, metadata_bytes) = WindowBuffers::create(&mut self.context, &next, &slots)
            .map_err(|_| {
                Refusal::Unsupported(CapacityLimit {
                    resource: LimitedResource::Allocation,
                    requested: next.window_bytes(),
                    available: 0,
                })
            })?;
        let peak_overlap_bytes = self.context.allocations().requested_bytes;
        let mut encoder =
            self.context
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("resize migration"),
                });
        for kept in &moves {
            encoder.copy_buffer_to_buffer(
                &self.window.cells,
                u64::from(kept.from) * CHUNK_BYTES,
                &window.cells,
                u64::from(kept.to) * CHUNK_BYTES,
                CHUNK_BYTES,
            );
        }
        let serial = self.context.submit([encoder.finish()]);
        self.note_submission();
        let old = std::mem::replace(&mut self.window, window);
        self.retired.push(Retired {
            buffers: old,
            serial,
        });
        self.bind_groups = self
            .pipelines
            .bind_groups(&self.context, &self.fixed, &self.window);
        // The new window's activity starts empty: wake everything once, as the baseline's
        // resize did.
        self.wake_all_pending = true;
        self.plan = next;
        self.slots = slots;
        let migrated_bytes = moves.len() as u64 * CHUNK_BYTES;
        self.counters.device_copy_bytes += migrated_bytes;
        self.counters.upload.metadata += metadata_bytes;
        self.refresh_counters();
        Ok(ResizeReport {
            retained: moves
                .iter()
                .map(|kept| (kept.key, self.slots.slot_ref_for(kept.to)))
                .collect(),
            migrated_bytes,
            requested_bytes: self.context.allocations().requested_bytes,
            peak_overlap_bytes,
        })
    }

    fn poll(&mut self, outcomes: &mut Vec<Outcome>) -> PollReport {
        self.poll_into(outcomes)
    }

    fn stop(&mut self, timeout: Duration, outcomes: &mut Vec<Outcome>) -> DrainReport {
        self.drain(timeout, outcomes)
    }

    fn counters(&self) -> TransferCounters {
        self.counters
    }
}
