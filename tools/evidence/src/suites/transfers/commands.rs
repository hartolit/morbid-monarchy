//! Checkpoint B: ordered uploads, edits and queries through the real command path, identity
//! rules and refusals.

use super::{abi::boundary_cells, *};
pub(super) fn upload_capture_roundtrip(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 2))?;
    let generator = WorldGenerator::new(69);
    let chunks = [
        (ChunkKey::new(-2, -2, 0), pattern(1)),
        (
            ChunkKey::new(0, 0, 0),
            generator
                .generate_chunk(ChunkKey::new(0, 0, 0))
                .cells
                .into_vec(),
        ),
        (ChunkKey::new(2, 1, 0), pattern(3)),
        (ChunkKey::new(-1, 2, 0), boundary_cells()),
    ];
    let mut slots = Vec::new();
    for (key, cells) in &chunks {
        slots.push(driver.upload(*key, cells)?);
    }
    for ((key, cells), slot) in chunks.iter().zip(&slots) {
        let captured = driver.capture(*slot)?;
        let differing = captured.iter().zip(cells).filter(|(a, b)| a != b).count();
        check.equal(&format!("differing cells of {key:?}"), differing, 0);
    }
    let counters = driver.world.counters();
    check.equal("chunk upload bytes", counters.upload.chunks, 4 * 32_768);
    check.equal("capture bytes", counters.readback.captures, 4 * 32_768);
    Ok(())
}

pub(super) fn ordered_edit_query(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 1))?;
    let east = driver.upload(ChunkKey::new(0, 0, 0), &flat(2))?;
    let west = driver.upload(ChunkKey::new(-1, 0, 0), &flat(2))?;
    let results = driver.run(&[
        edit(EditTool::Raise, 10, 10, 0, 5),
        edit(EditTool::Lower, 10, 10, 0, 3),
        query(10, 10),
        edit(EditTool::Lower, -10, 10, 0, 3),
        edit(EditTool::Raise, -10, 10, 0, 5),
        query(-10, 10),
    ])?;
    check.equal(
        "raise then lower",
        column(results[2]).map(|cell| cell.elevation()),
        Some(4),
    );
    check.equal(
        "lower (clamped) then raise",
        column(results[5]).map(|cell| cell.elevation()),
        Some(5),
    );
    let later = driver.run(&[query(10, 10), query(-10, 10)])?;
    check.equal(
        "next tick sees the same",
        later
            .iter()
            .map(|r| column(*r).map(|c| c.elevation()))
            .collect::<Vec<_>>(),
        vec![Some(4), Some(5)],
    );
    // A stroke across the seam between chunks -1 and 0.
    let seam = driver.run(&[edit(EditTool::Raise, 0, 30, 3, 1)])?;
    check.equal(
        "seam stroke",
        seam[0],
        CommandResult::Edited {
            changed: 29,
            not_resident: 0,
        },
    );
    let east_cells = driver.capture(east)?;
    let west_cells = driver.capture(west)?;
    let raised = |cells: &[WorldCell]| cells.iter().filter(|cell| cell.elevation() == 3).count();
    // Columns dx = 0..=3 of the radius-3 disc hold 7 + 5 + 5 + 1 cells; the other 11 are west.
    check.equal("seam cells on the east", raised(&east_cells), 18);
    check.equal("seam cells on the west", raised(&west_cells), 11);
    Ok(())
}

