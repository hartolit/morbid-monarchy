//! The runtime: fixed-tick admission over a live world, and the consumer of its outcomes.
//!
//! The runtime is a `bevy_ecs` world with one schedule that runs once per presented frame:
//!
//! ```text
//! advance_clock -> submit_due_ticks -> poll_world -> consume_outcomes
//! ```
//!
//! Commands enter through [`Runtime::submit`] (or [`Runtime::submit_at`], which names the
//! earliest tick a command may apply in), which reserves a receipt and queues them; due ticks
//! take queued commands in admission order. The live world sits behind the
//! [`PhysicalWorld`] port as a resource, so later stages add their systems (input, possession,
//! displays) to the same schedule, and the application composes the same runtime with a window.
//! The schedule runs single-threaded: its systems form one chain over the port, and nothing in
//! it gains from a thread pool.

use std::{collections::VecDeque, time::Duration};

use bevy_ecs::{
    prelude::*,
    schedule::{ScheduleLabel, SingleThreadedExecutor},
};
use world_model::{body::BodyHandle, cell::WorldCell, command::Command, spatial::ChunkKey};

use crate::{
    bodies::BodyMonitor,
    clock::{ClockError, FixedClock},
    ids::{ChunkRevision, CommandId, SessionId, Tick},
    ledger::{OutcomeLedger, ResidencyOutcome},
    port::{
        AbandonReason, AdmittedCommand, CaptureTicket, CapturedChunk, DrainReport, Obligation,
        Outcome, PhysicalWorld, PortCapabilities, PortHealth, Pressure, Refusal, ResidentWindow,
        ResizeReport, ResizeRequest, SlotRef, TickBatch, TransferCounters, UploadTicket,
    },
    receipts::{AcknowledgeError, Admission, AdmissionError, Receipt, Receipts},
};

/// Sizes of the runtime's own bounded queues and its clock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RuntimeConfig {
    pub tick_rate_hz: f64,
    pub max_ticks_per_frame: u32,
    /// Admitted commands waiting for a tick.
    pub pending_commands: u32,
    /// Receipts kept until acknowledged; covers pending, in-flight and unacknowledged ones.
    pub receipts: u32,
    /// Cosmetic events kept for displays.
    pub cosmetic_events: u32,
    /// Body events (landings, liquid entries, contacts, craters, splashes) kept until drained.
    pub body_events: u32,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            tick_rate_hz: world_model::config::DEFAULT_TICK_RATE_HZ,
            max_ticks_per_frame: world_model::config::DEFAULT_MAX_TICKS_PER_FRAME,
            pending_commands: 1024,
            receipts: 4096,
            cosmetic_events: 256,
            body_events: 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RuntimeError {
    Clock(ClockError),
    /// Receipts must cover every pending command.
    ReceiptsBelowPending {
        receipts: u32,
        pending: u32,
    },
}

/// What one frame did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameReport {
    pub ticks_due: u32,
    pub ticks_submitted: u32,
    /// The bounded resource that stopped further submission this frame, if any.
    pub pressure: Option<Pressure>,
    pub outcomes: u32,
    pub health: Option<PortHealth>,
}

/// A residency request refused by the runtime or the live world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidencyError {
    /// Collect residency outcomes before requesting more.
    OutcomesFull,
    Refused(Refusal),
}

#[derive(Resource)]
struct Port(Box<dyn PhysicalWorld>);

/// A queued command and the earliest tick it may apply in.
#[derive(Debug, Clone, Copy)]
struct Pending {
    command: AdmittedCommand,
    not_before: Tick,
}

#[derive(Resource)]
struct CommandQueue {
    receipts: Receipts,
    pending: VecDeque<Pending>,
    pending_capacity: usize,
    capabilities: PortCapabilities,
    closed: bool,
}

#[derive(Resource)]
struct Clock {
    clock: FixedClock,
    /// Ticks forced by [`Runtime::frame_ticks`] instead of elapsed time.
    forced: Option<u32>,
    elapsed: Duration,
    next_tick: Tick,
    last_submitted: Option<Tick>,
    exhausted: bool,
}

#[derive(Resource)]
struct Ledger(OutcomeLedger);

#[derive(Resource, Default)]
struct Buffers {
    batch: Vec<AdmittedCommand>,
    observe: Vec<BodyHandle>,
    outcomes: Vec<Outcome>,
}

#[derive(Resource, Default)]
struct LastFrame(FrameReport);

