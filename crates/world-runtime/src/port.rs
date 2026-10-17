//! The physical-world port: what the runtime asks of the owner of the live world, and what
//! comes back.
//!
//! The runtime owns this interface; `gpu-world` implements it. Every operation takes a batch or
//! one bounded request and returns a ticket or a typed refusal at once. Results arrive later
//! through [`PhysicalWorld::poll`], which never blocks. Nothing here names a GPU type, so the
//! runtime's intent and residency decisions stay independent of the mechanism.
//!
//! An accepted request carries an obligation: it completes with an [`Outcome`], or it is
//! reported abandoned with a reason when the session can no longer complete it (device loss,
//! stop). It is never silently dropped. A refused request changed nothing and may be retried
//! when the refusal is [`Refusal::Pressure`].

use std::time::Duration;

use world_model::{
    cell::WorldCell,
    command::{Command, CommandKind},
    config,
    glam::IVec2,
    observation::WorldTotals,
    spatial::{CHUNK_CELL_COUNT, ChunkKey, ChunkView},
};

use crate::ids::{ChunkRevision, CommandId, SessionId, Tick};

/// A command the runtime admitted, under the identity its receipt is kept.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdmittedCommand {
    pub id: CommandId,
    pub command: Command,
}

/// Every command of one logical tick, in application order. Later commands see the effects of
/// earlier ones; a query after an edit reads the edited column.
#[derive(Debug, Clone, Copy)]
pub struct TickBatch<'a> {
    pub session: SessionId,
    pub tick: Tick,
    pub commands: &'a [AdmittedCommand],
}

/// Proof that a tick was submitted. `serial` orders submissions within the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubmissionTicket {
    pub session: SessionId,
    pub tick: Tick,
    pub serial: u64,
}

/// The rectangle of chunks that may be resident: `radius_x` and `radius_y` chunks on each side
/// of `center`, in the plane `z = 0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResidentWindow {
    center: ChunkKey,
    radius_x: u32,
    radius_y: u32,
}

impl ResidentWindow {
    /// Refuses radii outside [`config::ACTIVE_RADIUS_CHUNKS`], a key off the plane and a
    /// window whose corner keys do not fit an `i32`.
    pub fn new(center: ChunkKey, radius_x: u32, radius_y: u32) -> Result<Self, PayloadError> {
        for radius in [radius_x, radius_y] {
            if !config::ACTIVE_RADIUS_CHUNKS.contains(&radius) {
                return Err(PayloadError::WindowRadius(radius));
            }
        }
        if center.key.z != 0 {
            return Err(PayloadError::KeyOffPlane(center));
        }
        let fits = |coordinate: i32, radius: u32| {
            coordinate.checked_sub_unsigned(radius).is_some()
                && coordinate.checked_add_unsigned(radius).is_some()
        };
        if !fits(center.key.x, radius_x) || !fits(center.key.y, radius_y) {
            return Err(PayloadError::WindowOutOfRange(center));
        }
        Ok(Self {
            center,
            radius_x,
            radius_y,
        })
    }

    pub fn center(&self) -> ChunkKey {
        self.center
    }

    pub fn radius_x(&self) -> u32 {
        self.radius_x
    }

    pub fn radius_y(&self) -> u32 {
        self.radius_y
    }

    /// Chunks along x; at most 65.
    pub fn width(&self) -> u32 {
        2 * self.radius_x + 1
    }

    /// Chunks along y; at most 65.
    pub fn height(&self) -> u32 {
        2 * self.radius_y + 1
    }

    pub fn chunk_count(&self) -> u32 {
        self.width() * self.height()
    }

    pub fn view(&self) -> ChunkView {
        ChunkView::from_rect_xy(self.center, self.radius_x as i32, self.radius_y as i32)
    }

    /// The window's lowest key, whose first cell is the origin of window-local coordinates.
    pub fn min_key(&self) -> ChunkKey {
        self.view().min
    }

    pub fn contains(&self, key: ChunkKey) -> bool {
        self.view().contains(&key)
    }
}

/// A chunk slot in the live world: its session, index and generation. Retiring or migrating a
/// slot advances its generation, so an older reference names no slot instead of the wrong one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SlotRef {
    session: SessionId,
    index: u32,
    generation: u32,
}

