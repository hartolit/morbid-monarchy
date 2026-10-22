//! Checkpoint C: bounded transfers under pressure, delayed and reordered deliveries, event
//! overflow, resize, device loss, refused mapped ranges and the orderly stop.

use gpu_world::{
    context::{DeviceFault, DeviceNeeds, ErrorKind, GpuContext},
    staging::{DeliveryOrder, Direction, StagingPool},
};

use super::*;

pub(super) fn in_flight_backpressure(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 1))?;
    for _ in 0..3 {
        driver
            .submit(&[])
            .map_err(|refusal| format!("{refusal:?}"))?;
    }
    let started = Instant::now();
    let refused = driver.submit(&[]);
    let elapsed = started.elapsed();
    check.equal(
        "fourth tick",
        refused.err(),
        Some(Refusal::Pressure(Pressure::InFlightSubmissions)),
    );
    check.that(elapsed < Duration::from_millis(5), || {
        format!("a refused submission took {elapsed:?}")
    });
    check.metric("refusal_microseconds", elapsed.as_micros() as u64);
    driver.poll_until("a completion", |driver| {
        driver.world.counters().ticks_observed >= 1
    })?;
    check.that(driver.submit(&[]).is_ok(), || {
        "no tick was admitted after a completion".to_owned()
    });
    driver.settle()?;
    let counters = driver.world.counters();
    check.equal("frame waits", counters.frame_waits, 0);
    check.equal("maintenance waits", counters.maintenance_waits, 0);
    check.that(counters.in_flight_high_water <= 3, || {
        format!("in flight {}", counters.in_flight_high_water)
    });
    check.equal(
        "in-flight refusals",
        counters.pressure_refusals[Pressure::InFlightSubmissions.index()],
        1,
    );
    Ok(())
}

