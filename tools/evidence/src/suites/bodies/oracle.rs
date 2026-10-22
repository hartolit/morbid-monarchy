//! The broadphase against an exhaustive oracle.
//!
//! The production kernels (`bins_clear`, `bins_count`, `bins_scan`, `bins_scatter`) bin
//! crafted bodies, and a probe kernel walks each body's candidates with the production
//! enumeration (`pair_prepare`, `pair_candidate`, one workgroup per body), keeps the generated
//! pairs
//! (`generated`) and classifies them with the production narrowphase (`pair_contact`). On the
//! CPU, every pair closer than the generation bound — the radii, the slop, both bodies' travel
//! and the solver margin, beyond which no contact kind is possible — must have been visited, no
//! body may be visited twice, and the bins must find exactly the pairs and contacts the
//! all-pairs traversal finds.
//!
//! Then the production `pairs_count` fills the pair lists, and a second probe enumerates each
//! body's candidates as a solver pass does: from its list when they fit
//! ([`PAIR_LIST_CAPACITY`]), from the bins otherwise. Every body in contact must find exactly its
//! generated candidates, from the list exactly when they fit, and the configurations must
//! exercise both.

use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use gpu_world::{
    layout::{BodyConfig, BodyParams, BodyState, GpuRecord, TickParams, body_work, codes},
    plan::{BodyLayout, PAIR_LIST_CAPACITY},
    shader::{Assembly, SourceModule, body_modules},
    world::GpuWorld,
};
use world_model::{body::tuning::CONTACT_SLOP, glam::Vec3, units::MAX_BODY_SUBSTEP_SECONDS};

use super::super::{
    Check,
    driver::{Driver, config},
};

// The probe writes into the `claims` binding, which the broadphase kernels never use, so the
// probe needs no ninth storage buffer beyond WebGPU's default limit.
const PROBE: &str = r#"
@compute @workgroup_size(64, 1, 1)
fn probe_bounds(@builtin(global_invocation_id) id: vec3<u32>) {
    let b = id.x;
    if (b >= body_params.body_count) {
        return;
    }
    set_bounds(load_body_config(config_base(b)).radius, distance(work_vec3(b, WORK_POS_A), work_vec3(b, WORK_PREV)));
}

// Per body: visited count, contact count, then each visited body with its contact kind in the
// high half (in whatever order the lanes reach them).
@compute @workgroup_size(64, 1, 1)
fn probe_candidates(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let i = wg.x;
    let n = body_params.body_count;
    let frame = pair_prepare(i, li, true);
    if (frame.gather == 0u) {
        return;
    }
    let stride = n + 2u;
    for (var k = li; k < frame.total; k += 64u) {
        let j = pair_candidate(frame, i, k);
        if (j == i || j == BODY_NONE || !generated(i, j)) {
            continue;
        }
        let pair = pair_contact(i, j, 0u, true);
        let visited = atomicAdd(&pair_tally[0], 1u);
        if (visited < n) {
            atomicStore(&claims[i * stride + 2u + visited], j | (pair.kind << 16u));
        }
        if (pair.kind != 0u) {
            atomicAdd(&pair_tally[1], 1u);
        }
    }
    workgroupBarrier();
    if (li == 0u) {
        atomicStore(&claims[i * stride], atomicLoad(&pair_tally[0]));
        atomicStore(&claims[i * stride + 1u], atomicLoad(&pair_tally[1]));
    }
}

// After pairs_count, per body in contact, after the first n * (n + 2) words: the candidates a
// solver pass enumerates, 1 when they came from the pair list, then the candidates.
@compute @workgroup_size(64, 1, 1)
fn probe_listed(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let i = wg.x;
    let n = body_params.body_count;
    let frame = pair_prepare(i, li, false);
    if (frame.gather == 0u) {
        return;
    }
    let listed = frame.groups == LISTED;
    let base = (n + i) * (n + 2u);
    for (var k = li; k < frame.total; k += 64u) {
        let j = pair_candidate(frame, i, k);
        if (!listed && (j == i || j == BODY_NONE || !generated(i, j))) {
            continue;
        }
        let visited = atomicAdd(&pair_tally[0], 1u);
        if (visited < n) {
            atomicStore(&claims[base + 2u + visited], j);
        }
    }
    workgroupBarrier();
    if (li == 0u) {
        atomicStore(&claims[base], atomicLoad(&pair_tally[0]));
        atomicStore(&claims[base + 1u], select(0u, 1u, listed));
    }
}
"#;