#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
struct FrameSchedule;

/// The CPU runtime of one live session.
pub struct Runtime {
    world: World,
    schedule: Schedule,
}

impl Runtime {
    pub fn new(port: Box<dyn PhysicalWorld>, config: RuntimeConfig) -> Result<Self, RuntimeError> {
        if config.receipts < config.pending_commands {
            return Err(RuntimeError::ReceiptsBelowPending {
                receipts: config.receipts,
                pending: config.pending_commands,
            });
        }
        let clock = FixedClock::new(config.tick_rate_hz, config.max_ticks_per_frame)
            .map_err(RuntimeError::Clock)?;
        let capabilities = port.capabilities();
        let next_tick = port.next_tick();
        let residency = (capabilities.chunk_upload_slots + capabilities.capture_slots) as usize;

        let mut world = World::new();
        world.insert_resource(Port(port));
        world.insert_resource(CommandQueue {
            receipts: Receipts::new(config.receipts),
            pending: VecDeque::with_capacity(config.pending_commands as usize),
            pending_capacity: config.pending_commands as usize,
            capabilities,
            closed: false,
        });
        world.insert_resource(Clock {
            clock,
            forced: None,
            elapsed: Duration::ZERO,
            next_tick,
            last_submitted: None,
            exhausted: false,
        });
        world.insert_resource(Ledger(OutcomeLedger::new(
            config.cosmetic_events as usize,
            residency,
            BodyMonitor::new(capabilities.body_capacity, config.body_events),
        )));
        world.insert_resource(Buffers {
            batch: Vec::with_capacity(capabilities.commands_per_tick as usize),
            observe: Vec::with_capacity(capabilities.body_samples_per_tick as usize),
            outcomes: Vec::with_capacity(capabilities.outcomes_per_poll as usize),
        });
        world.insert_resource(LastFrame::default());

        let mut schedule = Schedule::new(FrameSchedule);
        schedule.set_executor(SingleThreadedExecutor::new());
        schedule.add_systems(
            (
                advance_clock,
                submit_due_ticks,
                poll_world,
                consume_outcomes,
            )
                .chain(),
        );
        Ok(Self { world, schedule })
    }

    pub fn session(&self) -> SessionId {
        self.world.resource::<Port>().0.session()
    }

    pub fn capabilities(&self) -> PortCapabilities {
        self.world.resource::<CommandQueue>().capabilities
    }

    /// Admits a command under `id`, reserving its receipt, or refuses it without effect.
    pub fn submit(&mut self, id: CommandId, command: Command) -> Result<Admission, AdmissionError> {
        self.submit_at(id, command, Tick::ZERO)
    }

    /// Admits a command that applies in tick `not_before` at the earliest, so a scripted input
    /// lands in the same logical tick however frames pace the clock. Commands still apply in
    /// admission order: one that is not yet due holds back those admitted after it.
    pub fn submit_at(
        &mut self,
        id: CommandId,
        command: Command,
        not_before: Tick,
    ) -> Result<Admission, AdmissionError> {
        let last_submitted = self.world.resource::<Clock>().last_submitted;
        let mut admission = self.world.resource_mut::<CommandQueue>();
        if admission.closed {
            return Err(AdmissionError::Closed);
        }
        let supported = admission.capabilities.supported;
        if let Some(state) = admission.receipts.check(id, &command, supported)? {
            return Ok(Admission::Duplicate(state));
        }
        if admission.pending.len() >= admission.pending_capacity {
            return Err(AdmissionError::Pressure(Pressure::PendingCommands));
        }
        admission.receipts.insert(id, command, last_submitted)?;
        admission.pending.push_back(Pending {
            command: AdmittedCommand { id, command },
            not_before,
        });
        Ok(Admission::Admitted)
    }

    /// Runs one presented frame: the ticks `elapsed` makes due, then a poll.
    pub fn frame(&mut self, elapsed: Duration) -> FrameReport {
        self.world.resource_mut::<Clock>().elapsed = elapsed;
        self.run()
    }

    /// Runs one frame owing exactly `ticks` ticks (capped like any frame), for headless runs
    /// scheduled by tick rather than by wall time.
    pub fn frame_ticks(&mut self, ticks: u32) -> FrameReport {
        self.world.resource_mut::<Clock>().forced = Some(ticks);
        self.run()
    }

