//! The `transfers` GPU suite: the device context, resource plans, the packed ABI, ordered
//! commands and queries, bounded transfers, fault handling and steady-state reuse, executed on
//! the selected adapter through `gpu-world` and `world-runtime`.

use std::time::{Duration, Instant};

use gpu_world::{
    commands::stroke_cell_count,
    context::{AdapterInventory, ContextError, LimitCeiling},
    layout::{self, GpuRecord, RECORDS},
    plan::{Budgets, PlanError, TransferConfig},
    shader::{SourceModule, generated_declarations},
    world::{GpuWorld, OpenError, TimestampSupport},
};
use world_model::{
    body::BodyPreset,
    cell::WorldCell,
    command::{BodySpawn, Command, EditTool, MaterialEdit},
    generation::WorldGenerator,
    glam::IVec2,
    material::{FluidMat, GranularMat, SurfaceMat},
    spatial::{CHUNK_CELL_COUNT, CHUNK_SIZE, ChunkKey},
};
use world_runtime::{
    ids::{ChunkRevision, CommandId, SessionEpoch, SessionId, Tick},
    ledger::ResidencyOutcome,
    port::{
        AbandonReason, AdmittedCommand, CapacityLimit, CommandResult, LimitedResource, Obligation,
        Outcome, PayloadError, PhysicalWorld, PortHealth, Pressure, Refusal, ResidentWindow,
        ResizeRequest, SlotRef, TickBatch,
    },
    receipts::{Admission, AdmissionError, ReceiptState},
    runtime::{Runtime, RuntimeConfig},
};

use super::{
    Case, Check, alloc,
    driver::{Driver, config, flat, pattern},
    probe::{self, Binding, CELL_PROBE_WORDS, SENTINEL},
};

mod abi;
mod commands;
mod lifecycle;
mod plan;
mod steady;

pub const CASES: &[Case] = &[
    Case {
        name: "adapter-selection",
        summary: "the named adapter is opened, a missing one is refused, a software one needs consent",
        run: plan::adapter_selection,
    },
    Case {
        name: "requested-limits",
        summary: "the device is granted the limits the largest supported plan needs, no more",
        run: plan::requested_limits,
    },
    Case {
        name: "limited-capability",
        summary: "with default limits radius 32 is refused at open and resize, radius 31 runs",
        run: plan::limited_capability,
    },
    Case {
        name: "radius-32-allocation",
        summary: "a radius-32 world allocates, edits and captures at its far corners",
        run: plan::radius_32_allocation,
    },
    Case {
        name: "invalid-configuration",
        summary: "unsupported transfer sizes and budgets are refused before allocation",
        run: plan::invalid_configuration,
    },
    Case {
        name: "cell-layout-probe",
        summary: "kernels decode and set every packed field as world-model does, at partial workgroups",
        run: abi::cell_layout_probe,
    },
    Case {
        name: "record-layout-probe",
        summary: "every shared record reads back word for word on the device",
        run: abi::record_layout_probe,
    },
    Case {
        name: "layout-specimens-detected",
        summary: "a shifted cell field and swapped record fields are detected by the probes",
        run: abi::layout_specimens_detected,
    },
    Case {
        name: "upload-capture-roundtrip",
        summary: "uploaded chunks come back byte for byte, including negative keys",
        run: commands::upload_capture_roundtrip,
    },
    Case {
        name: "ordered-edit-query",
        summary: "edits apply in command order, queries see earlier edits, strokes cross seams",
        run: commands::ordered_edit_query,
    },
    Case {
        name: "edit-tools-match-rules",
        summary: "every brush tool matches the CPU column rules on arbitrary and generated cells",
        run: commands::edit_tools_match_rules,
    },
    Case {
        name: "edit-partial-workgroups",
        summary: "strokes of every radius change exactly their footprint",
        run: commands::edit_partial_workgroups,
    },
    Case {
        name: "edit-outside-window",
        summary: "cells outside the window or in missing chunks are counted and never written",
        run: commands::edit_outside_window,
    },
    Case {
        name: "empty-batches",
        summary: "ticks without commands complete with no command outcomes",
        run: commands::empty_batches,
    },
    Case {
        name: "command-count-boundaries",
        summary: "1, 63, 64, 65, 255 and 256 commands complete; 257 is refused",
        run: commands::command_count_boundaries,
    },
    Case {
        name: "runtime-retry-conflict-expiry",
        summary: "a retried identity applies once, a reused one conflicts, an acknowledged one expires",
        run: commands::runtime_retry_conflict_expiry,
    },
    Case {
        name: "stale-session-and-slot",
        summary: "retired slots and other sessions' references are refused",
        run: commands::stale_session_and_slot,
    },
    Case {
        name: "out-of-bounds-payload",
        summary: "malformed or misplaced requests are refused with their reason",
        run: commands::out_of_bounds_payload,
    },
    Case {
        name: "full-receipts-before-admission",
        summary: "full receipt and command queues refuse admission and apply nothing",
        run: commands::full_receipts_before_admission,
    },
    Case {
        name: "in-flight-backpressure",
        summary: "a fourth uncompleted tick is refused at once, without waiting",
        run: lifecycle::in_flight_backpressure,
    },
    Case {
        name: "staging-exhaustion-and-reuse",
        summary: "full upload and capture pools refuse, then reuse their buffers",
        run: lifecycle::staging_exhaustion_and_reuse,
    },
    Case {
        name: "delayed-delivery",
        summary: "held map and completion callbacks bound admission and deliver later intact",
        run: lifecycle::delayed_delivery,
    },
    Case {
        name: "out-of-order-delivery",
        summary: "reversed deliveries complete every receipt and keep the latest observed tick",
        run: lifecycle::out_of_order_delivery,
    },
    Case {
        name: "event-overflow-and-reset",
        summary: "a flooded event buffer reports its drops and starts empty next tick",
        run: lifecycle::event_overflow_and_reset,
    },
    Case {
        name: "resize-success-and-refusal",
        summary: "a resize migrates resident chunks on the device; refused resizes change nothing",
        run: lifecycle::resize_success_and_refusal,
    },
    Case {
        name: "device-loss-and-recreation",
        summary: "a lost device abandons every obligation; a new session refuses old handles",
        run: lifecycle::device_loss_and_recreation,
    },
    Case {
        name: "stop-and-drain",
        summary: "an orderly stop delivers outstanding work; a stalled one times out visibly",
        run: lifecycle::stop_and_drain,
    },
    Case {
        name: "steady-state-reuse",
        summary: "1,200 ticks of edits, queries and residency churn reuse every resource",
        run: steady::steady_state_reuse,
    },
];

