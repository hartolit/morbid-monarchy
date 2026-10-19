//! The device context: adapter selection, the device and its limits, device health, buffer
//! allocation accounting and the single submission timeline.
//!
//! One context exists per session. Every buffer the session owns is created through
//! [`GpuContext::create_buffer`], so requested bytes are counted in one place, and every
//! submission goes through [`GpuContext::submit`], so the number of submitted, uncompleted
//! batches is bounded and their completion is observed without blocking.
//!
//! Adapters are chosen only by the stable identifier `inspect-adapter` prints. Nothing here
//! prefers or falls back to another adapter: an identifier that is missing, or that names a
//! software adapter when hardware was requested, is a typed error.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use futures_lite::future::block_on;

/// The most submitted, uncompleted batches a session may have; the budgets' initial target.
pub const MAX_IN_FLIGHT_LIMIT: u32 = 3;

/// The most submitted, uncompleted presentation frames (a renderer's work on the same
/// timeline). They are bounded separately so a frame never takes a tick's place and a busy
/// device paces the renderer instead of queueing frames without limit; two lets the CPU encode
/// the next frame while the previous one renders.
pub const PRESENTATION_IN_FLIGHT_LIMIT: u32 = 2;

/// An adapter as `inspect-adapter` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterRecord {
    /// `<backend>-<vendor>-<device>` in lowercase hex, with `-<n>` when several adapters share
    /// it. GPU commands name this identifier.
    pub id: String,
    pub name: String,
    pub backend: String,
    pub device_type: String,
    /// CPU or software rasterizers: they add correctness coverage but never qualify hardware.
    pub software: bool,
    pub vendor: u32,
    pub device: u32,
    pub pci_bus: String,
    pub driver: String,
    pub driver_info: String,
    pub features: Vec<String>,
    /// `Limits` field names and values as wgpu reports them.
    pub limits: Vec<(String, String)>,
}

/// Every adapter on every compiled backend, with stable identifiers.
pub struct AdapterInventory {
    instance: wgpu::Instance,
    adapters: Vec<(AdapterRecord, wgpu::Adapter)>,
}

impl AdapterInventory {
    /// Enumerates adapters. Shader and API validation keep wgpu's defaults; nothing here turns
    /// a check off.
    pub fn enumerate() -> Self {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapters = block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        let mut entries: Vec<(AdapterRecord, wgpu::Adapter)> = adapters
            .into_iter()
            .map(|adapter| (record(&adapter), adapter))
            .collect();
        disambiguate(&mut entries);
        Self {
            instance,
            adapters: entries,
        }
    }

    pub fn records(&self) -> impl Iterator<Item = &AdapterRecord> {
        self.adapters.iter().map(|(record, _)| record)
    }

    fn take(self, id: &str, allow_software: bool) -> Result<SelectedAdapter, ContextError> {
        let known: Vec<String> = self.records().map(|record| record.id.clone()).collect();
        let (record, adapter) = self
            .adapters
            .into_iter()
            .find(|(record, _)| record.id == id)
            .ok_or_else(|| ContextError::AdapterNotFound {
                id: id.to_owned(),
                available: known,
            })?;
        if record.software && !allow_software {
            return Err(ContextError::SoftwareAdapterRefused(record.id));
        }
        Ok(SelectedAdapter {
            instance: self.instance,
            record,
            adapter,
        })
    }
}

struct SelectedAdapter {
    instance: wgpu::Instance,
    record: AdapterRecord,
    adapter: wgpu::Adapter,
}

/// The identifiers of the hardware adapters that can present to `target`: the choices a
/// windowed client offers. Each is tested with a surface of its own instance, which is dropped
/// again; a session opened later creates its own.
pub fn presentable_adapters(target: wgpu::SurfaceTarget<'static>) -> Vec<AdapterRecord> {
    let inventory = AdapterInventory::enumerate();
    let Ok(surface) = inventory.instance.create_surface(target) else {
        return Vec::new();
    };
    inventory
        .adapters
        .iter()
        .filter(|(record, adapter)| !record.software && adapter.is_surface_supported(&surface))
        .map(|(record, _)| record.clone())
        .collect()
}

