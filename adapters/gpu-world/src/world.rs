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
//! Each tick runs the material phases after its commands, then the body substeps
//! (`docs/reference/gpu-contract.md`, "Cell phases" and "Bodies"). The session holds the
//! simulation configuration (rules, ambient temperature, elevation scale, rule seed) on the CPU
//! and applies configuration commands to the tick they arrive in; a rule or climate change wakes
//! every resident cell for that tick, as a resize does for the first tick after it. Body
//! identity (which slots hold which generation) is kept here in a [`BodyTable`]; body physics
//! lives on the device.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use world_model::{
    body::BodyHandle,
    cell::WorldCell,
    command::{AmbientTemperature, CommandKind, ElevationScale},
    config::DEFAULT_AMBIENT_TEMPERATURE_C,
    resume::BodySet,
    rules::SimulationRules,
    spatial::{CHUNK_CELL_COUNT, ChunkKey},
};
use world_runtime::{
    ids::{ChunkRevision, SessionEpoch, SessionId, Tick, WorldId},
    port::{
        CapacityLimit, CaptureTicket, CapturedChunk, DrainReport, LimitedResource, Outcome,
        PayloadError, PhysicalWorld, PollReport, PortCapabilities, PortHealth, Pressure, Refusal,
        ResidentWindow, ResizeReport, ResizeRequest, SlotRef, SnapshotTicket, SubmissionTicket,
        SupportedCommands, TickBatch, TransferCounters, UploadTicket, WorldSnapshotTicket,
        check_chunk_payload,
    },
};

use crate::{
    bodies::BodyTable,
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
    snapshots::Snapshots,
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
    /// How body pairs are found. Bins are the production broadphase; all pairs is the
    /// comparison the measurements keep.
    pub broadphase: Broadphase,
    /// Jacobi passes of the body pair solver per substep.
    pub pair_iterations: PairIterations,
    /// World units per elevation unit; `SetElevationScale` changes it at a tick boundary.
    pub elevation_scale: ElevationScale,
    /// The first tick the session runs: 0 for a new world, the next tick after a checkpoint
    /// for a resumed one.
    pub first_tick: Tick,
}

/// Jacobi passes of the pair solver per substep: position corrections, then velocity
/// impulses. Each count is even (a pass reads one half of the ping-pong buffers and writes the
/// other, and a substep starts and ends in the first) and at most
/// [`plan::BODY_DISPATCH_ITERATIONS`].
///
/// The default is the most the plan allows: in the `bodies` suite's dense cluster (512 mixed
/// bodies crushed together, then left to settle) fewer velocity passes leave the pile
/// compressed, since a body averages its impulses over every partner it touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairIterations {
    pub position: u32,
    pub velocity: u32,
}

impl PairIterations {
    pub const DEFAULT: Self = Self {
        position: 16,
        velocity: 16,
    };

    fn validate(self) -> Result<Self, PlanError> {
        for (field, value) in [
            ("pair_iterations.position", self.position),
            ("pair_iterations.velocity", self.velocity),
        ] {
            if value % 2 != 0 || !(2..=plan::BODY_DISPATCH_ITERATIONS).contains(&value) {
                return Err(PlanError::Config {
                    field,
                    value,
                    min: 2,
                    max: plan::BODY_DISPATCH_ITERATIONS,
                });
            }
        }
        Ok(self)
    }
}

/// How the body solver finds candidate pairs. Both find every pair that can touch; bins visit
/// only nearby bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Broadphase {
    #[default]
    Bins,
    /// Every body against every body: the baseline's arrangement, kept for comparison.
    AllPairs,
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
            broadphase: Broadphase::Bins,
            pair_iterations: PairIterations::DEFAULT,
            elevation_scale: ElevationScale::DEFAULT,
            first_tick: Tick::ZERO,
        }
    }
}

