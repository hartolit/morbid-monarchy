//! Checkpoint D: steady-state reuse, with transfer bytes by purpose, waits, allocations and
//! pass times.

use super::*;
pub(super) fn steady_state_reuse(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut request = config(adapter, 2);
    request.timestamps = true;
    let mut driver = Driver::open(request)?;
    let session = driver.session();
    let generator = WorldGenerator::new(69);
    let mut resident: Vec<(ChunkKey, SlotRef)> = Vec::new();
    for y in -1..=1 {
        for x in -1..=1 {
            let key = ChunkKey::new(x, y, 0);
            resident.push((
                key,
                driver.upload(key, &generator.generate_chunk(key).cells)?,
            ));
        }
    }
    let mut free_keys =
        std::collections::VecDeque::from([ChunkKey::new(2, 2, 0), ChunkKey::new(-2, 2, 0)]);
    let warmup = 120u64;
    let measured = 1200u64;
    let mut retries = 0u64;
    let mut capture_buffer = vec![WorldCell::default(); CHUNK_CELL_COUNT];
    let mut pending_capture = None;
    let mut baseline = None;
    let mut commands_sent = 0u64;
    let mut commands: Vec<AdmittedCommand> = Vec::with_capacity(16);
    // Allocations counted around the port's own calls, so the harness's are excluded.
    let (mut submit_allocations, mut poll_allocations) = (0u64, 0u64);
    let started_at = Instant::now();
    let mut measure_started = Instant::now();
    let tools = EditTool::ALL;
    for step in 0..(warmup + measured) {
        if step == warmup {
            driver.settle()?;
            driver.outcomes.clear();
            driver.outcomes.reserve(measured as usize * 24);
            baseline = Some((driver.world.counters(), alloc::snapshot()));
            measure_started = Instant::now();
            commands_sent = 0;
            submit_allocations = 0;
            poll_allocations = 0;
        }
        commands.clear();
        for k in 0..8u64 {
            let x = ((step * 7 + k * 13) % 192) as i32 - 64;
            let y = ((step * 5 + k * 29) % 192) as i32 - 64;
            let first = driver.next_id();
            commands.push(AdmittedCommand {
                id: first,
                command: edit(tools[((step + k) % 8) as usize], x, y, 3, 2),
            });
            let second = driver.next_id();
            commands.push(AdmittedCommand {
                id: second,
                command: query(x, y),
            });
        }
        if step % 60 == 30 && pending_capture.is_none() {
            let (_, slot) = resident[(step as usize / 60) % resident.len()];
            pending_capture = Some(
                driver
                    .world
                    .capture_chunk(slot)
                    .map_err(|refusal| format!("capture: {refusal:?}"))?,
            );
        }
        if step % 120 == 60 {
            // Residency churn: retire the oldest chunk and bring in a key that is not resident.
            let (old_key, old_slot) = resident.remove(0);
            driver
                .world
                .retire_slot(old_slot)
                .map_err(|refusal| format!("retire: {refusal:?}"))?;
            free_keys.push_back(old_key);
            let key = free_keys.pop_front().ok_or("no free key")?;
            let slot = driver
                .world
                .reserve_slot(session, key)
                .map_err(|refusal| format!("reserve: {refusal:?}"))?;
            driver
                .world
                .upload_chunk(
                    slot,
                    ChunkRevision::new(step),
                    &generator.generate_chunk(key).cells,
                )
                .map_err(|refusal| format!("upload: {refusal:?}"))?;
            resident.push((key, slot));
        }
        loop {
            let before = alloc::snapshot().0;
            let submitted = driver.submit_admitted(&commands);
            submit_allocations += alloc::snapshot().0 - before;
            match submitted {
                Ok(_) => break,
                Err(Refusal::Pressure(_)) => {
                    retries += 1;
                    let before = alloc::snapshot().0;
                    driver.poll();
                    poll_allocations += alloc::snapshot().0 - before;
                }
                Err(other) => return Err(format!("submit: {other:?}")),
            }
            if started_at.elapsed() > Duration::from_secs(120) {
                return Err("steady run exceeded two minutes".to_owned());
            }
        }
        commands_sent += commands.len() as u64;
        let before = alloc::snapshot().0;
        driver.poll();
        poll_allocations += alloc::snapshot().0 - before;
        let ready = |ticket| {
            driver.outcomes.iter().any(|outcome| match outcome {
                Outcome::CaptureReady { ticket: ready, .. } => *ready == ticket,
                _ => false,
            })
        };
        if let Some(ticket) = pending_capture
            && ready(ticket)
        {
            driver
                .world
                .take_capture(ticket, &mut capture_buffer)
                .map_err(|refusal| format!("take: {refusal:?}"))?;
            pending_capture = None;
        }
    }
    driver.settle()?;
    let elapsed = measure_started.elapsed();
    let (before, allocations_before) = baseline.ok_or("no warm-up baseline")?;
    let after = driver.world.counters();
    let allocations_after = alloc::snapshot();
    let ticks = after.ticks_observed - before.ticks_observed;
    let command_outcomes = driver
        .outcomes
        .iter()
        .filter(|outcome| matches!(outcome, Outcome::Command(_)))
        .count() as u64;
    check.equal("measured ticks", ticks, measured);
    check.equal("command outcomes", command_outcomes, commands_sent);
    check.equal(
        "buffers created while measuring",
        after.buffers_created - before.buffers_created,
        0,
    );
    check.equal(
        "pipelines created while measuring",
        after.pipelines_created - before.pipelines_created,
        0,
    );
    check.equal(
        "requested bytes while measuring",
        after.requested_bytes,
        before.requested_bytes,
    );
    check.equal("frame waits", after.frame_waits, 0);
    check.equal("stale deliveries", after.stale_deliveries, 0);
    check.that(after.in_flight_high_water <= 3, || {
        format!("in flight high water {}", after.in_flight_high_water)
    });
    let plan = driver.world.plan().clone();
    let pools = |role| plan.buffer(role).count;
    use gpu_world::plan::BufferRole;
    check.that(
        after.upload_staging_high_water
            <= pools(BufferRole::TickUploadStaging) + pools(BufferRole::ChunkUploadStaging),
        || "upload staging exceeded its pools".to_owned(),
    );
    let per_tick = |value: u64| value as f64 / ticks as f64;
    check.metric("ticks", ticks);
    // Unpaced: the harness submits as fast as backpressure allows; not a frame-rate figure.
    check.metric(
        "unpaced_ticks_per_second",
        ticks as f64 / elapsed.as_secs_f64(),
    );
    check.metric("pressure_retries", retries);
    check.metric(
        "upload_commands_bytes_per_tick",
        per_tick(after.upload.commands - before.upload.commands),
    );
    check.metric(
        "upload_tick_parameters_bytes_per_tick",
        per_tick(after.upload.tick_parameters - before.upload.tick_parameters),
    );
    check.metric(
        "upload_chunks_bytes_per_tick",
        per_tick(after.upload.chunks - before.upload.chunks),
    );
    check.metric(
        "upload_metadata_bytes_per_tick",
        per_tick(after.upload.metadata - before.upload.metadata),
    );
    check.metric(
        "readback_results_bytes_per_tick",
        per_tick(after.readback.results - before.readback.results),
    );
    check.metric(
        "readback_events_bytes_per_tick",
        per_tick(after.readback.events - before.readback.events),
    );
    check.metric(
        "readback_diagnostics_bytes_per_tick",
        per_tick(after.readback.diagnostics - before.readback.diagnostics),
    );
    check.metric(
        "readback_timestamps_bytes_per_tick",
        per_tick(after.readback.timestamps - before.readback.timestamps),
    );
    check.metric(
        "readback_captures_bytes_per_tick",
        per_tick(after.readback.captures - before.readback.captures),
    );
    check.metric(
        "device_copy_bytes",
        after.device_copy_bytes - before.device_copy_bytes,
    );
    check.metric(
        "in_flight_high_water",
        u64::from(after.in_flight_high_water),
    );
    check.metric(
        "upload_staging_high_water",
        u64::from(after.upload_staging_high_water),
    );
    check.metric(
        "readback_staging_high_water",
        u64::from(after.readback_staging_high_water),
    );
    check.metric("requested_device_bytes", after.requested_bytes);
    check.metric("peak_requested_device_bytes", after.peak_requested_bytes);
    check.metric(
        "maintenance_waits",
        after.maintenance_waits - before.maintenance_waits,
    );
    if alloc::installed() {
        let (count, bytes) = (
            allocations_after.0 - allocations_before.0,
            allocations_after.1 - allocations_before.1,
        );
        check.metric("process_allocations_per_tick", per_tick(count));
        check.metric("process_allocated_bytes_per_tick", per_tick(bytes));
        check.metric(
            "submit_path_allocations_per_tick",
            per_tick(submit_allocations),
        );
        check.metric("poll_path_allocations_per_tick", per_tick(poll_allocations));
        check.note("allocations include wgpu's and the driver's own inside the port's calls; application-owned GPU buffers created while measuring: 0");
    } else {
        check.note("allocation counting is unavailable outside the gpu-suite binary");
    }
    let latencies: Vec<f64> = driver
        .outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            Outcome::TickCompleted(report) => Some(report.latency.as_secs_f64() * 1000.0),
            _ => None,
        })
        .collect();
    let mut sorted = latencies.clone();
    sorted.sort_by(f64::total_cmp);
    if !sorted.is_empty() {
        check.metric("tick_latency_ms_median", sorted[sorted.len() / 2]);
        check.metric("tick_latency_ms_p95", sorted[sorted.len() * 95 / 100]);
        check.metric("tick_latency_ms_max", sorted[sorted.len() - 1]);
    }
    // Since 03 a tick is three passes; the command pass is the first, the whole tick all three.
    let micros = |time: Duration| time.as_secs_f64() * 1_000_000.0;
    let (gpu_times, tick_times): (Vec<f64>, Vec<f64>) = driver
        .outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            Outcome::TickCompleted(report) => report
                .phases
                .zip(report.gpu_time)
                .map(|(phases, total)| (micros(phases.commands), micros(total))),
            _ => None,
        })
        .unzip();
    match driver.world.report().timestamps {
        TimestampSupport::Enabled { period_ns } => {
            check.that(!gpu_times.is_empty(), || {
                "timestamps enabled but no tick carried one".to_owned()
            });
            if !gpu_times.is_empty() {
                let mut times = gpu_times.clone();
                times.sort_by(f64::total_cmp);
                check.metric("command_pass_us_median", times[times.len() / 2]);
                check.metric("command_pass_us_max", times[times.len() - 1]);
                let mut ticks = tick_times.clone();
                ticks.sort_by(f64::total_cmp);
                check.metric("tick_gpu_us_median", ticks[ticks.len() / 2]);
                check.metric("tick_gpu_us_max", ticks[ticks.len() - 1]);
                check.metric("timestamp_period_ns", f64::from(period_ns));
            }
        }
        other => check.note(format!("pass timestamps: {other:?}")),
    }
    Ok(())
}
