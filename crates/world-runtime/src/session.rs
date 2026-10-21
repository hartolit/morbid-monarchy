//! A live session of a saved world: the runtime, its residency and checkpoint owners and the
//! workers between them and the store, run as one lifecycle.
//!
//! ```text
//! SessionPlan::prepare(store)          header, latest checkpoint, its bodies; the store marks
//!                                      a writing session (reclaiming an unclean one's leftovers)
//! composition root opens the live world at the checkpoint's window, tick and configuration
//! WorldSession::open(port, store, plan) restores the bodies, starts the workers
//! frame(elapsed) ...                   ticks, residency, checkpoints; never blocks
//! close(timeout) -> CloseReport        admission closed, final checkpoint at the last completed
//!                                      tick, work joined, an honest result
//! ```
//!
//! A lost device ends the session's live world. Whatever the session already holds on the CPU
//! is still written, and a checkpoint that was fully captured is still published; everything
//! else since the latest checkpoint is reported as possibly lost, never as recovered. Recovery
//! is a new session (a later epoch) from the latest complete checkpoint.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use world_model::{resume::BodySet, spatial::ChunkKey};

use crate::{
    checkpoint::{
        CheckpointConfig, Checkpoints, SaveFailure, SaveId, SaveReason, SaveReceipt, SaveState,
    },
    ids::{SessionEpoch, Tick},
    port::{AbandonReason, PhysicalWorld, PortHealth, Refusal, ResidentWindow},
    residency::{Residency, ResidencyConfig},
    runtime::{FrameReport, Runtime, RuntimeConfig, RuntimeError},
    store::{CheckpointManifest, SessionStart, StoreError, WorldHeader, WorldStore},
    workers::{Done, WorkerConfig, Workers},
};

/// What a session starts from, read from the store before the live world is opened.
#[derive(Debug, Clone)]
pub struct SessionPlan {
    pub header: WorldHeader,
    pub manifest: CheckpointManifest,
    pub bodies: BodySet,
    pub start: SessionStart,
}

impl SessionPlan {
    /// Reads the latest checkpoint and its bodies and starts a writing session.
    pub fn prepare(store: &dyn WorldStore) -> Result<Self, StoreError> {
        let header = *store.header();
        let manifest = store.latest()?;
        let bodies = store.read_bodies(manifest.id)?;
        let start = store.begin_session()?;
        Ok(Self {
            header,
            manifest,
            bodies,
            start,
        })
    }