impl SlotRef {
    pub const fn new(session: SessionId, index: u32, generation: u32) -> Self {
        Self {
            session,
            index,
            generation,
        }
    }

    pub const fn session(&self) -> SessionId {
        self.session
    }

    pub const fn index(&self) -> u32 {
        self.index
    }

    pub const fn generation(&self) -> u32 {
        self.generation
    }
}

/// An accepted chunk upload. It activates with the next submitted tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UploadTicket {
    pub slot: SlotRef,
    pub key: ChunkKey,
    pub revision: ChunkRevision,
}

/// An accepted chunk capture. The copy is taken after the next submitted tick's commands, and
/// its staging stays reserved until [`PhysicalWorld::take_capture`] collects it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CaptureTicket {
    session: SessionId,
    staging_index: u32,
    staging_generation: u32,
    slot: SlotRef,
}

impl CaptureTicket {
    pub const fn new(
        session: SessionId,
        staging_index: u32,
        staging_generation: u32,
        slot: SlotRef,
    ) -> Self {
        Self {
            session,
            staging_index,
            staging_generation,
            slot,
        }
    }

    pub const fn session(&self) -> SessionId {
        self.session
    }

    pub const fn staging_index(&self) -> u32 {
        self.staging_index
    }

    pub const fn staging_generation(&self) -> u32 {
        self.staging_generation
    }

    pub const fn slot(&self) -> SlotRef {
        self.slot
    }
}

/// What a collected capture holds: the chunk's cells after `tick`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapturedChunk {
    pub slot: SlotRef,
    pub key: ChunkKey,
    pub revision: ChunkRevision,
    pub tick: Tick,
}

/// A change of the resident window's size or position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResizeRequest {
    pub session: SessionId,
    pub window: ResidentWindow,
}

/// A successful resize. Chunks that remain inside the new window keep their cells (moved on
/// the device) under new slot references; every earlier slot reference is stale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResizeReport {
    pub retained: Vec<(ChunkKey, SlotRef)>,
    /// Device-to-device bytes moved for the retained chunks.
    pub migrated_bytes: u64,
    /// Device bytes requested after the resize.
    pub requested_bytes: u64,
    /// The most device bytes requested at once while old and new allocations coexisted.
    pub peak_overlap_bytes: u64,
}

/// Why a request was not accepted. Nothing changed; the caller still owns the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// A bounded resource is full. Retry after polling; this is backpressure, not an error.
    Pressure(Pressure),
    /// The request names another session, normally one that ended with its device.
    StaleSession {
        current: SessionId,
        presented: SessionId,
    },
    /// The slot was retired, migrated or never reserved in this session.
    StaleSlot(SlotRef),
    /// Ticks are submitted in order without gaps.
    TickOutOfOrder {
        expected: Tick,
        presented: Tick,
    },
    /// The logical clock reached its last value.
    TickExhausted,
    InvalidPayload(PayloadError),
    /// The live world does not execute this kind of command yet.
    NotSupported(CommandKind),
    /// The request exceeds a device limit or a resource budget.
    Unsupported(CapacityLimit),
    /// Admission closed by an orderly stop.
    Stopped,
    /// The device was lost; this session can complete nothing more.
    DeviceLost,
    /// The session detected an internal fault (an uncaptured device error or a result that
    /// failed its identity check) and accepts nothing more.
    Faulted,
}

/// The bounded resource behind a [`Refusal::Pressure`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pressure {
    /// The configured number of submitted, uncompleted ticks is reached.
    InFlightSubmissions,
    /// Every staging buffer for tick commands is in use.
    TickUploadStaging,
    /// Every staging buffer for tick results is in use.
    TickReadbackStaging,
    /// Every chunk upload staging buffer is in use.
    ChunkUploadStaging,
    /// Every capture staging buffer is in use or holds an uncollected capture.
    CaptureStaging,
    /// Every chunk slot is reserved or resident.
    Slots,
    /// The next tick already carries as many slot activations and retirements as it can, or
    /// a resize waits for outstanding slot changes to finish.
    SlotChanges,
    /// The runtime's command queue is full.
    PendingCommands,
    /// Every receipt is held by an unacknowledged command.
    Receipts,
}

