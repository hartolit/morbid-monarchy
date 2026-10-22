//! The complete resource plan of a session, checked before anything is allocated.
//!
//! A plan lists every buffer the session will own, its size, usage and binding, and checks
//! them against the device's granted limits and the budgets in
//! `docs/reference/performance-budgets.md`. Allocation size and storage-binding size are
//! separate limits: the radius-32 cell array (132 MiB) exceeds WebGPU's default 128 MiB
//! binding, so it needs a device granted a larger binding, and is refused otherwise. All size
//! arithmetic is checked; an overflow is a refusal, not a wrapped size.
//!
//! The plan is a pure function of its configuration and the limits, so a refused resize leaves
//! the current world untouched: nothing is allocated until the whole plan passes.
//!
//! Terrain-related state per window of `N` cells, `C` chunks and `T = 64 C` tiles, in bytes:
//! cells `8N`, flow words `8N`, scratch cells `8N`, the activity buffer
//! `4·(16 + 2C + 2T + 16T)` (since 05 with one change word per slot), body claim words `4N`, the
//! window map `4C` and slot records `32C`: `29.125 N + 44 C + 64`, inside the `48 N + 64 C`
//! envelope. Every list and scratch region is sized for every tile being active, so none can
//! overflow.
//!
//! Snapshots (05) are not live terrain and are planned beside it: a fixed image of
//! `snapshot_entries` chunks for chunk snapshots, `32 KiB` each, and the world image a world
//! snapshot allocates while it drains, `8N + 144 B` bytes (every slot's cells and every body's
//! state and configuration). The fixed image is allocated with the plan; the world image is
//! reserved in it ([`ResourcePlan::reserved_bytes`]), so the device budget holds with a world
//! snapshot draining, and a world snapshot is always within the plan it was requested under.
//!
//! Body state is sized by the body capacity `B` and the samples per tick `S`, not by the window
//! ([`BodyLayout`]): `4·(92 B + 26 S)` bytes of bodies buffer and `4·(3 H + B + 4)` of bins, with
//! `H` the bin hash table (a power of two at least `2 B`). Bins hold every body and are counted
//! and prefix-summed every substep, so no bin can overflow and no pair is dropped.

use world_model::{
    body::{MAX_BODIES, MAX_SUBSTEPS_PER_TICK},
    spatial::CHUNK_CELL_COUNT,
};
use world_runtime::port::{CapacityLimit, LimitedResource, ResidentWindow};

use crate::layout::{
    BODY_WORK_WORDS, BodyConfig, BodyDiagnosticsRecord, BodyDispatch, BodyParams, BodySampleRecord,
    BodyState, ChunkSlotRecord, CommandRecord, DiagnosticsRecord, DispatchParams, EventHeader,
    EventRecord, GpuRecord, ResultRecord, TickParams, TotalsRecord, activity,
};

/// Bytes of one packed cell: two `u32` words.
pub const CELL_BYTES: u64 = 8;
/// Bytes of one chunk's cells.
pub const CHUNK_BYTES: u64 = CHUNK_CELL_COUNT as u64 * CELL_BYTES;
/// Bytes of one window-map entry (a slot index).
pub const WINDOW_ENTRY_BYTES: u64 = 4;
/// Invocations per workgroup of the command kernels, along x and y for 2-D kernels.
pub const EDIT_WORKGROUP_EDGE: u32 = 8;
pub const LINEAR_WORKGROUP_SIZE: u32 = 64;
/// Storage buffers the command kernels bind: commands, cells, window map, slot records,
/// results, events, activity and diagnostics; WebGPU's default limit. The material kernels bind
/// six: cells, window map, flow, scratch, activity and diagnostics.
pub const STORAGE_BUFFERS_PER_STAGE: u32 = 8;
/// Bytes of one flow entry: a proposal and a resolution word per cell.
pub const FLOW_BYTES_PER_CELL: u64 = 8;
/// The largest brush radius in cells (`world_model::config::BRUSH_RADIUS_CELLS`).
const MAX_BRUSH_RADIUS: u32 = 64;
/// The tick's four compute passes (commands, activity, material, bodies) each record a begin
/// and an end timestamp.
pub const TIMESTAMP_QUERIES: u32 = 8;
/// Solver iterations a body dispatch entry can name per substep ([`BodyDispatch`]).
pub const BODY_DISPATCH_ITERATIONS: u32 = 16;
/// Bytes the resolved timestamps occupy; the resolve buffer must start at a 256-byte offset.
pub const TIMESTAMP_BYTES: u64 = TIMESTAMP_QUERIES as u64 * wgpu::QUERY_SIZE as u64;
pub const TIMESTAMP_RESOLVE_BYTES: u64 = wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT;

/// The budgets a plan is held to, separate from device limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budgets {
    /// Mapped staging the CPU holds at once (budgets: 64 MiB combined working set).
    pub staging_bytes: u64,
    /// Terrain-related device bytes per cell (budgets: at most 48 per cell).
    pub terrain_bytes_per_cell: u64,
    /// Fixed terrain-related device bytes per chunk: a slot record, a window entry and room for
    /// later per-chunk metadata.
    pub terrain_bytes_per_chunk: u64,
    /// Device bytes requested at once, counting old and new allocations during a resize. `None`
    /// leaves only the device's own limits and allocation failures.
    pub device_bytes: Option<u64>,
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            staging_bytes: 64 << 20,
            terrain_bytes_per_cell: 48,
            terrain_bytes_per_chunk: 64,
            device_bytes: None,
        }
    }
}

/// Sizes of the transfer rings and pools, each with a supported range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferConfig {
    /// Submitted, uncompleted ticks (budgets: at most 3).
    pub max_in_flight: u32,
    /// Commands one tick encodes. With the default catch-up of four ticks per frame, 256
    /// commands and their results are at most 4 × 256 × (48 + 48) bytes = 96 KiB per frame,
    /// inside the 256 KiB command-plus-observation budget.
    pub commands_per_tick: u32,
    /// Cosmetic events one tick can record before it counts drops.
    pub events_per_tick: u32,
    pub chunk_upload_slots: u32,
    pub capture_slots: u32,
    /// Chunks the chunk snapshot image holds: snapshots of outgoing chunks copied on the device
    /// and waiting to report whether they changed or to stream out.
    pub snapshot_entries: u32,
    /// Slot-record and window-entry changes one tick can carry (activations and retirements).
    pub metadata_patches_per_tick: u32,
    /// Bodies the world holds at once (at most `world_model::body::MAX_BODIES`).
    pub bodies: u32,
    /// Bodies one tick samples for the CPU. 64 samples are 6 KiB per tick, 24 KiB per frame at
    /// four ticks.
    pub body_samples_per_tick: u32,
}