    /// The tick the live world must start at.
    pub fn first_tick(&self) -> Option<Tick> {
        self.manifest.next_tick()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SessionConfig {
    pub runtime: RuntimeConfig,
    pub workers: WorkerConfig,
    pub residency: ResidencyConfig,
    pub checkpoints: CheckpointConfig,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionError {
    Runtime(RuntimeError),
    /// The live world was not opened at the checkpoint's next tick or window.
    Mismatch {
        expected_tick: Option<Tick>,
        live_tick: Tick,
        expected_window: ResidentWindow,
        live_window: ResidentWindow,
    },
    /// The checkpoint's bodies could not be restored.
    Restore(Refusal),
}

/// Where the session's last frame spent its CPU time, each owner's share measured with the
/// monotonic clock: the runtime's frame (polling, outcomes, and encoding and submitting the
/// frame's ticks), the residency owner with the workers' results, the checkpoint owner, and the
/// snapshot and streaming copies no tick took along. Reports and displays add them up; nothing
/// decides on them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameTimes {
    pub runtime: Duration,
    pub residency: Duration,
    pub checkpoints: Duration,
    pub transfers: Duration,
}

impl FrameTimes {
    pub fn total(&self) -> Duration {
        self.runtime + self.residency + self.checkpoints + self.transfers
    }
}

/// What a lost or faulted live world cost: everything after the latest complete checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LossReport {
    pub reason: AbandonReason,
    /// The checkpoint a recovery reopens, and the tick it holds.
    pub last_complete: crate::store::CheckpointId,
    pub last_complete_tick: Option<Tick>,
    /// The last tick submitted before the loss: ticks after the checkpoint's are lost.
    pub last_submitted: Option<Tick>,
    pub ticks_lost: u64,
    /// Chunks resident when the device was lost: any change since their last snapshot is gone.
    pub resident_chunks: u32,
    /// Revisions captured on the CPU after the latest checkpoint. They are written, but no
    /// complete checkpoint holds them, so a recovery does not use them.
    pub captured_not_published: u32,
    /// The checkpoint in progress had everything on the CPU and was finished despite the loss.
    pub finished_captured_save: bool,
}

/// What an orderly close did.
#[derive(Debug, Clone, PartialEq)]
pub struct CloseReport {
    /// The final save, and whether it committed.
    pub save: Option<SaveReceipt>,
    /// The latest complete checkpoint after the close: the state a reopen finds.
    pub latest: CheckpointManifest,
    /// Commands admitted but never run (admission closed before their tick).
    pub abandoned_commands: u32,
    /// Captured revisions that never became durable.
    pub unwritten: u32,
    pub workers_joined: bool,
    pub timed_out: bool,
    pub waited: Duration,
    /// Whether every resident change up to the last completed tick is in `latest`.
    pub saved: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Health {
    Live,
    Lost(AbandonReason),
}

pub struct WorldSession {
    runtime: Runtime,
    store: Arc<dyn WorldStore>,
    workers: Option<Workers>,
    residency: Residency,
    checkpoints: Checkpoints,
    done: Vec<Done>,
    header: WorldHeader,
    health: Health,
    loss: Option<LossReport>,
    start: SessionStart,
    /// The tick of the checkpoint the session opened.
    opened_tick: Option<Tick>,
    frame_times: FrameTimes,
}

/// Whether `receipt` is a committed save of the world at `boundary`.
fn holds(receipt: Option<&SaveReceipt>, boundary: Option<Tick>) -> bool {
    receipt.is_some_and(
        |receipt| matches!(receipt.state, SaveState::Committed { tick, .. } if tick == boundary),
    )
}

impl WorldSession {
    /// Starts a session over `port`, which the composition root opened at `plan`'s checkpoint
    /// (window, next tick, configuration and rule seed).
    pub fn open(
        port: Box<dyn PhysicalWorld>,
        store: Arc<dyn WorldStore>,
        plan: SessionPlan,
        config: SessionConfig,
    ) -> Result<Self, SessionError> {
        let live_tick = port.next_tick();
        let live_window = port.capabilities().window;
        if plan.first_tick() != Some(live_tick) || live_window != plan.manifest.window {
            return Err(SessionError::Mismatch {
                expected_tick: plan.first_tick(),
                live_tick,
                expected_window: plan.manifest.window,
                live_window,
            });
        }
        let mut runtime = Runtime::new(port, config.runtime).map_err(SessionError::Runtime)?;
        runtime
            .restore_bodies(&plan.bodies, plan.manifest.tick)
            .map_err(SessionError::Restore)?;
        let workers = Workers::start(
            Arc::clone(&store),
            plan.header.generator.seed,
            config.workers,
        );
        Ok(Self {
            runtime,
            store,
            workers: Some(workers),
            residency: Residency::new(plan.manifest.window, config.residency),
            checkpoints: Checkpoints::new(plan.manifest, config.checkpoints),
            done: Vec::new(),
            header: plan.header,
            health: Health::Live,
            loss: None,
            start: plan.start,
            opened_tick: plan.manifest.tick,
            frame_times: FrameTimes::default(),
        })
    }

    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    pub fn runtime_mut(&mut self) -> &mut Runtime {
        &mut self.runtime
    }

    pub fn residency(&self) -> &Residency {
        &self.residency
    }

    pub fn checkpoints(&self) -> &Checkpoints {
        &self.checkpoints
    }

    pub fn header(&self) -> &WorldHeader {
        &self.header
    }

    pub fn store(&self) -> &Arc<dyn WorldStore> {
        &self.store
    }

    /// What the store found when this session began.
    pub fn start(&self) -> SessionStart {
        self.start
    }

    /// The loss of the live world, once it happened.
    pub fn loss(&self) -> Option<LossReport> {
        self.loss
    }

    /// Where the last frame's CPU time went.
    pub fn frame_times(&self) -> FrameTimes {
        self.frame_times
    }

    /// Moves the resident window: centre and radii.
    pub fn set_window(&mut self, window: ResidentWindow) {
        self.residency.set_target(window);
    }

    /// Moves the window's centre to `focus`, keeping its radii.
    pub fn set_focus(&mut self, focus: ChunkKey) -> Result<(), crate::port::PayloadError> {
        let current = self.residency.target();
        let window = ResidentWindow::new(focus, current.radius_x(), current.radius_y())?;
        self.residency.set_target(window);
        Ok(())
    }

    /// Changes the autosave interval (ticks since the latest checkpoint); `None` turns it off.
    pub fn set_autosave(&mut self, ticks: Option<u64>) {
        self.checkpoints.set_autosave(ticks);
    }

    /// Asks for a checkpoint; the receipt tells how it went.
    pub fn save(&mut self) -> SaveId {
        self.checkpoints.request(SaveReason::Requested)
    }