pub(super) fn staging_exhaustion_and_reuse(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut request = config(adapter, 1);
    request.transfer = TransferConfig {
        chunk_upload_slots: 2,
        capture_slots: 2,
        metadata_patches_per_tick: 4,
        ..TransferConfig::default()
    };
    let mut driver = Driver::open(request)?;
    let session = driver.session();
    let keys = [
        ChunkKey::new(-1, -1, 0),
        ChunkKey::new(0, -1, 0),
        ChunkKey::new(1, -1, 0),
    ];
    let slots: Vec<SlotRef> = keys
        .iter()
        .map(|key| driver.world.reserve_slot(session, *key))
        .collect::<Result<_, _>>()
        .map_err(|refusal| format!("{refusal:?}"))?;
    for (index, slot) in slots.iter().take(2).enumerate() {
        driver
            .world
            .upload_chunk(*slot, ChunkRevision::new(1), &pattern(index as u64))
            .map_err(|refusal| format!("{refusal:?}"))?;
    }
    check.equal(
        "third upload",
        driver
            .world
            .upload_chunk(slots[2], ChunkRevision::new(1), &pattern(2))
            .err(),
        Some(Refusal::Pressure(Pressure::ChunkUploadStaging)),
    );
    driver
        .submit(&[])
        .map_err(|refusal| format!("{refusal:?}"))?;
    driver.settle()?;
    let mut reused = false;
    driver.poll_until("upload staging reuse", |_| true)?;
    for _ in 0..100 {
        match driver
            .world
            .upload_chunk(slots[2], ChunkRevision::new(1), &pattern(2))
        {
            Ok(_) => {
                reused = true;
                break;
            }
            Err(Refusal::Pressure(Pressure::ChunkUploadStaging)) => {
                driver.poll();
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(other) => return Err(format!("{other:?}")),
        }
    }
    check.that(reused, || {
        "upload staging was not reused after its tick".to_owned()
    });
    driver
        .submit(&[])
        .map_err(|refusal| format!("{refusal:?}"))?;
    driver.settle()?;
    driver.poll_until("activations", |driver| {
        driver
            .outcomes
            .iter()
            .filter(|outcome| matches!(outcome, Outcome::UploadActivated { .. }))
            .count()
            == 3
    })?;
    let tickets: Vec<_> = slots
        .iter()
        .take(2)
        .map(|slot| driver.world.capture_chunk(*slot))
        .collect::<Result<_, _>>()
        .map_err(|refusal| format!("{refusal:?}"))?;
    check.equal(
        "third capture",
        driver.world.capture_chunk(slots[2]).err(),
        Some(Refusal::Pressure(Pressure::CaptureStaging)),
    );
    driver
        .submit(&[])
        .map_err(|refusal| format!("{refusal:?}"))?;
    driver.poll_until("captures ready", |driver| {
        driver
            .outcomes
            .iter()
            .filter(|outcome| matches!(outcome, Outcome::CaptureReady { .. }))
            .count()
            == 2
    })?;
    let mut cells = vec![WorldCell::default(); CHUNK_CELL_COUNT];
    let first = driver
        .world
        .take_capture(tickets[0], &mut cells)
        .map_err(|refusal| format!("{refusal:?}"))?;
    check.equal("captured key", first.key, keys[0]);
    check.that(cells == pattern(0), || "captured cells differ".to_owned());
    let third = driver
        .world
        .capture_chunk(slots[2])
        .map_err(|refusal| format!("third capture after a take: {refusal:?}"))?;
    check.that(
        third.staging_index() == tickets[0].staging_index()
            && third.staging_generation() != tickets[0].staging_generation(),
        || "capture staging was not reused under a new generation".to_owned(),
    );
    check.equal(
        "an uncollected ticket of the reused buffer is stale",
        driver.world.take_capture(tickets[0], &mut cells).err(),
        Some(Refusal::StaleSlot(tickets[0].slot())),
    );
    let counters = driver.world.counters();
    check.equal(
        "buffers created after open",
        counters.buffers_created,
        driver.world.report().allocations.buffers_created,
    );
    check.metric(
        "upload_staging_high_water",
        u64::from(counters.upload_staging_high_water),
    );
    check.metric(
        "readback_staging_high_water",
        u64::from(counters.readback_staging_high_water),
    );
    Ok(())
}

pub(super) fn delayed_delivery(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut request = config(adapter, 1);
    request.fault_injection = true;
    let mut driver = Driver::open(request)?;
    driver.upload(ChunkKey::new(0, 0, 0), &flat(20))?;
    let faults = driver
        .world
        .fault_controller()
        .ok_or("no fault controller")?;
    faults.hold_deliveries(true);
    let mut submitted = Vec::new();
    let refusal = loop {
        match driver.submit(&[query(submitted.len() as i32, 0)]) {
            Ok(ids) => submitted.push(ids[0]),
            Err(refusal) => break refusal,
        }
        driver.poll();
        if submitted.len() > 16 {
            break Refusal::Faulted;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    check.that(
        matches!(
            refusal,
            Refusal::Pressure(
                Pressure::TickUploadStaging
                    | Pressure::TickReadbackStaging
                    | Pressure::InFlightSubmissions
            )
        ),
        || format!("held deliveries ended with {refusal:?}"),
    );
    check.that(submitted.len() <= 4, || {
        format!(
            "{} ticks admitted while deliveries were held",
            submitted.len()
        )
    });
    check.that(
        submitted.iter().all(|id| driver.result(*id).is_none()),
        || "an outcome arrived while held".to_owned(),
    );
    std::thread::sleep(Duration::from_millis(50));
    faults.hold_deliveries(false);
    driver.poll_until("held outcomes", |driver| {
        submitted.iter().all(|id| driver.result(*id).is_some())
    })?;
    for (index, id) in submitted.iter().enumerate() {
        let result = driver
            .result(*id)
            .and_then(column)
            .map(|cell| cell.elevation());
        check.equal(&format!("held query {index}"), result, Some(20));
    }
    let latency = driver
        .outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            Outcome::TickCompleted(report) => Some(report.latency),
            _ => None,
        })
        .max()
        .unwrap_or_default();
    check.that(latency >= Duration::from_millis(50), || {
        format!("held ticks report latency {latency:?}")
    });
    check.metric("held_ticks", submitted.len() as u64);
    check.metric("max_latency_ms", latency.as_secs_f64() * 1000.0);
    driver.run(&[query(0, 0)])?;
    check.equal(
        "stale deliveries",
        driver.world.counters().stale_deliveries,
        0,
    );

    // Delayed completion callbacks: the device finishes, but the timeline does not observe it.
    // Admission stays bounded by the in-flight limit even though results still decode.
    faults.hold_completions(true);
    let mut admitted = Vec::new();
    let refusal = loop {
        match driver.submit(&[query(1, 1)]) {
            Ok(ids) => admitted.push(ids[0]),
            Err(refusal) => break refusal,
        }
        driver.poll();
        if admitted.len() > 8 {
            break Refusal::Faulted;
        }
    };
    check.equal(
        "refusal with completions held",
        refusal,
        Refusal::Pressure(Pressure::InFlightSubmissions),
    );
    check.equal("ticks admitted with completions held", admitted.len(), 3);
    driver.poll_until("results with completions held", |driver| {
        admitted.iter().all(|id| driver.result(*id).is_some())
    })?;
    faults.hold_completions(false);
    driver.poll_until("completions observed", |driver| {
        driver.poll_report_in_flight() == 0
    })?;
    check.that(driver.submit(&[]).is_ok(), || {
        "admission did not resume after completions".to_owned()
    });
    driver.settle()?;
    Ok(())
}

pub(super) fn out_of_order_delivery(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut request = config(adapter, 1);
    request.fault_injection = true;
    let mut driver = Driver::open(request)?;
    driver.upload(ChunkKey::new(0, 0, 0), &flat(0))?;
    driver.outcomes.clear();
    let faults = driver
        .world
        .fault_controller()
        .ok_or("no fault controller")?;
    faults.hold_deliveries(true);
    let mut admitted = Vec::new();
    for index in 0..3u8 {
        let command = AdmittedCommand {
            id: CommandId::new(100 + u64::from(index)),
            command: edit(EditTool::Raise, i32::from(index), 0, 0, 1 + index),
        };
        driver
            .submit_admitted(&[command])
            .map_err(|refusal| format!("{refusal:?}"))?;
        admitted.push(command);
    }
    // Every submission completes while its deliveries stay held.
    driver.poll_until("completions", |driver| driver.poll_report_in_flight() == 0)?;
    check.that(driver.outcomes.is_empty(), || {
        "an outcome arrived while held".to_owned()
    });
    faults.reverse_deliveries(true);
    faults.hold_deliveries(false);
    driver.poll_until("reversed outcomes", |driver| {
        admitted
            .iter()
            .all(|command| driver.result(command.id).is_some())
    })?;
    let order: Vec<u64> = driver
        .outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            Outcome::TickCompleted(report) => Some(report.tick.value()),
            _ => None,
        })
        .collect();
    check.that(order.windows(2).any(|pair| pair[0] > pair[1]), || {
        format!("deliveries were not reordered: {order:?}")
    });
    check.metric("delivery_order", format!("{order:?}"));
    for command in &admitted {
        check.equal(
            &format!("edit {:?}", command.id),
            driver.result(command.id),
            Some(CommandResult::Edited {
                center: edit_center(&command.command),
                changed: 1,
                not_resident: 0,
            }),
        );
    }
    // The runtime's consumer, fed the reordered outcomes, completes every receipt and keeps
    // the latest tick as the latest observation.
    let mut receipts = world_runtime::receipts::Receipts::new(8);
    let supported = driver.world.capabilities().supported;
    for command in &admitted {
        receipts
            .check(command.id, &command.command, supported)
            .map_err(|error| format!("{error:?}"))?;
        receipts
            .insert(command.id, command.command, None)
            .map_err(|error| format!("{error:?}"))?;
    }
    let mut ledger = world_runtime::ledger::OutcomeLedger::new(
        16,
        0,
        world_runtime::bodies::BodyMonitor::new(0, 0),
    );
    for outcome in driver.outcomes.drain(..) {
        ledger.consume(outcome, &mut receipts, Some(Tick::new(10)));
    }
    let last = order.iter().max().copied().map(Tick::new);
    check.equal(
        "latest observed tick",
        ledger.summary().last_observed_tick,
        last,
    );
    check.that(
        admitted.iter().all(|command| {
            matches!(
                receipts.get(command.id).map(|receipt| receipt.state),
                Some(ReceiptState::Completed { .. })
            )
        }),
        || "a receipt was not completed".to_owned(),
    );
    check.equal("unknown outcomes", receipts.unknown_outcomes(), 0);
    Ok(())
}