/// One crafted body: centre, radius and this substep's displacement.
#[derive(Debug, Clone, Copy)]
pub struct Crafted {
    pub position: Vec3,
    pub radius: f32,
    pub travel: Vec3,
}

/// What the probe found for each body: the visited bodies with their contact kinds, and for a
/// body in contact the candidates a solver pass enumerates and whether its pair list held them.
pub struct Found {
    pub visited: Vec<Vec<(u32, u32)>>,
    pub passes: Vec<Option<(bool, Vec<u32>)>>,
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
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

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn words_buffer(
    world: &mut GpuWorld,
    words: &[u32],
    usage: wgpu::BufferUsages,
) -> Result<wgpu::Buffer, String> {
    let size = (words.len() as u64 * 4).max(16).next_multiple_of(16);
    let buffer = world
        .create_buffer("broadphase probe", size, usage, true)
        .map_err(|error| format!("probe buffer: {error:?}"))?;
    {
        let mut bytes = vec![0u8; size as usize];
        let (chunks, _) = bytes.as_chunks_mut::<4>();
        for (chunk, word) in chunks.iter_mut().zip(words) {
            *chunk = word.to_le_bytes();
        }
        buffer.get_mapped_range_mut(..).copy_from_slice(&bytes);
    }
    buffer.unmap();
    Ok(buffer)
}

/// Runs the production broadphase and the probe over `bodies`.
pub fn probe(world: &mut GpuWorld, bodies: &[Crafted], all_pairs: bool) -> Result<Found, String> {
    let n = bodies.len() as u32;
    let layout = BodyLayout::new(n, 1, 1).ok_or("layout")?;
    let dt = MAX_BODY_SUBSTEP_SECONDS;
    let mut body_words = vec![0u32; layout.body_words as usize];
    for (index, body) in bodies.iter().enumerate() {
        let state = BodyState {
            flags: codes::BODY_ALIVE,
            local_x: body.position.x,
            local_y: -body.position.z,
            height: body.position.y,
            ..BodyState::default()
        };
        let base = (layout.state + index as u32 * BodyState::WORDS as u32) as usize;
        state.write_words(&mut body_words[base..base + BodyState::WORDS]);
        let config = BodyConfig {
            mass: 1.0,
            radius: body.radius,
            relative_density: 1.0,
            inverse_mass: 1.0,
        };
        let base = (layout.config + index as u32 * BodyConfig::WORDS as u32) as usize;
        config.write_words(&mut body_words[base..base + BodyConfig::WORDS]);
        let work = (layout.work + index as u32 * gpu_world::layout::BODY_WORK_WORDS) as usize;
        let mut put = |offset: u32, v: Vec3| {
            for (k, value) in v.to_array().iter().enumerate() {
                body_words[work + offset as usize + k] = value.to_bits();
            }
        };
        put(body_work::WORK_POS_A, body.position);
        put(body_work::WORK_BINNED, body.position);
        put(body_work::WORK_PREV, body.position - body.travel);
        put(body_work::WORK_VEL_A, body.travel / dt);
        body_words[work + body_work::WORK_FLAGS as usize] = body_work::WORK_ACTIVE;
    }
    let params = BodyParams {
        body_count: n,
        capacity: n,
        substeps: 1,
        substep_seconds: dt,
        tick_seconds: dt,
        elevation_scale: 0.5,
        broadphase: if all_pairs {
            codes::BROADPHASE_ALL_PAIRS
        } else {
            codes::BROADPHASE_BINS
        },
        hash_mask: layout.hash_slots - 1,
        state_base: layout.state,
        config_base: layout.config,
        work_base: layout.work,
        samples_base: layout.sample_records,
        observe_base: layout.observe,
        counts_base: layout.counts,
        starts_base: layout.starts,
        fill_base: layout.fill,
        entries_base: layout.entries,
        bounds_base: layout.bounds,
        diagnostics_base: layout.diagnostics,
        pair_counts_base: layout.pair_counts,
        pair_lists_base: layout.pair_lists,
        ..BodyParams::default()
    };
    let record = |write: &dyn Fn(&mut [u32]), words: usize| {
        let mut out = vec![0u32; words];
        write(&mut out);
        out
    };
    let tick = record(
        &|out| TickParams::default().write_words(out),
        TickParams::WORDS,
    );
    let body_params = record(&|out| params.write_words(out), BodyParams::WORDS);
    use wgpu::BufferUsages as Usage;
    let storage = Usage::STORAGE;
    let buffers = [
        words_buffer(world, &tick, Usage::UNIFORM)?,
        words_buffer(world, &body_params, Usage::UNIFORM)?,
        words_buffer(world, &[0; 4], Usage::UNIFORM)?,
        words_buffer(world, &[0; 4], storage)?,
        words_buffer(world, &[codes::SLOT_NONE; 4], storage)?,
        words_buffer(world, &body_words, storage)?,
        words_buffer(world, &vec![0; layout.bin_words as usize], storage)?,
        words_buffer(world, &[0; 4], storage)?,
        words_buffer(world, &[0; 16], storage)?,
        words_buffer(world, &vec![0; layout.diagnostics as usize + 64], storage)?,
        words_buffer(world, &[0; 8], storage)?,
    ];
    let pairs_words = (2 * n * (n + 2)) as usize;
    let output = words_buffer(world, &vec![0; pairs_words], storage | Usage::COPY_SRC)?;
    let mut buffers = buffers;
    buffers[7] = output.clone();
    let readback = world
        .create_buffer(
            "broadphase readback",
            output.size(),
            Usage::MAP_READ | Usage::COPY_DST,
            false,
        )
        .map_err(|error| format!("readback: {error:?}"))?;

    let mut modules = body_modules();
    modules.push(SourceModule::file("probe/broadphase.wgsl", PROBE));
    let assembly = Assembly::new("broadphase probe", &modules);
    assembly.validate().map_err(|error| error.to_string())?;
    let device = world.view().device.clone();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("broadphase probe"),
        source: wgpu::ShaderSource::Wgsl(assembly.source().to_owned().into()),
    });
    let group0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("probe bodies"),
        entries: &[
            uniform_entry(0),
            uniform_entry(1),
            uniform_entry(2),
            storage_entry(3, false),
            storage_entry(4, true),
            storage_entry(5, false),
            storage_entry(6, false),
            storage_entry(7, false),
            storage_entry(8, false),
            storage_entry(9, false),
            storage_entry(10, false),
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("broadphase probe"),
        bind_group_layouts: &[Some(&group0)],
        immediate_size: 0,
    });
    let pipeline = |entry: &str| {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some(entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        })
    };
    let steps = [
        (pipeline("bins_clear"), layout.hash_slots.div_ceil(64)),
        (pipeline("probe_bounds"), n.div_ceil(64)),
        (pipeline("bins_count"), n.div_ceil(64)),
        (pipeline("bins_scan"), 1),
        (pipeline("bins_scatter"), n.div_ceil(64)),
        (pipeline("probe_candidates"), n),
        (pipeline("pairs_count"), n),
        (pipeline("probe_listed"), n),
    ];
    let entries: Vec<wgpu::BindGroupEntry<'_>> = buffers
        .iter()
        .enumerate()
        .map(|(index, buffer)| wgpu::BindGroupEntry {
            binding: index as u32,
            resource: buffer.as_entire_binding(),
        })
        .collect();
    let bind0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("probe bodies"),
        layout: &group0,
        entries: &entries,
    });
    if let Some(error) = futures_lite::future::block_on(scope.pop()) {
        return Err(format!("broadphase probe: {error:?}"));
    }
    world
        .submit_external("broadphase probe", |_, encoder| {
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
                pass.set_bind_group(0, &bind0, &[]);
                for (pipeline, groups) in &steps {
                    pass.set_pipeline(pipeline);
                    pass.dispatch_workgroups(*groups, 1, 1);
                }
            }
            encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, output.size());
        })
        .map_err(|refusal| format!("submit: {refusal:?}"))?;
    let mapped = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&mapped);
    readback.map_async(wgpu::MapMode::Read, .., move |result| {
        flag.store(result.is_ok(), Ordering::Relaxed);
    });
    for _ in 0..100 {
        if mapped.load(Ordering::Relaxed) {
            break;
        }
        world.wait_idle(Duration::from_millis(100));
    }
    if !mapped.load(Ordering::Relaxed) {
        return Err("broadphase readback never mapped".to_owned());
    }
    let words: Vec<u32> = {
        let view = readback.get_mapped_range(..);
        let (chunks, _) = view.as_chunks::<4>();
        chunks
            .iter()
            .map(|chunk| u32::from_le_bytes(*chunk))
            .collect()
    };
    readback.unmap();
    let stride = (n + 2) as usize;
    let visited = (0..n as usize)
        .map(|i| {
            let count = (words[i * stride] as usize).min(n as usize);
            (0..count)
                .map(|k| {
                    let word = words[i * stride + 2 + k];
                    (word & 0xFFFF, word >> 16)
                })
                .collect()
        })
        .collect();
    let passes = (0..n as usize)
        .map(|i| {
            let base = (n as usize + i) * stride;
            let count = words[base] as usize;
            (count > 0).then(|| {
                let candidates = (0..count.min(n as usize))
                    .map(|k| words[base + 2 + k])
                    .collect();
                (words[base + 1] == 1, candidates)
            })
        })
        .collect();
    Ok(Found { visited, passes })
}