    /// Releases final receipts through `through` and raises the identity floor past it.
    pub fn acknowledge(&mut self, through: CommandId) -> Result<usize, AcknowledgeError> {
        self.world
            .resource_mut::<CommandQueue>()
            .receipts
            .acknowledge(through)
    }

    pub fn receipt(&self, id: CommandId) -> Option<Receipt> {
        self.world
            .resource::<CommandQueue>()
            .receipts
            .get(id)
            .copied()
    }

    pub fn pending_commands(&self) -> usize {
        self.world.resource::<CommandQueue>().pending.len()
    }

    pub fn unknown_outcomes(&self) -> u64 {
        self.world
            .resource::<CommandQueue>()
            .receipts
            .unknown_outcomes()
    }

    pub fn ledger(&self) -> &OutcomeLedger {
        &self.world.resource::<Ledger>().0
    }

    pub fn ledger_mut(&mut self) -> &mut OutcomeLedger {
        &mut self.world.resource_mut::<Ledger>().into_inner().0
    }

    /// What the runtime knows about the live world's bodies.
    pub fn bodies(&self) -> &BodyMonitor {
        self.ledger().bodies()
    }

    pub fn bodies_mut(&mut self) -> &mut BodyMonitor {
        self.ledger_mut().bodies_mut()
    }

    /// Samples `body` every tick (the possessed body a camera follows), or stops doing so.
    pub fn set_controlled(&mut self, body: Option<BodyHandle>) {
        self.bodies_mut().set_controlled(body);
    }

    pub fn last_submitted_tick(&self) -> Option<Tick> {
        self.world.resource::<Clock>().last_submitted
    }

    pub fn discarded_ticks(&self) -> u64 {
        self.world.resource::<Clock>().clock.discarded_ticks()
    }

    /// Changes the tick rate and the catch-up cap from the next frame on, keeping the time
    /// already accumulated; refused, with the clock unchanged, outside the supported ranges.
    pub fn set_clock(
        &mut self,
        tick_rate_hz: f64,
        max_ticks_per_frame: u32,
    ) -> Result<(), ClockError> {
        self.world
            .resource_mut::<Clock>()
            .clock
            .reconfigure(tick_rate_hz, max_ticks_per_frame)
    }

    /// The fixed tick's duration.
    pub fn tick_duration(&self) -> Duration {
        self.world.resource::<Clock>().clock.tick_duration()
    }

    pub fn counters(&self) -> TransferCounters {
        self.world.resource::<Port>().0.counters()
    }

    pub fn last_frame(&self) -> FrameReport {
        self.world.resource::<LastFrame>().0
    }

    pub fn reserve_slot(&mut self, key: ChunkKey) -> Result<SlotRef, Refusal> {
        let session = self.session();
        self.port_mut().reserve_slot(session, key)
    }

    /// Stages a chunk upload; its outcome arrives through [`drain_residency`](Self::drain_residency).
    pub fn upload_chunk(
        &mut self,
        slot: SlotRef,
        revision: ChunkRevision,
        cells: &[WorldCell],
    ) -> Result<UploadTicket, ResidencyError> {
        self.with_residency_reservation(|port| port.upload_chunk(slot, revision, cells))
    }

    /// Requests a capture; its readiness arrives through [`drain_residency`](Self::drain_residency).
    pub fn capture_chunk(&mut self, slot: SlotRef) -> Result<CaptureTicket, ResidencyError> {
        self.with_residency_reservation(|port| port.capture_chunk(slot))
    }

    pub fn take_capture(
        &mut self,
        ticket: CaptureTicket,
        into: &mut [WorldCell],
    ) -> Result<CapturedChunk, Refusal> {
        self.port_mut().take_capture(ticket, into)
    }

    pub fn retire_slot(&mut self, slot: SlotRef) -> Result<(), Refusal> {
        self.port_mut().retire_slot(slot)
    }

    /// Moves residency outcomes into `into`, freeing room for further requests.
    pub fn drain_residency(&mut self, into: &mut Vec<ResidencyOutcome>) {
        into.extend(self.ledger_mut().drain_residency());
    }

    pub fn resize(&mut self, window: ResidentWindow) -> Result<ResizeReport, Refusal> {
        let session = self.session();
        let report = self.port_mut().resize(&ResizeRequest { session, window })?;
        self.world
            .resource_mut::<CommandQueue>()
            .capabilities
            .window = window;
        Ok(report)
    }