pub(super) fn edit_tools_match_rules(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 1))?;
    let generator = WorldGenerator::new(69);
    let sources = [
        ("arbitrary words", ChunkKey::new(0, 0, 0), pattern(11)),
        (
            "generated terrain",
            ChunkKey::new(1, 0, 0),
            generator
                .generate_chunk(ChunkKey::new(1, 0, 0))
                .cells
                .into_vec(),
        ),
    ];
    for (label, key, cells) in sources {
        let slot = driver.upload(key, &cells)?;
        let origin = key.origin_cell();
        let mut commands = Vec::new();
        let mut expected = cells.clone();
        let mut index = 0usize;
        for tool in EditTool::ALL {
            for strength in [1u8, 7, 255] {
                let (x, y) = ((index % 64) as i32, (index / 64) as i32 * 3);
                commands.push(edit(tool, origin.x + x, origin.y + y, 0, strength));
                let at = local_index(x, y);
                expected[at] = reference_edit(cells[at], tool, strength);
                index += 1;
            }
        }
        // Every tool in turn on one more cell, in order, within the same tick.
        let (x, y) = (40, 40);
        let at = local_index(x, y);
        for tool in EditTool::ALL {
            commands.push(edit(tool, origin.x + x, origin.y + y, 0, 9));
            expected[at] = reference_edit(expected[at], tool, 9);
        }
        driver.run(&commands)?;
        let captured = driver.capture(slot)?;
        let differing: Vec<usize> = (0..CHUNK_CELL_COUNT)
            .filter(|i| captured[*i] != expected[*i])
            .collect();
        check.that(differing.is_empty(), || {
            let i = differing[0];
            format!(
                "{label}: {} cells differ; cell {i}: {:#018x}, expected {:#018x}",
                differing.len(),
                captured[i].0,
                expected[i].0
            )
        });
    }
    Ok(())
}

pub(super) fn edit_partial_workgroups(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 2))?;
    let mut slots = Vec::new();
    for y in -2..=2 {
        for x in -2..=2 {
            let key = ChunkKey::new(x, y, 0);
            slots.push((key, driver.upload(key, &flat(100))?));
        }
    }
    let center = IVec2::new(32, 32);
    let radii = [0, 1, 2, 3, 4, 7, 8, 9, 15, 16, 63, 64];
    for radius in radii {
        let result = driver.run(&[edit(EditTool::Raise, center.x, center.y, radius, 1)])?;
        check.equal(
            &format!("radius {radius}"),
            result[0],
            CommandResult::Edited {
                changed: stroke_cell_count(radius),
                not_resident: 0,
            },
        );
    }
    let mut wrong = 0;
    for (key, slot) in slots {
        let cells = driver.capture(slot)?;
        let origin = key.origin_cell();
        for (index, cell) in cells.iter().enumerate() {
            let dx = i64::from(origin.x + (index % 64) as i32 - center.x);
            let dy = i64::from(origin.y + (index / 64) as i32 - center.y);
            let strokes = radii
                .iter()
                .filter(|r| dx * dx + dy * dy <= i64::from(**r) * i64::from(**r))
                .count();
            if u32::from(cell.elevation()) != 100 + strokes as u32 {
                wrong += 1;
            }
        }
    }
    check.equal("cells outside their expected footprint count", wrong, 0);
    Ok(())
}

pub(super) fn edit_outside_window(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 1))?;
    let slot = driver.upload(ChunkKey::new(0, 0, 0), &flat(10))?;
    let results = driver.run(&[
        edit(EditTool::Raise, 32, 32, 3, 1),
        edit(EditTool::Raise, 0, 32, 3, 1),
        edit(EditTool::Raise, -200, 0, 2, 1),
        edit(EditTool::Raise, i32::MAX - 10, i32::MIN + 10, 2, 1),
        query(-10, 5),
        query(500, 500),
        query(i32::MIN, i32::MAX),
    ])?;
    check.equal(
        "resident stroke",
        results[0],
        CommandResult::Edited {
            changed: 29,
            not_resident: 0,
        },
    );
    // Columns x = 0..=3 are resident (18 cells); the west is chunk (-1, 0), which is missing.
    check.equal(
        "stroke into a missing chunk",
        results[1],
        CommandResult::Edited {
            changed: 18,
            not_resident: 11,
        },
    );
    check.equal(
        "stroke outside the window",
        results[2],
        CommandResult::Edited {
            changed: 0,
            not_resident: 13,
        },
    );
    check.equal(
        "far stroke",
        results[3],
        CommandResult::Edited {
            changed: 0,
            not_resident: 13,
        },
    );
    for (index, cell) in [
        (4, IVec2::new(-10, 5)),
        (5, IVec2::new(500, 500)),
        (6, IVec2::new(i32::MIN, i32::MAX)),
    ] {
        check.equal(
            &format!("query {cell}"),
            results[index],
            CommandResult::ColumnNotResident { cell },
        );
    }
    let cells = driver.capture(slot)?;
    let raised = cells.iter().filter(|cell| cell.elevation() != 10).count();
    check.equal("cells changed in the resident chunk", raised, 29 + 18);
    Ok(())
}