fn edit(tool: EditTool, x: i32, y: i32, radius: i32, strength: u8) -> Command {
    Command::EditMaterial(
        MaterialEdit::new(tool, IVec2::new(x, y), radius, strength).expect("supported edit"),
    )
}

fn query(x: i32, y: i32) -> Command {
    Command::QueryColumn {
        cell: IVec2::new(x, y),
    }
}

fn column(result: CommandResult) -> Option<WorldCell> {
    match result {
        CommandResult::Column { column, .. } => Some(column),
        _ => None,
    }
}

/// The cell index of absolute cell `(x, y)` within its chunk.
fn local_index(x: i32, y: i32) -> usize {
    let size = CHUNK_SIZE as i32;
    (y.rem_euclid(size) * size + x.rem_euclid(size)) as usize
}

/// The baseline brush (`morbid-app` `apply_brush`) expressed with world-model's column rules:
/// an independent CPU reference for the edit kernel.
fn reference_edit(mut cell: WorldCell, tool: EditTool, strength: u8) -> WorldCell {
    if let Some(fluid) = tool.fluid() {
        cell.inject_fluid(fluid, u16::from(strength));
        return cell;
    }
    match tool {
        EditTool::Fire => {
            let fluid = cell.fluid_mat();
            if cell.surface_mat() != SurfaceMat::SURFACE_FIRE
                && (fluid == FluidMat::EMPTY || fluid == FluidMat::FLUID_OIL)
            {
                cell.set_surface_mat(SurfaceMat::SURFACE_FIRE);
                cell.set_surface_state(0);
            }
        }
        EditTool::Sand => {
            let is_sand = cell.granular_mat() == GranularMat::GRANULAR_SAND;
            let old = if is_sand { cell.granular_vol() } else { 0 };
            let new = (old + u16::from(strength))
                .min(WorldCell::MAX_ELEVATION as u16 + WorldCell::MAX_GRANULAR_VOL);
            if !is_sand || old != new {
                cell.set_granular_mat(GranularMat::GRANULAR_SAND);
                cell.set_granular_vol(new);
            }
        }
        EditTool::Raise => {
            let raised =
                (u32::from(cell.elevation()) + u32::from(strength)).min(WorldCell::MAX_ELEVATION);
            cell.set_elevation(raised as u16);
        }
        EditTool::Lower => cell.set_elevation(cell.elevation().saturating_sub(u16::from(strength))),
        EditTool::Water | EditTool::Lava | EditTool::Oil | EditTool::Ice => {}
    }
    cell
}

fn opened(result: Result<GpuWorld, OpenError>) -> Result<GpuWorld, String> {
    result.map_err(|error| format!("open: {error:?}"))
}

pub(super) fn runtime_world(
    adapter: &str,
    config_change: impl FnOnce(&mut gpu_world::world::GpuWorldConfig),
    runtime: RuntimeConfig,
) -> Result<Runtime, String> {
    let mut request = config(adapter, 1);
    config_change(&mut request);
    let world = opened(GpuWorld::open(request))?;
    Runtime::new(Box::new(world), runtime).map_err(|error| format!("runtime: {error:?}"))
}

/// Runs frames of one tick until `done` or the device patience runs out.
pub(super) fn frames_until(
    runtime: &mut Runtime,
    what: &str,
    mut done: impl FnMut(&mut Runtime) -> bool,
) -> Result<(), String> {
    let started = Instant::now();
    loop {
        runtime.frame_ticks(1);
        if done(runtime) {
            return Ok(());
        }
        if started.elapsed() > super::driver::DEVICE_PATIENCE {
            return Err(format!("{what}: not done"));
        }
        std::thread::sleep(Duration::from_micros(200));
    }
}

pub(super) fn runtime_upload(
    runtime: &mut Runtime,
    key: ChunkKey,
    cells: &[WorldCell],
) -> Result<SlotRef, String> {
    let slot = runtime
        .reserve_slot(key)
        .map_err(|refusal| format!("reserve: {refusal:?}"))?;
    runtime
        .upload_chunk(slot, ChunkRevision::new(1), cells)
        .map_err(|error| format!("upload: {error:?}"))?;
    let mut drained = Vec::new();
    frames_until(runtime, "activation", |runtime| {
        runtime.drain_residency(&mut drained);
        drained.iter().any(|outcome| match outcome {
            ResidencyOutcome::UploadActivated { ticket, .. } => ticket.slot == slot,
            _ => false,
        })
    })?;
    Ok(slot)
}

pub(super) fn completed(runtime: &Runtime, id: CommandId) -> Option<CommandResult> {
    match runtime.receipt(id)?.state {
        ReceiptState::Completed { result, .. } => Some(result),
        _ => None,
    }
}
