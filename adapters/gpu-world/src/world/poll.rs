//! Observing completed work without blocking, ending a session, and the orderly stop.
//!
//! A poll lets the device run its callbacks, then advances what waits for a completed
//! submission (upload remaps, slot activations, retired buffers) and decodes delivered
//! readbacks. A lost device or an unexplained fault ends the session: every outstanding
//! obligation is reported abandoned once, and later deliveries for it are counted as stale.
//! The orderly stop is the one place this owner blocks, with a time limit, as maintenance.

use std::time::{Duration, Instant};

use world_runtime::port::{
    AbandonReason, DrainReport, Obligation, Outcome, PollReport, UploadTicket,
};

use super::{CaptureState, GpuWorld, Lifecycle};
use crate::{
    context::WaitOutcome,
    readback::DecodeError,
    staging::{Completion, DeliveryOrder, StagingRef},
};

impl GpuWorld {
    /// Advances everything that waits for a completed submission.
    pub(super) fn on_completed(&mut self, outcomes: &mut Vec<Outcome>) {
        let completed = self.context.completed_serial();
        self.tick_uploads.completed(completed);
        self.chunk_uploads.completed(completed);
        self.scratch.activated.clear();
        self.slots.completed(completed, &mut self.scratch.activated);
        for activated in &self.scratch.activated {
            outcomes.push(Outcome::UploadActivated {
                ticket: UploadTicket {
                    slot: activated.slot,
                    key: activated.key,
                    revision: activated.revision,
                },
                tick: activated.tick,
            });
        }
        let mut index = 0;
        while index < self.retired.len() {
            if self.retired[index].serial <= completed {
                let retired = self.retired.swap_remove(index);
                retired.buffers.release(&mut self.context);
            } else {
                index += 1;
            }
        }
    }

    pub(super) fn fault(&mut self, _error: DecodeError, outcomes: &mut Vec<Outcome>) {
        if matches!(self.lifecycle, Lifecycle::Lost | Lifecycle::Faulted) {
            return;
        }
        self.lifecycle = Lifecycle::Faulted;
        self.abandon_all(AbandonReason::Faulted, outcomes);
    }

    pub(super) fn lose(&mut self, outcomes: &mut Vec<Outcome>) {
        if self.lifecycle == Lifecycle::Lost {
            return;
        }
        self.lifecycle = Lifecycle::Lost;
        self.abandon_all(AbandonReason::DeviceLost, outcomes);
    }

    /// Reports every outstanding obligation abandoned.
    pub(super) fn abandon_all(&mut self, reason: AbandonReason, outcomes: &mut Vec<Outcome>) {
        for record in &mut self.ticks {
            if !record.active {
                continue;
            }
            record.active = false;
            for expected in &record.expected {
                outcomes.push(Outcome::Abandoned {
                    obligation: Obligation::Command(expected.id),
                    reason,
                });
            }
            outcomes.push(Outcome::Abandoned {
                obligation: Obligation::Tick(record.tick),
                reason,
            });
        }
        for staged in self.slots.staged() {
            outcomes.push(Outcome::Abandoned {
                obligation: Obligation::Upload(UploadTicket {
                    slot: staged.slot,
                    key: staged.key,
                    revision: staged.revision,
                }),
                reason,
            });
        }
        for (slot, key, revision) in self.slots.activating() {
            outcomes.push(Outcome::Abandoned {
                obligation: Obligation::Upload(UploadTicket {
                    slot,
                    key,
                    revision,
                }),
                reason,
            });
        }
        // A ready capture is already in CPU-visible memory; only a lost or faulted device
        // makes it unreadable.
        let keep_ready = reason == AbandonReason::StopTimedOut;
        let mut index = 0;
        while index < self.captures.len() {
            let ready = matches!(self.captures[index].state, CaptureState::Ready { .. });
            if keep_ready && ready {
                index += 1;
                continue;
            }
            let capture = self.captures.swap_remove(index);
            outcomes.push(Outcome::Abandoned {
                obligation: Obligation::Capture(capture.ticket),
                reason,
            });
        }
        self.abandon_snapshots(reason, keep_ready, outcomes);
    }

    /// Whether anything submitted still owes an outcome.
    pub(super) fn outstanding(&self) -> bool {
        self.ticks.iter().any(|record| record.active)
            || self
                .captures
                .iter()
                .any(|capture| matches!(capture.state, CaptureState::Submitted { .. }))
            || self.snapshots.in_flight()
            || self.context.in_flight() > 0
    }