pub(super) fn event_overflow_and_reset(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut request = config(adapter, 1);
    request.fault_injection = true;
    let world = opened(GpuWorld::open(request))?;
    let faults = world.fault_controller().ok_or("no fault controller")?;
    let mut runtime = Runtime::new(Box::new(world), RuntimeConfig::default())
        .map_err(|error| format!("{error:?}"))?;
    faults.flood_events(3 * 64);
    runtime.frame_ticks(1);
    let flooded = runtime.last_submitted_tick().ok_or("no tick")?;
    runtime.frame_ticks(1);
    let next = runtime.last_submitted_tick().ok_or("no tick")?;
    frames_until(&mut runtime, "both ticks", |runtime| {
        runtime.ledger().summary().last_observed_tick >= Some(next)
    })?;
    let summary = *runtime.ledger().summary();
    check.equal("events received", summary.cosmetic_received, 64);
    check.equal(
        "events dropped on the device",
        summary.cosmetic_dropped_on_device,
        128,
    );
    let events: Vec<_> = runtime.ledger_mut().drain_cosmetic().collect();
    check.that(events.iter().all(|event| event.tick == flooded), || {
        "an event arrived from the following tick: the counter was not reset".to_owned()
    });
    check.that(
        events.iter().all(|event| {
            event.kind == layout::codes::EVENT_INJECTED && event.data[1] == flooded.to_words()[0]
        }),
        || "events carry the wrong kind or tick".to_owned(),
    );
    Ok(())
}

