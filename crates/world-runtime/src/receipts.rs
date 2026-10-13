//! Command receipts: admission, retries, conflicts and bounded retention.
//!
//! A receipt is reserved when a command is admitted, before anything can consume it, so an
//! accepted command always has somewhere to record its outcome. The table has a fixed
//! capacity; when every receipt belongs to an unacknowledged command, admission is refused
//! with [`Pressure::Receipts`] instead of forgetting an older outcome.
//!
//! Identity rules within one session:
//! - an identity at or above the floor that is not in the table is admitted;
//! - the same identity with the same command returns the existing receipt and admits nothing,
//!   so a retry never applies a command twice;
//! - the same identity with a different command is a conflict;
//! - identities below the floor have expired. Acknowledging receipts through an identity
//!   raises the floor past it, which is what lets the table forget them.

use std::collections::HashMap;

use world_model::command::{Command, CommandKind};

use crate::{
    ids::{CommandId, Tick},
    port::{AbandonReason, CommandResult, Pressure, SupportedCommands},
};

/// Where a command stands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReceiptState {
    /// Admitted; waiting for a tick.
    Pending,
    /// Encoded into `tick`'s submission.
    Submitted { tick: Tick },
    /// Applied in `tick`, and observed by the CPU.
    Completed { tick: Tick, result: CommandResult },
    /// Will never complete in this session.
    Abandoned { reason: AbandonReason },
}