impl Pressure {
    pub const ALL: [Self; 9] = [
        Self::InFlightSubmissions,
        Self::TickUploadStaging,
        Self::TickReadbackStaging,
        Self::ChunkUploadStaging,
        Self::CaptureStaging,
        Self::Slots,
        Self::SlotChanges,
        Self::PendingCommands,
        Self::Receipts,
    ];

    pub const fn index(self) -> usize {
        self as usize
    }
}

/// A malformed or out-of-range request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadError {
    TooManyCommands {
        count: usize,
        limit: u32,
    },
    CellCount {
        expected: usize,
        presented: usize,
    },
    WindowRadius(u32),
    KeyOffPlane(ChunkKey),
    WindowOutOfRange(ChunkKey),
    KeyOutsideWindow(ChunkKey),
    /// The chunk already has a reserved or resident slot.
    KeyAlreadyResident(ChunkKey),
    /// The slot is not in the state the request needs (for example, uploading into a slot
    /// that is already resident).
    SlotState(SlotRef),
    /// A resize would drop resident chunks; retire them first.
    ResidentOutsideNewWindow {
        count: u32,
    },
    /// A resize found reserved slots with no upload; upload or retire them first.
    UnfilledReservations {
        count: u32,
    },
    /// The capture is not ready yet; poll until its outcome arrives.
    CaptureNotReady,
}

/// A request beyond what the device or a budget supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapacityLimit {
    pub resource: LimitedResource,
    pub requested: u64,
    pub available: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LimitedResource {
    /// One buffer allocation.
    BufferSize,
    /// One storage binding.
    StorageBinding,
    /// Workgroups along one dispatch dimension.
    DispatchWorkgroups,
    /// Storage buffers one kernel binds.
    StorageBuffersPerStage,
    /// Bytes of mapped staging the CPU holds at once.
    StagingBudget,
    /// Device bytes of terrain-related state, against the envelope in the budgets.
    TerrainEnvelope,
    /// Device bytes requested at once, counting old and new allocations during a resize.
    DeviceBytes,
    /// Index arithmetic that would not fit 32 bits in a kernel.
    IndexRange,
    /// The device refused an allocation it reported as within its limits.
    Allocation,
    /// A configured size outside its supported range.
    Configuration,
}

/// What the live world reports back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    Command(CommandOutcome),
    TickCompleted(TickReport),
    /// An upload became resident with `tick`'s submission.
    UploadActivated {
        ticket: UploadTicket,
        tick: Tick,
    },
    /// A capture is ready to collect.
    CaptureReady {
        ticket: CaptureTicket,
        tick: Tick,
    },
    Cosmetic(CosmeticEvent),
    /// Cosmetic events of `tick` that did not fit the bounded event buffer.
    CosmeticOverflow {
        tick: Tick,
        dropped: u32,
    },
    /// An accepted obligation that will never complete in this session.
    Abandoned {
        obligation: Obligation,
        reason: AbandonReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommandOutcome {
    pub id: CommandId,
    /// The tick the command was applied in.
    pub tick: Tick,
    pub result: CommandResult,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CommandResult {
    /// A brush stroke ran. `changed` cells took a new value; `not_resident` cells of the
    /// stroke lay outside the resident window or in a chunk that is not resident (a closed
    /// boundary), and were left alone.
    Edited { changed: u32, not_resident: u32 },
    /// The column at `cell` after the tick's earlier commands.
    Column { cell: IVec2, column: WorldCell },
    /// `cell` is outside the resident window or in a chunk that is not resident.
    ColumnNotResident { cell: IVec2 },
    /// A configuration change took effect from this tick on.
    Configured,
    /// The resident world's totals after the tick's earlier commands.
    Totals(WorldTotals),
}

/// One completed tick, as observed by the CPU.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TickReport {
    pub tick: Tick,
    pub commands: u32,
    /// Device time of the whole tick, when timestamps are available.
    pub gpu_time: Option<Duration>,
    /// Device time of each of the tick's phases, when timestamps are available.
    pub phases: Option<PhaseTimes>,
    /// From submission to the CPU decoding the tick's results.
    pub latency: Duration,
    pub upload_bytes: u32,
    pub readback_bytes: u32,
    /// What the material phases did.
    pub cells: CellReport,
}

/// Device time of a tick's phases: the ordered commands, the activity scheduling and the
/// material passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PhaseTimes {
    pub commands: Duration,
    pub activity: Duration,
    pub material: Duration,
}

