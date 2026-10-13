//! Layout probes: kernels that read the shared declarations on the device and write back what
//! they read, so the CPU can compare it with what it wrote.
//!
//! A probe is a layout test, not a simulation. It is assembled from `gpu-world`'s exported
//! modules (the generated declarations and the cell accessors) like any other consumer of the
//! layout would be, runs on the session's own device through its submission timeline, and
//! reads the resident cell buffer through the read-only world view. Probes run only here.

use std::{
    borrow::Cow,
    fmt::Write as _,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use gpu_world::{
    layout::{RecordLayout, WordType},
    shader::{Assembly, SourceModule},
    world::GpuWorld,
};

/// Words the cell probe writes per cell: 11 fields, then 9 setters × 2 words.
pub const CELL_PROBE_WORDS: u32 = 29;
/// What probe outputs hold before a kernel writes them.
pub const SENTINEL: u32 = 0xDEAD_BEEF;

const CELL_PROBE: &str = r#"
struct ProbeParams {
    first: u32,
    count: u32,
    reserved0: u32,
    reserved1: u32,
}

@group(0) @binding(0) var<storage, read> probe_cells: array<vec2<u32>>;
@group(0) @binding(1) var<storage, read_write> probe_fields: array<u32>;
@group(0) @binding(2) var<storage, read_write> probe_sums: array<u32>;
@group(0) @binding(3) var<uniform> probe_params: ProbeParams;

var<workgroup> probe_partial: array<u32, 64>;

fn probe_set(base: u32, slot: u32, cell: vec2<u32>) {
    probe_fields[base + 11u + slot * 2u] = cell.x;
    probe_fields[base + 12u + slot * 2u] = cell.y;
}

// One invocation per cell; inactive invocations of the last workgroup still reach every
// barrier and contribute zero to the elevation sum.
@compute @workgroup_size(64, 1, 1)
fn probe_cells_kernel(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) lid: u32,
    @builtin(workgroup_id) wid: vec3<u32>,
) {
    var elevation = 0u;
    if (gid.x < probe_params.count) {
        let c = probe_cells[probe_params.first + gid.x];
        let base = gid.x * 29u;
        probe_fields[base + 0u] = cell_terrain_material(c);
        probe_fields[base + 1u] = cell_surface_material(c);
        probe_fields[base + 2u] = cell_granular_material(c);
        probe_fields[base + 3u] = cell_fluid_material(c);
        probe_fields[base + 4u] = cell_variants(c);
        probe_fields[base + 5u] = cell_elevation(c);
        probe_fields[base + 6u] = cell_fluid_volume(c);
        probe_fields[base + 7u] = cell_granular_volume(c);
        probe_fields[base + 8u] = cell_surface_state(c);
        probe_fields[base + 9u] = cell_terrain_state(c);
        probe_fields[base + 10u] = cell_compass(c);
        probe_set(base, 0u, cell_with_terrain_material(c, CELL_TERRAIN_MATERIAL_MASK));
        probe_set(base, 1u, cell_with_surface_material(c, CELL_SURFACE_MATERIAL_MASK));
        probe_set(base, 2u, cell_with_granular_material(c, CELL_GRANULAR_MATERIAL_MASK));
        probe_set(base, 3u, cell_with_fluid_material(c, CELL_FLUID_MATERIAL_MASK));
        probe_set(base, 4u, cell_with_elevation(c, 0u));
        probe_set(base, 5u, cell_with_fluid_volume(c, CELL_FLUID_VOLUME_MASK));
        probe_set(base, 6u, cell_with_granular_volume(c, 0u));
        probe_set(base, 7u, cell_with_surface_state(c, CELL_SURFACE_STATE_MASK));
        probe_set(base, 8u, cell_with_compass(c, 0u));
        elevation = cell_elevation(c);
    }
    probe_partial[lid] = elevation;
    workgroupBarrier();
    for (var stride = 32u; stride > 0u; stride = stride >> 1u) {
        if (lid < stride) {
            probe_partial[lid] = probe_partial[lid] + probe_partial[lid + stride];
        }
        workgroupBarrier();
    }
    if (lid == 0u) {
        probe_sums[wid.x] = probe_partial[0];
    }
}
"#;