    /// One presented frame: the runtime's ticks and poll, then residency and checkpoints, then
    /// the transfers no tick carried. Never blocks.
    pub fn frame(&mut self, elapsed: Duration) -> FrameReport {
        let started = Instant::now();
        let report = self.runtime.frame(elapsed);
        self.after_frame(report, started);
        report
    }

    /// One frame owing exactly `ticks` ticks (headless runs scheduled by tick).
    pub fn frame_ticks(&mut self, ticks: u32) -> FrameReport {
        let started = Instant::now();
        let report = self.runtime.frame_ticks(ticks);
        self.after_frame(report, started);
        report
    }

    /// The owners' share of a frame whose runtime part began at `started`.
    fn after_frame(&mut self, report: FrameReport, started: Instant) {
        let mut mark = Instant::now();
        self.frame_times = FrameTimes {
            runtime: mark - started,
            ..FrameTimes::default()
        };
        let lap = |mark: &mut Instant| {
            let now = Instant::now();
            let elapsed = now - *mark;
            *mark = now;
            elapsed
        };
        if self.health == Health::Live {
            let lost = match report.health {
                Some(PortHealth::Lost) => Some(AbandonReason::DeviceLost),
                Some(PortHealth::Faulted) => Some(AbandonReason::Faulted),
                _ => None,
            };
            if let Some(reason) = lost {
                self.lose(reason);
            }
        }
        let Some(workers) = self.workers.as_mut() else {
            return;
        };
        workers.collect(&mut self.done);
        self.residency
            .frame(&mut self.runtime, workers, &mut self.done);
        self.frame_times.residency = lap(&mut mark);
        let outcomes: Vec<_> = self.residency.other_outcomes().copied().collect();
        self.checkpoints.frame(
            &mut self.runtime,
            &mut self.residency,
            workers,
            &outcomes,
            &mut self.done,
        );
        // Results nobody wanted (reclamation) are dropped here; nothing else is left.
        self.done.clear();
        self.frame_times.checkpoints = lap(&mut mark);
        if self.health == Health::Live {
            // A snapshot requested this frame, or a change that is streaming, goes out now when
            // no tick of this frame took it along.
            let _ = self.runtime.submit_transfers();
        }
        self.frame_times.transfers = lap(&mut mark);
    }

    fn lose(&mut self, reason: AbandonReason) {
        self.health = Health::Lost(reason);
        let latest = *self.checkpoints.latest();
        let last_submitted = self.runtime.last_submitted_tick();
        let ticks_lost = match (last_submitted, latest.tick) {
            (Some(last), Some(saved)) => last.since(saved),
            (Some(last), None) => last.value() + 1,
            (None, _) => 0,
        };
        self.loss = Some(LossReport {
            reason,
            last_complete: latest.id,
            last_complete_tick: latest.tick,
            last_submitted,
            ticks_lost,
            resident_chunks: self.residency.resident_keys().count() as u32,
            captured_not_published: self.residency.unpublished().count() as u32,
            finished_captured_save: false,
        });
        if !self.checkpoints.captured() {
            self.checkpoints.give_up(SaveFailure::Abandoned(reason));
        }
    }

    /// After a loss: finishes what the CPU still holds (writes, and a fully captured
    /// checkpoint), joins the workers and reports what was lost. The store is then ready for a
    /// new session from its latest complete checkpoint.
    pub fn finish_after_loss(mut self, timeout: Duration) -> (Arc<dyn WorldStore>, LossReport) {
        let deadline = Instant::now() + timeout;
        let mut loss = self.loss.unwrap_or(LossReport {
            reason: AbandonReason::DeviceLost,
            last_complete: self.checkpoints.latest().id,
            last_complete_tick: self.checkpoints.latest().tick,
            last_submitted: self.runtime.last_submitted_tick(),
            ticks_lost: 0,
            resident_chunks: self.residency.resident_keys().count() as u32,
            captured_not_published: 0,
            finished_captured_save: false,
        });
        let had_captured = self.checkpoints.captured();
        while Instant::now() < deadline && (self.checkpoints.busy() || self.residency.busy()) {
            self.residency.retry_writes_now();
            self.checkpoints.retry_now();
            self.frame_ticks(0);
            std::thread::sleep(Duration::from_millis(1));
        }
        if had_captured
            && self
                .checkpoints
                .receipt()
                .and_then(SaveReceipt::committed)
                .is_some()
        {
            loss.finished_captured_save = true;
            loss.last_complete = self.checkpoints.latest().id;
            loss.last_complete_tick = self.checkpoints.latest().tick;
            loss.ticks_lost = match (loss.last_submitted, loss.last_complete_tick) {
                (Some(last), Some(saved)) => last.since(saved),
                (Some(last), None) => last.value() + 1,
                (None, _) => 0,
            };
        }
        if let Some(workers) = self.workers.take() {
            let mut done = Vec::new();
            let remaining = deadline.saturating_duration_since(Instant::now());
            workers.close(remaining, &mut done);
        }
        (self.store, loss)
    }