/// A tick's accounting of the material phases and the tools, and the work its scheduling did.
///
/// `*_in`/`*_out` sum every processed cell's increase and decrease of the fluid volume (every
/// liquid and ice) or the solid column (bedrock plus loose material). The classified quantities
/// say where the rest went: magma turned into stone (`quench`), coolant boiled beside magma
/// (`boil`), oil burned (`burn`) and water drawn by foliage (`uptake`). Tool quantities are what
/// the tick's brush strokes added or removed before the material phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellReport {
    pub fluid_in: u64,
    pub fluid_out: u64,
    pub solid_in: u64,
    pub solid_out: u64,
    pub quench: u64,
    pub boil: u64,
    pub burn: u64,
    pub uptake: u64,
    /// Bedrock eroded into loose material: moved within the solid column.
    pub erosion: u64,
    pub tool_fluid_in: u64,
    pub tool_solid_in: u64,
    pub tool_solid_out: u64,
    /// Tiles the scheduling considered, tiles the material passes visited and tiles with an
    /// active cell.
    pub candidate_tiles: u32,
    pub process_tiles: u32,
    pub active_tiles: u32,
    pub awake_cells: u32,
    pub active_cells: u32,
    /// Cells whose value changed.
    pub changed_cells: u32,
    /// Liquid transfers a receiver accepted less of than the donor proposed.
    pub rejected_transfers: u32,
    /// Transfer words read with another tick's stamp. Zero unless scheduling is wrong.
    pub stale_flow: u32,
    /// Receivers that took another amount than they accepted. Zero unless the rules disagree.
    pub receive_mismatches: u32,
}

impl CellReport {
    /// `(ΔL + quench + boil + burn + uptake, ΔS − quench)` of the material phases: both zero
    /// when every unit is accounted for.
    pub fn imbalance(&self) -> (i128, i128) {
        let fluid = i128::from(self.fluid_in) - i128::from(self.fluid_out)
            + i128::from(self.quench)
            + i128::from(self.boil)
            + i128::from(self.burn)
            + i128::from(self.uptake);
        let solid =
            i128::from(self.solid_in) - i128::from(self.solid_out) - i128::from(self.quench);
        (fluid, solid)
    }

    /// Whether the tick balanced its accounting and saw no inconsistent transfer.
    pub fn consistent(&self) -> bool {
        self.imbalance() == (0, 0) && self.stale_flow == 0 && self.receive_mismatches == 0
    }
}

/// A transient notification that may be coalesced or dropped under pressure, with the drop
/// counted. Gameplay-relevant results never travel this way; they are command outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CosmeticEvent {
    pub tick: Tick,
    /// Producer-defined code; `gpu-world`'s record layout lists them.
    pub kind: u32,
    pub data: [u32; 3],
}

/// An accepted request that a session gave up on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Obligation {
    Command(CommandId),
    Tick(Tick),
    Upload(UploadTicket),
    Capture(CaptureTicket),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbandonReason {
    DeviceLost,
    /// The session stopped before the request's tick ran.
    Stopped,
    /// An orderly stop did not finish within its time limit.
    StopTimedOut,
    Faulted,
}

/// What the live world accepts per tick and per poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortCapabilities {
    pub commands_per_tick: u32,
    pub max_in_flight: u32,
    /// The most outcomes one poll can deliver, so a consumer can reserve its buffer once.
    pub outcomes_per_poll: u32,
    pub chunk_upload_slots: u32,
    pub capture_slots: u32,
    pub window: ResidentWindow,
    pub supported: SupportedCommands,
}

/// The command kinds a live world executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SupportedCommands(u32);

impl SupportedCommands {
    pub const fn with(self, kind: CommandKind) -> Self {
        Self(self.0 | 1 << kind as u32)
    }

