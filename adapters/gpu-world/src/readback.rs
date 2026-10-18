//! Decoding a tick's readback into outcomes, with identity and consistency checks.
//!
//! A tick readback holds the tick's result records, the cosmetic event buffer, the tick's
//! diagnostics record, the totals records of its totals commands, the body substeps' record,
//! the selected body samples and, when enabled, a begin and an end timestamp per compute pass.
//! Window-local results (hit cells, stroke centres, target points) become absolute values here,
//! with the window origin the tick was encoded against. Decoding checks every record against what the CPU encoded: the
//! command identity, its position and the tick. A mismatch means a layout error or a result
//! that is not this tick's, and faults the session instead of reporting a wrong outcome. The
//! event header must account for every dropped append; a header whose drops do not match its
//! overflow is the "missing overflow signal" defect and is rejected the same way.

use std::time::Duration;

use world_model::{
    body::BodyHandle,
    cell::WorldCell,
    command::CommandKind,
    glam::{DVec3, IVec2, Vec3},
    material::FluidMat,
    observation::{BodyCounters, BodyHit, BodySample, SurfaceHit, WorldTotals},
    spatial::CHUNK_SIZE,
};
use world_runtime::{
    ids::{CommandId, Tick},
    port::{
        BodyReport, CellReport, CommandOutcome, CommandResult, CosmeticEvent, Outcome, PhaseTimes,
    },
};

