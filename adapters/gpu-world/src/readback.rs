//! Decoding a tick's readback into outcomes, with identity and consistency checks.
//!
//! A tick readback holds the tick's result records, the cosmetic event buffer, the tick's
//! diagnostics record, the totals records of its totals commands and, when enabled, a begin and
//! an end timestamp per compute pass. Decoding checks every record against what the CPU encoded: the
//! command identity, its position and the tick. A mismatch means a layout error or a result
//! that is not this tick's, and faults the session instead of reporting a wrong outcome. The
//! event header must account for every dropped append; a header whose drops do not match its
//! overflow is the "missing overflow signal" defect and is rejected the same way.

use std::time::Duration;

use world_model::{cell::WorldCell, command::CommandKind, observation::WorldTotals};
use world_runtime::{
    ids::{CommandId, Tick},
    port::{CellReport, CommandOutcome, CommandResult, CosmeticEvent, Outcome, PhaseTimes},
};

use crate::{
    commands::Expected,
    layout::{
        DiagnosticsRecord, EventHeader, EventRecord, GpuRecord, ResultRecord, TotalsRecord, codes,
        wide,
    },
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

/// Decodes `results` (one record per expected command) into command outcomes. `totals` holds
/// one totals record per command position; only totals commands' records are read.
pub fn decode_results(
    results: &[u8],
    totals: &[u8],
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
    for (position, (bytes, expected)) in results.chunks_exact(stride).zip(expected).enumerate() {
        let record = ResultRecord::read_bytes(bytes);
        let result = match (expected.kind, record.status) {
            (CommandKind::EditMaterial, _) => CommandResult::Edited {
                changed: record.changed,
                not_resident: record.not_resident + expected.outside_window,
            },
            (_, codes::RESULT_CONFIGURED) => CommandResult::Configured,
            (_, codes::RESULT_TOTALS) => {
                let at = position * TotalsRecord::BYTES as usize;
                CommandResult::Totals(decode_totals(
                    &totals[at..at + TotalsRecord::BYTES as usize],
                ))
            }
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
        CommandKind::SetAmbientTemperature | CommandKind::SetSimulationRules => {
            record.status == codes::RESULT_CONFIGURED
        }
        CommandKind::MeasureTotals => record.status == codes::RESULT_TOTALS,
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

/// A totals record as [`WorldTotals`].
pub fn decode_totals(bytes: &[u8]) -> WorldTotals {
    let record = TotalsRecord::read_bytes(bytes);
    let mut words = [0u32; TotalsRecord::WORDS];
    record.write_words(&mut words);
    let pair = |index: usize| wide(words[2 * index], words[2 * index + 1]);
    WorldTotals {
        fluid: std::array::from_fn(pair),
        elevation: pair(8),
        granular: pair(9),
        cells: pair(10),
    }
}

/// The tick's diagnostics record as a [`CellReport`].
pub fn decode_diagnostics(bytes: &[u8]) -> CellReport {
    let r = DiagnosticsRecord::read_bytes(bytes);
    CellReport {
        fluid_in: wide(r.fluid_in_lo, r.fluid_in_hi),
        fluid_out: wide(r.fluid_out_lo, r.fluid_out_hi),
        solid_in: wide(r.solid_in_lo, r.solid_in_hi),
        solid_out: wide(r.solid_out_lo, r.solid_out_hi),
        quench: wide(r.quench_lo, r.quench_hi),
        boil: wide(r.boil_lo, r.boil_hi),
        burn: wide(r.burn_lo, r.burn_hi),
        uptake: wide(r.uptake_lo, r.uptake_hi),
        erosion: wide(r.erosion_lo, r.erosion_hi),
        tool_fluid_in: wide(r.tool_fluid_in_lo, r.tool_fluid_in_hi),
        tool_solid_in: wide(r.tool_solid_in_lo, r.tool_solid_in_hi),
        tool_solid_out: wide(r.tool_solid_out_lo, r.tool_solid_out_hi),
        candidate_tiles: r.candidate_tiles,
        process_tiles: r.process_tiles,
        active_tiles: r.active_tiles,
        awake_cells: r.awake_cells,
        active_cells: r.active_cells,
        changed_cells: r.changed_cells,
        rejected_transfers: r.rejected_transfers,
        stale_flow: r.stale_flow,
        receive_mismatches: r.receive_mismatches,
    }
}

fn timestamp(bytes: &[u8], index: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(index * 8..index * 8 + 8)?.try_into().ok()?,
    ))
}

fn span(begin: u64, end: u64, period_ns: f32) -> Option<Duration> {
    if end < begin {
        return None;
    }
    let nanoseconds = (end - begin) as f64 * f64::from(period_ns);
    Some(Duration::from_nanos(nanoseconds.round() as u64))
}

/// The tick's device time and its three passes' times from the resolved timestamps (begin and
/// end of each pass), or `None` when they are not ordered (a device may report unusable values;
/// they are not turned into a measurement).
pub fn decode_timestamps(bytes: &[u8], period_ns: f32) -> Option<(Duration, PhaseTimes)> {
    let mut stamps = [0u64; 6];
    for (index, stamp) in stamps.iter_mut().enumerate() {
        *stamp = timestamp(bytes, index)?;
    }
    if stamps.windows(2).any(|pair| pair[1] < pair[0]) {
        return None;
    }
    Some((
        span(stamps[0], stamps[5], period_ns)?,
        PhaseTimes {
            commands: span(stamps[0], stamps[1], period_ns)?,
            activity: span(stamps[2], stamps[3], period_ns)?,
            material: span(stamps[4], stamps[5], period_ns)?,
        },
    ))
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
        bytes.extend(record(12, 2, 4, codes::RESULT_CONFIGURED));
        bytes.extend(record(13, 3, 4, codes::RESULT_TOTALS));
        // Totals of position 3: water 5 + 2^32 * 3, elevation 2^40, granular 9, cells 7.
        let mut totals = vec![0u8; 4 * TotalsRecord::BYTES as usize];
        let record3 = TotalsRecord {
            fluid1_lo: 5,
            fluid1_hi: 3,
            elevation_hi: 1 << 8,
            granular_lo: 9,
            cells_lo: 7,
            ..TotalsRecord::default()
        };
        record3.write_bytes(&mut totals[3 * TotalsRecord::BYTES as usize..]);
        let mut out = Vec::new();
        decode_results(
            &bytes,
            &totals,
            &[
                expected(10, CommandKind::EditMaterial),
                expected(11, CommandKind::QueryColumn),
                expected(12, CommandKind::SetSimulationRules),
                expected(13, CommandKind::MeasureTotals),
            ],
            Tick::new(4),
            &mut out,
        )
        .expect("consistent");
        let result = |index: usize| match out[index] {
            Outcome::Command(outcome) => outcome.result,
            other => panic!("{other:?}"),
        };
        assert_eq!(result(2), CommandResult::Configured);
        let CommandResult::Totals(world) = result(3) else {
            panic!("totals: {:?}", result(3));
        };
        assert_eq!(world.fluid[1], 5 + (3 << 32));
        assert_eq!(world.fluid_volume(), 5 + (3 << 32));
        assert_eq!(
            (world.elevation, world.granular, world.cells),
            (1 << 40, 9, 7)
        );
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
            decode_results(&bytes, &[], &both, Tick::new(4), &mut out),
            Err(DecodeError::Identity { position: 1, .. })
        ));
        assert!(out.is_empty());
        let stale = record(10, 0, 3, codes::RESULT_EDITED);
        assert!(matches!(
            decode_results(&stale, &[], &both[..1], Tick::new(4), &mut out),
            Err(DecodeError::WrongTick { .. })
        ));
        let unanswered = record(11, 0, 4, codes::RESULT_PENDING);
        assert!(matches!(
            decode_results(
                &unanswered,
                &[],
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
        let mut bytes = [0u8; 48];
        for (index, stamp) in [1_000u64, 1_400, 1_500, 1_600, 2_000, 3_000]
            .iter()
            .enumerate()
        {
            bytes[index * 8..index * 8 + 8].copy_from_slice(&stamp.to_le_bytes());
        }
        assert_eq!(
            decode_timestamps(&bytes, 1.5),
            Some((
                Duration::from_nanos(3_000),
                PhaseTimes {
                    commands: Duration::from_nanos(600),
                    activity: Duration::from_nanos(150),
                    material: Duration::from_nanos(1_500),
                }
            ))
        );
        bytes[40..].copy_from_slice(&500u64.to_le_bytes());
        assert_eq!(decode_timestamps(&bytes, 1.0), None);
    }

    #[test]
    fn diagnostics_carry_every_wrap_into_their_high_words() {
        let record = DiagnosticsRecord {
            fluid_out_lo: 0xFFFF_FFFF,
            fluid_out_hi: 2,
            quench_lo: 7,
            stale_flow: 1,
            process_tiles: 12,
            ..DiagnosticsRecord::default()
        };
        let mut bytes = vec![0u8; DiagnosticsRecord::BYTES as usize];
        record.write_bytes(&mut bytes);
        let report = decode_diagnostics(&bytes);
        assert_eq!(report.fluid_out, 0x2_FFFF_FFFF);
        assert_eq!(report.quench, 7);
        assert_eq!(report.process_tiles, 12);
        assert!(!report.consistent());
    }
}