fn record(adapter: &wgpu::Adapter) -> AdapterRecord {
    let info = adapter.get_info();
    let backend = format!("{:?}", info.backend).to_lowercase();
    AdapterRecord {
        id: format!("{backend}-{:04x}-{:04x}", info.vendor, info.device),
        name: info.name.clone(),
        software: matches!(info.device_type, wgpu::DeviceType::Cpu),
        device_type: format!("{:?}", info.device_type),
        vendor: info.vendor,
        device: info.device,
        pci_bus: info.device_pci_bus_id.clone(),
        driver: info.driver.clone(),
        driver_info: info.driver_info.clone(),
        backend,
        features: adapter
            .features()
            .iter_names()
            .map(|(name, _)| name.to_owned())
            .collect(),
        limits: debug_fields(&format!("{:#?}", adapter.limits())),
    }
}

/// Appends `-<n>` to identifiers several adapters share, in enumeration order.
pub fn disambiguate_ids(ids: &mut [String]) {
    let original: Vec<String> = ids.to_vec();
    for (index, id) in ids.iter_mut().enumerate() {
        if original.iter().filter(|other| **other == *id).count() > 1 {
            let ordinal = original[..index]
                .iter()
                .filter(|other| **other == *id)
                .count();
            *id = format!("{id}-{ordinal}");
        }
    }
}

fn disambiguate(entries: &mut [(AdapterRecord, wgpu::Adapter)]) {
    let mut ids: Vec<String> = entries
        .iter()
        .map(|(record, _)| record.id.clone())
        .collect();
    disambiguate_ids(&mut ids);
    for ((record, _), id) in entries.iter_mut().zip(ids) {
        record.id = id;
    }
}

/// Splits pretty `Debug` output of a flat struct into `(field, value)` pairs.
pub fn debug_fields(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (name, value) = line.trim().split_once(": ")?;
            Some((name.to_owned(), value.trim_end_matches(',').to_owned()))
        })
        .collect()
}

/// Which limits a session may request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitCeiling {
    /// Up to what the adapter supports, but no more than the largest supported plan needs.
    Adapter,
    /// WebGPU's default limits only: a deliberately limited capability, used to prove that
    /// plans beyond it are refused rather than attempted.
    WebGpuDefaults,
}

/// What the device must provide for the largest plan the session may ever use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceNeeds {
    pub max_buffer_size: u64,
    pub max_storage_binding: u64,
    pub timestamps: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextError {
    AdapterNotFound {
        id: String,
        available: Vec<String>,
    },
    SoftwareAdapterRefused(String),
    /// The adapter cannot even provide WebGPU's default limits.
    DefaultLimitsUnsupported(String),
    /// The adapter refused a device with limits and features it reported.
    DeviceRequest(String),
    /// The window could not be given a surface on this instance.
    Surface(String),
    /// The adapter cannot present to the window's surface.
    SurfaceUnsupported(String),
}

/// A device-level failure noticed by a callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceFault {
    Lost {
        destroyed: bool,
        message: String,
    },
    /// An error no error scope captured. Every operation of a correct session is valid, so
    /// this is a defect or a symptom of loss; the session stops admitting work.
    Uncaptured {
        kind: ErrorKind,
        description: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    OutOfMemory,
    Validation,
    Internal,
}

/// Device health, written by wgpu callbacks and read by the owner on its own thread.
#[derive(Debug, Default)]
struct HealthState {
    lost: Option<DeviceFault>,
    /// The first few uncaptured errors; later ones are only counted.
    uncaptured: Vec<DeviceFault>,
    uncaptured_count: u64,
}

const KEPT_UNCAPTURED_ERRORS: usize = 8;

#[derive(Debug, Default)]
pub struct Health(Mutex<HealthState>);

impl Health {
    fn record_loss(&self, fault: DeviceFault) {
        if let Ok(mut state) = self.0.lock() {
            state.lost.get_or_insert(fault);
        }
    }

    fn record_uncaptured(&self, fault: DeviceFault) {
        if let Ok(mut state) = self.0.lock() {
            state.uncaptured_count += 1;
            if state.uncaptured.len() < KEPT_UNCAPTURED_ERRORS {
                state.uncaptured.push(fault);
            }
        }
    }