impl Default for TransferConfig {
    fn default() -> Self {
        Self {
            max_in_flight: 3,
            commands_per_tick: 256,
            events_per_tick: 64,
            chunk_upload_slots: 32,
            capture_slots: 32,
            snapshot_entries: 32,
            metadata_patches_per_tick: 96,
            bodies: MAX_BODIES,
            body_samples_per_tick: 64,
        }
    }
}

impl TransferConfig {
    /// Checks every size against its supported range.
    pub fn validate(&self) -> Result<(), PlanError> {
        let ranges: [(&'static str, u32, u32, u32); 9] = [
            (
                "max_in_flight",
                self.max_in_flight,
                1,
                crate::context::MAX_IN_FLIGHT_LIMIT,
            ),
            ("commands_per_tick", self.commands_per_tick, 1, 4096),
            ("events_per_tick", self.events_per_tick, 1, 65_536),
            ("chunk_upload_slots", self.chunk_upload_slots, 1, 1024),
            ("capture_slots", self.capture_slots, 2, 1024),
            ("snapshot_entries", self.snapshot_entries, 1, 1024),
            (
                "metadata_patches_per_tick",
                self.metadata_patches_per_tick,
                1,
                4096,
            ),
            ("bodies", self.bodies, 1, MAX_BODIES),
            // A tick may sample stale handles too, so samples are not bounded by the bodies.
            ("body_samples_per_tick", self.body_samples_per_tick, 1, 1024),
        ];
        for (field, value, min, max) in ranges {
            if !(min..=max).contains(&value) {
                return Err(PlanError::Config {
                    field,
                    value,
                    min,
                    max,
                });
            }
        }
        if self.metadata_patches_per_tick < self.chunk_upload_slots {
            // Every staged upload must be able to activate in the next tick.
            return Err(PlanError::Config {
                field: "metadata_patches_per_tick",
                value: self.metadata_patches_per_tick,
                min: self.chunk_upload_slots,
                max: 4096,
            });
        }
        Ok(())
    }
}

/// The device limits a plan is checked against: the fields the session uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanLimits {
    pub max_buffer_size: u64,
    pub max_storage_binding: u64,
    pub max_uniform_binding: u64,
    pub min_uniform_offset_alignment: u32,
    pub max_workgroups_per_dimension: u32,
    pub max_invocations_per_workgroup: u32,
    pub max_workgroup_size_x: u32,
    pub max_workgroup_size_y: u32,
    pub max_storage_buffers_per_stage: u32,
}

impl PlanLimits {
    pub fn from_wgpu(limits: &wgpu::Limits) -> Self {
        Self {
            max_buffer_size: limits.max_buffer_size,
            max_storage_binding: limits.max_storage_buffer_binding_size,
            max_uniform_binding: limits.max_uniform_buffer_binding_size,
            min_uniform_offset_alignment: limits.min_uniform_buffer_offset_alignment,
            max_workgroups_per_dimension: limits.max_compute_workgroups_per_dimension,
            max_invocations_per_workgroup: limits.max_compute_invocations_per_workgroup,
            max_workgroup_size_x: limits.max_compute_workgroup_size_x,
            max_workgroup_size_y: limits.max_compute_workgroup_size_y,
            max_storage_buffers_per_stage: limits.max_storage_buffers_per_shader_stage,
        }
    }
}

/// Why a plan cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanError {
    Config {
        field: &'static str,
        value: u32,
        min: u32,
        max: u32,
    },
    /// Size arithmetic overflowed `u64`.
    Overflow(BufferRole),
    Limit {
        role: Option<BufferRole>,
        limit: CapacityLimit,
    },
}

impl PlanError {
    /// The refusal a port reports for this plan.
    pub fn refusal(self) -> world_runtime::port::Refusal {
        world_runtime::port::Refusal::Unsupported(self.capacity_limit())
    }

    pub fn capacity_limit(self) -> CapacityLimit {
        match self {
            Self::Config { value, max, .. } => CapacityLimit {
                resource: LimitedResource::Configuration,
                requested: u64::from(value),
                available: u64::from(max),
            },
            Self::Overflow(_) => CapacityLimit {
                resource: LimitedResource::IndexRange,
                requested: u64::MAX,
                available: u64::MAX,
            },
            Self::Limit { limit, .. } => limit,
        }
    }
}

/// What a buffer is for. Every plan holds exactly one [`BufferPlan`] per role, in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BufferRole {
    /// Packed cells of every slot, `slots × 4096 × 8` bytes.
    Cells,
    /// One slot index per window chunk, or `SLOT_NONE`.
    WindowMap,
    /// One [`ChunkSlotRecord`] per slot.
    SlotRecords,
    /// A proposal and a resolution word per cell, indexed like the cells.
    Flow,
    /// The new value of every processed cell between `react` and `commit`.
    Scratch,
    /// Tile records, the candidate and process lists, chunk wake requests and list counts
    /// ([`ActivityLayout`]).
    Activity,
    TickParams,
    /// One [`DispatchParams`] per command position, written once at creation.
    DispatchParams,
    Commands,
    Results,
    Events,
    /// The tick's [`DiagnosticsRecord`], then one [`TotalsRecord`] per command position.
    Diagnostics,
    /// Indirect dispatch arguments of the candidate and process lists.
    DispatchArgs,
    /// One owner word per cell for the body substeps' deformation and displacement claims.
    Claims,
    /// Body state, configuration, per-substep scratch, samples and the observe list
    /// ([`BodyLayout`]).
    Bodies,
    /// The body broadphase: bin counts, starts, fill cursors, entries and live bounds.
    Bins,
    BodyParams,
    /// One [`BodyDispatch`] per substep and iteration, written once at creation.
    BodyDispatch,
    TimestampResolve,
    TickUploadStaging,
    TickReadbackStaging,
    ChunkUploadStaging,
    CaptureStaging,
    /// Chunk snapshots copied on the device: `snapshot_entries` chunks (05).
    SnapshotImage,
}