    /// Closes admission and waits up to `timeout` for outstanding work, consuming every
    /// outcome. Commands still queued for a tick are abandoned: a stopped session runs no more
    /// ticks.
    pub fn stop(&mut self, timeout: Duration) -> DrainReport {
        let mut outcomes = std::mem::take(&mut self.world.resource_mut::<Buffers>().outcomes);
        let report = self.port_mut().stop(timeout, &mut outcomes);
        let observed_at = self.last_submitted_tick();
        self.world
            .resource_scope(|world, mut admission: Mut<CommandQueue>| {
                let mut ledger = world.resource_mut::<Ledger>();
                admission.closed = true;
                for outcome in outcomes.drain(..) {
                    ledger
                        .0
                        .consume(outcome, &mut admission.receipts, observed_at);
                }
                abandon_pending(&mut admission, &mut ledger.0, AbandonReason::Stopped);
            });
        self.world.resource_mut::<Buffers>().outcomes = outcomes;
        report
    }

    /// The live world, for queries of its capabilities and counters.
    pub fn port(&self) -> &dyn PhysicalWorld {
        self.world.resource::<Port>().0.as_ref()
    }

    fn port_mut(&mut self) -> &mut dyn PhysicalWorld {
        self.world.resource_mut::<Port>().into_inner().0.as_mut()
    }

    fn with_residency_reservation<T>(
        &mut self,
        request: impl FnOnce(&mut dyn PhysicalWorld) -> Result<T, Refusal>,
    ) -> Result<T, ResidencyError> {
        if !self.ledger_mut().reserve_residency() {
            return Err(ResidencyError::OutcomesFull);
        }
        match request(self.port_mut()) {
            Ok(ticket) => Ok(ticket),
            Err(refusal) => {
                self.ledger_mut().release_residency();
                Err(ResidencyError::Refused(refusal))
            }
        }
    }

    fn run(&mut self) -> FrameReport {
        self.world.resource_mut::<LastFrame>().0 = FrameReport::default();
        self.schedule.run(&mut self.world);
        self.last_frame()
    }
}

fn advance_clock(mut clock: ResMut<Clock>, mut frame: ResMut<LastFrame>) {
    let clock = clock.as_mut();
    frame.0.ticks_due = match clock.forced.take() {
        Some(ticks) => ticks.min(clock.clock.max_ticks_per_frame()),
        None => clock.clock.advance(clock.elapsed),
    };
    clock.elapsed = Duration::ZERO;
}

fn submit_due_ticks(
    mut clock: ResMut<Clock>,
    mut admission: ResMut<CommandQueue>,
    mut port: ResMut<Port>,
    mut buffers: ResMut<Buffers>,
    mut ledger: ResMut<Ledger>,
    mut frame: ResMut<LastFrame>,
) {
    if admission.closed {
        return;
    }
    let session = port.0.session();
    let per_tick = admission.capabilities.commands_per_tick as usize;
    let samples = admission.capabilities.body_samples_per_tick as usize;
    let tick_rate_hz = clock.clock.tick_rate_hz();
    for _ in 0..frame.0.ticks_due {
        if clock.exhausted {
            break;
        }
        let tick = clock.next_tick;
        buffers.batch.clear();
        let take = admission
            .pending
            .iter()
            .take(per_tick)
            .take_while(|pending| pending.not_before <= tick)
            .count();
        buffers.batch.extend(
            admission
                .pending
                .iter()
                .take(take)
                .map(|pending| pending.command),
        );
        let buffers = &mut *buffers;
        ledger.0.bodies_mut().select(samples, &mut buffers.observe);
        let batch = TickBatch {
            session,
            tick,
            tick_rate_hz,
            commands: &buffers.batch,
            observe: &buffers.observe,
        };
        match port.0.submit_tick(&batch) {
            Ok(_) => {
                admission.pending.drain(..take);
                for command in &buffers.batch {
                    admission.receipts.mark_submitted(command.id, tick);
                }
                clock.clock.consume();
                clock.last_submitted = Some(tick);
                match tick.next() {
                    Some(next) => clock.next_tick = next,
                    None => clock.exhausted = true,
                }
                frame.0.ticks_submitted += 1;
            }
            Err(Refusal::Pressure(pressure)) => {
                frame.0.pressure = Some(pressure);
                break;
            }
            Err(Refusal::TickExhausted) => {
                clock.exhausted = true;
                break;
            }
            Err(refusal) => {
                // Lost, stopped or faulted, or a refusal that means the runtime and the live
                // world disagree. Either way no further tick can run in this session.
                let reason = match refusal {
                    Refusal::DeviceLost => AbandonReason::DeviceLost,
                    Refusal::Stopped => AbandonReason::Stopped,
                    _ => AbandonReason::Faulted,
                };
                admission.closed = true;
                abandon_pending(&mut admission, &mut ledger.0, reason);
                break;
            }
        }
    }
}

