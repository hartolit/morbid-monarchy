//! Checkpoint A: adapter selection, requested limits, limited capability and allocation plans.

use super::*;

pub(super) fn adapter_selection(adapter: &str, check: &mut Check) -> Result<(), String> {
    let inventory = AdapterInventory::enumerate();
    let ids: Vec<String> = inventory
        .records()
        .map(|record| record.id.clone())
        .collect();
    check.that(ids.iter().any(|id| id == adapter), || {
        format!("{adapter} is not enumerated: {ids:?}")
    });
    {
        let driver = Driver::open(config(adapter, 1))?;
        let report = driver.world.report();
        check.equal("selected adapter", report.adapter.id.as_str(), adapter);
        check.that(!report.adapter.software, || {
            "a hardware request opened a software adapter".to_owned()
        });
        check.metric("adapter", report.adapter.name.clone());
        check.metric("backend", report.adapter.backend.clone());
        check.metric(
            "driver",
            format!("{} {}", report.adapter.driver, report.adapter.driver_info),
        );
    }
    match GpuWorld::open(config("vulkan-ffff-ffff", 1)) {
        Err(OpenError::Context(ContextError::AdapterNotFound { available, .. })) => {
            check.equal("available adapters listed", available, ids.clone());
        }
        other => check.fail(format!(
            "a missing adapter was not refused: {:?}",
            other.err()
        )),
    }
    match inventory.records().find(|record| record.software) {
        Some(software) => match GpuWorld::open(config(&software.id, 1)) {
            Err(OpenError::Context(ContextError::SoftwareAdapterRefused(id))) => {
                check.equal("refused software adapter", id, software.id.clone());
            }
            other => check.fail(format!(
                "software adapter opened without consent: {:?}",
                other.err()
            )),
        },
        None => check.note("no software adapter is enumerated here; its refusal is not exercised"),
    }
    Ok(())
}

pub(super) fn requested_limits(adapter: &str, check: &mut Check) -> Result<(), String> {
    let driver = Driver::open(config(adapter, 1))?;
    let report = driver.world.report();
    let defaults = gpu_world::plan::PlanLimits::from_wgpu(&wgpu::Limits::default());
    let needed = 138_444_800u64;
    let expected_binding = needed
        .max(defaults.max_storage_binding)
        .min(report.adapter_limits.max_storage_binding);
    check.equal(
        "granted storage binding",
        report.granted_limits.max_storage_binding,
        expected_binding,
    );
    check.equal(
        "granted buffer size",
        report.granted_limits.max_buffer_size,
        needed
            .max(defaults.max_buffer_size)
            .min(report.adapter_limits.max_buffer_size),
    );
    check.equal(
        "features without timestamps",
        report.features.clone(),
        Vec::<String>::new(),
    );
    check.equal(
        "default workgroup limit",
        report.granted_limits.max_invocations_per_workgroup,
        defaults.max_invocations_per_workgroup,
    );
    check.metric(
        "adapter_max_storage_binding",
        report.adapter_limits.max_storage_binding,
    );
    check.metric(
        "granted_max_storage_binding",
        report.granted_limits.max_storage_binding,
    );
    check.metric(
        "adapter_max_buffer_size",
        report.adapter_limits.max_buffer_size,
    );
    check.metric(
        "granted_max_buffer_size",
        report.granted_limits.max_buffer_size,
    );
    drop(driver);

    let mut timed = config(adapter, 1);
    timed.timestamps = true;
    let world = opened(GpuWorld::open(timed))?;
    let report = world.report();
    let has = report
        .adapter
        .features
        .iter()
        .any(|feature| feature == "TIMESTAMP_QUERY");
    match report.timestamps {
        TimestampSupport::Enabled { period_ns } => {
            check.that(has, || {
                "timestamps enabled on an adapter without TIMESTAMP_QUERY".to_owned()
            });
            check.equal(
                "requested features",
                report.features.clone(),
                vec!["TIMESTAMP_QUERY".to_owned()],
            );
            check.metric("timestamp_period_ns", f64::from(period_ns));
        }
        TimestampSupport::Unavailable => {
            check.that(!has, || {
                "TIMESTAMP_QUERY is supported but was not enabled".to_owned()
            });
            check.note("timestamps unavailable: the adapter lacks TIMESTAMP_QUERY");
        }
        TimestampSupport::NotRequested => check.fail("timestamps were requested"),
    }
    Ok(())
}