pub(super) fn resize_success_and_refusal(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 1))?;
    let session = driver.session();
    let chunks = [
        (ChunkKey::new(0, 0, 0), pattern(21)),
        (ChunkKey::new(1, -1, 0), pattern(22)),
        (ChunkKey::new(-1, 1, 0), pattern(23)),
    ];
    let mut slots = Vec::new();
    for (key, cells) in &chunks {
        slots.push(driver.upload(*key, cells)?);
    }
    let old_plan = driver.world.plan().clone();
    // A capture submitted before the resize and collected after it.
    let early = driver
        .world
        .capture_chunk(slots[0])
        .map_err(|refusal| format!("{refusal:?}"))?;
    driver
        .submit(&[])
        .map_err(|refusal| format!("{refusal:?}"))?;
    let larger = ResidentWindow::new(ChunkKey::new(0, 0, 0), 2, 2).expect("window");
    let report = driver
        .world
        .resize(&ResizeRequest {
            session,
            window: larger,
        })
        .map_err(|refusal| format!("resize: {refusal:?}"))?;
    check.equal("retained chunks", report.retained.len(), 3);
    check.equal("migrated bytes", report.migrated_bytes, 3 * 32_768);
    check.that(
        report.peak_overlap_bytes >= old_plan.device_bytes + driver.world.plan().window_bytes(),
        || {
            format!(
                "peak {} below old {} + new window {}",
                report.peak_overlap_bytes,
                old_plan.device_bytes,
                driver.world.plan().window_bytes()
            )
        },
    );
    check.metric("peak_overlap_bytes", report.peak_overlap_bytes);
    check.metric("old_device_bytes", old_plan.device_bytes);
    check.metric("new_device_bytes", driver.world.plan().device_bytes);
    driver.poll_until("early capture", |driver| {
        driver.outcomes.iter().any(
            |outcome| matches!(outcome, Outcome::CaptureReady { ticket, .. } if *ticket == early),
        )
    })?;
    let mut cells = vec![WorldCell::default(); CHUNK_CELL_COUNT];
    driver
        .world
        .take_capture(early, &mut cells)
        .map_err(|refusal| format!("{refusal:?}"))?;
    check.that(cells == chunks[0].1, || {
        "a capture submitted before the resize differs".to_owned()
    });
    for slot in &slots {
        check.equal(
            "old slot after resize",
            driver.world.capture_chunk(*slot).err(),
            Some(Refusal::StaleSlot(*slot)),
        );
    }
    for (key, slot) in &report.retained {
        let expected = &chunks
            .iter()
            .find(|(chunk, _)| chunk == key)
            .ok_or("unknown key")?
            .1;
        check.that(driver.capture(*slot)? == *expected, || {
            format!("migrated {key:?} differs")
        });
    }
    driver.settle()?;
    driver.poll_until("old window released", |driver| {
        driver.world.counters().requested_bytes < report.peak_overlap_bytes
    })?;
    check.equal(
        "bytes after release",
        driver.world.counters().requested_bytes,
        report.requested_bytes - old_plan.window_bytes(),
    );

    let elsewhere = ResidentWindow::new(ChunkKey::new(5, 5, 0), 1, 1).expect("window");
    check.equal(
        "resize that would drop chunks",
        driver
            .world
            .resize(&ResizeRequest {
                session,
                window: elsewhere,
            })
            .err(),
        Some(Refusal::InvalidPayload(
            PayloadError::ResidentOutsideNewWindow { count: 3 },
        )),
    );
    let reserved = driver
        .world
        .reserve_slot(session, ChunkKey::new(2, 2, 0))
        .map_err(|refusal| format!("{refusal:?}"))?;
    check.equal(
        "resize with an unfilled reservation",
        driver
            .world
            .resize(&ResizeRequest {
                session,
                window: ResidentWindow::new(ChunkKey::new(0, 0, 0), 3, 3).expect("window"),
            })
            .err(),
        Some(Refusal::InvalidPayload(
            PayloadError::UnfilledReservations { count: 1 },
        )),
    );
    driver
        .world
        .retire_slot(reserved)
        .map_err(|refusal| format!("{refusal:?}"))?;
    let (key, slot) = report.retained[0];
    let expected = &chunks
        .iter()
        .find(|(chunk, _)| *chunk == key)
        .ok_or("unknown key")?
        .1;
    check.that(driver.capture(slot)? == *expected, || {
        "a refused resize changed a chunk".to_owned()
    });

    // A device budget that holds the larger plan alone but not the old and new windows
    // together while the migration runs.
    let larger_bytes = driver.world.plan().device_bytes;
    let mut budgeted = config(adapter, 1);
    budgeted.budgets = Budgets {
        device_bytes: Some(larger_bytes + 4096),
        ..Budgets::default()
    };
    let mut small = Driver::open(budgeted)?;
    let kept = small.upload(ChunkKey::new(0, 0, 0), &pattern(5))?;
    let refused = small.world.resize(&ResizeRequest {
        session: small.session(),
        window: larger,
    });
    check.that(
        matches!(
            refused,
            Err(Refusal::Unsupported(CapacityLimit {
                resource: LimitedResource::DeviceBytes,
                ..
            }))
        ),
        || format!("over-budget resize: {refused:?}"),
    );
    check.that(small.capture(kept)? == pattern(5), || {
        "an over-budget resize changed the world".to_owned()
    });
    let after = driver.run(&[edit(EditTool::Raise, 1, 1, 1, 1)])?;
    check.equal(
        "edits after resize",
        after[0],
        CommandResult::Edited {
            center: IVec2::new(1, 1),
            changed: 5,
            not_resident: 0,
        },
    );
    Ok(())
}