impl BufferRole {
    pub const ALL: [Self; 24] = [
        Self::Cells,
        Self::WindowMap,
        Self::SlotRecords,
        Self::Flow,
        Self::Scratch,
        Self::Activity,
        Self::TickParams,
        Self::DispatchParams,
        Self::Commands,
        Self::Results,
        Self::Events,
        Self::Diagnostics,
        Self::DispatchArgs,
        Self::Claims,
        Self::Bodies,
        Self::Bins,
        Self::BodyParams,
        Self::BodyDispatch,
        Self::TimestampResolve,
        Self::TickUploadStaging,
        Self::TickReadbackStaging,
        Self::ChunkUploadStaging,
        Self::CaptureStaging,
        Self::SnapshotImage,
    ];

    /// Buffers whose size depends on the resident window; a resize replaces them.
    pub fn follows_window(self) -> bool {
        matches!(
            self,
            Self::Cells
                | Self::WindowMap
                | Self::SlotRecords
                | Self::Flow
                | Self::Scratch
                | Self::Activity
                | Self::Claims
        )
    }

    /// Buffers the CPU maps.
    pub fn is_staging(self) -> bool {
        matches!(
            self,
            Self::TickUploadStaging
                | Self::TickReadbackStaging
                | Self::ChunkUploadStaging
                | Self::CaptureStaging
        )
    }

    /// Buffers that hold terrain-related state.
    pub fn is_terrain(self) -> bool {
        self.follows_window()
    }
}

/// How a kernel binds a buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    Storage,
    Uniform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferPlan {
    pub role: BufferRole,
    /// Bytes of each buffer.
    pub size: u64,
    /// Buffers of this role (staging pools have several).
    pub count: u32,
    pub usage: wgpu::BufferUsages,
    pub binding: Option<Binding>,
    /// Bytes one binding covers, when it is not the whole buffer.
    pub binding_size: u64,
}

impl BufferPlan {
    pub fn total(&self) -> Option<u64> {
        self.size.checked_mul(u64::from(self.count))
    }
}

/// Byte offsets inside one tick upload staging buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickUploadLayout {
    pub params: u64,
    /// The event buffer's header for the tick: zero counts and the capacity.
    pub event_header: u64,
    pub commands: u64,
    pub slot_records: u64,
    pub window_entries: u64,
    /// One word holding 1, copied into the chunk wake request of each activated or retired
    /// chunk.
    pub one: u64,
    /// The tick's [`BodyParams`] and the handles to sample (two words each).
    pub body_params: u64,
    pub observe: u64,
    pub size: u64,
}

/// Byte offsets inside one tick readback staging buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickReadbackLayout {
    pub results: u64,
    pub events: u64,
    pub diagnostics: u64,
    /// One [`TotalsRecord`] per command position; only totals commands' records are copied.
    pub totals: u64,
    /// The tick's [`BodyDiagnosticsRecord`], then its body samples.
    pub body_diagnostics: u64,
    pub samples: u64,
    pub timestamps: u64,
    pub size: u64,
}

/// Word offsets of the activity buffer's regions. The header holds the list counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivityLayout {
    /// One wake request word per window chunk.
    pub requests: u32,
    /// Candidate tiles of the tick, then process tiles: room for every tile in each.
    pub candidates: u32,
    pub process: u32,
    /// One record of `TILE_RECORD_WORDS` per window tile.
    pub records: u32,
    /// One change word per slot (05): nonzero once any cell of the slot's chunk changed since
    /// the slot was activated or last snapshotted. Every writer of cell values sets it; an
    /// activation and a snapshot clear it, in timeline order.
    pub changes: u32,
    pub words: u32,
}

impl ActivityLayout {
    fn for_window(chunks: u32) -> Option<Self> {
        let tiles = chunks.checked_mul(activity::TILES_PER_CHUNK)?;
        let requests = activity::HEADER_WORDS;
        let candidates = requests.checked_add(chunks)?;
        let process = candidates.checked_add(tiles)?;
        let records = process.checked_add(tiles)?;
        let changes = records.checked_add(tiles.checked_mul(activity::TILE_RECORD_WORDS)?)?;
        let words = changes.checked_add(chunks)?;
        Some(Self {
            requests,
            candidates,
            process,
            records,
            changes,
            words,
        })
    }

    /// Byte offset of a slot's change word.
    pub fn change_word(&self, slot: u32) -> u64 {
        (u64::from(self.changes) + u64::from(slot)) * 4
    }

    /// Byte offset and size of a window chunk's tile records.
    pub fn chunk_records(&self, window_index: u32) -> (u64, u64) {
        let words = u64::from(activity::TILES_PER_CHUNK * activity::TILE_RECORD_WORDS);
        (
            (u64::from(self.records) + u64::from(window_index) * words) * 4,
            words * 4,
        )
    }
}

/// Word offsets of the regions of the bodies and bins buffers, fixed by the body capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyLayout {
    pub capacity: u32,
    pub samples: u32,
    pub state: u32,
    pub config: u32,
    pub work: u32,
    pub sample_records: u32,
    pub observe: u32,
    pub body_words: u32,
    /// Bin hash table slots: a power of two at least twice the capacity.
    pub hash_slots: u32,
    pub counts: u32,
    pub starts: u32,
    pub fill: u32,
    pub entries: u32,
    pub bounds: u32,
    pub bin_words: u32,
    /// Word offset of the [`BodyDiagnosticsRecord`] in the diagnostics buffer.
    pub diagnostics: u32,
}

/// Words of the bins buffer's live-bounds record: the largest radius and the farthest travel
/// of the substep, as float bits.
pub const BIN_BOUNDS_WORDS: u32 = 4;