impl GpuWorldConfig {
    /// The configuration that resumes a saved world's latest checkpoint: its window, the tick
    /// after the one it holds, its physical configuration and the world's rule seed, under
    /// `epoch` (a recovery uses a later epoch than the session it replaces). `None` when the
    /// checkpoint holds the last representable tick.
    pub fn resume(
        plan: &world_runtime::session::SessionPlan,
        adapter: impl Into<String>,
        epoch: SessionEpoch,
    ) -> Option<Self> {
        let mut config = Self::new(plan.header.world, adapter, plan.manifest.window);
        config.epoch = epoch;
        config.first_tick = plan.first_tick()?;
        config.rules = plan.manifest.config.rules;
        config.ambient_temperature = plan.manifest.config.ambient_temperature;
        config.elevation_scale = plan.manifest.config.elevation_scale;
        config.rule_seed = plan.header.rule_seed;
        Some(config)
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
///
/// Its buffers are the live ones: a pass encoded with this view and submitted after the ticks
/// reads the world as the last submitted tick leaves it, in queue order, without the CPU
/// waiting for that tick. The window's buffers are replaced when the window moves or resizes;
/// `buffers_generation` changes exactly then, so a consumer rebuilds its bindings only then.
pub struct WorldView<'a> {
    pub session: SessionId,
    pub device: &'a wgpu::Device,
    /// For small per-frame writes; they land before the next submission on the timeline.
    pub queue: &'a wgpu::Queue,
    pub cells: &'a wgpu::Buffer,
    pub window_map: &'a wgpu::Buffer,
    pub slot_records: &'a wgpu::Buffer,
    /// Tile records and lists of the material phases, for diagnostic inspection.
    pub activity: &'a wgpu::Buffer,
    /// Body state, configuration and samples (`plan::BodyLayout`).
    pub bodies: &'a wgpu::Buffer,
    pub plan: &'a ResourcePlan,
    pub window: ResidentWindow,
    pub last_submitted_tick: Option<Tick>,
    /// Body slots below this index may hold a live body (as of the commands encoded so far);
    /// every slot from it on is free.
    pub body_extent: u32,
    /// Changes whenever the window's buffers are replaced (a move or resize).
    pub buffers_generation: u64,
    /// The elevation scale of the last encoded tick (a `SetElevationScale` applies at a tick
    /// boundary), which the surface the renderer draws must use.
    pub elevation_scale: ElevationScale,
}

/// Why a presentation frame was not submitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentRefusal {
    /// Two frames are still rendering; skip this one (the device sets the pace).
    Busy,
    /// The session admits no work: it stopped, faulted or lost its device.
    Closed(Refusal),
}

/// What the live world would admit now, as [`GpuWorld::poll_admission`] found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Admissible {
    /// A presentation frame, as [`GpuWorld::presentation_ready`] reports it.
    pub presentation: Result<(), PresentRefusal>,
    /// A tick, as far as the in-flight limit of world work goes. Its staging can still refuse
    /// it; the tick's own submission reports that.
    pub tick: bool,
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
    /// The bodies sampled after the tick, in sample order.
    observed: Vec<BodyHandle>,
    substeps: u32,
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
    broadphase: Broadphase,
    pair_iterations: PairIterations,
    elevation_scale: ElevationScale,
    bodies: BodyTable,
    /// The next tick wakes every resident cell (after a resize).
    wake_all_pending: bool,
    /// Counts replacements of the window's buffers ([`WorldView::buffers_generation`]).
    buffers_generation: u64,
    snapshots: Snapshots,
    /// Body restoration copies the first submission carries: upload staging, offset in the
    /// bodies buffer and length.
    body_restore: Vec<(StagingRef, u64, u64)>,
    /// Whether the session has submitted anything (bodies are restored only before).
    submitted_any: bool,
    first_tick: Tick,
}

fn pressure(kind: Pressure) -> Refusal {
    Refusal::Pressure(kind)
}

impl GpuWorld {
    /// Opens the adapter, requests a device for the largest supported plan, checks and
    /// allocates the plan for `config.window`, and compiles the kernels.
    pub fn open(config: GpuWorldConfig) -> Result<Self, OpenError> {
        Self::open_with(config, None).map(|(world, _)| world)
    }

    /// Opens the session as [`open`](Self::open) does, and a surface for `target` (a window)
    /// on the same instance, which the adapter must be able to present to. A renderer draws
    /// into it with [`submit_presentation`](Self::submit_presentation): one device and one
    /// timeline for simulation and presentation. A recovered session opens a new surface for
    /// the same window, since a surface belongs to its instance.
    pub fn open_presenting(
        config: GpuWorldConfig,
        target: wgpu::SurfaceTarget<'static>,
    ) -> Result<(Self, wgpu::Surface<'static>), OpenError> {
        let (world, surface) = Self::open_with(config, Some(target))?;
        let surface = surface.ok_or_else(|| {
            OpenError::Context(ContextError::Surface("no surface was created".to_owned()))
        })?;
        Ok((world, surface))
    }