pub(super) fn device_loss_and_recreation(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut request = config(adapter, 1);
    request.fault_injection = true;
    let world = opened(GpuWorld::open(request.clone()))?;
    let faults = world.fault_controller().ok_or("no fault controller")?;
    let mut runtime = Runtime::new(Box::new(world), RuntimeConfig::default())
        .map_err(|error| format!("{error:?}"))?;
    let slot = runtime_upload(&mut runtime, ChunkKey::new(0, 0, 0), &flat(3))?;
    let old_session = runtime.session();
    let capture = runtime
        .capture_chunk(slot)
        .map_err(|error| format!("{error:?}"))?;
    faults.hold_deliveries(true);
    let ids: Vec<CommandId> = (0..3).map(|index| CommandId::new(50 + index)).collect();
    for id in &ids {
        runtime
            .submit(*id, edit(EditTool::Raise, 2, 2, 1, 1))
            .map_err(|error| format!("{error:?}"))?;
    }
    runtime.frame_ticks(1);
    let queued = CommandId::new(60);
    runtime
        .submit(queued, query(2, 2))
        .map_err(|error| format!("{error:?}"))?;
    faults.lose_device();
    let frame = runtime.frame_ticks(1);
    check.equal("health after loss", frame.health, Some(PortHealth::Lost));
    for id in ids.iter().chain([&queued]) {
        check.equal(
            &format!("receipt {id:?}"),
            runtime.receipt(*id).map(|receipt| receipt.state),
            Some(ReceiptState::Abandoned {
                reason: AbandonReason::DeviceLost,
            }),
        );
    }
    let mut residency = Vec::new();
    runtime.drain_residency(&mut residency);
    check.that(
        residency.iter().any(|outcome| match outcome {
            ResidencyOutcome::CaptureAbandoned { ticket, reason } => {
                *ticket == capture && *reason == AbandonReason::DeviceLost
            }
            _ => false,
        }),
        || format!("the capture was not abandoned: {residency:?}"),
    );
    check.equal(
        "admission after loss",
        runtime.submit(CommandId::new(70), query(0, 0)),
        Err(AdmissionError::Closed),
    );
    check.equal(
        "uploads after loss",
        runtime.reserve_slot(ChunkKey::new(1, 0, 0)).err(),
        Some(Refusal::DeviceLost),
    );
    check.metric(
        "abandoned_commands",
        runtime.ledger().summary().commands_abandoned,
    );

    // Loss reported through the device callback, observed by polling alone.
    {
        let mut lost = Driver::open(request.clone())?;
        let controller = lost.world.fault_controller().ok_or("no fault controller")?;
        controller.lose_device();
        lost.poll_until("loss callback", |driver| {
            driver.world.report().device_loss.is_some()
        })?;
        check.note(format!(
            "device loss callback: {:?}",
            lost.world.report().device_loss
        ));
    }

    let mut recreated = request;
    recreated.epoch = old_session.epoch.next().ok_or("epochs exhausted")?;
    recreated.fault_injection = false;
    let mut driver = Driver::open(recreated)?;
    let session = driver.session();
    check.that(session.epoch > old_session.epoch, || {
        "the new session kept the old epoch".to_owned()
    });
    let stale = Some(Refusal::StaleSession {
        current: session,
        presented: old_session,
    });
    check.equal(
        "old slot in the new session",
        driver.world.capture_chunk(slot).err(),
        stale,
    );
    let mut cells = vec![WorldCell::default(); CHUNK_CELL_COUNT];
    check.equal(
        "old capture ticket",
        driver.world.take_capture(capture, &mut cells).err(),
        stale,
    );
    let old_batch = TickBatch {
        session: old_session,
        tick: Tick::ZERO,
        tick_rate_hz: 60.0,
        commands: &[],
        observe: &[],
    };
    check.equal(
        "old session's tick",
        driver.world.submit_tick(&old_batch).err(),
        stale,
    );
    let new_slot = driver.upload(ChunkKey::new(0, 0, 0), &flat(3))?;
    check.that(new_slot.session() == session, || {
        "a new slot carries the old session".to_owned()
    });
    let results = driver.run(&[edit(EditTool::Raise, 2, 2, 0, 1), query(2, 2)])?;
    check.equal(
        "the new session runs",
        column(results[1]).map(|cell| cell.elevation()),
        Some(4),
    );
    Ok(())
}