impl BodyLayout {
    /// The regions for `capacity` bodies, `samples` samples per tick and `commands` command
    /// positions (whose totals records precede the body record in the diagnostics buffer).
    pub fn new(capacity: u32, samples: u32, commands: u32) -> Option<Self> {
        let words = |count: u32, per: usize| count.checked_mul(per as u32);
        let state = 0;
        let config = state + words(capacity, BodyState::WORDS)?;
        let work = config.checked_add(words(capacity, BodyConfig::WORDS)?)?;
        let sample_records = work.checked_add(capacity.checked_mul(BODY_WORK_WORDS)?)?;
        let observe = sample_records.checked_add(words(samples, BodySampleRecord::WORDS)?)?;
        let body_words = observe.checked_add(samples.checked_mul(2)?)?;
        let hash_slots = capacity
            .checked_mul(2)?
            .checked_next_power_of_two()?
            .max(64);
        let counts = 0;
        let starts = hash_slots;
        let fill = starts.checked_add(hash_slots)?;
        let entries = fill.checked_add(hash_slots)?;
        let bounds = entries.checked_add(capacity)?;
        let bin_words = bounds.checked_add(BIN_BOUNDS_WORDS)?;
        let diagnostics = (DiagnosticsRecord::WORDS as u32)
            .checked_add(commands.checked_mul(TotalsRecord::WORDS as u32)?)?;
        Some(Self {
            capacity,
            samples,
            state,
            config,
            work,
            sample_records,
            observe,
            body_words,
            hash_slots,
            counts,
            starts,
            fill,
            entries,
            bounds,
            bin_words,
            diagnostics,
        })
    }
}

/// A checked plan for one resident window and transfer configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourcePlan {
    pub window: ResidentWindow,
    pub transfer: TransferConfig,
    pub slots: u32,
    pub cells: u64,
    pub tiles: u32,
    pub activity: ActivityLayout,
    pub dispatch_param_stride: u64,
    pub tick_upload: TickUploadLayout,
    pub tick_readback: TickReadbackLayout,
    pub bodies: BodyLayout,
    pub body_dispatch_stride: u64,
    /// One entry per [`BufferRole`], in [`BufferRole::ALL`] order.
    pub buffers: [BufferPlan; BufferRole::ALL.len()],
    pub device_bytes: u64,
    pub staging_bytes: u64,
    pub terrain_bytes: u64,
    /// Bytes of the world image a world snapshot copies into for this window: every slot's
    /// cells, then every body slot's state and configuration. Not in `device_bytes` (it is
    /// allocated at the first world snapshot and kept for the next ones while the window keeps
    /// its size) but in [`reserved_bytes`](Self::reserved_bytes).
    pub world_image_bytes: u64,
    pub timestamps: bool,
}

impl ResourcePlan {
    pub fn buffer(&self, role: BufferRole) -> &BufferPlan {
        &self.buffers[role as usize]
    }

    /// Device bytes of the buffers a resize replaces.
    pub fn window_bytes(&self) -> u64 {
        self.buffers
            .iter()
            .filter(|buffer| buffer.role.follows_window())
            .filter_map(BufferPlan::total)
            .sum()
    }

    /// Device bytes the plan allocates plus the world image it may allocate.
    pub fn reserved_bytes(&self) -> u64 {
        self.device_bytes.saturating_add(self.world_image_bytes)
    }

    /// Device bytes requested at most while this plan's buffers and `next`'s window buffers
    /// coexist, with one world snapshot of either window allocated: an old one still draining,
    /// or a new one requested before the old window buffers are released.
    pub fn overlap_with(&self, next: &ResourcePlan) -> u64 {
        self.device_bytes
            .saturating_add(next.window_bytes())
            .saturating_add(self.world_image_bytes.max(next.world_image_bytes))
    }
}

fn checked(role: BufferRole, value: Option<u64>) -> Result<u64, PlanError> {
    value.ok_or(PlanError::Overflow(role))
}

fn align_up(value: u64, alignment: u64) -> Option<u64> {
    let alignment = alignment.max(1);
    value
        .checked_add(alignment - 1)
        .map(|v| v / alignment * alignment)
}

/// One buffer a kernel binds whole.
fn bound(role: BufferRole, size: u64, usage: wgpu::BufferUsages, binding: Binding) -> BufferPlan {
    BufferPlan {
        role,
        size,
        count: 1,
        usage,
        binding: Some(binding),
        binding_size: size,
    }
}

/// `count` buffers no kernel binds (staging, query resolution).
fn unbound(role: BufferRole, size: u64, count: u32, usage: wgpu::BufferUsages) -> BufferPlan {
    BufferPlan {
        role,
        size,
        count,
        usage,
        binding: None,
        binding_size: 0,
    }
}

fn limit(
    role: Option<BufferRole>,
    resource: LimitedResource,
    requested: u64,
    available: u64,
) -> PlanError {
    PlanError::Limit {
        role,
        limit: CapacityLimit {
            resource,
            requested,
            available,
        },
    }
}

