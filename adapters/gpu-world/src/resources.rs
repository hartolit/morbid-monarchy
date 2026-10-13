//! The session's device buffers, pipelines and bind group.
//!
//! Buffers are created from a checked [`ResourcePlan`] through the context, so their bytes are
//! counted. The window buffers (cells, window map, slot records) are replaced by a resize; the
//! old ones stay alive and counted until the last submission that used them completes, which
//! is what the reported old/new overlap measures. Pipelines are created once per session from
//! the assembled source and reused for every tick; nothing compiles on the frame path, and a
//! new device (a new session) compiles them again.

use std::num::NonZeroU64;

use crate::{
    context::{AllocationError, GpuContext},
    layout::{DispatchParams, GpuRecord},
    plan::{BufferRole, ResourcePlan},
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
}

impl WindowBuffers {
    /// Creates the window buffers. The window map and slot records are written while mapped
    /// at creation from `slots`; the cells start zeroed and are filled by uploads or a
    /// migration copy.
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
        Ok((
            Self {
                cells,
                window_map,
                slot_records,
            },
            map_plan.size + records_plan.size,
        ))
    }

    pub fn release(self, context: &mut GpuContext) {
        context.release_buffer(self.cells);
        context.release_buffer(self.window_map);
        context.release_buffer(self.slot_records);
    }
}

/// Buffers whose size does not depend on the window.
pub struct FixedBuffers {
    pub tick_params: wgpu::Buffer,
    pub dispatch_params: wgpu::Buffer,
    pub commands: wgpu::Buffer,
    pub results: wgpu::Buffer,
    pub events: wgpu::Buffer,
    pub timestamps: Option<(wgpu::QuerySet, wgpu::Buffer)>,
}

impl FixedBuffers {
    /// Creates the fixed buffers. The dispatch parameters never change: entry `i` names
    /// command `i`, written once while mapped at creation.
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
                        count: 2,
                    });
                Some((query_set, resolve))
            } else {
                None
            };
            Ok((
                tick_params,
                dispatch_params,
                commands,
                results,
                events,
                timestamps,
            ))
        })();
        let (tick_params, dispatch_params, commands, results, events, timestamps) = match result {
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
        Ok((
            Self {
                tick_params,
                dispatch_params,
                commands,
                results,
                events,
                timestamps,
            },
            bytes.len() as u64,
        ))
    }

    pub fn release(self, context: &mut GpuContext) {
        context.release_buffer(self.tick_params);
        context.release_buffer(self.dispatch_params);
        context.release_buffer(self.commands);
        context.release_buffer(self.results);
        context.release_buffer(self.events);
        if let Some((_, resolve)) = self.timestamps {
            context.release_buffer(resolve);
        }
    }
}

/// The compiled kernels and their shared layout.
pub struct Pipelines {
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub begin_tick: wgpu::ComputePipeline,
    pub apply_edit: wgpu::ComputePipeline,
    pub query_column: wgpu::ComputePipeline,
    /// The pressure-injection kernel, only in sessions with fault injection.
    pub flood_events: Option<wgpu::ComputePipeline>,
    pub created: u64,
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
    let device = context.device();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry_point),
        layout: Some(layout),
        module,
        entry_point: Some(entry_point),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
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
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("world"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let world = compile(context, &shader::world_assembly())?;
        let begin_tick = pipeline(context, &layout, &world, "begin_tick")?;
        let apply_edit = pipeline(context, &layout, &world, "apply_edit")?;
        let query_column = pipeline(context, &layout, &world, "query_column")?;
        let mut created = 3;
        let flood_events = if fault_injection {
            let faults = compile(context, &shader::fault_assembly())?;
            created += 1;
            Some(pipeline(context, &layout, &faults, "flood_events")?)
        } else {
            None
        };
        Ok(Self {
            bind_group_layout,
            begin_tick,
            apply_edit,
            query_column,
            flood_events,
            created,
        })
    }

    pub fn bind_group(
        &self,
        context: &GpuContext,
        fixed: &FixedBuffers,
        window: &WindowBuffers,
    ) -> wgpu::BindGroup {
        fn whole(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: buffer.as_entire_binding(),
            }
        }
        context
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("world"),
                layout: &self.bind_group_layout,
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
                ],
            })
    }
}

/// Window buffers replaced by a resize, kept until `serial` (the last submission that used
/// them) completes.
pub struct Retired {
    pub buffers: WindowBuffers,
    pub serial: u64,
}