    pub const fn supports(self, kind: CommandKind) -> bool {
        self.0 & (1 << kind as u32) != 0
    }
}

/// The live world's state as seen by its last poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortHealth {
    Running,
    /// Admission is closed and outstanding work is draining.
    Stopping,
    Stopped,
    Lost,
    Faulted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollReport {
    pub health: PortHealth,
    pub in_flight: u32,
    pub delivered: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrainReport {
    pub health: PortHealth,
    pub delivered: u32,
    pub abandoned: u32,
    /// Time spent blocked waiting for the device; a maintenance wait, not a frame wait.
    pub waited: Duration,
    pub timed_out: bool,
}

/// Bytes moved for each purpose since the session opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UploadBytes {
    /// Command records and their per-command parameters.
    pub commands: u64,
    /// Per-tick parameters.
    pub tick_parameters: u64,
    /// Chunk cells.
    pub chunks: u64,
    /// Slot records and window entries.
    pub metadata: u64,
}

impl UploadBytes {
    pub fn total(&self) -> u64 {
        self.commands + self.tick_parameters + self.chunks + self.metadata
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReadbackBytes {
    pub results: u64,
    pub events: u64,
    /// The per-tick accounting record and the totals of totals commands.
    pub diagnostics: u64,
    pub timestamps: u64,
    pub captures: u64,
}

impl ReadbackBytes {
    pub fn total(&self) -> u64 {
        self.results + self.events + self.diagnostics + self.timestamps + self.captures
    }
}

/// Transfer and resource counters of a live world. Mechanism-neutral numbers: the runtime and
/// its displays read them without knowing the device API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TransferCounters {
    pub upload: UploadBytes,
    pub readback: ReadbackBytes,
    /// Device-to-device copies (resize migration).
    pub device_copy_bytes: u64,
    /// Device-side clears of per-tick counters and of activated chunks' activity records.
    pub device_clear_bytes: u64,
    pub ticks_submitted: u64,
    pub ticks_observed: u64,
    pub in_flight_high_water: u32,
    /// Refusals by [`Pressure`] kind, indexed by [`Pressure::index`].
    pub pressure_refusals: [u64; Pressure::ALL.len()],
    /// Blocking device waits on the submit/poll path. The design has none; a nonzero value is
    /// a defect.
    pub frame_waits: u64,
    /// Blocking waits in explicit maintenance (stop, drain), and their total duration.
    pub maintenance_waits: u64,
    pub maintenance_wait_time: Duration,
    pub upload_staging_high_water: u32,
    pub readback_staging_high_water: u32,
    /// Completions whose identity no longer matched a live reservation.
    pub stale_deliveries: u64,
    pub buffers_created: u64,
    pub pipelines_created: u64,
    /// Device bytes currently requested, and the most at any time.
    pub requested_bytes: u64,
    pub peak_requested_bytes: u64,
}

/// The owner of the live physical world, as the runtime sees it.
///
/// Implementations never block in [`submit_tick`](Self::submit_tick) or
/// [`poll`](Self::poll); [`stop`](Self::stop) is the explicit maintenance wait.
pub trait PhysicalWorld: Send + Sync {
    fn session(&self) -> SessionId;

    fn capabilities(&self) -> PortCapabilities;

    /// The tick the live world accepts next. A runtime attaching to a world starts its clock
    /// here; ticks are submitted in order without gaps.
    fn next_tick(&self) -> Tick;