fn poll_world(mut port: ResMut<Port>, mut buffers: ResMut<Buffers>, mut frame: ResMut<LastFrame>) {
    let report = port.0.poll(&mut buffers.outcomes);
    frame.0.health = Some(report.health);
    frame.0.outcomes = buffers.outcomes.len() as u32;
}

fn consume_outcomes(
    clock: Res<Clock>,
    mut admission: ResMut<CommandQueue>,
    mut ledger: ResMut<Ledger>,
    mut buffers: ResMut<Buffers>,
    frame: Res<LastFrame>,
) {
    let observed_at = clock.last_submitted;
    for outcome in buffers.outcomes.drain(..) {
        ledger
            .0
            .consume(outcome, &mut admission.receipts, observed_at);
    }
    let ended = match frame.0.health {
        Some(PortHealth::Lost) => Some(AbandonReason::DeviceLost),
        Some(PortHealth::Faulted) => Some(AbandonReason::Faulted),
        Some(PortHealth::Stopped) => Some(AbandonReason::Stopped),
        _ => None,
    };
    if let Some(reason) = ended {
        admission.closed = true;
        abandon_pending(&mut admission, &mut ledger.0, reason);
    }
}

/// Abandons commands that were admitted but never submitted.
fn abandon_pending(
    admission: &mut CommandQueue,
    ledger: &mut OutcomeLedger,
    reason: AbandonReason,
) {
    while let Some(Pending { command, .. }) = admission.pending.pop_front() {
        ledger.consume(
            Outcome::Abandoned {
                obligation: Obligation::Command(command.id),
                reason,
            },
            &mut admission.receipts,
            None,
        );
    }
}

/// The runtime's own rules, against a scripted port. The port answers like a live world would
/// (outcomes one poll after submission) so the runtime's ordering, backpressure and session
/// handling can be checked without a device; the GPU implementation has its own suite.
#[cfg(test)]
mod tests {
    use world_model::{
        command::{EditTool, MaterialEdit},
        glam::IVec2,
    };

    use super::*;
    use crate::{
        ids::{SessionEpoch, WorldId},
        port::{
            CapturedChunk, CommandOutcome, CommandResult, PayloadError, SubmissionTicket,
            SupportedCommands, TickReport,
        },
        receipts::ReceiptState,
    };

    #[derive(Default)]
    struct Script {
        batches: Vec<(Tick, Vec<CommandId>)>,
        unanswered: Vec<(Tick, Vec<CommandId>)>,
        refuse_next: Option<Refusal>,
        lost: bool,
        next_tick: Tick,
    }

    struct ScriptedPort {
        script: std::sync::Arc<std::sync::Mutex<Script>>,
        per_tick: u32,
    }

    fn session() -> SessionId {
        SessionId {
            world: WorldId::new(9),
            epoch: SessionEpoch::FIRST,
        }
    }

    impl PhysicalWorld for ScriptedPort {
        fn session(&self) -> SessionId {
            session()
        }

        fn capabilities(&self) -> PortCapabilities {
            PortCapabilities {
                commands_per_tick: self.per_tick,
                max_in_flight: 3,
                outcomes_per_poll: 64,
                chunk_upload_slots: 1,
                capture_slots: 1,
                window: ResidentWindow::new(ChunkKey::new(0, 0, 0), 1, 1).expect("window"),
                supported: SupportedCommands::default()
                    .with(world_model::command::CommandKind::EditMaterial),
                body_capacity: 4,
                body_samples_per_tick: 2,
            }
        }

        fn next_tick(&self) -> Tick {
            self.script.lock().expect("script").next_tick
        }