impl ReceiptState {
    /// Whether the receipt holds its final state and may be acknowledged.
    pub fn is_final(&self) -> bool {
        matches!(self, Self::Completed { .. } | Self::Abandoned { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Receipt {
    pub id: CommandId,
    pub command: Command,
    pub state: ReceiptState,
    /// The last tick submitted before admission; the command can apply no earlier than the
    /// tick after it.
    pub admitted_after: Option<Tick>,
    /// The runtime's last submitted tick when the outcome was consumed.
    pub observed_at: Option<Tick>,
}

impl Receipt {
    /// Ticks from admission to application, counting the tick it applied in.
    pub fn input_latency_ticks(&self) -> Option<u64> {
        let ReceiptState::Completed { tick, .. } = self.state else {
            return None;
        };
        let after = self.admitted_after.map_or(0, |after| after.value() + 1);
        Some(tick.value().saturating_sub(after) + 1)
    }

    /// Ticks between application and the CPU observing the outcome.
    pub fn observation_age_ticks(&self) -> Option<u64> {
        match (self.state, self.observed_at) {
            (ReceiptState::Completed { tick, .. }, Some(observed)) => Some(observed.since(tick)),
            _ => None,
        }
    }
}

/// The result of admitting a command that was not refused.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Admission {
    Admitted,
    /// The identity is already held by the same command; nothing new was admitted.
    Duplicate(ReceiptState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionError {
    /// The identity is below the acknowledged floor.
    Expired {
        floor: CommandId,
    },
    /// The identity is held by a different command.
    Conflict(CommandId),
    NotSupported(CommandKind),
    Pressure(Pressure),
    /// The session accepts no more commands (stopped, lost or faulted).
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcknowledgeError {
    /// `id` has not reached a final state; the floor cannot pass it.
    Incomplete(CommandId),
}

/// The bounded receipt table of one session.
#[derive(Debug)]
pub struct Receipts {
    entries: Vec<Option<Receipt>>,
    by_id: HashMap<CommandId, u32>,
    free: Vec<u32>,
    floor: CommandId,
    /// Outcomes that named no receipt; nonzero means a defect in the port.
    unknown_outcomes: u64,
}

impl Receipts {
    pub fn new(capacity: u32) -> Self {
        let capacity = capacity.max(1);
        Self {
            entries: vec![None; capacity as usize],
            by_id: HashMap::with_capacity(capacity as usize),
            free: (0..capacity).rev().collect(),
            floor: CommandId::new(0),
            unknown_outcomes: 0,
        }
    }

    pub fn capacity(&self) -> u32 {
        self.entries.len() as u32
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn floor(&self) -> CommandId {
        self.floor
    }

    pub fn unknown_outcomes(&self) -> u64 {
        self.unknown_outcomes
    }

    pub fn get(&self, id: CommandId) -> Option<&Receipt> {
        let index = *self.by_id.get(&id)?;
        self.entries[index as usize].as_ref()
    }

    /// Classifies an admission without changing anything: `Ok(None)` means a new receipt is
    /// needed and a free entry exists.
    pub fn check(
        &self,
        id: CommandId,
        command: &Command,
        supported: SupportedCommands,
    ) -> Result<Option<ReceiptState>, AdmissionError> {
        if id < self.floor {
            return Err(AdmissionError::Expired { floor: self.floor });
        }
        if let Some(existing) = self.get(id) {
            return if existing.command == *command {
                Ok(Some(existing.state))
            } else {
                Err(AdmissionError::Conflict(id))
            };
        }
        if !supported.supports(command.kind()) {
            return Err(AdmissionError::NotSupported(command.kind()));
        }
        if self.free.is_empty() {
            return Err(AdmissionError::Pressure(Pressure::Receipts));
        }
        Ok(None)
    }

    /// Inserts a pending receipt. The caller has run [`check`](Self::check) and found that a
    /// new receipt is needed.
    pub fn insert(
        &mut self,
        id: CommandId,
        command: Command,
        admitted_after: Option<Tick>,
    ) -> Result<(), AdmissionError> {
        let index = self
            .free
            .pop()
            .ok_or(AdmissionError::Pressure(Pressure::Receipts))?;
        self.entries[index as usize] = Some(Receipt {
            id,
            command,
            state: ReceiptState::Pending,
            admitted_after,
            observed_at: None,
        });
        self.by_id.insert(id, index);
        Ok(())
    }

    pub fn mark_submitted(&mut self, id: CommandId, tick: Tick) {
        if let Some(receipt) = self.entry_mut(id) {
            receipt.state = ReceiptState::Submitted { tick };
        } else {
            self.unknown_outcomes += 1;
        }
    }

    /// Records a command's outcome. Returns the completed receipt, or `None` when no receipt
    /// held the identity (counted as an unknown outcome).
    pub fn complete(
        &mut self,
        id: CommandId,
        tick: Tick,
        result: CommandResult,
        observed_at: Option<Tick>,
    ) -> Option<&Receipt> {
        let Some(index) = self.by_id.get(&id).copied() else {
            self.unknown_outcomes += 1;
            return None;
        };
        let receipt = self.entries[index as usize].as_mut()?;
        receipt.state = ReceiptState::Completed { tick, result };
        receipt.observed_at = observed_at;
        Some(receipt)
    }

    pub fn abandon(&mut self, id: CommandId, reason: AbandonReason) {
        if let Some(receipt) = self.entry_mut(id) {
            if !receipt.state.is_final() {
                receipt.state = ReceiptState::Abandoned { reason };
            }
        } else {
            self.unknown_outcomes += 1;
        }
    }

    /// Releases every receipt up to and including `through` and raises the floor past it.
    /// Refused, without releasing anything, while one of them is not final.
    pub fn acknowledge(&mut self, through: CommandId) -> Result<usize, AcknowledgeError> {
        if through < self.floor {
            return Ok(0);
        }
        if let Some(incomplete) = self
            .entries
            .iter()
            .flatten()
            .filter(|receipt| receipt.id <= through && !receipt.state.is_final())
            .map(|receipt| receipt.id)
            .min()
        {
            return Err(AcknowledgeError::Incomplete(incomplete));
        }
        let mut released = 0;
        for (index, entry) in self.entries.iter_mut().enumerate() {
            if entry.is_some_and(|receipt| receipt.id <= through) {
                if let Some(receipt) = entry.take() {
                    self.by_id.remove(&receipt.id);
                }
                self.free.push(index as u32);
                released += 1;
            }
        }
        self.floor = CommandId::new(through.value().saturating_add(1));
        Ok(released)
    }

    /// Every receipt that is not final, for abandoning them when the session ends.
    pub fn unfinished(&self) -> impl Iterator<Item = CommandId> + '_ {
        self.entries
            .iter()
            .flatten()
            .filter(|receipt| !receipt.state.is_final())
            .map(|receipt| receipt.id)
    }

    fn entry_mut(&mut self, id: CommandId) -> Option<&mut Receipt> {
        let index = *self.by_id.get(&id)?;
        self.entries[index as usize].as_mut()
    }
}

#[cfg(test)]
mod tests {
    use world_model::{command::EditTool, command::MaterialEdit, glam::IVec2};

    use super::*;

    fn edit(strength: u8) -> Command {
        Command::EditMaterial(
            MaterialEdit::new(EditTool::Raise, IVec2::new(1, 2), 3, strength).expect("valid"),
        )
    }

    fn supported() -> SupportedCommands {
        SupportedCommands::default().with(CommandKind::EditMaterial)
    }

    fn admit(
        receipts: &mut Receipts,
        id: u64,
        command: Command,
    ) -> Result<Admission, AdmissionError> {
        let id = CommandId::new(id);
        match receipts.check(id, &command, supported())? {
            Some(state) => Ok(Admission::Duplicate(state)),
            None => {
                receipts.insert(id, command, None)?;
                Ok(Admission::Admitted)
            }
        }
    }

    #[test]
    fn a_retry_is_a_duplicate_and_reuse_with_another_command_conflicts() {
        let mut receipts = Receipts::new(4);
        assert_eq!(admit(&mut receipts, 7, edit(1)), Ok(Admission::Admitted));
        assert_eq!(
            admit(&mut receipts, 7, edit(1)),
            Ok(Admission::Duplicate(ReceiptState::Pending))
        );
        assert_eq!(
            admit(&mut receipts, 7, edit(2)),
            Err(AdmissionError::Conflict(CommandId::new(7)))
        );
        assert_eq!(receipts.len(), 1, "a retry reserves nothing new");
    }

    #[test]
    fn unsupported_kinds_are_refused_before_a_receipt_is_taken() {
        let mut receipts = Receipts::new(2);
        let query = Command::QueryColumn {
            cell: IVec2::new(0, 0),
        };
        assert_eq!(
            admit(&mut receipts, 1, query),
            Err(AdmissionError::NotSupported(CommandKind::QueryColumn))
        );
        assert!(receipts.is_empty());
    }

    #[test]
    fn a_full_table_refuses_until_final_receipts_are_acknowledged() {
        let mut receipts = Receipts::new(2);
        admit(&mut receipts, 1, edit(1)).expect("first");
        admit(&mut receipts, 2, edit(1)).expect("second");
        assert_eq!(
            admit(&mut receipts, 3, edit(1)),
            Err(AdmissionError::Pressure(Pressure::Receipts))
        );
        assert_eq!(
            receipts.acknowledge(CommandId::new(1)),
            Err(AcknowledgeError::Incomplete(CommandId::new(1))),
            "an outcome nobody has seen cannot be forgotten"
        );
        let result = CommandResult::Edited {
            changed: 3,
            not_resident: 0,
        };
        receipts.mark_submitted(CommandId::new(1), Tick::new(4));
        receipts.complete(CommandId::new(1), Tick::new(4), result, Some(Tick::new(6)));
        assert_eq!(receipts.acknowledge(CommandId::new(1)), Ok(1));
        assert_eq!(receipts.floor(), CommandId::new(2));
        assert_eq!(admit(&mut receipts, 3, edit(1)), Ok(Admission::Admitted));
        assert_eq!(
            admit(&mut receipts, 1, edit(1)),
            Err(AdmissionError::Expired {
                floor: CommandId::new(2)
            }),
            "an acknowledged identity cannot apply again"
        );
    }

    #[test]
    fn latencies_count_ticks_from_admission_to_observation() {
        let mut receipts = Receipts::new(1);
        receipts
            .insert(CommandId::new(5), edit(1), Some(Tick::new(9)))
            .expect("room");
        let result = CommandResult::Edited {
            changed: 1,
            not_resident: 0,
        };
        let receipt = receipts
            .complete(
                CommandId::new(5),
                Tick::new(10),
                result,
                Some(Tick::new(12)),
            )
            .copied()
            .expect("receipt");
        assert_eq!(receipt.input_latency_ticks(), Some(1));
        assert_eq!(receipt.observation_age_ticks(), Some(2));
        receipts.complete(CommandId::new(99), Tick::new(10), result, None);
        assert_eq!(receipts.unknown_outcomes(), 1);
    }
}