/// A fixed pseudo-random sequence in `[0, 1)`.
struct Sequence(u64);

impl Sequence {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    fn vector(&mut self, scale: f32) -> Vec3 {
        Vec3::new(self.next() - 0.5, self.next() - 0.5, self.next() - 0.5) * 2.0 * scale
    }
}

/// The configurations: a dense mixed-radius cluster, coincident centres, a lattice on the bin
/// edges, fast bodies, and a sparse spread.
fn configurations() -> Vec<(&'static str, Vec<Crafted>)> {
    let mut sequence = Sequence(0x00B0_D1E5);
    let mut out = Vec::new();
    let center = Vec3::new(200.0, 60.0, -200.0);
    out.push((
        "dense mixed-radius cluster",
        (0..256)
            .map(|index| Crafted {
                position: center + sequence.vector(15.0),
                radius: 2.0 + (index % 7) as f32,
                travel: Vec3::ZERO,
            })
            .collect(),
    ));
    let mut coincident: Vec<Crafted> = (0..48)
        .map(|_| Crafted {
            position: center,
            radius: 4.0,
            travel: Vec3::ZERO,
        })
        .collect();
    coincident.extend((0..16).map(|_| Crafted {
        position: center + sequence.vector(9.0),
        radius: 3.0,
        travel: Vec3::ZERO,
    }));
    out.push(("coincident centres", coincident));
    // Bodies on the bin lattice and a hair either side of it: radius 8 and no travel give a
    // bin edge of 16 + slop + the solver margin (0.5) + 0.01.
    let edge = 2.0 * 8.0 + CONTACT_SLOP + 0.5 + 0.01;
    let mut lattice = Vec::new();
    for k in 0..4 {
        for (dx, dy) in [(0.0, 0.0), (1e-4, 0.0), (-1e-4, 1e-4), (0.0, -1e-4)] {
            let at = Vec3::new(k as f32 * edge + dx, 3.0 * edge + dy, -edge);
            lattice.push(Crafted {
                position: at,
                radius: 8.0,
                travel: Vec3::ZERO,
            });
            lattice.push(Crafted {
                position: at + Vec3::new(edge * 0.999, 0.0, 0.0),
                radius: 8.0,
                travel: Vec3::ZERO,
            });
        }
    }
    out.push(("bin-edge lattice", lattice));
    out.push((
        "fast bodies",
        (0..128)
            .map(|index| Crafted {
                position: center + sequence.vector(30.0),
                radius: 2.0 + (index % 2) as f32,
                travel: sequence.vector(5.0).clamp_length_max(5.0),
            })
            .collect(),
    ));
    out.push((
        "sparse spread",
        (0..256)
            .map(|index| Crafted {
                position: Vec3::new(
                    sequence.next() * 1000.0,
                    sequence.next() * 200.0,
                    -sequence.next() * 1000.0,
                ),
                radius: 2.0 + (index % 7) as f32,
                travel: Vec3::ZERO,
            })
            .collect(),
    ));
    out
}