pub(super) fn empty_batches(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 1))?;
    for _ in 0..20 {
        driver
            .submit(&[])
            .map_err(|refusal| format!("{refusal:?}"))?;
        driver.settle()?;
    }
    let ticks: Vec<_> = driver
        .outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            Outcome::TickCompleted(report) => Some(*report),
            _ => None,
        })
        .collect();
    check.equal("completed ticks", ticks.len(), 20);
    check.that(ticks.iter().all(|report| report.commands == 0), || {
        "an empty tick reported commands".to_owned()
    });
    check.that(
        !driver
            .outcomes
            .iter()
            .any(|outcome| matches!(outcome, Outcome::Command(_))),
        || "an empty tick produced a command outcome".to_owned(),
    );
    let ordered = ticks
        .windows(2)
        .all(|pair| pair[1].tick.value() == pair[0].tick.value() + 1);
    check.that(ordered, || "ticks were not completed in order".to_owned());
    // Tick parameters (128), the event header reset (16) and the wake-request word (4) up; the
    // event buffer (16 + 64 x 16) and the diagnostics record (144) back.
    let events = 16 + 64 * 16;
    check.equal(
        "upload bytes per empty tick",
        ticks[0].upload_bytes,
        128 + 16 + 4,
    );
    check.equal(
        "readback bytes per empty tick",
        ticks[0].readback_bytes,
        events + 144,
    );
    Ok(())
}

pub(super) fn command_count_boundaries(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 1))?;
    driver.upload(ChunkKey::new(0, 0, 0), &flat(0))?;
    for count in [1usize, 63, 64, 65, 255, 256] {
        let commands: Vec<Command> = (0..count)
            .map(|index| {
                let (x, y) = ((index % 64) as i32, (index / 64) as i32);
                if index % 2 == 0 {
                    edit(EditTool::Raise, x, y, 0, 1)
                } else {
                    query(x, y)
                }
            })
            .collect();
        let results = driver.run(&commands)?;
        let ok = results.iter().enumerate().all(|(index, result)| {
            if index % 2 == 0 {
                *result
                    == CommandResult::Edited {
                        changed: 1,
                        not_resident: 0,
                    }
            } else {
                column(*result).is_some()
            }
        });
        check.that(ok && results.len() == count, || {
            format!(
                "{count} commands: {} results, consistent {ok}",
                results.len()
            )
        });
    }
    let too_many = vec![query(0, 0); 257];
    let tick = driver.next_tick;
    check.equal(
        "257 commands",
        driver.submit(&too_many).err(),
        Some(Refusal::InvalidPayload(PayloadError::TooManyCommands {
            count: 257,
            limit: 256,
        })),
    );
    check.equal("refused tick is not consumed", driver.next_tick, tick);
    let after = driver.run(&[query(0, 0)])?;
    check.that(column(after[0]).is_some(), || {
        "the world stopped after a refusal".to_owned()
    });
    Ok(())
}