use crate::{
    commands::Expected,
    layout::{
        BodyDiagnosticsRecord, BodySampleRecord, DiagnosticsRecord, EventHeader, EventRecord,
        GpuRecord, ResultRecord, TotalsRecord, codes, wide,
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
    /// Body sample `position` names another handle than the one selected there.
    Sample {
        position: u32,
    },
    /// Record `position`'s ray result names no surface outcome, or a body slot beyond the
    /// capacity.
    RayResult {
        position: u32,
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
        let at = position * TotalsRecord::BYTES as usize;
        let result = decode_result(
            &record,
            expected,
            &totals[at..at + TotalsRecord::BYTES as usize],
        );
        out.push(Outcome::Command(CommandOutcome {
            id: expected.id,
            tick,
            result,
        }));
    }
    Ok(())
}

fn f32_word(word: u32) -> f32 {
    f32::from_bits(word)
}

/// Where a ray went, from its result's `v0`–`v6`.
fn decode_ray(record: &ResultRecord, expected: &Expected) -> (SurfaceHit, Option<BodyHit>) {
    let point = |distance: f32| {
        expected.ray.map_or(DVec3::ZERO, |ray| {
            ray.origin() + ray.direction().as_dvec3() * f64::from(distance)
        })
    };
    let distance = f32_word(record.v3);
    let surface = match record.v0 {
        codes::SURFACE_HIT => SurfaceHit::Hit {
            cell: expected.absolute_cell(IVec2::new(record.v1 as i32, record.v2 as i32)),
            position: point(distance),
            distance,
        },
        codes::SURFACE_NOT_RESIDENT => SurfaceHit::NotResident { distance },
        _ => SurfaceHit::Miss,
    };
    let body = (record.v4 != codes::BODY_NONE).then(|| {
        let distance = f32_word(record.v6);
        BodyHit {
            body: BodyHandle::new(record.v4, record.v5),
            position: point(distance),
            distance,
        }
    });
    (surface, body)
}

fn target(record: &ResultRecord, expected: &Expected) -> DVec3 {
    expected.absolute_point([
        f32_word(record.v7),
        f32_word(record.v8),
        f32_word(record.v9),
    ])
}

fn stale(expected: &Expected) -> CommandResult {
    CommandResult::StaleBody {
        body: expected.body.unwrap_or(BodyHandle::new(u32::MAX, 0)),
    }
}

fn decode_result(record: &ResultRecord, expected: &Expected, totals: &[u8]) -> CommandResult {
    let status = record.status;
    match status {
        codes::RESULT_TARGET_MISSED => {
            return CommandResult::TargetMissed(decode_ray(record, expected).0);
        }
        codes::RESULT_TARGET_NOT_RESIDENT => {
            return CommandResult::TargetNotResident {
                cell: expected.cell,
            };
        }
        codes::RESULT_STALE_BODY => return stale(expected),
        codes::RESULT_BODY_CAPACITY => {
            return CommandResult::BodyCapacity {
                capacity: expected.body_capacity,
            };
        }
        _ => {}
    }
    let body = expected.body.unwrap_or(BodyHandle::new(u32::MAX, 0));
    match expected.kind {
        CommandKind::EditMaterial => CommandResult::Edited {
            center: if expected.outside_window > 0 {
                expected.cell
            } else {
                expected.absolute_cell(IVec2::new(record.word0 as i32, record.word1 as i32))
            },
            changed: record.changed,
            not_resident: record.not_resident + expected.outside_window,
        },
        CommandKind::SetAmbientTemperature
        | CommandKind::SetSimulationRules
        | CommandKind::SetElevationScale => CommandResult::Configured,
        CommandKind::MeasureTotals => CommandResult::Totals(decode_totals(totals)),
        CommandKind::QueryColumn if status == codes::RESULT_COLUMN => CommandResult::Column {
            cell: expected.cell,
            column: WorldCell::from_words([record.word0, record.word1]),
        },
        CommandKind::QueryColumn => CommandResult::ColumnNotResident {
            cell: expected.cell,
        },
        CommandKind::QueryRay => {
            let (surface, body) = decode_ray(record, expected);
            CommandResult::Ray { surface, body }
        }
        CommandKind::SpawnBody => {
            let anchor = DVec3::new(
                f64::from(record.word0 as i32) * CHUNK_SIZE as f64,
                0.0,
                -f64::from(record.word1 as i32) * CHUNK_SIZE as f64,
            );
            CommandResult::BodySpawned {
                body,
                position: anchor
                    + DVec3::new(
                        f64::from(f32_word(record.v7)),
                        f64::from(f32_word(record.v9)),
                        -f64::from(f32_word(record.v8)),
                    ),
            }
        }
        CommandKind::RemoveBody => CommandResult::BodyRemoved {
            body,
            counters: BodyCounters {
                landings: record.word0,
                liquid_entries: record.word1,
                body_contacts: record.slot_index,
                craters: record.slot_generation,
                splashes: record.v7,
                absorbed_energy: record.v8,
            },
        },
        CommandKind::DriveBody => CommandResult::Driven { body },
        CommandKind::JumpBody => CommandResult::Jumped {
            body,
            jumped: record.count > 0,
        },
        CommandKind::AttractBodies => CommandResult::Attracted {
            bodies: record.count,
            target: target(record, expected),
        },
        CommandKind::LiftBodies => CommandResult::Lifted {
            bodies: record.count,
            target: target(record, expected),
        },
    }
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
    let ray = expected.ray.is_some();
    let body = |success: u32| record.status == success || record.status == codes::RESULT_STALE_BODY;
    let targeted = |success: u32| {
        record.status == success
            || (ray && record.status == codes::RESULT_TARGET_MISSED)
            || (!ray && record.status == codes::RESULT_TARGET_NOT_RESIDENT)
    };
    let valid = match expected.kind {
        CommandKind::EditMaterial => targeted(codes::RESULT_EDITED),
        CommandKind::QueryColumn => matches!(
            record.status,
            codes::RESULT_COLUMN | codes::RESULT_COLUMN_NOT_RESIDENT
        ),
        CommandKind::SetAmbientTemperature
        | CommandKind::SetSimulationRules
        | CommandKind::SetElevationScale => record.status == codes::RESULT_CONFIGURED,
        CommandKind::MeasureTotals => record.status == codes::RESULT_TOTALS,
        CommandKind::QueryRay => record.status == codes::RESULT_RAY,
        CommandKind::SpawnBody => {
            record.status == codes::RESULT_BODY_CAPACITY
                || record.status == codes::RESULT_TARGET_NOT_RESIDENT
                || targeted(codes::RESULT_BODY_SPAWNED)
        }
        CommandKind::RemoveBody => body(codes::RESULT_BODY_REMOVED),
        CommandKind::DriveBody => body(codes::RESULT_DRIVEN),
        CommandKind::JumpBody => body(codes::RESULT_JUMPED),
        CommandKind::AttractBodies => targeted(codes::RESULT_ATTRACTED),
        CommandKind::LiftBodies => targeted(codes::RESULT_LIFTED),
    };
    if !valid {
        return Err(DecodeError::Status {
            position,
            status: record.status,
        });
    }
    // A resolved ray (answered, or used by a command that was not refused on the CPU) names
    // one surface outcome and at most a body inside the table.
    let resolved = record.status == codes::RESULT_RAY
        || (ray
            && record.status != codes::RESULT_STALE_BODY
            && record.status != codes::RESULT_BODY_CAPACITY);
    let surface = matches!(
        record.v0,
        codes::SURFACE_MISS | codes::SURFACE_HIT | codes::SURFACE_NOT_RESIDENT
    );
    let body = record.v4 == codes::BODY_NONE || record.v4 < expected.body_capacity;
    if resolved && !(surface && body) {
        return Err(DecodeError::RayResult { position });
    }
    Ok(())
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

/// The tick's device time and its four passes' times from the resolved timestamps (begin and
/// end of each pass), or `None` when they are not ordered (a device may report unusable values;
/// they are not turned into a measurement).
pub fn decode_timestamps(bytes: &[u8], period_ns: f32) -> Option<(Duration, PhaseTimes)> {
    let mut stamps = [0u64; 8];
    for (index, stamp) in stamps.iter_mut().enumerate() {
        *stamp = timestamp(bytes, index)?;
    }
    if stamps.windows(2).any(|pair| pair[1] < pair[0]) {
        return None;
    }
    Some((
        span(stamps[0], stamps[7], period_ns)?,
        PhaseTimes {
            commands: span(stamps[0], stamps[1], period_ns)?,
            activity: span(stamps[2], stamps[3], period_ns)?,
            material: span(stamps[4], stamps[5], period_ns)?,
            bodies: span(stamps[6], stamps[7], period_ns)?,
        },
    ))
}

/// The body substeps' record as a [`BodyReport`]; `substeps` comes from the CPU.
pub fn decode_body_diagnostics(bytes: &[u8], substeps: u32) -> BodyReport {
    let r = BodyDiagnosticsRecord::read_bytes(bytes);
    BodyReport {
        substeps,
        live: r.live,
        suspended: r.suspended,
        candidate_pairs: wide(r.candidate_pairs_lo, r.candidate_pairs_hi),
        contact_pairs: r.contact_pairs,
        swept_contacts: r.swept_contacts,
        terrain_contacts: r.terrain_contacts,
        residual_overlap: f32::from_bits(r.residual_overlap),
        residual_approach: f32::from_bits(r.residual_approach),
        speed_limited: r.speed_limited,
        swept_bodies: r.swept_bodies,
        fluid_in: wide(r.fluid_in_lo, r.fluid_in_hi),
        fluid_out: wide(r.fluid_out_lo, r.fluid_out_hi),
        solid_in: wide(r.solid_in_lo, r.solid_in_hi),
        solid_out: wide(r.solid_out_lo, r.solid_out_hi),
        compaction: wide(r.compaction_lo, r.compaction_hi),
        ice_broken: r.ice_broken,
        organics_cleared: r.organics_cleared,
        craters: r.craters,
        ruts: r.ruts,
        splashes: r.splashes,
        wakes: r.wakes,
        claims_lost: r.claims_lost,
    }
}

/// Decodes `count` body samples of `tick` into outcomes, checking each echoes the handle the
/// CPU selected at its position.
pub fn decode_samples(
    bytes: &[u8],
    selected: &[BodyHandle],
    tick: Tick,
    out: &mut Vec<Outcome>,
) -> Result<(), DecodeError> {
    const SAMPLE_BYTES: usize = BodySampleRecord::BYTES as usize;
    let (records, _) = bytes.as_chunks::<SAMPLE_BYTES>();
    for (position, (bytes, handle)) in records.iter().zip(selected).enumerate() {
        let r = BodySampleRecord::read_bytes(bytes);
        let found = BodyHandle::new(r.index, r.generation);
        if found != *handle {
            return Err(DecodeError::Sample {
                position: position as u32,
            });
        }
        match r.status {
            codes::SAMPLE_OK => {
                let size = CHUNK_SIZE as f64;
                let position = DVec3::new(
                    f64::from(r.anchor_x) * size + f64::from(r.local_x),
                    f64::from(r.height),
                    -(f64::from(r.anchor_y) * size + f64::from(r.local_y)),
                );
                out.push(Outcome::Body(BodySample {
                    body: found,
                    tick: tick.value(),
                    position,
                    velocity: Vec3::new(r.velocity_x, r.velocity_y, r.velocity_z),
                    ground_normal: Vec3::new(r.normal_x, r.normal_y, r.normal_z),
                    grounded: r.flags & codes::BODY_GROUNDED != 0,
                    suspended: r.flags & codes::BODY_SUSPENDED != 0,
                    submerged_fraction: r.submerged,
                    liquid: (r.liquid != u32::from(FluidMat::EMPTY.0))
                        .then_some(FluidMat(r.liquid as u8)),
                    touching: r.touching,
                    counters: BodyCounters {
                        landings: r.landings,
                        liquid_entries: r.liquid_entries,
                        body_contacts: r.body_contacts,
                        craters: r.craters,
                        splashes: r.splashes,
                        absorbed_energy: r.absorbed_energy,
                    },
                }));
            }
            codes::SAMPLE_GONE => out.push(Outcome::BodyGone { body: found, tick }),
            status => {
                return Err(DecodeError::Status {
                    position: position as u32,
                    status,
                });
            }
        }
    }
    Ok(())
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
            ..ResultRecord::default()
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
            body: None,
            ray: None,
            origin: (0, 0),
            body_capacity: 0,
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
                    center: IVec2::new(3, -4),
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
    fn body_results_become_absolute_and_samples_must_echo_their_handles() {
        use world_model::command::Ray;

        let body = BodyHandle::new(5, 2);
        let base = |id: u64, kind: CommandKind| Expected {
            body: Some(body),
            origin: (-128, 64),
            body_capacity: 8,
            ..expected(id, kind)
        };
        let ray = Ray::new(DVec3::new(-100.0, 50.0, -70.0), Vec3::NEG_Y, 200.0).expect("ray");
        let mut spawned = ResultRecord::read_bytes(&record(1, 0, 9, codes::RESULT_BODY_SPAWNED));
        spawned.word0 = (-2i32) as u32;
        spawned.word1 = 1;
        spawned.v7 = 10.5f32.to_bits();
        spawned.v8 = 3.0f32.to_bits();
        spawned.v9 = 21.0f32.to_bits();
        let mut hit = ResultRecord::read_bytes(&record(2, 1, 9, codes::RESULT_RAY));
        hit.v0 = codes::SURFACE_HIT;
        hit.v1 = 28;
        hit.v2 = 5;
        hit.v3 = 30.0f32.to_bits();
        hit.v4 = 5;
        hit.v5 = 2;
        hit.v6 = 12.0f32.to_bits();
        let mut removed = ResultRecord::read_bytes(&record(3, 2, 9, codes::RESULT_BODY_REMOVED));
        removed.word0 = 4;
        removed.v8 = u32::MAX;
        let stale = ResultRecord::read_bytes(&record(4, 3, 9, codes::RESULT_STALE_BODY));
        let mut bytes = vec![0u8; 4 * ResultRecord::BYTES as usize];
        for (index, record) in [spawned, hit, removed, stale].iter().enumerate() {
            record.write_bytes(&mut bytes[index * ResultRecord::BYTES as usize..]);
        }
        let totals = vec![0u8; 4 * TotalsRecord::BYTES as usize];
        let mut out = Vec::new();
        let ray_query = Expected {
            ray: Some(ray),
            ..base(2, CommandKind::QueryRay)
        };
        // A body slot beyond the table (an f32-rounded "none", say) is a decode error.
        let mut phantom = hit;
        phantom.v4 = 0xFFFF_FF00;
        let mut phantom_bytes = vec![0u8; ResultRecord::BYTES as usize];
        ResultRecord {
            sequence: 0,
            ..phantom
        }
        .write_bytes(&mut phantom_bytes);
        assert_eq!(
            decode_results(
                &phantom_bytes,
                &totals[..TotalsRecord::BYTES as usize],
                &[ray_query],
                Tick::new(9),
                &mut out,
            ),
            Err(DecodeError::RayResult { position: 0 })
        );
        decode_results(
            &bytes,
            &totals,
            &[
                base(1, CommandKind::SpawnBody),
                ray_query,
                base(3, CommandKind::RemoveBody),
                base(4, CommandKind::DriveBody),
            ],
            Tick::new(9),
            &mut out,
        )
        .expect("consistent");
        let results: Vec<CommandResult> = out
            .iter()
            .map(|outcome| match outcome {
                Outcome::Command(command) => command.result,
                other => panic!("{other:?}"),
            })
            .collect();
        // Anchor (-2, 1): cell x -128 + 10.5, cell y 64 + 3, height 21.
        assert_eq!(
            results[0],
            CommandResult::BodySpawned {
                body,
                position: DVec3::new(-117.5, 21.0, -67.0)
            }
        );
        // Hit cell (28, 5) in a window starting at cell (-128, 64).
        let CommandResult::Ray {
            surface: SurfaceHit::Hit { cell, position, .. },
            body: Some(crossed),
        } = results[1]
        else {
            panic!("{:?}", results[1]);
        };
        assert_eq!(cell, IVec2::new(-100, 69));
        assert_eq!(position, DVec3::new(-100.0, 20.0, -70.0));
        assert_eq!(crossed.body, body);
        assert_eq!(crossed.position, DVec3::new(-100.0, 38.0, -70.0));
        let CommandResult::BodyRemoved { counters, .. } = results[2] else {
            panic!("{:?}", results[2]);
        };
        assert_eq!((counters.landings, counters.absorbed_energy), (4, u32::MAX));
        assert_eq!(results[3], CommandResult::StaleBody { body });

        let mut sample = BodySampleRecord {
            status: codes::SAMPLE_OK,
            index: 5,
            generation: 2,
            flags: codes::BODY_ALIVE | codes::BODY_GROUNDED,
            anchor_x: -1,
            local_x: 2.0,
            local_y: 1.0,
            height: 3.0,
            liquid: u32::from(FluidMat::FLUID_WATER.0),
            craters: 7,
            ..BodySampleRecord::default()
        };
        let mut bytes = vec![0u8; 2 * BodySampleRecord::BYTES as usize];
        sample.write_bytes(&mut bytes);
        let gone = BodySampleRecord {
            status: codes::SAMPLE_GONE,
            index: 1,
            generation: 0,
            ..BodySampleRecord::default()
        };
        gone.write_bytes(&mut bytes[BodySampleRecord::BYTES as usize..]);
        let mut out = Vec::new();
        let selected = [body, BodyHandle::new(1, 0)];
        decode_samples(&bytes, &selected, Tick::new(9), &mut out).expect("echoed");
        let Outcome::Body(decoded) = out[0] else {
            panic!("{:?}", out[0]);
        };
        assert_eq!(decoded.position, DVec3::new(-62.0, 3.0, -1.0));
        assert!(decoded.grounded && !decoded.suspended);
        assert_eq!(decoded.liquid, Some(FluidMat::FLUID_WATER));
        assert_eq!(decoded.counters.craters, 7);
        assert_eq!(
            out[1],
            Outcome::BodyGone {
                body: BodyHandle::new(1, 0),
                tick: Tick::new(9)
            }
        );
        // A sample that names another handle than the one selected is refused.
        sample.generation = 3;
        sample.write_bytes(&mut bytes);
        assert_eq!(
            decode_samples(&bytes, &selected, Tick::new(9), &mut Vec::new()),
            Err(DecodeError::Sample { position: 0 })
        );
    }

    #[test]
    fn timestamps_convert_with_the_period_and_refuse_unordered_pairs() {
        let mut bytes = [0u8; 64];
        for (index, stamp) in [1_000u64, 1_400, 1_500, 1_600, 2_000, 3_000, 3_100, 3_300]
            .iter()
            .enumerate()
        {
            bytes[index * 8..index * 8 + 8].copy_from_slice(&stamp.to_le_bytes());
        }
        assert_eq!(
            decode_timestamps(&bytes, 1.5),
            Some((
                Duration::from_nanos(3_450),
                PhaseTimes {
                    commands: Duration::from_nanos(600),
                    activity: Duration::from_nanos(150),
                    material: Duration::from_nanos(1_500),
                    bodies: Duration::from_nanos(300),
                }
            ))
        );
        bytes[40..48].copy_from_slice(&500u64.to_le_bytes());
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