        fn submit_tick(&mut self, batch: &TickBatch<'_>) -> Result<SubmissionTicket, Refusal> {
            let mut script = self.script.lock().expect("script");
            if script.lost {
                return Err(Refusal::DeviceLost);
            }
            if let Some(refusal) = script.refuse_next.take() {
                return Err(refusal);
            }
            if batch.tick != script.next_tick {
                return Err(Refusal::TickOutOfOrder {
                    expected: script.next_tick,
                    presented: batch.tick,
                });
            }
            let ids: Vec<CommandId> = batch.commands.iter().map(|command| command.id).collect();
            script.batches.push((batch.tick, ids.clone()));
            script.unanswered.push((batch.tick, ids));
            script.next_tick = Tick::new(batch.tick.value() + 1);
            Ok(SubmissionTicket {
                session: batch.session,
                tick: batch.tick,
                serial: batch.tick.value(),
            })
        }

        fn reserve_slot(&mut self, _: SessionId, _: ChunkKey) -> Result<SlotRef, Refusal> {
            Err(Refusal::Pressure(Pressure::Slots))
        }

        fn upload_chunk(
            &mut self,
            slot: SlotRef,
            _: ChunkRevision,
            _: &[WorldCell],
        ) -> Result<UploadTicket, Refusal> {
            Err(Refusal::StaleSlot(slot))
        }

