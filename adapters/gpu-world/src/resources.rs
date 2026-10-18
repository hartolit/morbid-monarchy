//! The session's device buffers, pipelines and bind groups.
//!
//! Buffers are created from a checked [`ResourcePlan`] through the context, so their bytes are
//! counted. The window buffers (cells, window map, slot records, flow, scratch and activity) are
//! replaced by a resize; the old ones stay alive and counted until the last submission that used
//! them completes, which is what the reported old/new overlap measures. Pipelines are created
//! once per session from the assembled source and reused for every tick; nothing compiles on the
//! frame path, and a new device (a new session) compiles them again.
//!
//! Five pipeline layouts: the command kernels' (eight storage buffers, WebGPU's default limit),
//! the material phases' (six), the argument kernels', which add the indirect-argument buffer
//! as group 1 so no dispatch binds the buffer it is dispatched from as writable storage, the
//! body commands' (seven) and the body substeps' (eight).

use std::num::NonZeroU64;

use crate::{
    context::{AllocationError, GpuContext},
    layout::{BodyDispatch, BodyParams, DispatchParams, GpuRecord},
    plan::{BODY_DISPATCH_ITERATIONS, BufferRole, ResourcePlan},
    shader::{self, Assembly, ShaderDiagnostic},
    slots::SlotTable,
};

/// Why compiling the session's kernels failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineError {
    /// The assembled source failed offline validation, at a module file and line.
    Source(ShaderDiagnostic),
    /// The device refused a module or pipeline the offline validator accepted.
    Device {
        assembly: &'static str,
        description: String,
    },
}

pub struct WindowBuffers {
    pub cells: wgpu::Buffer,
    pub window_map: wgpu::Buffer,
    pub slot_records: wgpu::Buffer,
    pub flow: wgpu::Buffer,
    pub scratch: wgpu::Buffer,
    pub activity: wgpu::Buffer,
    pub claims: wgpu::Buffer,
}

impl WindowBuffers {
    /// Creates the window buffers. The window map and slot records are written while mapped
    /// at creation from `slots`; the cells start zeroed and are filled by uploads or a
    /// migration copy; flow, scratch and activity start zeroed (no tile awake, no flag set).
    pub fn create(
        context: &mut GpuContext,
        plan: &ResourcePlan,
        slots: &SlotTable,
    ) -> Result<(Self, u64), AllocationError> {
        let cells_plan = plan.buffer(BufferRole::Cells);
        let cells = context.create_buffer("cells", cells_plan.size, cells_plan.usage, false)?;
        let map_plan = plan.buffer(BufferRole::WindowMap);
        let window_map =
            match context.create_buffer("window map", map_plan.size, map_plan.usage, true) {
                Ok(buffer) => buffer,
                Err(error) => {
                    context.release_buffer(cells);
                    return Err(error);
                }
            };
        let records_plan = plan.buffer(BufferRole::SlotRecords);
        let slot_records = match context.create_buffer(
            "slot records",
            records_plan.size,
            records_plan.usage,
            true,
        ) {
            Ok(buffer) => buffer,
            Err(error) => {
                context.release_buffer(cells);
                context.release_buffer(window_map);
                return Err(error);
            }
        };
        {
            let mut entries = vec![0u32; plan.slots as usize];
            slots.write_window_map(&mut entries);
            let mut bytes = Vec::with_capacity(entries.len() * 4);
            for entry in entries {
                bytes.extend_from_slice(&entry.to_le_bytes());
            }
            window_map.get_mapped_range_mut(..).copy_from_slice(&bytes);
            let mut record_bytes = vec![0u8; records_plan.size as usize];
            let stride = crate::layout::ChunkSlotRecord::BYTES as usize;
            for (record, out) in slots
                .slot_records()
                .zip(record_bytes.chunks_exact_mut(stride))
            {
                record.write_bytes(out);
            }
            slot_records
                .get_mapped_range_mut(..)
                .copy_from_slice(&record_bytes);
        }
        window_map.unmap();
        slot_records.unmap();
        let mut zeroed = Vec::with_capacity(4);
        for (role, label) in [
            (BufferRole::Flow, "flow"),
            (BufferRole::Scratch, "scratch"),
            (BufferRole::Activity, "activity"),
            (BufferRole::Claims, "body claims"),
        ] {
            let buffer_plan = plan.buffer(role);
            match context.create_buffer(label, buffer_plan.size, buffer_plan.usage, false) {
                Ok(buffer) => zeroed.push(buffer),
                Err(error) => {
                    for buffer in [cells, window_map, slot_records].into_iter().chain(zeroed) {
                        context.release_buffer(buffer);
                    }
                    return Err(error);
                }
            }
        }
        let [flow, scratch, activity, claims] = <[wgpu::Buffer; 4]>::try_from(zeroed)
            .unwrap_or_else(|_| unreachable!("four buffers were created"));
        Ok((
            Self {
                cells,
                window_map,
                slot_records,
                flow,
                scratch,
                activity,
                claims,
            },
            map_plan.size + records_plan.size,
        ))
    }