/// A buffer a probe binds.
pub enum Binding<'a> {
    /// The world's resident cell buffer, read-only.
    WorldCells,
    /// A storage buffer initialized with these words.
    Storage(&'a [u32]),
    Uniform(&'a [u32]),
    /// An output of this many words, initialized to [`SENTINEL`] and read back.
    Output(u32),
}

/// Compiles `entry` from `assembly`, binds `bindings` (binding `i` at index `i`), dispatches
/// `groups` workgroups on the session's timeline and returns every output's words in binding
/// order. Waiting here is a test harness wait, counted as maintenance by the world.
pub fn run(
    world: &mut GpuWorld,
    assembly: &Assembly,
    entry: &str,
    bindings: &[Binding<'_>],
    groups: [u32; 3],
) -> Result<Vec<Vec<u32>>, String> {
    assembly
        .validate()
        .map_err(|diagnostic| diagnostic.to_string())?;
    let device = world.view().device.clone();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(assembly.name()),
        source: wgpu::ShaderSource::Wgsl(Cow::Owned(assembly.source().to_owned())),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry),
        layout: None,
        module: &module,
        entry_point: Some(entry),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });
    if let Some(error) = futures_lite::future::block_on(scope.pop()) {
        return Err(format!("probe pipeline {entry}: {error}"));
    }

    let mut buffers: Vec<Option<wgpu::Buffer>> = Vec::new();
    let mut outputs: Vec<(usize, wgpu::Buffer)> = Vec::new();
    for (index, binding) in bindings.iter().enumerate() {
        let (words, usage, sentinel): (Cow<'_, [u32]>, wgpu::BufferUsages, bool) = match binding {
            Binding::WorldCells => {
                buffers.push(None);
                continue;
            }
            Binding::Storage(words) => (Cow::Borrowed(*words), wgpu::BufferUsages::STORAGE, false),
            Binding::Uniform(words) => (Cow::Borrowed(*words), wgpu::BufferUsages::UNIFORM, false),
            Binding::Output(count) => (
                Cow::Owned(vec![SENTINEL; *count as usize]),
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                true,
            ),
        };
        let size = (words.len() as u64 * 4).max(16).next_multiple_of(16);
        let buffer = world
            .create_buffer("probe", size, usage, true)
            .map_err(|error| format!("probe buffer: {error:?}"))?;
        {
            let mut bytes = vec![0u8; size as usize];
            let (chunks, _) = bytes.as_chunks_mut::<4>();
            for (out, word) in chunks.iter_mut().zip(words.iter()) {
                *out = word.to_le_bytes();
            }
            buffer.get_mapped_range_mut(..).copy_from_slice(&bytes);
        }
        buffer.unmap();
        if sentinel {
            let readback = world
                .create_buffer(
                    "probe readback",
                    size,
                    wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    false,
                )
                .map_err(|error| format!("probe readback: {error:?}"))?;
            outputs.push((index, readback));
        }
        buffers.push(Some(buffer));
    }

    world
        .submit_external("layout probe", |view, encoder| {
            let layout = pipeline.get_bind_group_layout(0);
            let entries: Vec<wgpu::BindGroupEntry<'_>> = buffers
                .iter()
                .enumerate()
                .map(|(index, buffer)| wgpu::BindGroupEntry {
                    binding: index as u32,
                    resource: buffer.as_ref().unwrap_or(view.cells).as_entire_binding(),
                })
                .collect();
            let bind_group = view.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("probe"),
                layout: &layout,
                entries: &entries,
            });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.dispatch_workgroups(groups[0], groups[1], groups[2]);
            }
            for (index, readback) in &outputs {
                if let Some(source) = &buffers[*index] {
                    encoder.copy_buffer_to_buffer(source, 0, readback, 0, readback.size());
                }
            }
        })
        .map_err(|refusal| format!("probe submit: {refusal:?}"))?;

    let mapped = Arc::new(AtomicUsize::new(0));
    for (_, readback) in &outputs {
        let mapped = Arc::clone(&mapped);
        readback.map_async(wgpu::MapMode::Read, .., move |result| {
            if result.is_ok() {
                mapped.fetch_add(1, Ordering::Relaxed);
            }
        });
    }
    for _ in 0..100 {
        if mapped.load(Ordering::Relaxed) == outputs.len() {
            break;
        }
        world.wait_idle(Duration::from_millis(100));
    }
    if mapped.load(Ordering::Relaxed) != outputs.len() {
        return Err("probe readback never mapped".to_owned());
    }
    let mut results = Vec::with_capacity(outputs.len());
    for ((index, readback), binding) in outputs.iter().zip(
        bindings
            .iter()
            .filter(|binding| matches!(binding, Binding::Output(_))),
    ) {
        let Binding::Output(count) = binding else {
            continue;
        };
        let view = readback.get_mapped_range(..);
        let (chunks, _) = view.as_chunks::<4>();
        let words: Vec<u32> = chunks
            .iter()
            .take(*count as usize)
            .map(|bytes| u32::from_le_bytes(*bytes))
            .collect();
        drop(view);
        readback.unmap();
        let _ = index;
        results.push(words);
    }
    Ok(results)
}