    /// Closes the session in order: admission closes, the in-flight ticks complete, a final
    /// checkpoint captures the last completed tick, the workers finish and the live world
    /// stops. Waits up to `timeout` (a maintenance wait, never a frame's).
    pub fn close(mut self, timeout: Duration) -> CloseReport {
        let started = Instant::now();
        let deadline = started + timeout;
        let abandoned_commands = self.runtime.close_admission();
        // Every submitted tick completes and is observed: the boundary is the last of them.
        while Instant::now() < deadline && self.health == Health::Live {
            let counters = self.runtime.counters();
            if counters.ticks_observed >= counters.ticks_submitted {
                break;
            }
            self.frame_ticks(0);
            std::thread::sleep(Duration::from_micros(200));
        }
        // The close publishes everything up to the last completed tick (with none run, the
        // tick the session opened at, where a save captures the restored state).
        let boundary = self.runtime.last_submitted_tick().or(self.opened_tick);
        let save = if self.health == Health::Live {
            // A save already under way keeps its own boundary, which may be earlier; once it is
            // over, the close takes another. At most two run here.
            let mut receipt = None;
            for _ in 0..2 {
                self.checkpoints.request(SaveReason::Close);
                while Instant::now() < deadline
                    && (self.checkpoints.busy() || self.residency.busy())
                {
                    self.residency.retry_writes_now();
                    self.checkpoints.retry_now();
                    self.frame_ticks(0);
                    std::thread::sleep(Duration::from_micros(200));
                }
                receipt = self.checkpoints.receipt().cloned();
                let over = receipt
                    .as_ref()
                    .is_none_or(|receipt| !matches!(receipt.state, SaveState::Committed { .. }));
                if over || holds(receipt.as_ref(), boundary) || Instant::now() >= deadline {
                    break;
                }
            }
            receipt
        } else {
            self.checkpoints.receipt().cloned()
        };
        let timed_out =
            Instant::now() >= deadline && (self.checkpoints.busy() || self.residency.busy());
        let remaining = deadline.saturating_duration_since(Instant::now());
        self.runtime.stop(remaining.max(Duration::from_millis(1)));
        let mut done = Vec::new();
        let workers_joined = match self.workers.take() {
            Some(workers) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                workers.close(remaining, &mut done).joined
            }
            None => true,
        };
        let committed = holds(save.as_ref(), boundary);
        let unwritten = self.residency.retained_unwritten() as u32;
        let saved = committed && unwritten == 0 && workers_joined && self.health == Health::Live;
        if saved {
            // Only a close that saved everything marks the store clean; otherwise the next
            // session reclaims what this one left unpublished.
            let _ = self.store.end_session();
        }
        CloseReport {
            save,
            latest: *self.checkpoints.latest(),
            abandoned_commands,
            unwritten,
            workers_joined,
            timed_out,
            waited: started.elapsed(),
            saved,
        }
    }

    /// Ends the session without saving: the live world stops and the workers finish what they
    /// hold, but nothing since the latest checkpoint is published, and the store stays marked as
    /// not closed, so the next session reclaims what this one wrote and never published. What a
    /// reopen finds is the latest checkpoint.
    pub fn abandon(mut self, timeout: Duration) -> CloseReport {
        let started = Instant::now();
        let deadline = started + timeout;
        let abandoned_commands = self.runtime.close_admission();
        let stopped = self.runtime.stop(timeout);
        let mut done = Vec::new();
        let workers_joined = match self.workers.take() {
            Some(workers) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                workers.close(remaining, &mut done).joined
            }
            None => true,
        };
        CloseReport {
            save: self.checkpoints.receipt().cloned(),
            latest: *self.checkpoints.latest(),
            abandoned_commands,
            unwritten: self.residency.retained_unwritten() as u32,
            workers_joined,
            timed_out: stopped.timed_out,
            waited: started.elapsed(),
            saved: false,
        }
    }

    /// The session epoch a recovery after this one uses.
    pub fn next_epoch(&self) -> Option<SessionEpoch> {
        self.runtime.session().epoch.next()
    }
}

#[cfg(test)]
mod tests;
