//! The CPU consumer of the live world's outcomes.
//!
//! Outcomes come in two classes, and the difference decides what pressure may do to them:
//! - **Important** outcomes are command results and chunk residency results. Their storage is
//!   reserved before the request is accepted (a receipt, or a residency reservation here), so
//!   they cannot be lost to a full buffer. They are kept until acknowledged or collected.
//! - **Cosmetic** events are transient notifications. They pass through a bounded ring; when
//!   the device's event buffer or this ring is full they are dropped, and every drop is
//!   counted and shown.
//!
//! Later stages add producers (contacts, immersion) to these two classes rather than invent
//! another queue.

use std::collections::VecDeque;
use std::time::Duration;

use crate::{
    ids::Tick,
    port::{
        AbandonReason, CaptureTicket, CommandOutcome, CosmeticEvent, Obligation, Outcome,
        TickReport, UploadTicket,
    },
    receipts::Receipts,
};

/// A chunk residency result for the residency owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidencyOutcome {
    UploadActivated {
        ticket: UploadTicket,
        tick: Tick,
    },
    UploadAbandoned {
        ticket: UploadTicket,
        reason: AbandonReason,
    },
    CaptureReady {
        ticket: CaptureTicket,
        tick: Tick,
    },
    CaptureAbandoned {
        ticket: CaptureTicket,
        reason: AbandonReason,
    },
}

/// Summary statistics of a duration or tick-count series.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Series {
    pub count: u64,
    pub max: f64,
    pub sum: f64,
}

impl Series {
    fn add(&mut self, value: f64) {
        self.count += 1;
        self.sum += value;
        if value > self.max {
            self.max = value;
        }
    }

    pub fn mean(&self) -> Option<f64> {
        (self.count > 0).then(|| self.sum / self.count as f64)
    }
}

/// What the consumer has seen, for displays and reports.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LedgerSummary {
    pub last_observed_tick: Option<Tick>,
    pub ticks_observed: u64,
    pub commands_completed: u64,
    pub commands_abandoned: u64,
    /// Ticks from admission to application.
    pub input_latency_ticks: Series,
    /// Ticks from application to the CPU observing the outcome.
    pub observation_age_ticks: Series,
    /// Wall time from a tick's submission to its results being decoded, in milliseconds.
    pub tick_latency_ms: Series,
    /// Device time of the whole tick, in milliseconds, when timestamps are available.
    pub gpu_time_ms: Series,
    /// Ticks whose material phases did not balance their accounting or saw an inconsistent
    /// transfer, and the latest such tick: an important error, never a cosmetic one.
    pub inconsistent_ticks: u64,
    pub last_inconsistent_tick: Option<Tick>,
    pub cosmetic_received: u64,
    /// Cosmetic events the device could not record.
    pub cosmetic_dropped_on_device: u64,
    /// Cosmetic events this ring overwrote before anyone read them.
    pub cosmetic_overwritten: u64,
    pub ticks_abandoned: u64,
}

#[derive(Debug)]
pub struct OutcomeLedger {
    cosmetic: VecDeque<CosmeticEvent>,
    cosmetic_capacity: usize,
    residency: VecDeque<ResidencyOutcome>,
    residency_capacity: usize,
    residency_reserved: usize,
    summary: LedgerSummary,
    last_tick: Option<TickReport>,
}

impl OutcomeLedger {
    /// `residency_capacity` bounds outstanding uploads plus captures: each is reserved here
    /// before the request reaches the live world.
    pub fn new(cosmetic_capacity: usize, residency_capacity: usize) -> Self {
        Self {
            cosmetic: VecDeque::with_capacity(cosmetic_capacity),
            cosmetic_capacity,
            residency: VecDeque::with_capacity(residency_capacity),
            residency_capacity,
            residency_reserved: 0,
            summary: LedgerSummary::default(),
            last_tick: None,
        }
    }

    pub fn summary(&self) -> &LedgerSummary {
        &self.summary
    }

    pub fn last_tick(&self) -> Option<&TickReport> {
        self.last_tick.as_ref()
    }

    /// Reserves room for one residency outcome. `false` means the residency owner must collect
    /// earlier outcomes before requesting more.
    pub fn reserve_residency(&mut self) -> bool {
        if self.residency_reserved < self.residency_capacity {
            self.residency_reserved += 1;
            true
        } else {
            false
        }
    }

    /// Returns a reservation whose request the live world refused.
    pub fn release_residency(&mut self) {
        self.residency_reserved = self.residency_reserved.saturating_sub(1);
    }