pub fn broadphase_oracle(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 1))?;
    let (mut listed_bodies, mut overflowed_bodies) = (0u64, 0u64);
    for (label, bodies) in configurations() {
        let bins = probe(&mut driver.world, &bodies, false)?;
        let all = probe(&mut driver.world, &bodies, true)?;
        let mut missed = 0;
        let mut duplicates = 0;
        let mut disagreements = 0;
        let mut required = 0u64;
        let mut visited_total = 0u64;
        let mut example = None;
        let (mut pass_mismatches, mut listed, mut overflowed) = (0, 0u64, 0u64);
        for (i, body) in bodies.iter().enumerate() {
            let visited: BTreeSet<u32> = bins.visited[i].iter().map(|(j, _)| *j).collect();
            duplicates += bins.visited[i].len() - visited.len();
            visited_total += bins.visited[i].len() as u64;
            for (j, other) in bodies.iter().enumerate() {
                if i == j {
                    continue;
                }
                // The generation bound, with `pairs.wgsl`'s margin of 0.5.
                let bound = body.radius
                    + other.radius
                    + CONTACT_SLOP
                    + body.travel.length()
                    + other.travel.length()
                    + 0.5;
                if body.position.distance(other.position) < bound - 1e-3 {
                    required += 1;
                    if !visited.contains(&(j as u32)) {
                        missed += 1;
                        example.get_or_insert(format!("{i} misses {j}"));
                    }
                }
            }
            // Every generated pair with its contact kind: bins and all pairs must agree.
            let contacts = |found: &Found| -> BTreeSet<(u32, u32)> {
                found.visited[i].iter().copied().collect()
            };
            if contacts(&bins) != contacts(&all) {
                disagreements += 1;
            }
            // A solver pass enumerates exactly the generated candidates of a body in contact,
            // from its pair list exactly when they fit; a body without contacts is skipped.
            let in_contact = bins.visited[i].iter().any(|(_, kind)| *kind != 0);
            for found in [&bins, &all] {
                match &found.passes[i] {
                    Some((from_list, candidates)) => {
                        let enumerated: BTreeSet<u32> = candidates.iter().copied().collect();
                        if !in_contact
                            || enumerated != visited
                            || enumerated.len() != candidates.len()
                            || *from_list != (visited.len() <= PAIR_LIST_CAPACITY as usize)
                        {
                            pass_mismatches += 1;
                        }
                    }
                    None if in_contact => pass_mismatches += 1,
                    None => {}
                }
            }
            match &bins.passes[i] {
                Some((true, _)) => listed += 1,
                Some((false, _)) => overflowed += 1,
                None => {}
            }
        }
        check.equal(&format!("{label}: required pairs not visited"), missed, 0);
        check.equal(&format!("{label}: bodies visited twice"), duplicates, 0);
        check.equal(
            &format!("{label}: bodies whose contacts differ from all pairs"),
            disagreements,
            0,
        );
        check.equal(
            &format!("{label}: bodies whose solver passes enumerate other candidates"),
            pass_mismatches,
            0,
        );
        if let Some(example) = example {
            check.note(format!("{label}: {example}"));
        }
        let slug = label.replace([' ', '-'], "_");
        check.metric(&format!("{slug}_listed_bodies"), listed);
        check.metric(&format!("{slug}_overflowed_bodies"), overflowed);
        listed_bodies += listed;
        overflowed_bodies += overflowed;
        check.metric(&format!("{slug}_required_pairs"), required);
        check.metric(&format!("{slug}_candidates_visited"), visited_total);
        check.metric(
            &format!("{slug}_all_pairs_visited"),
            (bodies.len() * (bodies.len() - 1)) as u64,
        );
    }
    check.that(listed_bodies > 0 && overflowed_bodies > 0, || {
        format!(
            "the configurations listed {listed_bodies} bodies' candidates and overflowed \
             {overflowed_bodies}; both paths must be exercised"
        )
    });
    Ok(())
}