    pub fn release(self, context: &mut GpuContext) {
        for buffer in [
            self.cells,
            self.window_map,
            self.slot_records,
            self.flow,
            self.scratch,
            self.activity,
            self.claims,
        ] {
            context.release_buffer(buffer);
        }
    }
}

/// Buffers whose size does not depend on the window.
pub struct FixedBuffers {
    pub tick_params: wgpu::Buffer,
    pub dispatch_params: wgpu::Buffer,
    pub commands: wgpu::Buffer,
    pub results: wgpu::Buffer,
    pub events: wgpu::Buffer,
    pub diagnostics: wgpu::Buffer,
    pub dispatch_args: wgpu::Buffer,
    pub bodies: wgpu::Buffer,
    pub bins: wgpu::Buffer,
    pub body_params: wgpu::Buffer,
    pub body_dispatch: wgpu::Buffer,
    pub timestamps: Option<(wgpu::QuerySet, wgpu::Buffer)>,
}

impl FixedBuffers {
    /// Creates the fixed buffers. The dispatch parameters never change: entry `i` names
    /// command `i`, and body dispatch entry `substep * 16 + iteration` names that pair, both
    /// written once while mapped at creation. Body slots start zeroed: dead.
    pub fn create(
        context: &mut GpuContext,
        plan: &ResourcePlan,
    ) -> Result<(Self, u64), AllocationError> {
        let mut created: Vec<wgpu::Buffer> = Vec::new();
        let make =
            |context: &mut GpuContext, role: BufferRole, label: &'static str, mapped: bool| {
                let buffer_plan = plan.buffer(role);
                context.create_buffer(label, buffer_plan.size, buffer_plan.usage, mapped)
            };
        let result = (|| {
            let tick_params = make(context, BufferRole::TickParams, "tick parameters", false)?;
            created.push(tick_params.clone());
            let dispatch_params = make(
                context,
                BufferRole::DispatchParams,
                "dispatch parameters",
                true,
            )?;
            created.push(dispatch_params.clone());
            let commands = make(context, BufferRole::Commands, "commands", false)?;
            created.push(commands.clone());
            let results = make(context, BufferRole::Results, "results", false)?;
            created.push(results.clone());
            let events = make(context, BufferRole::Events, "events", false)?;
            created.push(events.clone());
            let diagnostics = make(context, BufferRole::Diagnostics, "diagnostics", false)?;
            created.push(diagnostics.clone());
            let dispatch_args = make(
                context,
                BufferRole::DispatchArgs,
                "dispatch arguments",
                false,
            )?;
            created.push(dispatch_args.clone());
            let bodies = make(context, BufferRole::Bodies, "bodies", false)?;
            created.push(bodies.clone());
            let bins = make(context, BufferRole::Bins, "body bins", false)?;
            created.push(bins.clone());
            let body_params = make(context, BufferRole::BodyParams, "body parameters", false)?;
            created.push(body_params.clone());
            let body_dispatch = make(
                context,
                BufferRole::BodyDispatch,
                "body dispatch parameters",
                true,
            )?;
            created.push(body_dispatch.clone());
            let timestamps = if plan.timestamps {
                let resolve = make(
                    context,
                    BufferRole::TimestampResolve,
                    "timestamp resolve",
                    false,
                )?;
                created.push(resolve.clone());
                let query_set = context
                    .device()
                    .create_query_set(&wgpu::QuerySetDescriptor {
                        label: Some("tick pass timestamps"),
                        ty: wgpu::QueryType::Timestamp,
                        count: crate::plan::TIMESTAMP_QUERIES,
                    });
                Some((query_set, resolve))
            } else {
                None
            };
            Ok((
                [
                    tick_params,
                    dispatch_params,
                    commands,
                    results,
                    events,
                    diagnostics,
                    dispatch_args,
                    bodies,
                    bins,
                    body_params,
                    body_dispatch,
                ],
                timestamps,
            ))
        })();
        let (
            [
                tick_params,
                dispatch_params,
                commands,
                results,
                events,
                diagnostics,
                dispatch_args,
                bodies,
                bins,
                body_params,
                body_dispatch,
            ],
            timestamps,
        ) = match result {
            Ok(buffers) => buffers,
            Err(error) => {
                for buffer in created {
                    context.release_buffer(buffer);
                }
                return Err(error);
            }
        };
        let stride = plan.dispatch_param_stride as usize;
        let mut bytes = vec![0u8; dispatch_params.size() as usize];
        for (index, entry) in bytes.chunks_exact_mut(stride).enumerate() {
            let params = DispatchParams {
                command_index: index as u32,
                ..DispatchParams::default()
            };
            params.write_bytes(&mut entry[..DispatchParams::BYTES as usize]);
        }
        dispatch_params
            .get_mapped_range_mut(..)
            .copy_from_slice(&bytes);
        dispatch_params.unmap();
        let stride = plan.body_dispatch_stride as usize;
        let mut body_bytes = vec![0u8; body_dispatch.size() as usize];
        for (index, entry) in body_bytes.chunks_exact_mut(stride).enumerate() {
            let index = index as u32;
            let params = BodyDispatch {
                substep: index / BODY_DISPATCH_ITERATIONS,
                iteration: index % BODY_DISPATCH_ITERATIONS,
                parity: index % 2,
                reserved0: 0,
            };
            params.write_bytes(&mut entry[..BodyDispatch::BYTES as usize]);
        }
        body_dispatch
            .get_mapped_range_mut(..)
            .copy_from_slice(&body_bytes);
        body_dispatch.unmap();
        Ok((
            Self {
                tick_params,
                dispatch_params,
                commands,
                results,
                events,
                diagnostics,
                dispatch_args,
                bodies,
                bins,
                body_params,
                body_dispatch,
                timestamps,
            },
            (bytes.len() + body_bytes.len()) as u64,
        ))
    }

    pub fn release(self, context: &mut GpuContext) {
        context.release_buffer(self.tick_params);
        context.release_buffer(self.dispatch_params);
        context.release_buffer(self.commands);
        context.release_buffer(self.results);
        context.release_buffer(self.events);
        context.release_buffer(self.diagnostics);
        context.release_buffer(self.dispatch_args);
        context.release_buffer(self.bodies);
        context.release_buffer(self.bins);
        context.release_buffer(self.body_params);
        context.release_buffer(self.body_dispatch);
        if let Some((_, resolve)) = self.timestamps {
            context.release_buffer(resolve);
        }
    }
}