    /// Collects residency outcomes, releasing their reservations.
    pub fn drain_residency(&mut self) -> impl Iterator<Item = ResidencyOutcome> + '_ {
        let drained = self.residency.len();
        self.residency_reserved = self.residency_reserved.saturating_sub(drained);
        self.residency.drain(..)
    }

    /// Collects cosmetic events in arrival order.
    pub fn drain_cosmetic(&mut self) -> impl Iterator<Item = CosmeticEvent> + '_ {
        self.cosmetic.drain(..)
    }

    /// Applies one outcome. `observed_at` is the runtime's last submitted tick.
    pub fn consume(
        &mut self,
        outcome: Outcome,
        receipts: &mut Receipts,
        observed_at: Option<Tick>,
    ) {
        match outcome {
            Outcome::Command(CommandOutcome { id, tick, result }) => {
                if let Some(receipt) = receipts.complete(id, tick, result, observed_at) {
                    self.summary.commands_completed += 1;
                    if let Some(latency) = receipt.input_latency_ticks() {
                        self.summary.input_latency_ticks.add(latency as f64);
                    }
                    if let Some(age) = receipt.observation_age_ticks() {
                        self.summary.observation_age_ticks.add(age as f64);
                    }
                }
            }
            Outcome::TickCompleted(report) => {
                self.summary.ticks_observed += 1;
                self.summary.last_observed_tick = Some(
                    self.summary
                        .last_observed_tick
                        .map_or(report.tick, |seen| seen.max(report.tick)),
                );
                self.summary.tick_latency_ms.add(millis(report.latency));
                if let Some(gpu_time) = report.gpu_time {
                    self.summary.gpu_time_ms.add(millis(gpu_time));
                }
                if !report.cells.consistent() {
                    self.summary.inconsistent_ticks += 1;
                    self.summary.last_inconsistent_tick = Some(
                        self.summary
                            .last_inconsistent_tick
                            .map_or(report.tick, |seen| seen.max(report.tick)),
                    );
                }
                if self.last_tick.is_none_or(|last| report.tick >= last.tick) {
                    self.last_tick = Some(report);
                }
            }
            Outcome::UploadActivated { ticket, tick } => {
                self.push_residency(ResidencyOutcome::UploadActivated { ticket, tick });
            }
            Outcome::CaptureReady { ticket, tick } => {
                self.push_residency(ResidencyOutcome::CaptureReady { ticket, tick });
            }
            Outcome::Cosmetic(event) => {
                self.summary.cosmetic_received += 1;
                if self.cosmetic.len() == self.cosmetic_capacity {
                    self.cosmetic.pop_front();
                    self.summary.cosmetic_overwritten += 1;
                }
                if self.cosmetic_capacity > 0 {
                    self.cosmetic.push_back(event);
                } else {
                    self.summary.cosmetic_overwritten += 1;
                }
            }
            Outcome::CosmeticOverflow { dropped, .. } => {
                self.summary.cosmetic_dropped_on_device += u64::from(dropped);
            }
            Outcome::Abandoned { obligation, reason } => match obligation {
                Obligation::Command(id) => {
                    receipts.abandon(id, reason);
                    self.summary.commands_abandoned += 1;
                }
                Obligation::Tick(_) => self.summary.ticks_abandoned += 1,
                Obligation::Upload(ticket) => {
                    self.push_residency(ResidencyOutcome::UploadAbandoned { ticket, reason });
                }
                Obligation::Capture(ticket) => {
                    self.push_residency(ResidencyOutcome::CaptureAbandoned { ticket, reason });
                }
            },
        }
    }

    fn push_residency(&mut self, outcome: ResidencyOutcome) {
        // Every residency request reserved its entry before it was accepted, so the queue has
        // room. Growing here instead of dropping keeps the outcome if that invariant is broken.
        self.residency.push_back(outcome);
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use world_model::{
        command::{Command, EditTool, MaterialEdit},
        glam::IVec2,
    };

    use super::*;
    use crate::{
        ids::CommandId,
        port::{CellReport, CommandResult, SupportedCommands},
    };

    fn report(tick: u64) -> Outcome {
        Outcome::TickCompleted(TickReport {
            tick: Tick::new(tick),
            commands: 0,
            gpu_time: None,
            phases: None,
            latency: Duration::from_millis(3),
            upload_bytes: 0,
            readback_bytes: 0,
            cells: CellReport::default(),
        })
    }

    #[test]
    fn an_unbalanced_tick_is_counted_as_inconsistent() {
        let mut ledger = OutcomeLedger::new(0, 0);
        let mut receipts = Receipts::new(1);
        let balanced = CellReport {
            fluid_in: 10,
            fluid_out: 14,
            boil: 4,
            solid_in: 3,
            quench: 0,
            solid_out: 3,
            ..CellReport::default()
        };
        assert!(balanced.consistent());
        let lost = CellReport {
            fluid_out: 1,
            ..CellReport::default()
        };
        assert_eq!(lost.imbalance(), (-1, 0));
        let stale = CellReport {
            stale_flow: 1,
            ..CellReport::default()
        };
        for (tick, cells) in [(1, balanced), (2, lost), (3, stale), (4, balanced)] {
            let Outcome::TickCompleted(report) = report(tick) else {
                unreachable!()
            };
            ledger.consume(
                Outcome::TickCompleted(TickReport { cells, ..report }),
                &mut receipts,
                None,
            );
        }
        assert_eq!(ledger.summary().inconsistent_ticks, 2);
        assert_eq!(ledger.summary().last_inconsistent_tick, Some(Tick::new(3)));
    }

    #[test]
    fn cosmetic_drops_are_counted_on_the_device_and_in_the_ring() {
        let mut ledger = OutcomeLedger::new(2, 0);
        let mut receipts = Receipts::new(1);
        for kind in 0..3 {
            let event = CosmeticEvent {
                tick: Tick::new(1),
                kind,
                data: [0; 3],
            };
            ledger.consume(Outcome::Cosmetic(event), &mut receipts, None);
        }
        ledger.consume(
            Outcome::CosmeticOverflow {
                tick: Tick::new(1),
                dropped: 5,
            },
            &mut receipts,
            None,
        );
        let summary = *ledger.summary();
        assert_eq!(summary.cosmetic_received, 3);
        assert_eq!(summary.cosmetic_overwritten, 1);
        assert_eq!(summary.cosmetic_dropped_on_device, 5);
        let kinds: Vec<u32> = ledger.drain_cosmetic().map(|event| event.kind).collect();
        assert_eq!(kinds, [1, 2], "the newest events are kept");
    }

    #[test]
    fn out_of_order_ticks_keep_the_latest_observation() {
        let mut ledger = OutcomeLedger::new(0, 0);
        let mut receipts = Receipts::new(1);
        for tick in [3, 5, 4] {
            ledger.consume(report(tick), &mut receipts, None);
        }
        assert_eq!(ledger.summary().last_observed_tick, Some(Tick::new(5)));
        assert_eq!(ledger.last_tick().map(|last| last.tick), Some(Tick::new(5)));
        assert_eq!(ledger.summary().ticks_observed, 3);
    }

    #[test]
    fn command_outcomes_complete_receipts_with_their_ages() {
        let mut ledger = OutcomeLedger::new(0, 0);
        let mut receipts = Receipts::new(2);
        let command = Command::EditMaterial(
            MaterialEdit::new(EditTool::Sand, IVec2::ZERO, 1, 1).expect("valid"),
        );
        let id = CommandId::new(1);
        let supported = SupportedCommands::default().with(command.kind());
        assert_eq!(receipts.check(id, &command, supported), Ok(None));
        receipts
            .insert(id, command, Some(Tick::new(1)))
            .expect("room");
        let result = CommandResult::Edited {
            changed: 5,
            not_resident: 0,
        };
        ledger.consume(
            Outcome::Command(CommandOutcome {
                id,
                tick: Tick::new(2),
                result,
            }),
            &mut receipts,
            Some(Tick::new(4)),
        );
        assert_eq!(ledger.summary().commands_completed, 1);
        assert_eq!(ledger.summary().observation_age_ticks.max, 2.0);
        ledger.consume(
            Outcome::Abandoned {
                obligation: Obligation::Command(id),
                reason: AbandonReason::DeviceLost,
            },
            &mut receipts,
            None,
        );
        assert!(
            matches!(
                receipts.get(id).map(|receipt| receipt.state),
                Some(crate::receipts::ReceiptState::Completed { .. })
            ),
            "a completed command stays completed"
        );
    }

    #[test]
    fn residency_reservations_bound_outstanding_requests() {
        let mut ledger = OutcomeLedger::new(0, 1);
        assert!(ledger.reserve_residency());
        assert!(!ledger.reserve_residency());
        ledger.release_residency();
        assert!(ledger.reserve_residency());
    }
}