pub(super) fn runtime_retry_conflict_expiry(
    adapter: &str,
    check: &mut Check,
) -> Result<(), String> {
    let mut runtime = runtime_world(
        adapter,
        |_| {},
        RuntimeConfig {
            receipts: 8,
            pending_commands: 8,
            ..RuntimeConfig::default()
        },
    )?;
    runtime_upload(&mut runtime, ChunkKey::new(0, 0, 0), &flat(10))?;
    let raise = edit(EditTool::Raise, 3, 3, 0, 5);
    let seven = CommandId::new(7);
    check.equal(
        "first admission",
        runtime.submit(seven, raise),
        Ok(Admission::Admitted),
    );
    check.equal(
        "retry while pending",
        runtime.submit(seven, raise),
        Ok(Admission::Duplicate(ReceiptState::Pending)),
    );
    check.equal(
        "same identity, other command",
        runtime.submit(seven, edit(EditTool::Lower, 3, 3, 0, 5)),
        Err(AdmissionError::Conflict(seven)),
    );
    frames_until(&mut runtime, "edit", |runtime| {
        completed(runtime, seven).is_some()
    })?;
    check.that(
        matches!(
            runtime.submit(seven, raise),
            Ok(Admission::Duplicate(ReceiptState::Completed { .. }))
        ),
        || "a completed command was not reported as a duplicate".to_owned(),
    );
    let eight = CommandId::new(8);
    runtime
        .submit(eight, query(3, 3))
        .map_err(|error| format!("{error:?}"))?;
    frames_until(&mut runtime, "query", |runtime| {
        completed(runtime, eight).is_some()
    })?;
    check.equal(
        "applied once",
        completed(&runtime, eight)
            .and_then(column)
            .map(|cell| cell.elevation()),
        Some(15),
    );
    check.equal("acknowledge through 8", runtime.acknowledge(eight), Ok(2));
    check.equal(
        "acknowledged identity expired",
        runtime.submit(seven, raise),
        Err(AdmissionError::Expired {
            floor: CommandId::new(9),
        }),
    );
    check.equal("unknown outcomes", runtime.unknown_outcomes(), 0);
    let summary = *runtime.ledger().summary();
    check.metric(
        "observation_age_ticks_max",
        summary.observation_age_ticks.max,
    );
    check.metric("input_latency_ticks_max", summary.input_latency_ticks.max);
    Ok(())
}

pub(super) fn stale_session_and_slot(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 1))?;
    let key = ChunkKey::new(1, 1, 0);
    let slot = driver.upload(key, &flat(5))?;
    driver
        .world
        .retire_slot(slot)
        .map_err(|refusal| format!("retire: {refusal:?}"))?;
    driver
        .submit(&[])
        .map_err(|refusal| format!("{refusal:?}"))?;
    driver.settle()?;
    check.equal(
        "retire twice",
        driver.world.retire_slot(slot),
        Err(Refusal::StaleSlot(slot)),
    );
    check.equal(
        "upload into a retired slot",
        driver
            .world
            .upload_chunk(slot, ChunkRevision::new(2), &flat(1))
            .err(),
        Some(Refusal::StaleSlot(slot)),
    );
    check.equal(
        "capture a retired slot",
        driver.world.capture_chunk(slot).err(),
        Some(Refusal::StaleSlot(slot)),
    );
    let results = driver.run(&[query(key.origin_cell().x, key.origin_cell().y)])?;
    check.equal(
        "retired chunk is not resident",
        results[0],
        CommandResult::ColumnNotResident {
            cell: key.origin_cell(),
        },
    );
    let session = driver.session();
    let again = driver
        .world
        .reserve_slot(session, key)
        .map_err(|refusal| format!("{refusal:?}"))?;
    check.that(again.generation() != slot.generation(), || {
        "a reused slot kept its generation".to_owned()
    });
    let other = SessionId {
        world: session.world,
        epoch: SessionEpoch::new(session.epoch.value() + 1),
    };
    let foreign = SlotRef::new(other, again.index(), again.generation());
    check.equal(
        "another session's slot",
        driver
            .world
            .upload_chunk(foreign, ChunkRevision::new(3), &flat(1))
            .err(),
        Some(Refusal::StaleSession {
            current: session,
            presented: other,
        }),
    );
    let batch = TickBatch {
        session: other,
        tick: driver.next_tick,
        commands: &[],
    };
    check.equal(
        "another session's tick",
        driver.world.submit_tick(&batch).err(),
        Some(Refusal::StaleSession {
            current: session,
            presented: other,
        }),
    );
    Ok(())
}