/// Plans every buffer for `window` and `transfer` and checks it against `limits` and `budgets`.
pub fn plan(
    window: ResidentWindow,
    transfer: TransferConfig,
    timestamps: bool,
    limits: &PlanLimits,
    budgets: &Budgets,
) -> Result<ResourcePlan, PlanError> {
    transfer.validate()?;
    let slots = window.chunk_count();
    let cells = u64::from(slots) * CHUNK_CELL_COUNT as u64;
    let cell_bytes = checked(BufferRole::Cells, cells.checked_mul(CELL_BYTES))?;
    // Kernels index cells and words with `u32`.
    let words = checked(BufferRole::Cells, cells.checked_mul(2))?;
    if words > u64::from(u32::MAX) {
        return Err(limit(
            Some(BufferRole::Cells),
            LimitedResource::IndexRange,
            words,
            u64::from(u32::MAX),
        ));
    }

    let commands = u64::from(transfer.commands_per_tick);
    let events = u64::from(transfer.events_per_tick);
    let patches = u64::from(transfer.metadata_patches_per_tick);
    let dispatch_param_stride = checked(
        BufferRole::DispatchParams,
        align_up(
            DispatchParams::BYTES,
            u64::from(limits.min_uniform_offset_alignment),
        ),
    )?;

    let body_layout = BodyLayout::new(
        transfer.bodies,
        transfer.body_samples_per_tick,
        transfer.commands_per_tick,
    )
    .ok_or(PlanError::Overflow(BufferRole::Bodies))?;
    let body_dispatch_stride = checked(
        BufferRole::BodyDispatch,
        align_up(
            BodyDispatch::BYTES,
            u64::from(limits.min_uniform_offset_alignment),
        ),
    )?;
    let tiles = slots
        .checked_mul(activity::TILES_PER_CHUNK)
        .ok_or(PlanError::Overflow(BufferRole::Activity))?;
    let activity_layout =
        ActivityLayout::for_window(slots).ok_or(PlanError::Overflow(BufferRole::Activity))?;
    let tick_upload = {
        let params = 0;
        let event_header = TickParams::BYTES;
        let commands_at = event_header + EventHeader::BYTES;
        let records_at = checked(
            BufferRole::TickUploadStaging,
            commands
                .checked_mul(CommandRecord::BYTES)
                .and_then(|bytes| bytes.checked_add(commands_at)),
        )?;
        let entries_at = checked(
            BufferRole::TickUploadStaging,
            patches
                .checked_mul(ChunkSlotRecord::BYTES)
                .and_then(|bytes| bytes.checked_add(records_at)),
        )?;
        let one = checked(
            BufferRole::TickUploadStaging,
            patches
                .checked_mul(WINDOW_ENTRY_BYTES)
                .and_then(|bytes| bytes.checked_add(entries_at)),
        )?;
        let body_params = checked(BufferRole::TickUploadStaging, align_up(one + 4, 16))?;
        let observe = body_params + BodyParams::BYTES;
        let size = checked(
            BufferRole::TickUploadStaging,
            align_up(
                observe + u64::from(transfer.body_samples_per_tick) * 8,
                wgpu::COPY_BUFFER_ALIGNMENT,
            ),
        )?;
        TickUploadLayout {
            params,
            event_header,
            commands: commands_at,
            slot_records: records_at,
            window_entries: entries_at,
            one,
            body_params,
            observe,
            size,
        }
    };
    let tick_readback = {
        let results = 0;
        let events_at = checked(
            BufferRole::TickReadbackStaging,
            commands.checked_mul(ResultRecord::BYTES),
        )?;
        let diagnostics_at = checked(
            BufferRole::TickReadbackStaging,
            events
                .checked_mul(EventRecord::BYTES)
                .and_then(|bytes| bytes.checked_add(EventHeader::BYTES + events_at)),
        )?;
        let totals_at = diagnostics_at + DiagnosticsRecord::BYTES;
        let body_diagnostics_at = checked(
            BufferRole::TickReadbackStaging,
            commands
                .checked_mul(TotalsRecord::BYTES)
                .and_then(|bytes| bytes.checked_add(totals_at)),
        )?;
        let samples_at = body_diagnostics_at + BodyDiagnosticsRecord::BYTES;
        let timestamps_at =
            samples_at + u64::from(transfer.body_samples_per_tick) * BodySampleRecord::BYTES;
        let size = timestamps_at + TIMESTAMP_BYTES;
        TickReadbackLayout {
            results,
            events: events_at,
            diagnostics: diagnostics_at,
            totals: totals_at,
            body_diagnostics: body_diagnostics_at,
            samples: samples_at,
            timestamps: timestamps_at,
            size,
        }
    };

    use wgpu::BufferUsages as Usage;
    let window_entries = u64::from(slots) * WINDOW_ENTRY_BYTES;
    let slot_records = u64::from(slots) * ChunkSlotRecord::BYTES;
    let dispatch_params = checked(
        BufferRole::DispatchParams,
        commands.checked_mul(dispatch_param_stride),
    )?;
    let event_bytes = EventHeader::BYTES + events * EventRecord::BYTES;
    let flow_bytes = checked(BufferRole::Flow, cells.checked_mul(FLOW_BYTES_PER_CELL))?;
    let activity_bytes = u64::from(activity_layout.words) * 4;
    let diagnostics_bytes =
        DiagnosticsRecord::BYTES + commands * TotalsRecord::BYTES + BodyDiagnosticsRecord::BYTES;
    let claims_bytes = checked(BufferRole::Claims, cells.checked_mul(4))?;
    let body_bytes = u64::from(body_layout.body_words) * 4;
    let bin_bytes = u64::from(body_layout.bin_words) * 4;
    let body_dispatch_bytes = checked(
        BufferRole::BodyDispatch,
        u64::from(MAX_SUBSTEPS_PER_TICK * BODY_DISPATCH_ITERATIONS)
            .checked_mul(body_dispatch_stride),
    )?;
    // One more tick staging buffer than submissions in flight: a buffer is still being
    // remapped (upload) or decoded (readback) while the next tick encodes.
    let in_flight_pool = transfer.max_in_flight + 1;
    let storage = Usage::STORAGE;
    let buffers = [
        bound(
            BufferRole::Cells,
            cell_bytes,
            storage | Usage::COPY_DST | Usage::COPY_SRC,
            Binding::Storage,
        ),
        bound(
            BufferRole::WindowMap,
            window_entries,
            storage | Usage::COPY_DST,
            Binding::Storage,
        ),
        bound(
            BufferRole::SlotRecords,
            slot_records,
            storage | Usage::COPY_DST,
            Binding::Storage,
        ),
        bound(BufferRole::Flow, flow_bytes, storage, Binding::Storage),
        // Copy usages: a window move carries the tile records through it between ticks.
        bound(
            BufferRole::Scratch,
            cell_bytes,
            storage | Usage::COPY_SRC | Usage::COPY_DST,
            Binding::Storage,
        ),
        bound(
            BufferRole::Activity,
            activity_bytes,
            storage | Usage::COPY_DST | Usage::COPY_SRC,
            Binding::Storage,
        ),
        bound(
            BufferRole::TickParams,
            TickParams::BYTES,
            Usage::UNIFORM | Usage::COPY_DST,
            Binding::Uniform,
        ),
        // Bound one entry at a time through a dynamic offset.
        BufferPlan {
            binding_size: DispatchParams::BYTES,
            ..bound(
                BufferRole::DispatchParams,
                dispatch_params,
                Usage::UNIFORM,
                Binding::Uniform,
            )
        },
        bound(
            BufferRole::Commands,
            commands * CommandRecord::BYTES,
            storage | Usage::COPY_DST,
            Binding::Storage,
        ),
        bound(
            BufferRole::Results,
            commands * ResultRecord::BYTES,
            storage | Usage::COPY_SRC,
            Binding::Storage,
        ),
        bound(
            BufferRole::Events,
            event_bytes,
            storage | Usage::COPY_SRC | Usage::COPY_DST,
            Binding::Storage,
        ),
        bound(
            BufferRole::Diagnostics,
            diagnostics_bytes,
            storage | Usage::COPY_SRC | Usage::COPY_DST,
            Binding::Storage,
        ),
        bound(
            BufferRole::DispatchArgs,
            u64::from(activity::ARGS_WORDS) * 4,
            storage | Usage::INDIRECT,
            Binding::Storage,
        ),
        bound(BufferRole::Claims, claims_bytes, storage, Binding::Storage),
        bound(
            BufferRole::Bodies,
            body_bytes,
            storage | Usage::COPY_SRC | Usage::COPY_DST,
            Binding::Storage,
        ),
        bound(BufferRole::Bins, bin_bytes, storage, Binding::Storage),
        bound(
            BufferRole::BodyParams,
            BodyParams::BYTES,
            Usage::UNIFORM | Usage::COPY_DST,
            Binding::Uniform,
        ),
        BufferPlan {
            binding_size: BodyDispatch::BYTES,
            ..bound(
                BufferRole::BodyDispatch,
                body_dispatch_bytes,
                Usage::UNIFORM,
                Binding::Uniform,
            )
        },
        unbound(
            BufferRole::TimestampResolve,
            TIMESTAMP_RESOLVE_BYTES,
            u32::from(timestamps),
            Usage::QUERY_RESOLVE | Usage::COPY_SRC,
        ),
        unbound(
            BufferRole::TickUploadStaging,
            tick_upload.size,
            in_flight_pool,
            Usage::MAP_WRITE | Usage::COPY_SRC,
        ),
        unbound(
            BufferRole::TickReadbackStaging,
            tick_readback.size,
            in_flight_pool,
            Usage::MAP_READ | Usage::COPY_DST,
        ),
        unbound(
            BufferRole::ChunkUploadStaging,
            CHUNK_BYTES,
            transfer.chunk_upload_slots,
            Usage::MAP_WRITE | Usage::COPY_SRC,
        ),
        unbound(
            BufferRole::CaptureStaging,
            CHUNK_BYTES,
            transfer.capture_slots,
            Usage::MAP_READ | Usage::COPY_DST,
        ),
        unbound(
            BufferRole::SnapshotImage,
            checked(
                BufferRole::SnapshotImage,
                u64::from(transfer.snapshot_entries).checked_mul(CHUNK_BYTES),
            )?,
            1,
            Usage::COPY_SRC | Usage::COPY_DST,
        ),
    ];
    debug_assert!(
        buffers
            .iter()
            .zip(BufferRole::ALL)
            .all(|(buffer, role)| buffer.role == role)
    );

    let mut device_bytes = 0u64;
    let mut staging_bytes = 0u64;
    let mut terrain_bytes = 0u64;
    for buffer in &buffers {
        if buffer.size > limits.max_buffer_size {
            return Err(limit(
                Some(buffer.role),
                LimitedResource::BufferSize,
                buffer.size,
                limits.max_buffer_size,
            ));
        }
        match buffer.binding {
            Some(Binding::Storage) if buffer.binding_size > limits.max_storage_binding => {
                return Err(limit(
                    Some(buffer.role),
                    LimitedResource::StorageBinding,
                    buffer.binding_size,
                    limits.max_storage_binding,
                ));
            }
            Some(Binding::Uniform) if buffer.binding_size > limits.max_uniform_binding => {
                return Err(limit(
                    Some(buffer.role),
                    LimitedResource::StorageBinding,
                    buffer.binding_size,
                    limits.max_uniform_binding,
                ));
            }
            _ => {}
        }
        let total = checked(buffer.role, buffer.total())?;
        device_bytes = checked(buffer.role, device_bytes.checked_add(total))?;
        if buffer.role.is_staging() {
            staging_bytes = checked(buffer.role, staging_bytes.checked_add(total))?;
        }
        if buffer.role.is_terrain() {
            terrain_bytes = checked(buffer.role, terrain_bytes.checked_add(total))?;
        }
    }

    // The world image the first world snapshot allocates (and later ones reuse) is part of the
    // plan: it must fit one buffer and the device budget like the buffers allocated at once.
    let world_image_bytes = checked(
        BufferRole::SnapshotImage,
        cell_bytes.checked_add(u64::from(transfer.bodies) * (BodyState::BYTES + BodyConfig::BYTES)),
    )?;
    if world_image_bytes > limits.max_buffer_size {
        return Err(limit(
            Some(BufferRole::SnapshotImage),
            LimitedResource::BufferSize,
            world_image_bytes,
            limits.max_buffer_size,
        ));
    }
    let reserved_bytes = checked(
        BufferRole::SnapshotImage,
        device_bytes.checked_add(world_image_bytes),
    )?;
    if STORAGE_BUFFERS_PER_STAGE > limits.max_storage_buffers_per_stage {
        return Err(limit(
            None,
            LimitedResource::StorageBuffersPerStage,
            u64::from(STORAGE_BUFFERS_PER_STAGE),
            u64::from(limits.max_storage_buffers_per_stage),
        ));
    }
    let edit_groups = (2 * MAX_BRUSH_RADIUS + 1).div_ceil(EDIT_WORKGROUP_EDGE);
    let begin_groups = transfer.commands_per_tick.div_ceil(LINEAR_WORKGROUP_SIZE);
    let flood_groups = transfer
        .events_per_tick
        .saturating_mul(4)
        .div_ceil(LINEAR_WORKGROUP_SIZE);
    // `scan_tiles` and `measure_totals` dispatch one workgroup (row) per window chunk; list
    // dispatches split at DISPATCH_SPLIT along x and continue along y. Body kernels dispatch
    // one workgroup per body slot or per 64 bin slots.
    let list_rows = tiles.div_ceil(activity::DISPATCH_SPLIT);
    for groups in [
        edit_groups,
        begin_groups,
        flood_groups,
        slots,
        activity::DISPATCH_SPLIT,
        list_rows,
        transfer.bodies,
        body_layout.hash_slots / LINEAR_WORKGROUP_SIZE,
    ] {
        if groups > limits.max_workgroups_per_dimension {
            return Err(limit(
                None,
                LimitedResource::DispatchWorkgroups,
                u64::from(groups),
                u64::from(limits.max_workgroups_per_dimension),
            ));
        }
    }
    let invocations = EDIT_WORKGROUP_EDGE * EDIT_WORKGROUP_EDGE;
    if invocations > limits.max_invocations_per_workgroup
        || LINEAR_WORKGROUP_SIZE > limits.max_invocations_per_workgroup
        || LINEAR_WORKGROUP_SIZE > limits.max_workgroup_size_x
        || EDIT_WORKGROUP_EDGE > limits.max_workgroup_size_y
    {
        return Err(limit(
            None,
            LimitedResource::DispatchWorkgroups,
            u64::from(LINEAR_WORKGROUP_SIZE.max(invocations)),
            u64::from(limits.max_invocations_per_workgroup),
        ));
    }

    if staging_bytes > budgets.staging_bytes {
        return Err(limit(
            None,
            LimitedResource::StagingBudget,
            staging_bytes,
            budgets.staging_bytes,
        ));
    }
    let envelope = cells
        .checked_mul(budgets.terrain_bytes_per_cell)
        .and_then(|bytes| bytes.checked_add(u64::from(slots) * budgets.terrain_bytes_per_chunk));
    let envelope = checked(BufferRole::Cells, envelope)?;
    if terrain_bytes > envelope {
        return Err(limit(
            Some(BufferRole::Cells),
            LimitedResource::TerrainEnvelope,
            terrain_bytes,
            envelope,
        ));
    }
    if let Some(budget) = budgets.device_bytes
        && reserved_bytes > budget
    {
        return Err(limit(
            None,
            LimitedResource::DeviceBytes,
            reserved_bytes,
            budget,
        ));
    }

    Ok(ResourcePlan {
        window,
        transfer,
        slots,
        cells,
        tiles,
        activity: activity_layout,
        dispatch_param_stride,
        tick_upload,
        tick_readback,
        bodies: body_layout,
        body_dispatch_stride,
        buffers,
        device_bytes,
        staging_bytes,
        terrain_bytes,
        world_image_bytes,
        timestamps,
    })
}