/// The compiled kernels and their layouts.
pub struct Pipelines {
    pub world_layout: wgpu::BindGroupLayout,
    pub sim_layout: wgpu::BindGroupLayout,
    pub args_layout: wgpu::BindGroupLayout,
    pub begin_tick: wgpu::ComputePipeline,
    pub apply_edit: wgpu::ComputePipeline,
    pub query_column: wgpu::ComputePipeline,
    pub measure_totals: wgpu::ComputePipeline,
    pub scan_tiles: wgpu::ComputePipeline,
    pub candidate_args: wgpu::ComputePipeline,
    pub wake_tiles: wgpu::ComputePipeline,
    pub process_args: wgpu::ComputePipeline,
    pub propose: wgpu::ComputePipeline,
    pub resolve: wgpu::ComputePipeline,
    pub react: wgpu::ComputePipeline,
    pub commit: wgpu::ComputePipeline,
    /// The pressure-injection kernel, only in sessions with fault injection.
    pub flood_events: Option<wgpu::ComputePipeline>,
    /// `react` built with `fault_unmatched_transfer`, only in sessions with fault injection.
    pub react_unmatched: Option<wgpu::ComputePipeline>,
    pub body_layout: wgpu::BindGroupLayout,
    pub body_command_layout: wgpu::BindGroupLayout,
    pub body: BodyPipelines,
    pub body_commands: BodyCommandPipelines,
    pub created: u64,
}