pub(super) fn out_of_bounds_payload(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 1))?;
    let session = driver.session();
    let key = ChunkKey::new(0, 0, 0);
    let slot = driver
        .world
        .reserve_slot(session, key)
        .map_err(|refusal| format!("{refusal:?}"))?;
    check.equal(
        "short payload",
        driver
            .world
            .upload_chunk(
                slot,
                ChunkRevision::new(1),
                &flat(1)[..CHUNK_CELL_COUNT - 1],
            )
            .err(),
        Some(Refusal::InvalidPayload(PayloadError::CellCount {
            expected: CHUNK_CELL_COUNT,
            presented: CHUNK_CELL_COUNT - 1,
        })),
    );
    for outside in [ChunkKey::new(2, 0, 0), ChunkKey::new(0, 0, 1)] {
        check.equal(
            &format!("key {outside:?}"),
            driver.world.reserve_slot(session, outside).err(),
            Some(Refusal::InvalidPayload(PayloadError::KeyOutsideWindow(
                outside,
            ))),
        );
    }
    check.equal(
        "second reservation",
        driver.world.reserve_slot(session, key).err(),
        Some(Refusal::InvalidPayload(PayloadError::KeyAlreadyResident(
            key,
        ))),
    );
    driver
        .world
        .upload_chunk(slot, ChunkRevision::new(1), &flat(1))
        .map_err(|refusal| format!("{refusal:?}"))?;
    check.equal(
        "second upload before activation",
        driver
            .world
            .upload_chunk(slot, ChunkRevision::new(2), &flat(1))
            .err(),
        Some(Refusal::InvalidPayload(PayloadError::SlotState(slot))),
    );
    let skipped = TickBatch {
        session,
        tick: Tick::new(5),
        commands: &[],
    };
    check.equal(
        "tick out of order",
        driver.world.submit_tick(&skipped).err(),
        Some(Refusal::TickOutOfOrder {
            expected: Tick::ZERO,
            presented: Tick::new(5),
        }),
    );
    let spawn =
        Command::SpawnBody(BodySpawn::new(BodyPreset::Wood, IVec2::ZERO, 2.0).expect("valid"));
    check.equal(
        "body command before prompt 04",
        driver.submit(&[spawn]).err(),
        Some(Refusal::NotSupported(
            world_model::command::CommandKind::SpawnBody,
        )),
    );
    let mut runtime = runtime_world(adapter, |_| {}, RuntimeConfig::default())?;
    check.equal(
        "runtime admission of a body command",
        runtime.submit(CommandId::new(1), spawn),
        Err(AdmissionError::NotSupported(
            world_model::command::CommandKind::SpawnBody,
        )),
    );
    Ok(())
}

pub(super) fn full_receipts_before_admission(
    adapter: &str,
    check: &mut Check,
) -> Result<(), String> {
    let mut runtime = runtime_world(
        adapter,
        |_| {},
        RuntimeConfig {
            receipts: 4,
            pending_commands: 4,
            ..RuntimeConfig::default()
        },
    )?;
    runtime_upload(&mut runtime, ChunkKey::new(0, 0, 0), &flat(0))?;
    for id in 1..=4 {
        runtime
            .submit(CommandId::new(id), edit(EditTool::Raise, 1, 1, 0, 1))
            .map_err(|error| format!("{error:?}"))?;
    }
    let fifth = CommandId::new(5);
    check.equal(
        "fifth admission",
        runtime.submit(fifth, edit(EditTool::Raise, 1, 1, 0, 100)),
        Err(AdmissionError::Pressure(Pressure::Receipts)),
    );
    frames_until(&mut runtime, "four edits", |runtime| {
        (1..=4).all(|id| completed(runtime, CommandId::new(id)).is_some())
    })?;
    check.equal(
        "refused command left no receipt",
        runtime.receipt(fifth).is_none(),
        true,
    );
    check.equal(
        "acknowledged",
        runtime.acknowledge(CommandId::new(4)),
        Ok(4),
    );
    runtime
        .submit(fifth, query(1, 1))
        .map_err(|error| format!("{error:?}"))?;
    frames_until(&mut runtime, "query", |runtime| {
        completed(runtime, fifth).is_some()
    })?;
    check.equal(
        "only admitted edits applied",
        completed(&runtime, fifth)
            .and_then(column)
            .map(|cell| cell.elevation()),
        Some(4),
    );
    let mut queued = runtime_world(
        adapter,
        |_| {},
        RuntimeConfig {
            receipts: 8,
            pending_commands: 2,
            ..RuntimeConfig::default()
        },
    )?;
    for id in 1..=2 {
        queued
            .submit(CommandId::new(id), query(0, 0))
            .map_err(|error| format!("{error:?}"))?;
    }
    check.equal(
        "third queued command",
        queued.submit(CommandId::new(3), query(0, 0)),
        Err(AdmissionError::Pressure(Pressure::PendingCommands)),
    );
    Ok(())
}