/// The largest buffer and storage binding any supported window needs with `transfer` (radius
/// 32 on both axes), which is what a session asks its device for. Planned without device
/// limits, so the answer depends only on the supported envelope.
pub fn largest_supported_needs(transfer: TransferConfig) -> Result<(u64, u64), PlanError> {
    let largest = *world_model::config::ACTIVE_RADIUS_CHUNKS.end();
    let window = ResidentWindow::new(
        world_model::spatial::ChunkKey::new(0, 0, 0),
        largest,
        largest,
    )
    .map_err(|_| PlanError::Overflow(BufferRole::Cells))?;
    let unlimited = PlanLimits {
        max_buffer_size: u64::MAX,
        max_storage_binding: u64::MAX,
        max_uniform_binding: u64::MAX,
        min_uniform_offset_alignment: 256,
        max_workgroups_per_dimension: u32::MAX,
        max_invocations_per_workgroup: u32::MAX,
        max_workgroup_size_x: u32::MAX,
        max_workgroup_size_y: u32::MAX,
        max_storage_buffers_per_stage: u32::MAX,
    };
    let budgets = Budgets {
        staging_bytes: u64::MAX,
        ..Budgets::default()
    };
    let plan = plan(window, transfer, false, &unlimited, &budgets)?;
    let largest_buffer = plan
        .buffers
        .iter()
        .map(|buffer| buffer.size)
        .max()
        .unwrap_or(0)
        .max(plan.world_image_bytes);
    let largest_binding = plan
        .buffers
        .iter()
        .filter(|buffer| buffer.binding == Some(Binding::Storage))
        .map(|buffer| buffer.binding_size)
        .max()
        .unwrap_or(0);
    Ok((largest_buffer, largest_binding))
}