    /// Encodes and submits one tick: pending uploads, then the commands in order, then pending
    /// captures. Refused without change when the tick is out of order or a bounded resource
    /// is full.
    fn submit_tick(&mut self, batch: &TickBatch<'_>) -> Result<SubmissionTicket, Refusal>;

    /// Reserves a slot for a chunk inside the resident window.
    fn reserve_slot(&mut self, session: SessionId, key: ChunkKey) -> Result<SlotRef, Refusal>;

    /// Stages a reserved slot's cells; they become resident with the next submitted tick.
    /// `cells` holds exactly [`CHUNK_CELL_COUNT`] cells, row-major.
    fn upload_chunk(
        &mut self,
        slot: SlotRef,
        revision: ChunkRevision,
        cells: &[WorldCell],
    ) -> Result<UploadTicket, Refusal>;

    /// Removes a chunk from the resident window. Its slot is reused only after every earlier
    /// submission that used it has completed. The cells are discarded: capture them first if
    /// they must be kept.
    fn retire_slot(&mut self, slot: SlotRef) -> Result<(), Refusal>;

    /// Requests a copy of a resident chunk after the next submitted tick.
    fn capture_chunk(&mut self, slot: SlotRef) -> Result<CaptureTicket, Refusal>;

    /// Copies a ready capture into `into` ([`CHUNK_CELL_COUNT`] cells) and releases its staging.
    fn take_capture(
        &mut self,
        ticket: CaptureTicket,
        into: &mut [WorldCell],
    ) -> Result<CapturedChunk, Refusal>;

    /// Changes the resident window. Refused, with the current world intact, when the new
    /// allocation exceeds a limit or budget or would drop a resident chunk.
    fn resize(&mut self, request: &ResizeRequest) -> Result<ResizeReport, Refusal>;

    /// Collects completed work into `outcomes` without blocking. Appends at most
    /// [`PortCapabilities::outcomes_per_poll`] outcomes.
    fn poll(&mut self, outcomes: &mut Vec<Outcome>) -> PollReport;

    /// Closes admission and waits up to `timeout` for outstanding work, delivering its
    /// outcomes; whatever does not finish is reported abandoned.
    fn stop(&mut self, timeout: Duration, outcomes: &mut Vec<Outcome>) -> DrainReport;

    fn counters(&self) -> TransferCounters;
}

/// Checks a chunk payload's length before a port accepts it.
pub fn check_chunk_payload(cells: &[WorldCell]) -> Result<(), PayloadError> {
    if cells.len() == CHUNK_CELL_COUNT {
        Ok(())
    } else {
        Err(PayloadError::CellCount {
            expected: CHUNK_CELL_COUNT,
            presented: cells.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_refuse_unsupported_radii_and_keys() {
        let origin = ChunkKey::new(0, 0, 0);
        let window = ResidentWindow::new(origin, 6, 2).expect("supported");
        assert_eq!((window.width(), window.height()), (13, 5));
        assert_eq!(window.chunk_count(), 65);
        assert_eq!(window.min_key(), ChunkKey::new(-6, -2, 0));
        assert!(window.contains(ChunkKey::new(6, 2, 0)));
        assert!(!window.contains(ChunkKey::new(7, 0, 0)));
        assert_eq!(
            ResidentWindow::new(origin, 0, 1),
            Err(PayloadError::WindowRadius(0))
        );
        assert_eq!(
            ResidentWindow::new(origin, 33, 1),
            Err(PayloadError::WindowRadius(33))
        );
        let raised = ChunkKey::new(0, 0, 1);
        assert_eq!(
            ResidentWindow::new(raised, 1, 1),
            Err(PayloadError::KeyOffPlane(raised))
        );
        let edge = ChunkKey::new(i32::MAX, 0, 0);
        assert_eq!(
            ResidentWindow::new(edge, 1, 1),
            Err(PayloadError::WindowOutOfRange(edge))
        );
    }

    #[test]
    fn supported_kinds_are_explicit() {
        let supported = SupportedCommands::default()
            .with(CommandKind::EditMaterial)
            .with(CommandKind::QueryColumn);
        assert!(supported.supports(CommandKind::EditMaterial));
        assert!(!supported.supports(CommandKind::MeasureTotals));
        assert!(supported.supports(CommandKind::QueryColumn));
        assert!(!supported.supports(CommandKind::SpawnBody));
        assert!(!SupportedCommands::default().supports(CommandKind::EditMaterial));
    }

    #[test]
    fn chunk_payloads_must_be_whole_chunks() {
        assert!(check_chunk_payload(&vec![WorldCell::default(); CHUNK_CELL_COUNT]).is_ok());
        assert_eq!(
            check_chunk_payload(&vec![WorldCell::default(); CHUNK_CELL_COUNT - 1]),
            Err(PayloadError::CellCount {
                expected: CHUNK_CELL_COUNT,
                presented: CHUNK_CELL_COUNT - 1
            })
        );
    }
}