pub(super) fn limited_capability(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut largest = config(adapter, 32);
    largest.limit_ceiling = LimitCeiling::WebGpuDefaults;
    match GpuWorld::open(largest) {
        Err(OpenError::Plan(PlanError::Limit { limit, .. })) => check.equal(
            "radius 32 under default limits",
            limit,
            CapacityLimit {
                resource: LimitedResource::StorageBinding,
                requested: 138_444_800,
                available: 128 << 20,
            },
        ),
        other => check.fail(format!("radius 32 was not refused: {:?}", other.err())),
    }
    let mut fits = config(adapter, 31);
    fits.limit_ceiling = LimitCeiling::WebGpuDefaults;
    let mut driver = Driver::new(opened(GpuWorld::open(fits))?);
    check.equal(
        "granted binding under default limits",
        driver.world.report().granted_limits.max_storage_binding,
        128 << 20,
    );
    let key = ChunkKey::new(31, -31, 0);
    let slot = driver.upload(key, &flat(40))?;
    let origin = key.origin_cell();
    driver.run(&[edit(EditTool::Raise, origin.x + 5, origin.y + 5, 2, 3)])?;
    let before = driver.world.counters().requested_bytes;
    let session = driver.session();
    let refused = driver.world.resize(&ResizeRequest {
        session,
        window: ResidentWindow::new(ChunkKey::new(0, 0, 0), 32, 32).expect("window"),
    });
    check.equal(
        "resize to radius 32 under default limits",
        refused.err(),
        Some(Refusal::Unsupported(CapacityLimit {
            resource: LimitedResource::StorageBinding,
            requested: 138_444_800,
            available: 128 << 20,
        })),
    );
    check.equal(
        "bytes after refused resize",
        driver.world.counters().requested_bytes,
        before,
    );
    let cells = driver.capture(slot)?;
    check.equal(
        "edited column survives",
        cells[local_index(origin.x + 5, origin.y + 5)].elevation(),
        43,
    );
    check.equal("unedited column survives", cells[0].elevation(), 40);
    driver.run(&[edit(EditTool::Lower, origin.x + 5, origin.y + 5, 0, 1)])?;
    let after = driver.query(IVec2::new(origin.x + 5, origin.y + 5))?;
    check.equal(
        "world still runs after the refusal",
        column(after).map(|cell| cell.elevation()),
        Some(42),
    );
    check.metric("radius_31_device_bytes", driver.world.plan().device_bytes);
    Ok(())
}

pub(super) fn radius_32_allocation(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 32))?;
    let plan = driver.world.plan().clone();
    check.equal(
        "cell buffer bytes",
        plan.buffer(gpu_world::plan::BufferRole::Cells).size,
        138_444_800,
    );
    check.metric("device_bytes", plan.device_bytes);
    check.metric("staging_bytes", plan.staging_bytes);
    check.metric("terrain_bytes", plan.terrain_bytes);
    check.metric("requested_bytes", driver.world.counters().requested_bytes);
    for (key, seed) in [
        (ChunkKey::new(32, 32, 0), 3),
        (ChunkKey::new(-32, -32, 0), 4),
    ] {
        let cells = pattern(seed);
        let slot = driver.upload(key, &cells)?;
        let origin = key.origin_cell();
        let results = driver.run(&[
            edit(EditTool::Raise, origin.x + 63, origin.y + 63, 0, 1),
            query(origin.x + 63, origin.y + 63),
        ])?;
        let expected = reference_edit(cells[CHUNK_CELL_COUNT - 1], EditTool::Raise, 1);
        check.equal("far-corner query", column(results[1]), Some(expected));
        let mut wanted = cells.clone();
        wanted[CHUNK_CELL_COUNT - 1] = expected;
        check.that(driver.capture(slot)? == wanted, || {
            format!("capture of {key:?} differs")
        });
    }
    Ok(())
}

pub(super) fn invalid_configuration(adapter: &str, check: &mut Check) -> Result<(), String> {
    let broken = [
        TransferConfig {
            max_in_flight: 0,
            ..TransferConfig::default()
        },
        TransferConfig {
            max_in_flight: 4,
            ..TransferConfig::default()
        },
        TransferConfig {
            commands_per_tick: 0,
            ..TransferConfig::default()
        },
        TransferConfig {
            chunk_upload_slots: 97,
            ..TransferConfig::default()
        },
    ];
    for transfer in broken {
        let mut request = config(adapter, 1);
        request.transfer = transfer;
        check.that(
            matches!(
                GpuWorld::open(request),
                Err(OpenError::Plan(PlanError::Config { .. }))
            ),
            || format!("{transfer:?} was not refused"),
        );
    }
    for (budgets, resource) in [
        (
            Budgets {
                staging_bytes: 1 << 20,
                ..Budgets::default()
            },
            LimitedResource::StagingBudget,
        ),
        (
            Budgets {
                device_bytes: Some(1 << 20),
                ..Budgets::default()
            },
            LimitedResource::DeviceBytes,
        ),
    ] {
        let mut request = config(adapter, 1);
        request.budgets = budgets;
        match GpuWorld::open(request) {
            Err(OpenError::Plan(PlanError::Limit { limit, .. })) => {
                check.equal("refused budget", limit.resource, resource);
            }
            other => check.fail(format!(
                "{resource:?} budget not enforced: {:?}",
                other.err()
            )),
        }
    }
    Ok(())
}