#[cfg(test)]
mod tests {
    use world_model::spatial::ChunkKey;

    use super::*;

    fn webgpu_defaults() -> PlanLimits {
        PlanLimits::from_wgpu(&wgpu::Limits::default())
    }

    fn window(radius: u32) -> ResidentWindow {
        ResidentWindow::new(ChunkKey::new(0, 0, 0), radius, radius).expect("supported")
    }

    #[test]
    fn the_budgets_arithmetic_is_reproduced() {
        // docs/reference/performance-budgets.md: one packed array at radius 6, 16 and 32.
        for (radius, mib) in [(6, 5.28125), (16, 34.03125), (32, 132.03125)] {
            let mut limits = webgpu_defaults();
            limits.max_buffer_size = u64::MAX;
            limits.max_storage_binding = u64::MAX;
            let plan = plan(
                window(radius),
                TransferConfig::default(),
                false,
                &limits,
                &Budgets::default(),
            )
            .expect("plan");
            assert_eq!(
                plan.buffer(BufferRole::Cells).size as f64 / f64::from(1 << 20),
                mib
            );
        }
    }

    #[test]
    fn radius_32_exceeds_a_default_storage_binding_and_radius_31_fits() {
        let refused = plan(
            window(32),
            TransferConfig::default(),
            false,
            &webgpu_defaults(),
            &Budgets::default(),
        );
        assert_eq!(
            refused,
            Err(PlanError::Limit {
                role: Some(BufferRole::Cells),
                limit: CapacityLimit {
                    resource: LimitedResource::StorageBinding,
                    requested: 138_444_800,
                    available: 128 << 20,
                },
            })
        );
        let fits = plan(
            window(31),
            TransferConfig::default(),
            false,
            &webgpu_defaults(),
            &Budgets::default(),
        )
        .expect("radius 31 fits a default binding");
        assert_eq!(fits.buffer(BufferRole::Cells).size, 130_056_192);
    }