    fn open_with(
        config: GpuWorldConfig,
        target: Option<wgpu::SurfaceTarget<'static>>,
    ) -> Result<(Self, Option<wgpu::Surface<'static>>), OpenError> {
        let session = SessionId {
            world: config.world,
            epoch: config.epoch,
        };
        let pair_iterations = config.pair_iterations.validate().map_err(OpenError::Plan)?;
        let (max_buffer_size, max_storage_binding) =
            plan::largest_supported_needs(config.transfer).map_err(OpenError::Plan)?;
        let (mut context, surface) = GpuContext::open(
            &config.adapter,
            config.allow_software,
            config.limit_ceiling,
            DeviceNeeds {
                max_buffer_size,
                max_storage_binding,
                timestamps: config.timestamps,
            },
            config.transfer.max_in_flight,
            target,
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
        let image_plan = plan.buffer(BufferRole::SnapshotImage);
        let image = context
            .create_buffer("chunk snapshots", image_plan.size, image_plan.usage, false)
            .map_err(OpenError::Allocation)?;

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
                observed: Vec::with_capacity(config.transfer.body_samples_per_tick as usize),
                substeps: 0,
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
            broadphase: config.broadphase,
            pair_iterations,
            elevation_scale: config.elevation_scale,
            bodies: BodyTable::new(config.transfer.bodies),
            wake_all_pending: false,
            buffers_generation: 0,
            snapshots: Snapshots::new(image, config.transfer.snapshot_entries),
            body_restore: Vec::new(),
            submitted_any: false,
            first_tick: config.first_tick,
        };
        world.refresh_counters();
        Ok((world, surface))
    }

    /// Plans `request` and checks the old and new allocations together against the budget.
    /// The slots' readiness is the resize's own check: a preflight runs before the caller has
    /// retired the chunks the new window drops.
    fn plan_resize(&self, request: &ResizeRequest) -> Result<ResourcePlan, Refusal> {
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
        Ok(next)
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

    pub fn elevation_scale(&self) -> ElevationScale {
        self.elevation_scale
    }

    /// Body slots in use, by the commands encoded so far.
    pub fn live_bodies(&self) -> u32 {
        self.bodies.live()
    }

    pub fn view(&self) -> WorldView<'_> {
        WorldView {
            session: self.session,
            device: self.context.device(),
            queue: self.context.queue(),
            cells: &self.window.cells,
            window_map: &self.window.window_map,
            slot_records: &self.window.slot_records,
            activity: &self.window.activity,
            bodies: &self.fixed.bodies,
            plan: &self.plan,
            window: self.slots.window(),
            last_submitted_tick: self.last_submitted,
            body_extent: self.bodies.extent(),
            buffers_generation: self.buffers_generation,
            elevation_scale: self.elevation_scale,
        }
    }