    pub fn lost(&self) -> Option<DeviceFault> {
        self.0.lock().ok().and_then(|state| state.lost.clone())
    }

    pub fn uncaptured(&self) -> (Vec<DeviceFault>, u64) {
        self.0
            .lock()
            .map(|state| (state.uncaptured.clone(), state.uncaptured_count))
            .unwrap_or_default()
    }

    pub fn uncaptured_count(&self) -> u64 {
        self.0
            .lock()
            .map(|state| state.uncaptured_count)
            .unwrap_or(0)
    }
}

/// A buffer allocation the device refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllocationError {
    OutOfMemory {
        label: &'static str,
        bytes: u64,
    },
    Invalid {
        label: &'static str,
        description: String,
    },
}

/// Requested device bytes, counted where buffers are created and released.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AllocationCounters {
    pub requested_bytes: u64,
    pub peak_requested_bytes: u64,
    pub buffers_created: u64,
}

/// A device wait's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    Completed,
    TimedOut,
    Lost,
}

/// What a submission carries: world work (ticks and transfers, bounded by the session's
/// in-flight limit) or a presentation frame (bounded by [`PRESENTATION_IN_FLIGHT_LIMIT`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubmissionKind {
    World,
    Presentation,
}

struct InFlight {
    serial: u64,
    index: wgpu::SubmissionIndex,
    submitted_at: Instant,
    kind: SubmissionKind,
}

/// The session's one submission timeline.
///
/// Submissions on one queue complete in order, so their completion callbacks only ever raise
/// the highest completed serial; one atomic holds it. The callback publishes nothing else: data
/// produced by a submission is read only after its own buffer-map callback.
struct Timeline {
    next_serial: u64,
    in_flight: Vec<InFlight>,
    capacity: usize,
    completed: Arc<AtomicU64>,
    /// The highest completed serial the owner has taken into account. It lags `completed`
    /// only while a poll is told not to observe completions (fault injection).
    observed_completed: u64,
}

/// The device of one session and everything that must be shared by all its GPU work.
pub struct GpuContext {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    record: AdapterRecord,
    adapter_limits: wgpu::Limits,
    device: wgpu::Device,
    queue: wgpu::Queue,
    limits: wgpu::Limits,
    features: wgpu::Features,
    health: Arc<Health>,
    timeline: Timeline,
    allocations: AllocationCounters,
    timestamp_period_ns: Option<f32>,
}