/// The body substeps' kernels, in the order a substep dispatches them.
pub struct BodyPipelines {
    pub bins_clear: wgpu::ComputePipeline,
    pub body_begin: wgpu::ComputePipeline,
    pub bins_count: wgpu::ComputePipeline,
    pub bins_scan: wgpu::ComputePipeline,
    pub bins_scatter: wgpu::ComputePipeline,
    pub pairs_count: wgpu::ComputePipeline,
    pub pairs_position: wgpu::ComputePipeline,
    pub pairs_velocity: wgpu::ComputePipeline,
    pub pairs_residual: wgpu::ComputePipeline,
    pub claims_clear: wgpu::ComputePipeline,
    pub contact_prepare: wgpu::ComputePipeline,
    pub contact_apply: wgpu::ComputePipeline,
    pub contact_response: wgpu::ComputePipeline,
    pub displace_prepare: wgpu::ComputePipeline,
    pub displace_apply: wgpu::ComputePipeline,
    pub body_end: wgpu::ComputePipeline,
    pub body_observe: wgpu::ComputePipeline,
}

pub struct BodyCommandPipelines {
    pub resolve_target: wgpu::ComputePipeline,
    pub spawn_body: wgpu::ComputePipeline,
    pub remove_body: wgpu::ComputePipeline,
    pub drive_body: wgpu::ComputePipeline,
    pub jump_body: wgpu::ComputePipeline,
    pub attract_bodies: wgpu::ComputePipeline,
    pub lift_bodies: wgpu::ComputePipeline,
}

/// The bind groups of one set of window buffers.
pub struct BindGroups {
    pub world: wgpu::BindGroup,
    pub sim: wgpu::BindGroup,
    pub args: wgpu::BindGroup,
    pub bodies: wgpu::BindGroup,
    pub body_commands: wgpu::BindGroup,
}

fn storage(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform(binding: u32, dynamic: bool, size: u64) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: dynamic,
            min_binding_size: NonZeroU64::new(size),
        },
        count: None,
    }
}

fn compile(context: &GpuContext, assembly: &Assembly) -> Result<wgpu::ShaderModule, PipelineError> {
    assembly.validate().map_err(PipelineError::Source)?;
    let device = context.device();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(assembly.name()),
        source: wgpu::ShaderSource::Wgsl(assembly.source().into()),
    });
    match futures_lite::future::block_on(scope.pop()) {
        Some(error) => Err(PipelineError::Device {
            assembly: assembly.name(),
            description: error.to_string(),
        }),
        None => Ok(module),
    }
}

fn pipeline(
    context: &GpuContext,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    entry_point: &'static str,
) -> Result<wgpu::ComputePipeline, PipelineError> {
    pipeline_with(context, layout, module, entry_point, &[])
}

fn pipeline_with(
    context: &GpuContext,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    entry_point: &'static str,
    constants: &[(&str, f64)],
) -> Result<wgpu::ComputePipeline, PipelineError> {
    let device = context.device();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry_point),
        layout: Some(layout),
        module,
        entry_point: Some(entry_point),
        compilation_options: wgpu::PipelineCompilationOptions {
            constants,
            ..wgpu::PipelineCompilationOptions::default()
        },
        cache: None,
    });
    match futures_lite::future::block_on(scope.pop()) {
        Some(error) => Err(PipelineError::Device {
            assembly: entry_point,
            description: error.to_string(),
        }),
        None => Ok(pipeline),
    }
}

