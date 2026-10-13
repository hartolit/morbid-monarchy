//! Decoding a tick's readback into outcomes, with identity and consistency checks.
//!
//! A tick readback holds the tick's result records, the cosmetic event buffer and, when
//! enabled, a timestamp pair. Decoding checks every record against what the CPU encoded: the
//! command identity, its position and the tick. A mismatch means a layout error or a result
//! that is not this tick's, and faults the session instead of reporting a wrong outcome. The
//! event header must account for every dropped append; a header whose drops do not match its
//! overflow is the "missing overflow signal" defect and is rejected the same way.

use std::time::Duration;

use world_model::{cell::WorldCell, command::CommandKind};
use world_runtime::{
    ids::{CommandId, Tick},
    port::{CommandOutcome, CommandResult, CosmeticEvent, Outcome},
};

use crate::{
    commands::Expected,
    layout::{EventHeader, EventRecord, GpuRecord, ResultRecord, codes},
};

/// Why a readback cannot be trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// Record `position` names another command, position or tick.
    Identity {
        position: u32,
        expected: CommandId,
        found: CommandId,
    },
    WrongTick {
        position: u32,
        found: Tick,
    },
    /// The record's status is not one its command kind produces.
    Status {
        position: u32,
        status: u32,
    },
    /// The event header's drops do not account for its overflow.
    EventAccounting {
        count: u32,
        dropped: u32,
        capacity: u32,
    },
}

/// Decodes `results` (one record per expected command) into command outcomes.
pub fn decode_results(
    results: &[u8],
    expected: &[Expected],
    tick: Tick,
    out: &mut Vec<Outcome>,
) -> Result<(), DecodeError> {
    let stride = ResultRecord::BYTES as usize;
    // Check every record before reporting any, so a bad tick reports nothing partial.
    for (position, (bytes, expected)) in results.chunks_exact(stride).zip(expected).enumerate() {
        check(
            position as u32,
            &ResultRecord::read_bytes(bytes),
            expected,
            tick,
        )?;
    }
    for (bytes, expected) in results.chunks_exact(stride).zip(expected) {
        let record = ResultRecord::read_bytes(bytes);
        let result = match (expected.kind, record.status) {
            (CommandKind::EditMaterial, _) => CommandResult::Edited {
                changed: record.changed,
                not_resident: record.not_resident + expected.outside_window,
            },
            (_, codes::RESULT_COLUMN) => CommandResult::Column {
                cell: expected.cell,
                column: WorldCell::from_words([record.word0, record.word1]),
            },
            _ => CommandResult::ColumnNotResident {
                cell: expected.cell,
            },
        };
        out.push(Outcome::Command(CommandOutcome {
            id: expected.id,
            tick,
            result,
        }));
    }
    Ok(())
}

fn check(
    position: u32,
    record: &ResultRecord,
    expected: &Expected,
    tick: Tick,
) -> Result<(), DecodeError> {
    let found = CommandId::from_words([record.id_lo, record.id_hi]);
    if found != expected.id || record.sequence != position {
        return Err(DecodeError::Identity {
            position,
            expected: expected.id,
            found,
        });
    }
    let found_tick = Tick::from_words([record.tick_lo, record.tick_hi]);
    if found_tick != tick {
        return Err(DecodeError::WrongTick {
            position,
            found: found_tick,
        });
    }
    let valid = match expected.kind {
        CommandKind::EditMaterial => record.status == codes::RESULT_EDITED,
        CommandKind::QueryColumn => matches!(
            record.status,
            codes::RESULT_COLUMN | codes::RESULT_COLUMN_NOT_RESIDENT
        ),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(DecodeError::Status {
            position,
            status: record.status,
        })
    }
}

/// Checks that an event header accounts for every append: a full buffer counts each dropped
/// append, an unfilled one drops none.
pub fn check_event_header(header: &EventHeader, capacity: u32) -> Result<(), DecodeError> {
    let expected_dropped = header.count.saturating_sub(capacity);
    if header.capacity != capacity || header.dropped != expected_dropped {
        return Err(DecodeError::EventAccounting {
            count: header.count,
            dropped: header.dropped,
            capacity: header.capacity,
        });
    }
    Ok(())
}

/// Decodes the event buffer (header then records) into cosmetic outcomes and an overflow
/// outcome when appends were dropped. Returns the number dropped.
pub fn decode_events(
    events: &[u8],
    capacity: u32,
    tick: Tick,
    out: &mut Vec<Outcome>,
) -> Result<u32, DecodeError> {
    let header = EventHeader::read_bytes(&events[..EventHeader::BYTES as usize]);
    check_event_header(&header, capacity)?;
    let recorded = header.count.min(capacity) as usize;
    let records = &events[EventHeader::BYTES as usize..];
    const EVENT_BYTES: usize = EventRecord::BYTES as usize;
    let (records, _) = records.as_chunks::<EVENT_BYTES>();
    for bytes in records.iter().take(recorded) {
        let record = EventRecord::read_bytes(bytes);
        out.push(Outcome::Cosmetic(CosmeticEvent {
            tick,
            kind: record.kind,
            data: [record.a, record.b, record.c],
        }));
    }
    if header.dropped > 0 {
        out.push(Outcome::CosmeticOverflow {
            tick,
            dropped: header.dropped,
        });
    }
    Ok(header.dropped)
}

/// The duration between a resolved timestamp pair, or `None` when the pair is not ordered (a
/// device may report unusable values; they are not turned into a measurement).
pub fn decode_timestamps(bytes: &[u8], period_ns: f32) -> Option<Duration> {
    let begin = u64::from_le_bytes(bytes.get(0..8)?.try_into().ok()?);
    let end = u64::from_le_bytes(bytes.get(8..16)?.try_into().ok()?);
    if end < begin {
        return None;
    }
    let nanoseconds = (end - begin) as f64 * f64::from(period_ns);
    Some(Duration::from_nanos(nanoseconds.round() as u64))
}