    pub(super) fn poll_into(&mut self, outcomes: &mut Vec<Outcome>) -> PollReport {
        let start = outcomes.len();
        if let Some(faults) = &self.faults
            && faults.take_device_loss()
            && !self.loss_injected
        {
            self.context.destroy_device();
            self.loss_injected = true;
        }
        let hold_completions = self
            .faults
            .as_ref()
            .is_some_and(|faults| faults.hold_completions());
        let usable = self.context.poll(!hold_completions);
        if matches!(self.lifecycle, Lifecycle::Lost | Lifecycle::Faulted) {
            return PollReport {
                health: self.health(),
                in_flight: self.context.world_in_flight(),
                delivered: 0,
            };
        }
        if !usable || self.loss_injected || self.context.health().lost().is_some() {
            self.lose(outcomes);
        } else if self.context.health().uncaptured_count() > 0 {
            self.fault(
                DecodeError::Status {
                    position: u32::MAX,
                    status: u32::MAX,
                },
                outcomes,
            );
        } else {
            self.on_completed(outcomes);
            let order = DeliveryOrder {
                hold: self
                    .faults
                    .as_ref()
                    .is_some_and(|faults| faults.hold_deliveries()),
                reverse: self
                    .faults
                    .as_ref()
                    .is_some_and(|faults| faults.reverse_deliveries()),
            };
            let mut completions = std::mem::take(&mut self.scratch.completions);
            completions.clear();
            self.tick_uploads.collect(order, &mut completions);
            self.chunk_uploads.collect(order, &mut completions);
            let readbacks_from = completions.len();
            self.tick_readbacks.collect(order, &mut completions);
            let captures_from = completions.len();
            self.capture_staging.collect(order, &mut completions);
            for (position, completion) in completions.iter().enumerate() {
                match *completion {
                    Completion::Ready(reservation)
                        if position < captures_from && position >= readbacks_from =>
                    {
                        self.decode_tick(reservation, outcomes);
                    }
                    Completion::Ready(reservation) => {
                        if !self.snapshot_ready(reservation, outcomes) {
                            self.capture_ready(reservation, outcomes);
                        }
                    }
                    Completion::Remapped(_) => {}
                    Completion::Failed(_) => self.lose(outcomes),
                }
            }
            self.scratch.completions = completions;
        }
        self.refresh_counters();
        PollReport {
            health: self.health(),
            in_flight: self.context.world_in_flight(),
            delivered: (outcomes.len() - start) as u32,
        }
    }

    pub(super) fn capture_ready(&mut self, reservation: StagingRef, outcomes: &mut Vec<Outcome>) {
        let Some(capture) = self
            .captures
            .iter_mut()
            .find(|capture| capture.staging == reservation)
        else {
            self.counters.stale_deliveries += 1;
            return;
        };
        if let CaptureState::Submitted { tick } = capture.state {
            capture.state = CaptureState::Ready { tick };
            outcomes.push(Outcome::CaptureReady {
                ticket: capture.ticket,
                tick,
            });
        }
    }

    /// Closes admission and waits up to `timeout` for outstanding work: the port's `stop`.
    pub(super) fn drain(&mut self, timeout: Duration, outcomes: &mut Vec<Outcome>) -> DrainReport {
        let start = outcomes.len();
        if self.lifecycle != Lifecycle::Running {
            return DrainReport {
                health: self.health(),
                delivered: 0,
                abandoned: 0,
                waited: Duration::ZERO,
                timed_out: false,
            };
        }
        self.lifecycle = Lifecycle::Stopping;
        let mut abandoned = 0;
        // Requests no tick will carry now: staged uploads and requested captures.
        let mut staged = std::mem::take(&mut self.scratch.staged);
        staged.clear();
        self.slots.cancel_staged(&mut staged);
        for upload in &staged {
            self.chunk_uploads.release(upload.upload);
            outcomes.push(Outcome::Abandoned {
                obligation: Obligation::Upload(UploadTicket {
                    slot: upload.slot,
                    key: upload.key,
                    revision: upload.revision,
                }),
                reason: AbandonReason::Stopped,
            });
            abandoned += 1;
        }
        self.scratch.staged = staged;
        let mut index = 0;
        while index < self.captures.len() {
            if self.captures[index].state == CaptureState::Requested {
                let capture = self.captures.swap_remove(index);
                self.capture_staging.release(capture.staging);
                outcomes.push(Outcome::Abandoned {
                    obligation: Obligation::Capture(capture.ticket),
                    reason: AbandonReason::Stopped,
                });
                abandoned += 1;
            } else {
                index += 1;
            }
        }
        let started = Instant::now();
        let deadline = started + timeout;
        let mut timed_out = false;
        loop {
            self.poll_into(outcomes);
            if self.lifecycle != Lifecycle::Stopping || !self.outstanding() {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                timed_out = true;
                break;
            }
            let last = self.context.next_serial().saturating_sub(1);
            let outcome = self.maintenance_wait(last, deadline - now);
            if outcome == WaitOutcome::Lost {
                self.poll_into(outcomes);
                break;
            }
            if self.context.in_flight() == 0 {
                // The device is idle but deliveries are still outstanding (their callbacks run
                // at the next poll); pause briefly instead of spinning.
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        // The whole stop blocked its caller: report it all as maintenance waiting.
        let waited = started.elapsed();
        if self.lifecycle == Lifecycle::Stopping {
            if timed_out {
                let before = outcomes.len();
                self.abandon_all(AbandonReason::StopTimedOut, outcomes);
                abandoned += (outcomes.len() - before) as u32;
            }
            // No submission follows a stop: snapshots that still needed one cannot finish.
            // Ready ones stay takeable.
            abandoned += self.abandon_unsubmitted_snapshots(outcomes);
            self.lifecycle = Lifecycle::Stopped;
        }
        DrainReport {
            health: self.health(),
            delivered: (outcomes.len() - start) as u32,
            abandoned,
            waited,
            timed_out,
        }
    }
}