/// The cell probe, assembled over `declarations` (normally the generated module).
pub fn cell_probe(declarations: SourceModule) -> Assembly {
    let modules = [
        declarations,
        gpu_world::shader::cell_accessors(),
        SourceModule::file("probe/cells.wgsl", CELL_PROBE),
    ];
    Assembly::new("cell probe", &modules)
}

/// A probe for one record: reads every field of `probe_input[i]` by name and writes it, as a
/// word, to `probe_output[i * fields + k]`.
pub fn record_probe(declarations: SourceModule, layout: &RecordLayout) -> Assembly {
    let fields = layout.fields.len();
    let mut text = String::new();
    let _ = writeln!(
        text,
        "@group(0) @binding(0) var<storage, read_write> probe_input: array<{}>;",
        layout.name
    );
    text.push_str("@group(0) @binding(1) var<storage, read_write> probe_output: array<u32>;\n\n");
    text.push_str("@compute @workgroup_size(1, 1, 1)\n");
    text.push_str("fn probe_record(@builtin(global_invocation_id) gid: vec3<u32>) {\n");
    text.push_str("    let i = gid.x;\n");
    for (index, field) in layout.fields.iter().enumerate() {
        let read = match field.ty {
            WordType::U32 => format!("probe_input[i].{}", field.name),
            WordType::I32 | WordType::F32 => format!("bitcast<u32>(probe_input[i].{})", field.name),
            WordType::AtomicU32 => format!("atomicLoad(&probe_input[i].{})", field.name),
        };
        let _ = writeln!(text, "    probe_output[i * {fields}u + {index}u] = {read};");
    }
    text.push_str("}\n");
    let modules = [
        declarations,
        SourceModule {
            path: "probe/record.wgsl",
            text: Cow::Owned(text),
        },
    ];
    Assembly::new("record probe", &modules)
}

/// Distinct boundary words for every field of `layout`, `samples` records long. Values depend
/// on the field's position, so two swapped fields read back in the wrong place.
pub fn record_samples(layout: &RecordLayout, samples: usize) -> Vec<u32> {
    let mut words = Vec::with_capacity(samples * layout.fields.len());
    for sample in 0..samples {
        for (index, field) in layout.fields.iter().enumerate() {
            let index = index as u32;
            let word = match (field.ty, sample % 4) {
                (WordType::F32, 0) => (-0.0f32).to_bits(),
                (WordType::F32, 1) => f32::MAX.to_bits(),
                (WordType::F32, 2) => f32::MIN_POSITIVE.to_bits(),
                (WordType::F32, _) => (-1.5 - index as f32).to_bits(),
                (WordType::I32, 0) => i32::MIN as u32,
                (WordType::I32, 1) => (-1i32 - index as i32) as u32,
                (WordType::I32, 2) => i32::MAX as u32,
                (WordType::I32, _) => index,
                (_, 0) => u32::MAX - index,
                (_, 1) => 0x8000_0000 | index,
                (_, 2) => index.wrapping_mul(0x0101_0101),
                (_, _) => 0x00FF_00FF ^ (index << 9),
            };
            words.push(word);
        }
    }
    words
}