#[cfg(test)]
mod tests {
    use world_model::glam::IVec2;

    use super::*;

    fn record(id: u64, sequence: u32, tick: u64, status: u32) -> Vec<u8> {
        let [id_lo, id_hi] = CommandId::new(id).to_words();
        let [tick_lo, tick_hi] = Tick::new(tick).to_words();
        let record = ResultRecord {
            id_lo,
            id_hi,
            sequence,
            status,
            tick_lo,
            tick_hi,
            changed: 7,
            not_resident: 2,
            word0: 0x89AB_CDEF,
            word1: 0x0123_4567,
            slot_index: 0,
            slot_generation: 1,
        };
        let mut bytes = vec![0u8; ResultRecord::BYTES as usize];
        record.write_bytes(&mut bytes);
        bytes
    }

    fn expected(id: u64, kind: CommandKind) -> Expected {
        Expected {
            id: CommandId::new(id),
            kind,
            cell: IVec2::new(3, -4),
            outside_window: 5,
        }
    }

    #[test]
    fn results_decode_to_outcomes_with_cpu_side_counts() {
        let mut bytes = record(10, 0, 4, codes::RESULT_EDITED);
        bytes.extend(record(11, 1, 4, codes::RESULT_COLUMN));
        let mut out = Vec::new();
        decode_results(
            &bytes,
            &[
                expected(10, CommandKind::EditMaterial),
                expected(11, CommandKind::QueryColumn),
            ],
            Tick::new(4),
            &mut out,
        )
        .expect("consistent");
        assert_eq!(
            out[0],
            Outcome::Command(CommandOutcome {
                id: CommandId::new(10),
                tick: Tick::new(4),
                result: CommandResult::Edited {
                    changed: 7,
                    not_resident: 7
                },
            })
        );
        let Outcome::Command(CommandOutcome {
            result: CommandResult::Column { cell, column },
            ..
        }) = out[1]
        else {
            panic!("a column: {:?}", out[1]);
        };
        assert_eq!(cell, IVec2::new(3, -4));
        assert_eq!(column, WorldCell(0x0123_4567_89AB_CDEF));
    }

    #[test]
    fn a_result_from_another_command_or_tick_is_rejected_without_partial_outcomes() {
        let mut bytes = record(10, 0, 4, codes::RESULT_EDITED);
        bytes.extend(record(99, 1, 4, codes::RESULT_EDITED));
        let both = [
            expected(10, CommandKind::EditMaterial),
            expected(11, CommandKind::EditMaterial),
        ];
        let mut out = Vec::new();
        assert!(matches!(
            decode_results(&bytes, &both, Tick::new(4), &mut out),
            Err(DecodeError::Identity { position: 1, .. })
        ));
        assert!(out.is_empty());
        let stale = record(10, 0, 3, codes::RESULT_EDITED);
        assert!(matches!(
            decode_results(&stale, &both[..1], Tick::new(4), &mut out),
            Err(DecodeError::WrongTick { .. })
        ));
        let unanswered = record(11, 0, 4, codes::RESULT_PENDING);
        assert!(matches!(
            decode_results(
                &unanswered,
                &[expected(11, CommandKind::QueryColumn)],
                Tick::new(4),
                &mut out
            ),
            Err(DecodeError::Status { .. })
        ));
    }

    fn events(count: u32, dropped: u32, capacity: u32) -> Vec<u8> {
        let header = EventHeader {
            count,
            dropped,
            capacity,
            reserved0: 0,
        };
        let mut bytes =
            vec![0u8; (EventHeader::BYTES + u64::from(capacity) * EventRecord::BYTES) as usize];
        header.write_bytes(&mut bytes[..16]);
        for index in 0..count.min(capacity) as usize {
            let record = EventRecord {
                kind: codes::EVENT_INJECTED,
                a: index as u32,
                b: 0,
                c: 0,
            };
            record.write_bytes(&mut bytes[16 + index * 16..32 + index * 16]);
        }
        bytes
    }

    #[test]
    fn overflowing_events_report_their_drops() {
        let mut out = Vec::new();
        assert_eq!(
            decode_events(&events(6, 2, 4), 4, Tick::new(1), &mut out),
            Ok(2)
        );
        assert_eq!(out.len(), 5);
        assert_eq!(
            out[4],
            Outcome::CosmeticOverflow {
                tick: Tick::new(1),
                dropped: 2
            }
        );
        out.clear();
        assert_eq!(
            decode_events(&events(0, 0, 4), 4, Tick::new(1), &mut out),
            Ok(0)
        );
        assert!(out.is_empty());
    }

    #[test]
    fn a_missing_overflow_signal_is_rejected() {
        // Deliberate failure specimen: six appends into four places, no drop recorded.
        let mut out = Vec::new();
        assert_eq!(
            decode_events(&events(6, 0, 4), 4, Tick::new(1), &mut out),
            Err(DecodeError::EventAccounting {
                count: 6,
                dropped: 0,
                capacity: 4
            })
        );
        assert!(out.is_empty());
    }

    #[test]
    fn timestamps_convert_with_the_period_and_refuse_unordered_pairs() {
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&1_000u64.to_le_bytes());
        bytes[8..].copy_from_slice(&3_000u64.to_le_bytes());
        assert_eq!(
            decode_timestamps(&bytes, 1.5),
            Some(Duration::from_nanos(3_000))
        );
        bytes[8..].copy_from_slice(&500u64.to_le_bytes());
        assert_eq!(decode_timestamps(&bytes, 1.0), None);
    }
}