/// A mapped range wgpu refuses (since wgpu 30 an error returned to the caller, before it a
/// panic) writes or reads nothing and is recorded as an uncaptured error, the record a session's
/// admission and poll fault on; the buffer stays usable for a valid range.
pub(super) fn refused_mapped_range(adapter: &str, check: &mut Check) -> Result<(), String> {
    let (mut context, _) = GpuContext::open(
        adapter,
        false,
        LimitCeiling::Adapter,
        DeviceNeeds {
            max_buffer_size: 1 << 20,
            max_storage_binding: 1 << 20,
            timestamps: false,
        },
        1,
        None,
    )
    .map_err(|error| format!("{error:?}"))?;
    let size = 256;
    let mut uploads = StagingPool::new(
        &mut context,
        "refusal uploads",
        Direction::Upload,
        1,
        size,
        wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
    )
    .map_err(|error| format!("{error:?}"))?;
    let mut readbacks = StagingPool::new(
        &mut context,
        "refusal readbacks",
        Direction::Readback,
        1,
        size,
        wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
    )
    .map_err(|error| format!("{error:?}"))?;
    let upload = uploads.reserve().ok_or("no upload buffer")?;
    // Inside the buffer, but not at a multiple of the map alignment (8).
    check.that(!uploads.write(upload, 2, &[1, 2, 3, 4]), || {
        "a misaligned write was accepted".to_owned()
    });
    check.equal(
        "faults after the write",
        context.health().uncaptured_count(),
        1,
    );
    check.that(uploads.write(upload, 8, &[1, 2, 3, 4]), || {
        "an aligned write after the refusal failed".to_owned()
    });
    // Nothing used the buffer, so its map resolves at the next poll.
    let readback = readbacks.reserve().ok_or("no readback buffer")?;
    readbacks.submitted(readback, context.next_serial());
    let mut completions = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !readbacks.is_ready(readback) && Instant::now() < deadline {
        context.poll(true);
        readbacks.collect(DeliveryOrder::default(), &mut completions);
    }
    check.that(readbacks.is_ready(readback), || {
        "the readback never mapped".to_owned()
    });
    check.equal(
        "misaligned read",
        readbacks.read(readback, 2..6, |bytes| bytes.len()),
        None,
    );
    check.equal(
        "aligned read",
        readbacks.read(readback, 8..12, |bytes| bytes.len()),
        Some(4),
    );
    let (faults, count) = context.health().uncaptured();
    check.equal("recorded refusals", count, 2);
    for fault in &faults {
        check.that(
            matches!(
                fault,
                DeviceFault::Uncaptured {
                    kind: ErrorKind::MappedRange,
                    ..
                }
            ),
            || format!("not a mapped-range fault: {fault:?}"),
        );
    }
    if let Some(DeviceFault::Uncaptured { description, .. }) = faults.first() {
        check.metric("refusal", description.clone());
    }
    check.that(context.health().lost().is_none(), || {
        "a refused range was reported as a lost device".to_owned()
    });
    uploads.release(upload);
    readbacks.release(readback);
    uploads.destroy(&mut context);
    readbacks.destroy(&mut context);
    Ok(())
}