impl GpuContext {
    /// Opens the adapter named `adapter_id` and requests a device with the limits `needs`
    /// describes, raised from WebGPU's defaults no further than the adapter and `ceiling`
    /// allow. A plan that later needs more than the granted limits is refused by the plan,
    /// not by the device.
    ///
    /// With a `surface` target, the window's surface is created on the same instance and the
    /// adapter must be able to present to it: a renderer then draws the resident world on this
    /// device and timeline, never on a second one.
    pub fn open(
        adapter_id: &str,
        allow_software: bool,
        ceiling: LimitCeiling,
        needs: DeviceNeeds,
        max_in_flight: u32,
        surface: Option<wgpu::SurfaceTarget<'static>>,
    ) -> Result<(Self, Option<wgpu::Surface<'static>>), ContextError> {
        let selected = AdapterInventory::enumerate().take(adapter_id, allow_software)?;
        let surface = match surface {
            None => None,
            Some(target) => {
                let surface = selected
                    .instance
                    .create_surface(target)
                    .map_err(|error| ContextError::Surface(error.to_string()))?;
                if !selected.adapter.is_surface_supported(&surface) {
                    return Err(ContextError::SurfaceUnsupported(selected.record.id));
                }
                Some(surface)
            }
        };
        let adapter_limits = selected.adapter.limits();
        let defaults = wgpu::Limits::default();
        if !defaults.check_limits(&adapter_limits) {
            return Err(ContextError::DefaultLimitsUnsupported(selected.record.id));
        }
        let mut limits = defaults.clone();
        if ceiling == LimitCeiling::Adapter {
            limits.max_buffer_size = needs
                .max_buffer_size
                .max(defaults.max_buffer_size)
                .min(adapter_limits.max_buffer_size);
            limits.max_storage_buffer_binding_size = needs
                .max_storage_binding
                .max(defaults.max_storage_buffer_binding_size)
                .min(adapter_limits.max_storage_buffer_binding_size);
        }
        let mut features = wgpu::Features::empty();
        if needs.timestamps
            && selected
                .adapter
                .features()
                .contains(wgpu::Features::TIMESTAMP_QUERY)
        {
            features |= wgpu::Features::TIMESTAMP_QUERY;
        }
        let descriptor = wgpu::DeviceDescriptor {
            label: Some("monarchy-world"),
            required_features: features,
            required_limits: limits.clone(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        };
        let (device, queue) = block_on(selected.adapter.request_device(&descriptor))
            .map_err(|error| ContextError::DeviceRequest(error.to_string()))?;

        let health = Arc::new(Health::default());
        let lost = Arc::clone(&health);
        device.set_device_lost_callback(move |reason, message| {
            lost.record_loss(DeviceFault::Lost {
                destroyed: reason == wgpu::DeviceLostReason::Destroyed,
                message,
            });
        });
        let uncaptured = Arc::clone(&health);
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            let (kind, description) = match error {
                wgpu::Error::OutOfMemory { source } => (ErrorKind::OutOfMemory, source.to_string()),
                wgpu::Error::Validation { description, .. } => (ErrorKind::Validation, description),
                wgpu::Error::Internal { description, .. } => (ErrorKind::Internal, description),
            };
            uncaptured.record_uncaptured(DeviceFault::Uncaptured { kind, description });
        }));

        let timestamp_period_ns = features
            .contains(wgpu::Features::TIMESTAMP_QUERY)
            .then(|| queue.get_timestamp_period())
            .filter(|period| *period > 0.0);
        let granted = device.limits();
        let capacity = max_in_flight.clamp(1, MAX_IN_FLIGHT_LIMIT) as usize;
        let context = Self {
            instance: selected.instance,
            adapter: selected.adapter,
            record: selected.record,
            adapter_limits,
            device,
            queue,
            limits: granted,
            features,
            health,
            timeline: Timeline {
                next_serial: 1,
                in_flight: Vec::with_capacity(capacity + PRESENTATION_IN_FLIGHT_LIMIT as usize),
                capacity,
                completed: Arc::new(AtomicU64::new(0)),
                observed_completed: 0,
            },
            allocations: AllocationCounters::default(),
            timestamp_period_ns,
        };
        Ok((context, surface))
    }

    pub fn adapter(&self) -> &AdapterRecord {
        &self.record
    }

    pub fn adapter_limits(&self) -> &wgpu::Limits {
        &self.adapter_limits
    }

    /// What `surface` (created by [`open`](Self::open) on this instance) supports on this
    /// adapter: formats, present modes and alpha modes.
    pub fn surface_capabilities(&self, surface: &wgpu::Surface<'_>) -> wgpu::SurfaceCapabilities {
        surface.get_capabilities(&self.adapter)
    }

    pub fn supports_surface(&self, surface: &wgpu::Surface<'_>) -> bool {
        self.adapter.is_surface_supported(surface)
    }

    /// The instance the device and any surface belong to.
    pub fn instance(&self) -> &wgpu::Instance {
        &self.instance
    }

    /// The queue, for consumers that write small per-frame data (a renderer's uniforms). Its
    /// writes take effect before the next submission on the timeline, whichever it is.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// The device, for creating pipelines and resources that share it. Buffers that belong to
    /// the session are created through [`create_buffer`](Self::create_buffer) instead.
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The limits the device was granted.
    pub fn limits(&self) -> &wgpu::Limits {
        &self.limits
    }

    pub fn features(&self) -> wgpu::Features {
        self.features
    }

    /// Nanoseconds per timestamp tick, when timestamp queries are enabled and meaningful.
    pub fn timestamp_period_ns(&self) -> Option<f32> {
        self.timestamp_period_ns
    }

    pub fn health(&self) -> &Health {
        &self.health
    }

    pub fn allocations(&self) -> AllocationCounters {
        self.allocations
    }

    /// Creates a buffer inside error scopes, so a refused allocation is a typed error rather
    /// than an uncaptured one, and counts its bytes.
    pub fn create_buffer(
        &mut self,
        label: &'static str,
        size: u64,
        usage: wgpu::BufferUsages,
        mapped_at_creation: bool,
    ) -> Result<wgpu::Buffer, AllocationError> {
        let out_of_memory = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage,
            mapped_at_creation,
        });
        // On native, popping a scope resolves at once; this is not a device wait.
        let invalid = block_on(validation.pop());
        let exhausted = block_on(out_of_memory.pop());
        if let Some(error) = invalid {
            return Err(AllocationError::Invalid {
                label,
                description: error.to_string(),
            });
        }
        if exhausted.is_some() {
            return Err(AllocationError::OutOfMemory { label, bytes: size });
        }
        self.allocations.buffers_created += 1;
        self.allocations.requested_bytes += size;
        self.allocations.peak_requested_bytes = self
            .allocations
            .peak_requested_bytes
            .max(self.allocations.requested_bytes);
        Ok(buffer)
    }

    /// Releases a buffer's bytes from the count. The caller drops the buffer after every
    /// submission that used it has completed.
    pub fn release_buffer(&mut self, buffer: wgpu::Buffer) {
        self.allocations.requested_bytes = self
            .allocations
            .requested_bytes
            .saturating_sub(buffer.size());
        drop(buffer);
    }

    /// Every submitted, uncompleted batch, world work and presentation alike.
    pub fn in_flight(&self) -> u32 {
        self.timeline.in_flight.len() as u32
    }

    fn in_flight_of(&self, kind: SubmissionKind) -> usize {
        self.timeline
            .in_flight
            .iter()
            .filter(|entry| entry.kind == kind)
            .count()
    }

    /// Submitted, uncompleted world batches (ticks and transfers): what the in-flight limit
    /// bounds.
    pub fn world_in_flight(&self) -> u32 {
        self.in_flight_of(SubmissionKind::World) as u32
    }

    /// Submitted, uncompleted presentation frames.
    pub fn presentations_in_flight(&self) -> u32 {
        self.in_flight_of(SubmissionKind::Presentation) as u32
    }

    pub fn max_in_flight(&self) -> u32 {
        self.timeline.capacity as u32
    }

    pub fn has_submission_capacity(&self) -> bool {
        self.in_flight_of(SubmissionKind::World) < self.timeline.capacity
    }

    pub fn has_presentation_capacity(&self) -> bool {
        self.in_flight_of(SubmissionKind::Presentation) < PRESENTATION_IN_FLIGHT_LIMIT as usize
    }

    /// The highest serial whose submission is known to have completed.
    pub fn completed_serial(&self) -> u64 {
        self.timeline.observed_completed
    }

    /// The serial the next submission will receive.
    pub fn next_serial(&self) -> u64 {
        self.timeline.next_serial
    }

    /// Submits command buffers on the session's timeline. The caller has checked
    /// [`has_submission_capacity`](Self::has_submission_capacity) before encoding them.
    pub fn submit(&mut self, buffers: impl IntoIterator<Item = wgpu::CommandBuffer>) -> u64 {
        self.submit_kind(buffers, SubmissionKind::World)
    }

    /// Submits a presentation frame on the same timeline, after every earlier submission. The
    /// caller has checked [`has_presentation_capacity`](Self::has_presentation_capacity).
    pub fn submit_presentation(
        &mut self,
        buffers: impl IntoIterator<Item = wgpu::CommandBuffer>,
    ) -> u64 {
        self.submit_kind(buffers, SubmissionKind::Presentation)
    }

    fn submit_kind(
        &mut self,
        buffers: impl IntoIterator<Item = wgpu::CommandBuffer>,
        kind: SubmissionKind,
    ) -> u64 {
        let serial = self.timeline.next_serial;
        self.timeline.next_serial += 1;
        let index = self.queue.submit(buffers);
        let completed = Arc::clone(&self.timeline.completed);
        // Registered right after `submit`, so it fires when this submission completes.
        self.queue.on_submitted_work_done(move || {
            completed.fetch_max(serial, Ordering::Release);
        });
        self.timeline.in_flight.push(InFlight {
            serial,
            index,
            submitted_at: Instant::now(),
            kind,
        });
        serial
    }

    /// Lets the device run its callbacks without waiting. Returns `false` when wgpu reported
    /// the device unusable.
    ///
    /// wgpu 29.0.4 panics from `Device::poll` when the backend reports a lost device
    /// (`handle_error_fatal`), which would take the whole application down with the device.
    /// The poll is the one place that can observe such a loss, so it is isolated here and
    /// turned into the session's lost state.
    pub fn poll(&mut self, observe_completions: bool) -> bool {
        let device = &self.device;
        let polled = catch_unwind(AssertUnwindSafe(|| device.poll(wgpu::PollType::Poll)));
        let usable = matches!(polled, Ok(Ok(_)));
        if !usable {
            self.health.record_loss(DeviceFault::Lost {
                destroyed: false,
                message: "device poll failed".to_owned(),
            });
        }
        if observe_completions {
            self.observe_completions();
        }
        usable
    }

    /// Takes completions the callbacks published into account.
    pub fn observe_completions(&mut self) {
        let completed = self.timeline.completed.load(Ordering::Acquire);
        self.timeline.observed_completed = self.timeline.observed_completed.max(completed);
        let done = self.timeline.observed_completed;
        self.timeline.in_flight.retain(|entry| entry.serial > done);
    }

    /// When the oldest uncompleted submission was submitted.
    pub fn oldest_in_flight(&self) -> Option<Instant> {
        self.timeline
            .in_flight
            .first()
            .map(|entry| entry.submitted_at)
    }

    /// Blocks until `serial` completes or `timeout` passes. Only explicit maintenance (stop,
    /// drain) calls this; the frame path never waits.
    pub fn wait(&mut self, serial: u64, timeout: Duration) -> WaitOutcome {
        let Some(index) = self
            .timeline
            .in_flight
            .iter()
            .find(|entry| entry.serial >= serial)
            .map(|entry| entry.index.clone())
        else {
            // Already complete: still let the device run callbacks registered since, such as
            // a map requested after the submission finished.
            return if self.poll(true) {
                WaitOutcome::Completed
            } else {
                WaitOutcome::Lost
            };
        };
        let device = &self.device;
        let waited = catch_unwind(AssertUnwindSafe(|| {
            device.poll(wgpu::PollType::Wait {
                submission_index: Some(index),
                timeout: Some(timeout),
            })
        }));
        self.observe_completions();
        match waited {
            Ok(Ok(_)) => WaitOutcome::Completed,
            Ok(Err(wgpu::PollError::Timeout)) => WaitOutcome::TimedOut,
            Ok(Err(_)) | Err(_) => {
                self.health.record_loss(DeviceFault::Lost {
                    destroyed: false,
                    message: "device wait failed".to_owned(),
                });
                WaitOutcome::Lost
            }
        }
    }

    /// Destroys the device: controlled loss injection. The loss callback reports it once the
    /// queue is empty, like a real loss.
    pub(crate) fn destroy_device(&self) {
        self.device.destroy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_identifiers_get_ordinals() {
        let mut ids = vec![
            "vulkan-10de-2c05".to_owned(),
            "gl-10de-2c05".to_owned(),
            "vulkan-10de-2c05".to_owned(),
        ];
        disambiguate_ids(&mut ids);
        assert_eq!(
            ids,
            ["vulkan-10de-2c05-0", "gl-10de-2c05", "vulkan-10de-2c05-1"]
        );
    }

    #[test]
    fn limits_debug_output_becomes_fields() {
        let fields = debug_fields(
            "Limits {\n    max_texture_dimension_1d: 8192,\n    max_bind_groups: 4,\n}",
        );
        assert_eq!(
            fields[0],
            ("max_texture_dimension_1d".to_owned(), "8192".to_owned())
        );
        assert_eq!(fields.len(), 2);
    }
}