    #[test]
    fn unsupported_configurations_and_budgets_are_refused() {
        let limits = webgpu_defaults();
        let budgets = Budgets::default();
        for broken in [
            TransferConfig {
                max_in_flight: 0,
                ..Default::default()
            },
            TransferConfig {
                max_in_flight: 4,
                ..Default::default()
            },
            TransferConfig {
                commands_per_tick: 0,
                ..Default::default()
            },
            TransferConfig {
                chunk_upload_slots: 97,
                ..Default::default()
            },
        ] {
            assert!(
                matches!(
                    plan(window(2), broken, false, &limits, &budgets),
                    Err(PlanError::Config { .. })
                ),
                "{broken:?}"
            );
        }
        let small = Budgets {
            staging_bytes: 1 << 20,
            ..Default::default()
        };
        assert!(matches!(
            plan(window(2), TransferConfig::default(), false, &limits, &small),
            Err(PlanError::Limit {
                limit: CapacityLimit {
                    resource: LimitedResource::StagingBudget,
                    ..
                },
                ..
            })
        ));
        let tight = Budgets {
            device_bytes: Some(1 << 20),
            ..Default::default()
        };
        assert!(matches!(
            plan(window(2), TransferConfig::default(), false, &limits, &tight),
            Err(PlanError::Limit {
                limit: CapacityLimit {
                    resource: LimitedResource::DeviceBytes,
                    ..
                },
                ..
            })
        ));
        let crowded = PlanLimits {
            max_storage_buffers_per_stage: 4,
            ..limits
        };
        assert!(matches!(
            plan(
                window(2),
                TransferConfig::default(),
                false,
                &crowded,
                &budgets
            ),
            Err(PlanError::Limit {
                limit: CapacityLimit {
                    resource: LimitedResource::StorageBuffersPerStage,
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    fn the_device_is_asked_for_what_radius_32_needs() {
        // The largest buffer is the world image: every slot's cells (138,444,800 bytes) and
        // 4,096 bodies of 128 + 16 bytes. The largest binding is still the cell array.
        assert_eq!(
            largest_supported_needs(TransferConfig::default()),
            Ok((138_444_800 + 4096 * 144, 138_444_800))
        );
    }

    #[test]
    fn snapshots_are_planned_with_the_window() {
        let plan = plan(
            window(6),
            TransferConfig::default(),
            false,
            &webgpu_defaults(),
            &Budgets::default(),
        )
        .expect("plan");
        assert_eq!(
            plan.buffer(BufferRole::SnapshotImage).size,
            32 * CHUNK_BYTES
        );
        assert_eq!(
            plan.world_image_bytes,
            plan.buffer(BufferRole::Cells).size + 4096 * 144
        );
        let allocated: u64 = plan.buffers.iter().filter_map(BufferPlan::total).sum();
        assert_eq!(plan.device_bytes, allocated);
        assert_eq!(plan.reserved_bytes(), allocated + plan.world_image_bytes);
        // One change word per slot, after the tile records.
        let layout = plan.activity;
        assert_eq!(
            layout.changes,
            layout.records + plan.tiles * activity::TILE_RECORD_WORDS
        );
        assert_eq!(layout.words, layout.changes + plan.slots);
        assert_eq!(layout.change_word(2), u64::from(layout.changes + 2) * 4);
        let budgets = Budgets {
            device_bytes: Some(allocated),
            ..Budgets::default()
        };
        assert!(
            matches!(
                super::plan(
                    window(6),
                    TransferConfig::default(),
                    false,
                    &webgpu_defaults(),
                    &budgets
                ),
                Err(PlanError::Limit {
                    limit: CapacityLimit {
                        resource: LimitedResource::DeviceBytes,
                        ..
                    },
                    ..
                })
            ),
            "a budget without room for the world image refuses the plan"
        );
    }

    #[test]
    fn layouts_are_aligned_and_staging_is_counted() {
        let plan = plan(
            window(6),
            TransferConfig::default(),
            true,
            &webgpu_defaults(),
            &Budgets::default(),
        )
        .expect("plan");
        assert_eq!(plan.dispatch_param_stride % 256, 0);
        for offset in [
            plan.tick_upload.commands,
            plan.tick_upload.slot_records,
            plan.tick_upload.window_entries,
            plan.tick_upload.size,
            plan.tick_readback.events,
            plan.tick_readback.diagnostics,
            plan.tick_readback.totals,
            plan.tick_readback.timestamps,
            plan.tick_readback.body_diagnostics,
            plan.tick_readback.samples,
            plan.tick_readback.size,
            plan.tick_upload.one,
            plan.tick_upload.body_params,
            plan.tick_upload.observe,
        ] {
            assert_eq!(offset % wgpu::COPY_BUFFER_ALIGNMENT, 0, "{offset}");
        }
        let staging: u64 = plan
            .buffers
            .iter()
            .filter(|buffer| buffer.role.is_staging())
            .map(|buffer| buffer.size * u64::from(buffer.count))
            .sum();
        assert_eq!(plan.staging_bytes, staging);
        assert!(plan.terrain_bytes <= plan.cells * 48 + u64::from(plan.slots) * 64);
        // The documented formula: 29.125 bytes per cell, 44 per chunk (since 05 with a change
        // word per slot) and the 64-byte header.
        assert_eq!(
            plan.terrain_bytes,
            plan.cells * 29 + plan.cells / 8 + u64::from(plan.slots) * 44 + 64
        );
        // Body state: 92 words per body and 26 per sample; bins 3 H + B + 4 words.
        let bodies = plan.bodies;
        assert_eq!(bodies.hash_slots, 8192);
        assert_eq!(bodies.body_words, 92 * 4096 + 26 * 64);
        assert_eq!(bodies.bin_words, 3 * 8192 + 4096 + 4);
        assert_eq!(
            plan.buffer(BufferRole::Bodies).size,
            4 * (92 * 4096 + 26 * 64)
        );
        assert_eq!(plan.body_dispatch_stride % 256, 0);
        assert_eq!(plan.tiles, plan.slots * 64);
        assert_eq!(
            plan.activity.words,
            16 + 2 * plan.slots + 2 * plan.tiles + 16 * plan.tiles
        );
        assert_eq!(plan.buffer(BufferRole::TimestampResolve).count, 1);
        let bigger = super::plan(
            window(7),
            TransferConfig::default(),
            true,
            &webgpu_defaults(),
            &Budgets::default(),
        )
        .expect("plan");
        // The old allocations, the new window buffers, and the larger window's world image.
        assert_eq!(
            plan.overlap_with(&bigger),
            plan.device_bytes + bigger.window_bytes() + bigger.world_image_bytes
        );
        assert_eq!(
            bigger.overlap_with(&plan),
            bigger.device_bytes + plan.window_bytes() + bigger.world_image_bytes
        );
    }
}