    /// A new surface for `target` on the session's instance (a surface reported lost must be
    /// created again), refused when the adapter cannot present to it.
    pub fn create_surface(
        &self,
        target: wgpu::SurfaceTarget<'static>,
    ) -> Result<wgpu::Surface<'static>, ContextError> {
        let surface = self
            .context
            .instance()
            .create_surface(target)
            .map_err(|error| ContextError::Surface(error.to_string()))?;
        if self.context.supports_surface(&surface) {
            Ok(surface)
        } else {
            Err(ContextError::SurfaceUnsupported(
                self.context.adapter().id.clone(),
            ))
        }
    }

    /// What a surface opened by [`open_presenting`](Self::open_presenting) supports.
    pub fn surface_capabilities(&self, surface: &wgpu::Surface<'_>) -> wgpu::SurfaceCapabilities {
        self.context.surface_capabilities(surface)
    }

    /// Whether a presentation frame would be admitted now: check before acquiring a surface
    /// texture, so a frame that cannot be submitted is not acquired.
    pub fn presentation_ready(&self) -> Result<(), PresentRefusal> {
        self.admission().map_err(PresentRefusal::Closed)?;
        if self.context.has_presentation_capacity() {
            Ok(())
        } else {
            Err(PresentRefusal::Busy)
        }
    }

    /// Encodes and submits a presentation frame (the renderer's passes over this view) on the
    /// session's timeline, after every tick submitted so far. Never waits: at most
    /// [`crate::context::PRESENTATION_IN_FLIGHT_LIMIT`] frames are outstanding, apart from the
    /// in-flight limit of world work. The caller presents its surface texture after this.
    pub fn submit_presentation(
        &mut self,
        label: &'static str,
        encode: impl FnOnce(&WorldView<'_>, &mut wgpu::CommandEncoder),
    ) -> Result<u64, PresentRefusal> {
        self.presentation_ready()?;
        let mut encoder = self
            .context
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
        encode(&self.view(), &mut encoder);
        Ok(self.context.submit_presentation([encoder.finish()]))
    }

    /// Presentation frames submitted and not yet completed.
    pub fn presentations_in_flight(&self) -> u32 {
        self.context.presentations_in_flight()
    }

    /// Lets submissions that completed free their places in flight, without waiting and
    /// without taking any of their results (the session's poll decodes those), and reports what
    /// would be admitted now. A client between frames uses it to decide whether a frame has
    /// anything to submit: while both presentation places are busy and no tick can go, a frame
    /// would change nothing. A completion hold injected for conformance is respected, and a
    /// loss the poll observes is reported by both refusals.
    pub fn poll_admission(&mut self) -> Admissible {
        let hold = self
            .faults
            .as_ref()
            .is_some_and(|faults| faults.hold_completions());
        self.context.poll(!hold);
        Admissible {
            presentation: self.presentation_ready(),
            tick: self.admission().is_ok() && self.context.has_submission_capacity(),
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
            .max(self.context.world_in_flight());
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
mod snapshot;
mod tick;

impl PhysicalWorld for GpuWorld {
    fn session(&self) -> SessionId {
        self.session
    }

    fn capabilities(&self) -> PortCapabilities {
        let ticks = self.tick_readbacks.capacity();
        let per_tick = self.transfer.commands_per_tick
            + self.transfer.events_per_tick
            + self.transfer.body_samples_per_tick
            + 3;
        PortCapabilities {
            commands_per_tick: self.transfer.commands_per_tick,
            max_in_flight: self.transfer.max_in_flight,
            outcomes_per_poll: ticks * per_tick
                + self.transfer.chunk_upload_slots * 2
                + self.transfer.capture_slots
                + self.transfer.snapshot_entries
                + self.slots.capacity()
                + 1,
            chunk_upload_slots: self.transfer.chunk_upload_slots,
            capture_slots: self.transfer.capture_slots,
            snapshot_entries: self.transfer.snapshot_entries,
            window: self.slots.window(),
            supported: [
                CommandKind::EditMaterial,
                CommandKind::SpawnBody,
                CommandKind::RemoveBody,
                CommandKind::DriveBody,
                CommandKind::JumpBody,
                CommandKind::AttractBodies,
                CommandKind::LiftBodies,
                CommandKind::SetAmbientTemperature,
                CommandKind::SetSimulationRules,
                CommandKind::SetElevationScale,
                CommandKind::QueryColumn,
                CommandKind::QueryRay,
                CommandKind::MeasureTotals,
            ]
            .into_iter()
            .fold(SupportedCommands::default(), SupportedCommands::with),
            body_capacity: self.transfer.bodies,
            body_samples_per_tick: self.transfer.body_samples_per_tick,
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

    fn snapshot_chunk(&mut self, slot: SlotRef) -> Result<SnapshotTicket, Refusal> {
        self.request_chunk_snapshot(slot)
    }

    fn snapshot_world(&mut self, session: SessionId) -> Result<WorldSnapshotTicket, Refusal> {
        self.request_world_snapshot(session)
    }

    fn take_snapshot(
        &mut self,
        ticket: SnapshotTicket,
        into: &mut [WorldCell],
    ) -> Result<(), Refusal> {
        self.take_snapshot_cells(ticket, into)
    }

    fn take_body_snapshot(
        &mut self,
        ticket: WorldSnapshotTicket,
        into: &mut BodySet,
    ) -> Result<(), Refusal> {
        self.take_world_bodies(ticket, into)
    }

    fn submit_transfers(&mut self, session: SessionId) -> Result<bool, Refusal> {
        self.submit_transfer(session)
    }

    fn restore_bodies(&mut self, session: SessionId, bodies: &BodySet) -> Result<(), Refusal> {
        self.restore(session, bodies)
    }

    fn preflight_resize(&self, request: &ResizeRequest) -> Result<(), Refusal> {
        self.admission()?;
        self.check_session(request.session)?;
        self.plan_resize(request).map(|_| ())
    }

    fn resize(&mut self, request: &ResizeRequest) -> Result<ResizeReport, Refusal> {
        self.admission()?;
        self.check_session(request.session)?;
        let next = self.plan_resize(request)?;
        if self
            .captures
            .iter()
            .any(|capture| capture.state == CaptureState::Requested)
            || self.snapshots.has_requested()
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
            // A kept chunk keeps its change word: a snapshot after the resize still sees a
            // change from before it.
            encoder.copy_buffer_to_buffer(
                &self.window.activity,
                self.plan.activity.change_word(kept.from),
                &window.activity,
                next.activity.change_word(kept.to),
                4,
            );
        }
        let serial = self.context.submit([encoder.finish()]);
        self.note_submission();
        let old = std::mem::replace(&mut self.window, window);
        self.buffers_generation += 1;
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
        // The kept chunks' change words move with them.
        self.counters.device_copy_bytes += migrated_bytes + moves.len() as u64 * 4;
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