pub(super) fn stop_and_drain(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 1))?;
    let slot = driver.upload(ChunkKey::new(0, 0, 0), &pattern(31))?;
    let mut ids = Vec::new();
    for index in 0..2 {
        ids.extend(
            driver
                .submit(&[query(index, 0)])
                .map_err(|refusal| format!("{refusal:?}"))?,
        );
    }
    let capture = driver
        .world
        .capture_chunk(slot)
        .map_err(|refusal| format!("{refusal:?}"))?;
    ids.extend(
        driver
            .submit(&[query(5, 5)])
            .map_err(|refusal| format!("{refusal:?}"))?,
    );
    let session = driver.session();
    let staged = driver
        .world
        .reserve_slot(session, ChunkKey::new(1, 0, 0))
        .map_err(|refusal| format!("{refusal:?}"))?;
    driver
        .world
        .upload_chunk(staged, ChunkRevision::new(1), &flat(1))
        .map_err(|refusal| format!("{refusal:?}"))?;
    let report = driver
        .world
        .stop(Duration::from_secs(5), &mut driver.outcomes);
    check.equal("stop health", report.health, PortHealth::Stopped);
    check.that(!report.timed_out, || {
        "an unobstructed stop timed out".to_owned()
    });
    check.that(ids.iter().all(|id| driver.result(*id).is_some()), || {
        "a submitted command was not delivered by stop".to_owned()
    });
    check.that(
        driver.outcomes.iter().any(|outcome| match outcome {
            Outcome::Abandoned {
                obligation: Obligation::Upload(ticket),
                reason,
            } => ticket.slot == staged && *reason == AbandonReason::Stopped,
            _ => false,
        }),
        || "the staged upload was not abandoned".to_owned(),
    );
    check.equal(
        "admission after stop",
        driver.submit(&[]).err(),
        Some(Refusal::Stopped),
    );
    let mut cells = vec![WorldCell::default(); CHUNK_CELL_COUNT];
    let taken = driver.world.take_capture(capture, &mut cells);
    check.that(taken.is_ok() && cells == pattern(31), || {
        format!("capture after stop: {taken:?}")
    });
    check.metric("drain_waited_ms", report.waited.as_secs_f64() * 1000.0);
    check.metric(
        "maintenance_waits",
        driver.world.counters().maintenance_waits,
    );

    let mut request = config(adapter, 1);
    request.fault_injection = true;
    let mut stalled = Driver::open(request)?;
    let faults = stalled
        .world
        .fault_controller()
        .ok_or("no fault controller")?;
    faults.hold_deliveries(true);
    let held = stalled
        .submit(&[query(0, 0)])
        .map_err(|refusal| format!("{refusal:?}"))?;
    let report = stalled
        .world
        .stop(Duration::from_millis(200), &mut stalled.outcomes);
    check.that(report.timed_out, || {
        "a stop with held deliveries did not time out".to_owned()
    });
    check.equal("stalled stop health", report.health, PortHealth::Stopped);
    check.that(
        stalled.outcomes.iter().any(|outcome| match outcome {
            Outcome::Abandoned {
                obligation: Obligation::Command(id),
                reason,
            } => *id == held[0] && *reason == AbandonReason::StopTimedOut,
            _ => false,
        }),
        || "the held command was not abandoned on timeout".to_owned(),
    );
    check.that(report.waited >= Duration::from_millis(150), || {
        format!("waited only {:?}", report.waited)
    });
    Ok(())
}

/// The centre cell of a column-targeted stroke.
fn edit_center(command: &world_model::command::Command) -> IVec2 {
    match command {
        world_model::command::Command::EditMaterial(edit) => edit.center().unwrap_or_default(),
        _ => IVec2::ZERO,
    }
}