        fn retire_slot(&mut self, slot: SlotRef) -> Result<(), Refusal> {
            Err(Refusal::StaleSlot(slot))
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

        fn resize(&mut self, _: &ResizeRequest) -> Result<ResizeReport, Refusal> {
            Err(Refusal::Stopped)
        }

        fn poll(&mut self, outcomes: &mut Vec<Outcome>) -> crate::port::PollReport {
            let mut script = self.script.lock().expect("script");
            if script.lost {
                return crate::port::PollReport {
                    health: PortHealth::Lost,
                    in_flight: 0,
                    delivered: 0,
                };
            }
            let answered = std::mem::take(&mut script.unanswered);
            for (tick, ids) in &answered {
                for id in ids {
                    outcomes.push(Outcome::Command(CommandOutcome {
                        id: *id,
                        tick: *tick,
                        result: CommandResult::Edited {
                            center: IVec2::ZERO,
                            changed: 1,
                            not_resident: 0,
                        },
                    }));
                }
                outcomes.push(Outcome::TickCompleted(TickReport {
                    tick: *tick,
                    commands: ids.len() as u32,
                    gpu_time: None,
                    phases: None,
                    latency: Duration::from_millis(1),
                    upload_bytes: 0,
                    readback_bytes: 0,
                    cells: crate::port::CellReport::default(),
                }));
            }
            crate::port::PollReport {
                health: PortHealth::Running,
                in_flight: 0,
                delivered: answered.len() as u32,
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
            TransferCounters::default()
        }
    }

    fn runtime(per_tick: u32) -> (Runtime, std::sync::Arc<std::sync::Mutex<Script>>) {
        let script = std::sync::Arc::new(std::sync::Mutex::new(Script::default()));
        let port = ScriptedPort {
            script: std::sync::Arc::clone(&script),
            per_tick,
        };
        let runtime = Runtime::new(Box::new(port), RuntimeConfig::default()).expect("runtime");
        (runtime, script)
    }

    fn raise(cell: i32) -> Command {
        Command::EditMaterial(
            MaterialEdit::new(EditTool::Raise, IVec2::new(cell, 0), 0, 1).expect("valid"),
        )
    }

    #[test]
    fn commands_are_submitted_in_admission_order_and_split_by_tick_capacity() {
        let (mut runtime, script) = runtime(2);
        for id in 1..=5 {
            assert_eq!(
                runtime.submit(CommandId::new(id), raise(id as i32)),
                Ok(Admission::Admitted)
            );
        }
        let report = runtime.frame_ticks(3);
        assert_eq!(report.ticks_submitted, 3);
        let batches = script.lock().expect("script").batches.clone();
        let ids = |values: &[u64]| {
            values
                .iter()
                .map(|value| CommandId::new(*value))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            batches,
            vec![
                (Tick::new(0), ids(&[1, 2])),
                (Tick::new(1), ids(&[3, 4])),
                (Tick::new(2), ids(&[5])),
            ]
        );
        assert!(matches!(
            runtime
                .receipt(CommandId::new(5))
                .map(|receipt| receipt.state),
            Some(ReceiptState::Completed { .. })
        ));
        assert_eq!(runtime.ledger().summary().commands_completed, 5);
        assert_eq!(
            runtime.ledger().summary().last_observed_tick,
            Some(Tick::new(2))
        );
    }

    #[test]
    fn backpressure_keeps_commands_queued_and_the_tick_unnumbered() {
        let (mut runtime, script) = runtime(8);
        runtime
            .submit(CommandId::new(1), raise(1))
            .expect("admitted");
        script.lock().expect("script").refuse_next =
            Some(Refusal::Pressure(Pressure::InFlightSubmissions));
        let report = runtime.frame_ticks(1);
        assert_eq!(report.pressure, Some(Pressure::InFlightSubmissions));
        assert_eq!(report.ticks_submitted, 0);
        assert_eq!(runtime.pending_commands(), 1);
        assert_eq!(
            runtime
                .receipt(CommandId::new(1))
                .map(|receipt| receipt.state),
            Some(ReceiptState::Pending)
        );
        let report = runtime.frame_ticks(1);
        assert_eq!(report.ticks_submitted, 1);
        assert_eq!(
            runtime.last_submitted_tick(),
            Some(Tick::new(0)),
            "no tick number was skipped"
        );
    }

    #[test]
    fn a_lost_world_abandons_queued_commands_and_closes_admission() {
        let (mut runtime, script) = runtime(8);
        runtime
            .submit(CommandId::new(1), raise(1))
            .expect("admitted");
        script.lock().expect("script").lost = true;
        let report = runtime.frame_ticks(1);
        assert_eq!(report.health, Some(PortHealth::Lost));
        assert_eq!(
            runtime
                .receipt(CommandId::new(1))
                .map(|receipt| receipt.state),
            Some(ReceiptState::Abandoned {
                reason: AbandonReason::DeviceLost
            })
        );
        assert_eq!(
            runtime.submit(CommandId::new(2), raise(2)),
            Err(AdmissionError::Closed)
        );
    }

    #[test]
    fn elapsed_time_drives_ticks_up_to_the_catch_up_cap() {
        let (mut runtime, script) = runtime(8);
        assert_eq!(runtime.frame(Duration::from_millis(50)).ticks_submitted, 3);
        assert_eq!(runtime.frame(Duration::from_secs(2)).ticks_submitted, 4);
        assert_eq!(runtime.discarded_ticks(), 116);
        assert_eq!(script.lock().expect("script").batches.len(), 7);
    }

    #[test]
    fn scheduled_commands_wait_for_their_tick_in_admission_order() {
        let (mut runtime, script) = runtime(8);
        runtime
            .submit_at(CommandId::new(1), raise(1), Tick::new(2))
            .expect("admitted");
        runtime
            .submit(CommandId::new(2), raise(2))
            .expect("admitted");
        runtime
            .submit_at(CommandId::new(3), raise(3), Tick::new(2))
            .expect("admitted");
        assert_eq!(runtime.frame_ticks(4).ticks_submitted, 4);
        let batches = script.lock().expect("script").batches.clone();
        let ids = |values: &[u64]| {
            values
                .iter()
                .map(|value| CommandId::new(*value))
                .collect::<Vec<_>>()
        };
        // Command 2 waits behind command 1, which is due in tick 2.
        assert_eq!(
            batches,
            vec![
                (Tick::new(0), ids(&[])),
                (Tick::new(1), ids(&[])),
                (Tick::new(2), ids(&[1, 2, 3])),
                (Tick::new(3), ids(&[])),
            ]
        );
    }

    #[test]
    fn the_clock_can_be_reconfigured_within_its_ranges() {
        let (mut runtime, _) = runtime(8);
        assert!(runtime.set_clock(5.0, 4).is_err());
        assert!(runtime.set_clock(30.0, 17).is_err());
        runtime.set_clock(30.0, 2).expect("supported");
        assert_eq!(runtime.tick_duration(), Duration::from_secs_f64(1.0 / 30.0));
        assert_eq!(runtime.frame(Duration::from_millis(110)).ticks_submitted, 2);
        assert_eq!(runtime.discarded_ticks(), 1);
    }

    #[test]
    fn the_runtime_starts_at_the_world_s_next_tick() {
        let script = std::sync::Arc::new(std::sync::Mutex::new(Script {
            next_tick: Tick::new(40),
            ..Script::default()
        }));
        let port = ScriptedPort {
            script: std::sync::Arc::clone(&script),
            per_tick: 1,
        };
        let mut runtime = Runtime::new(Box::new(port), RuntimeConfig::default()).expect("runtime");
        assert_eq!(runtime.frame_ticks(1).ticks_submitted, 1);
        assert_eq!(runtime.last_submitted_tick(), Some(Tick::new(40)));
    }
}