impl Pipelines {
    pub fn create(context: &GpuContext, fault_injection: bool) -> Result<Self, PipelineError> {
        let device = context.device();
        let world_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("world"),
            entries: &[
                uniform(0, false, crate::layout::TickParams::BYTES),
                uniform(1, true, DispatchParams::BYTES),
                storage(2, true),
                storage(3, false),
                storage(4, true),
                storage(5, true),
                storage(6, false),
                storage(7, false),
                storage(8, false),
                storage(9, false),
            ],
        });
        let sim_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material phases"),
            entries: &[
                uniform(0, false, crate::layout::TickParams::BYTES),
                storage(1, false),
                storage(2, true),
                storage(3, false),
                storage(4, false),
                storage(5, false),
                storage(6, false),
            ],
        });
        let args_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dispatch arguments"),
            entries: &[storage(0, false)],
        });
        let body_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("body substeps"),
            entries: &[
                uniform(0, false, crate::layout::TickParams::BYTES),
                uniform(1, false, BodyParams::BYTES),
                uniform(2, true, BodyDispatch::BYTES),
                storage(3, false),
                storage(4, true),
                storage(5, false),
                storage(6, false),
                storage(7, false),
                storage(8, false),
                storage(9, false),
                storage(10, false),
            ],
        });
        let body_command_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("body commands"),
                entries: &[
                    uniform(0, false, crate::layout::TickParams::BYTES),
                    uniform(1, true, DispatchParams::BYTES),
                    uniform(2, false, BodyParams::BYTES),
                    storage(3, true),
                    storage(4, false),
                    storage(5, true),
                    storage(6, true),
                    storage(7, false),
                    storage(8, false),
                    storage(9, false),
                ],
            });
        let layout = |label, groups: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: groups,
                immediate_size: 0,
            })
        };
        let world_pipeline_layout = layout("world", &[Some(&world_layout)]);
        let sim_pipeline_layout = layout("material phases", &[Some(&sim_layout)]);
        let args_pipeline_layout = layout(
            "dispatch arguments",
            &[Some(&sim_layout), Some(&args_layout)],
        );
        let body_pipeline_layout = layout("body substeps", &[Some(&body_layout)]);
        let body_command_pipeline_layout = layout("body commands", &[Some(&body_command_layout)]);

        let world = compile(context, &shader::world_assembly())?;
        let sim = compile(context, &shader::sim_assembly())?;
        let w = |entry| pipeline(context, &world_pipeline_layout, &world, entry);
        let m = |entry| pipeline(context, &sim_pipeline_layout, &sim, entry);
        let a = |entry| pipeline(context, &args_pipeline_layout, &sim, entry);
        let begin_tick = w("begin_tick")?;
        let apply_edit = w("apply_edit")?;
        let query_column = w("query_column")?;
        let measure_totals = w("measure_totals")?;
        let scan_tiles = m("scan_tiles")?;
        let wake_tiles = m("wake_tiles")?;
        let propose = m("propose")?;
        let resolve = m("resolve")?;
        let react = m("react")?;
        let commit = m("commit")?;
        let candidate_args = a("candidate_args")?;
        let process_args = a("process_args")?;
        let bodies = compile(context, &shader::body_assembly())?;
        let body_commands = compile(context, &shader::body_command_assembly())?;
        let s = |entry| pipeline(context, &body_pipeline_layout, &bodies, entry);
        let c = |entry| {
            pipeline(
                context,
                &body_command_pipeline_layout,
                &body_commands,
                entry,
            )
        };
        let body = BodyPipelines {
            bins_clear: s("bins_clear")?,
            body_begin: s("body_begin")?,
            bins_count: s("bins_count")?,
            bins_scan: s("bins_scan")?,
            bins_scatter: s("bins_scatter")?,
            pairs_count: s("pairs_count")?,
            pairs_position: s("pairs_position")?,
            pairs_velocity: s("pairs_velocity")?,
            pairs_residual: s("pairs_residual")?,
            claims_clear: s("claims_clear")?,
            contact_prepare: s("contact_prepare")?,
            contact_apply: s("contact_apply")?,
            contact_response: s("contact_response")?,
            displace_prepare: s("displace_prepare")?,
            displace_apply: s("displace_apply")?,
            body_end: s("body_end")?,
            body_observe: s("body_observe")?,
        };
        let body_commands = BodyCommandPipelines {
            resolve_target: c("resolve_target")?,
            spawn_body: c("spawn_body")?,
            remove_body: c("remove_body")?,
            drive_body: c("drive_body")?,
            jump_body: c("jump_body")?,
            attract_bodies: c("attract_bodies")?,
            lift_bodies: c("lift_bodies")?,
        };
        let mut created = 12 + 17 + 7;
        let (flood_events, react_unmatched) = if fault_injection {
            let faults = compile(context, &shader::fault_assembly())?;
            created += 2;
            (
                Some(pipeline(
                    context,
                    &world_pipeline_layout,
                    &faults,
                    "flood_events",
                )?),
                Some(pipeline_with(
                    context,
                    &sim_pipeline_layout,
                    &sim,
                    "react",
                    &[("fault_unmatched_transfer", 1.0)],
                )?),
            )
        } else {
            (None, None)
        };
        Ok(Self {
            world_layout,
            sim_layout,
            args_layout,
            begin_tick,
            apply_edit,
            query_column,
            measure_totals,
            scan_tiles,
            candidate_args,
            wake_tiles,
            process_args,
            propose,
            resolve,
            react,
            commit,
            flood_events,
            react_unmatched,
            body_layout,
            body_command_layout,
            body,
            body_commands,
            created,
        })
    }

    pub fn bind_groups<'a>(
        &self,
        context: &GpuContext,
        fixed: &'a FixedBuffers,
        window: &'a WindowBuffers,
    ) -> BindGroups {
        fn whole(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: buffer.as_entire_binding(),
            }
        }
        let device = context.device();
        let world = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("world"),
            layout: &self.world_layout,
            entries: &[
                whole(0, &fixed.tick_params),
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &fixed.dispatch_params,
                        offset: 0,
                        size: NonZeroU64::new(DispatchParams::BYTES),
                    }),
                },
                whole(2, &fixed.commands),
                whole(3, &window.cells),
                whole(4, &window.window_map),
                whole(5, &window.slot_records),
                whole(6, &fixed.results),
                whole(7, &fixed.events),
                whole(8, &window.activity),
                whole(9, &fixed.diagnostics),
            ],
        });
        let sim = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("material phases"),
            layout: &self.sim_layout,
            entries: &[
                whole(0, &fixed.tick_params),
                whole(1, &window.cells),
                whole(2, &window.window_map),
                whole(3, &window.flow),
                whole(4, &window.scratch),
                whole(5, &window.activity),
                whole(6, &fixed.diagnostics),
            ],
        });
        let args = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dispatch arguments"),
            layout: &self.args_layout,
            entries: &[whole(0, &fixed.dispatch_args)],
        });
        let dynamic = |binding: u32, buffer: &'a wgpu::Buffer, size: u64| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: NonZeroU64::new(size),
            }),
        };
        let bodies = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("body substeps"),
            layout: &self.body_layout,
            entries: &[
                whole(0, &fixed.tick_params),
                whole(1, &fixed.body_params),
                dynamic(2, &fixed.body_dispatch, BodyDispatch::BYTES),
                whole(3, &window.cells),
                whole(4, &window.window_map),
                whole(5, &fixed.bodies),
                whole(6, &fixed.bins),
                whole(7, &window.claims),
                whole(8, &window.activity),
                whole(9, &fixed.diagnostics),
                whole(10, &fixed.events),
            ],
        });
        let body_commands = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("body commands"),
            layout: &self.body_command_layout,
            entries: &[
                whole(0, &fixed.tick_params),
                dynamic(1, &fixed.dispatch_params, DispatchParams::BYTES),
                whole(2, &fixed.body_params),
                whole(3, &fixed.commands),
                whole(4, &fixed.results),
                whole(5, &window.cells),
                whole(6, &window.window_map),
                whole(7, &fixed.bodies),
                whole(8, &window.activity),
                whole(9, &fixed.diagnostics),
            ],
        });
        BindGroups {
            world,
            sim,
            args,
            bodies,
            body_commands,
        }
    }
}

/// Window buffers replaced by a resize, kept until `serial` (the last submission that used
/// them) completes.
pub struct Retired {
    pub buffers: WindowBuffers,
    pub serial: u64,
}
