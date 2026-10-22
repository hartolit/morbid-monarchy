//! The renderer's cost on generated terrain at the qualification, scale and capacity radii:
//! device time of the scene passes (camera, visibility, relight, sphere shadows, the four layers
//! and the spheres) and the post passes (bloom and tone map) from pass timestamps, the CPU time
//! of encoding a frame, and the renderer's own device memory. Frames are drawn one at a time with
//! the device idle between them, so each pass's time is its own; this is the renderer alone, not
//! a client frame. The unculled path, per-vertex lighting and the lighting cache's worst case
//! (every drawn chunk relit each frame) are measured in the same run.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use gpu_world::world::{GpuWorldConfig, TimestampSupport};
use world_model::{
    generation::WorldGenerator, glam::DVec3, rules::SimulationRules, spatial::ChunkKey,
};
use world_render::CameraRig;
use world_runtime::{
    ids::WorldId,
    port::{PhysicalWorld, ResidentWindow, ResizeReport, ResizeRequest, SlotRef},
};

use super::{
    super::{Check, allow_software, driver::Driver},
    cases::pass_times,
    offscreen::{Offscreen, request},
};

const SIZE: [u32; 2] = [1920, 1080];
const WARMUP: usize = 10;
const FRAMES: usize = 60;

fn percentile(values: &mut [f64], fraction: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    values
        .get(((values.len().saturating_sub(1)) as f64 * fraction).round() as usize)
        .copied()
        .unwrap_or(f64::NAN)
}

/// Opens a world of generated seed-69 terrain around chunk (3, 3) at `radius`, every chunk
/// resident, the rules off (the terrain holds still while it is drawn).
pub(super) fn generated(adapter: &str, radius: u32) -> Result<Driver, String> {
    Ok(generated_with(adapter, radius, SimulationRules::NONE)?.0)
}

/// [`generated`] under `rules`, with each chunk's slot.
pub(super) fn generated_with(
    adapter: &str,
    radius: u32,
    rules: SimulationRules,
) -> Result<(Driver, HashMap<ChunkKey, SlotRef>), String> {
    let window = ResidentWindow::new(ChunkKey::new(3, 3, 0), radius, radius)
        .map_err(|error| format!("{error:?}"))?;
    let mut config = GpuWorldConfig::new(WorldId::new(0x0600), adapter, window);
    config.allow_software = allow_software();
    config.rules = rules;
    config.timestamps = true;
    let mut driver = Driver::open(config)?;
    let mut slots = HashMap::new();
    let keys: Vec<ChunkKey> = window.view().iter().collect();
    load(&mut driver, &mut slots, &keys, |_| Ok(()))?;
    Ok((driver, slots))
}

/// Loads generated seed-69 `keys` as the residency owner does, a batch of the upload pool's
/// size a tick, calling `after` with each batch's settled tick.
pub(super) fn load(
    driver: &mut Driver,
    slots: &mut HashMap<ChunkKey, SlotRef>,
    keys: &[ChunkKey],
    mut after: impl FnMut(&mut Driver) -> Result<(), String>,
) -> Result<(), String> {
    let generator = WorldGenerator::new(69);
    let batch = driver.world.capabilities().chunk_upload_slots as usize;
    for group in keys.chunks(batch) {
        for key in group {
            let slot = driver.stage_upload(*key, &generator.generate_chunk(*key).cells)?;
            slots.insert(*key, slot);
        }
        driver
            .submit(&[])
            .map_err(|refusal| format!("{refusal:?}"))?;
        driver.settle()?;
        after(driver)?;
    }
    Ok(())
}

/// Moves the window to `center`, keeping its size, as the residency owner does: the chunks it
/// drops retire in a tick, then the window moves (in place). Returns the report, the time the
/// move's call took and the keys that arrive, which the caller loads.
pub(super) fn move_window(
    driver: &mut Driver,
    slots: &mut HashMap<ChunkKey, SlotRef>,
    center: ChunkKey,
) -> Result<(ResizeReport, Duration, Vec<ChunkKey>), String> {
    let current = driver.world.view().window;
    let target = ResidentWindow::new(center, current.radius_x(), current.radius_y())
        .map_err(|error| format!("{error:?}"))?;
    let leaving: Vec<ChunkKey> = slots
        .keys()
        .copied()
        .filter(|key| !target.contains(*key))
        .collect();
    for key in &leaving {
        let slot = slots.remove(key).expect("listed");
        driver
            .world
            .retire_slot(slot)
            .map_err(|refusal| format!("retire: {refusal:?}"))?;
    }
    driver
        .submit(&[])
        .map_err(|refusal| format!("{refusal:?}"))?;
    driver.settle()?;
    let session = driver.session();
    let started = Instant::now();
    let report = driver
        .world
        .resize(&ResizeRequest {
            session,
            window: target,
        })
        .map_err(|refusal| format!("move: {refusal:?}"))?;
    let took = started.elapsed();
    slots.clear();
    slots.extend(report.retained.iter().copied());
    let arriving = target
        .view()
        .iter()
        .filter(|key| !current.contains(*key))
        .collect();
    Ok((report, took, arriving))
}

/// How the terrain lighting is computed in a measured run.
#[derive(Clone, Copy)]
enum Lighting {
    /// From the cache, nothing changing: what a still scene costs.
    Cached,
    /// Per vertex every frame: the comparison path and the cost before the cache.
    PerVertex,
    /// From the cache, with the sun nudged every frame so every drawn chunk is relit: the
    /// cache's worst case.
    RelitEveryFrame,
}

/// Device time of the scene and post passes and the CPU time of encoding, over measured frames.
struct Costs {
    scene: Vec<f64>,
    post: Vec<f64>,
    encode: Vec<f64>,
}

fn measure_frames(
    driver: &mut Driver,
    offscreen: &mut Offscreen,
    frame: &world_render::FrameRequest,
    period_ns: Option<f32>,
    lighting: Lighting,
) -> Result<Costs, String> {
    offscreen
        .renderer
        .set_lighting_cache(!matches!(lighting, Lighting::PerVertex));
    let mut frame = *frame;
    let mut costs = Costs {
        scene: Vec::new(),
        post: Vec::new(),
        encode: Vec::new(),
    };
    for index in 0..WARMUP + FRAMES {
        if matches!(lighting, Lighting::RelitEveryFrame) {
            frame.lighting.sun_azimuth_degrees += if index % 2 == 0 { 0.01 } else { -0.01 };
        }
        let started = Instant::now();
        offscreen.draw(&mut driver.world, &frame)?;
        let cpu = started.elapsed().as_secs_f64() * 1000.0;
        let times = match period_ns {
            Some(period) => Some(pass_times(&mut driver.world, offscreen, period)?),
            None => {
                driver.world.wait_idle(std::time::Duration::from_secs(10));
                None
            }
        };
        if index >= WARMUP {
            costs.encode.push(cpu);
            if let Some((scene_ns, post_ns)) = times {
                costs.scene.push(scene_ns / 1.0e6);
                costs.post.push(post_ns / 1.0e6);
            }
        }
    }
    offscreen.renderer.set_lighting_cache(true);
    Ok(costs)
}

pub fn frame_cost(adapter: &str, check: &mut Check) -> Result<(), String> {
    for radius in [6u32, 16, 32] {
        let mut driver = generated(adapter, radius)?;
        let period_ns = match driver.world.report().timestamps {
            TimestampSupport::Enabled { period_ns } => Some(period_ns),
            other => {
                check.note(format!(
                    "radius {radius}: no pass timestamps ({other:?}); CPU encode time only"
                ));
                None
            }
        };
        let mut offscreen =
            Offscreen::new(&mut driver.world, SIZE, wgpu::TextureFormat::Bgra8UnormSrgb)?;
        if period_ns.is_some() {
            let device = driver.world.view().device.clone();
            offscreen
                .renderer
                .enable_timestamps(&device)
                .map_err(|error| error.to_string())?;
        }
        // The qualification profile's camera start over the lake shore.
        let mut frame = request(CameraRig::Free {
            position: DVec3::new(200.0, 150.0, -230.0),
            yaw: 30f32.to_radians(),
            pitch: (-40f32).to_radians(),
        });
        frame.lighting = world_render::Lighting::default();
        let label = |name: &str| format!("r{radius}_{name}");
        // The unculled comparison path first: every resident chunk of every layer.
        offscreen.renderer.set_culling(false);
        let mut unculled = measure_frames(
            &mut driver,
            &mut offscreen,
            &frame,
            period_ns,
            Lighting::Cached,
        )?;
        offscreen.renderer.set_culling(true);
        if !unculled.scene.is_empty() {
            check.metric(
                &label("unculled_scene_ms_median"),
                percentile(&mut unculled.scene, 0.5),
            );
        }
        for (name, lighting) in [
            ("per_vertex_", Lighting::PerVertex),
            ("relit_", Lighting::RelitEveryFrame),
            ("", Lighting::Cached),
        ] {
            let mut costs =
                measure_frames(&mut driver, &mut offscreen, &frame, period_ns, lighting)?;
            if !costs.scene.is_empty() {
                check.metric(
                    &label(&format!("{name}scene_ms_median")),
                    percentile(&mut costs.scene, 0.5),
                );
                check.metric(
                    &label(&format!("{name}scene_ms_p95")),
                    percentile(&mut costs.scene, 0.95),
                );
            }
            if name.is_empty() {
                if !costs.post.is_empty() {
                    check.metric(&label("post_ms_median"), percentile(&mut costs.post, 0.5));
                    check.metric(&label("post_ms_p95"), percentile(&mut costs.post, 0.95));
                }
                check.metric(
                    &label("encode_cpu_ms_median"),
                    percentile(&mut costs.encode, 0.5),
                );
                check.metric(
                    &label("encode_cpu_ms_p95"),
                    percentile(&mut costs.encode, 0.95),
                );
            }
        }
        let drawn = super::cases::draw_counts(&mut driver.world, &offscreen)?;
        check.metric(&label("drawn_per_layer"), format!("{drawn:?}"));
        let counters = offscreen.renderer.counters();
        check.metric(&label("chunk_instances"), counters.chunk_instances);
        check.metric(&label("renderer_static_bytes"), counters.static_bytes);
        check.metric(&label("renderer_target_bytes"), counters.target_bytes);
        check.metric(&label("lighting_cache_bytes"), counters.lighting_bytes);
        check.metric(
            &label("world_bind_groups"),
            counters.world_bind_groups_created,
        );
        check.metric(
            &label("uniform_bytes_per_frame"),
            counters.uniform_bytes_written / counters.frames.max(1),
        );
        check.equal(&label("bind groups"), counters.world_bind_groups_created, 1);
        check.that(counters.chunk_instances == (2 * radius + 1).pow(2), || {
            format!(
                "radius {radius}: {} chunk instances",
                counters.chunk_instances
            )
        });
    }
    Ok(())
}

/// One tick of no commands, waited for: its report.
fn quiet_tick(driver: &mut Driver) -> Result<world_runtime::port::TickReport, String> {
    let tick = driver.next_tick;
    driver.outcomes.clear();
    driver
        .submit(&[])
        .map_err(|refusal| format!("tick: {refusal:?}"))?;
    driver.settle()?;
    driver
        .outcomes
        .iter()
        .find_map(|outcome| match outcome {
            world_runtime::port::Outcome::TickCompleted(report) if report.tick == tick => {
                Some(*report)
            }
            _ => None,
        })
        .ok_or_else(|| format!("no report of tick {tick:?}"))
}

/// The device memory the allocator holds: (reserved in memory blocks, allocated in them).
fn allocator(driver: &Driver) -> Option<(u64, u64)> {
    driver
        .world
        .view()
        .device
        .generate_allocator_report()
        .map(|report| (report.total_reserved_bytes, report.total_allocated_bytes))
}

/// What a window move costs at radius 32 on generated terrain at rest, every rule on: the move
/// the residency owner makes when the observer crosses a chunk boundary (the leaving row retired,
/// the window moved by a row with the same radii, the entering row loaded a batch a tick), with
/// the ticks' phases and the frames' scene passes after it, and the allocator's device memory.
pub fn window_move_cost(adapter: &str, check: &mut Check) -> Result<(), String> {
    const RADIUS: u32 = 32;
    const SETTLE_TICKS: usize = 240;
    const AFTER: usize = 8;
    let (mut driver, mut slots) = generated_with(adapter, RADIUS, SimulationRules::ALL)?;
    let TimestampSupport::Enabled { period_ns } = driver.world.report().timestamps else {
        check.note("no timestamps: nothing to measure".to_owned());
        return Ok(());
    };
    for _ in 0..SETTLE_TICKS {
        quiet_tick(&mut driver)?;
    }
    let stage = |check: &mut Check, driver: &Driver, name: &str| {
        if let Some((reserved, allocated)) = allocator(driver) {
            check.metric(&format!("{name}_reserved_bytes"), reserved);
            check.metric(&format!("{name}_allocated_bytes"), allocated);
        }
    };
    stage(check, &driver, "world");
    let mut offscreen =
        Offscreen::new(&mut driver.world, SIZE, wgpu::TextureFormat::Bgra8UnormSrgb)?;
    let device = driver.world.view().device.clone();
    offscreen
        .renderer
        .enable_timestamps(&device)
        .map_err(|error| error.to_string())?;
    let frame = request(CameraRig::Free {
        position: DVec3::new(200.0, 150.0, -230.0),
        yaw: 30f32.to_radians(),
        pitch: (-40f32).to_radians(),
    });
    let draw = |driver: &mut Driver, offscreen: &mut Offscreen| -> Result<f64, String> {
        offscreen.draw(&mut driver.world, &frame)?;
        Ok(pass_times(&mut driver.world, offscreen, period_ns)?.0 / 1.0e6)
    };
    for _ in 0..WARMUP {
        draw(&mut driver, &mut offscreen)?;
    }
    stage(check, &driver, "renderer");
    let ms = |duration: Option<Duration>| {
        duration.map_or(f64::NAN, |value| value.as_secs_f64() * 1000.0)
    };
    let mut rest = (Vec::new(), Vec::new());
    for _ in 0..AFTER {
        rest.0.push(ms(quiet_tick(&mut driver)?.gpu_time));
        rest.1.push(draw(&mut driver, &mut offscreen)?);
    }
    check.metric("rest_tick_ms_median", percentile(&mut rest.0, 0.5));
    check.metric("rest_scene_ms_median", percentile(&mut rest.1, 0.5));
    let generation = driver.world.view().buffers_generation;
    let groups = offscreen.renderer.counters().world_bind_groups_created;
    let (report, took, arriving) = move_window(&mut driver, &mut slots, ChunkKey::new(3, 4, 0))?;
    check.metric("move_cpu_ms", took.as_secs_f64() * 1000.0);
    check.metric("move_copied_cell_bytes", report.migrated_bytes);
    check.metric("move_peak_overlap_bytes", report.peak_overlap_bytes);
    check.equal("chunks kept by the move", report.retained.len(), 64 * 65);
    check.equal("cell bytes copied by the move", report.migrated_bytes, 0);
    stage(check, &driver, "moved");
    let started = Instant::now();
    let scene = draw(&mut driver, &mut offscreen)?;
    check.metric(
        "first_frame_after_move_wall_ms",
        started.elapsed().as_secs_f64() * 1000.0,
    );
    check.metric("first_frame_after_move_scene_ms", scene);
    let mut woke = 0;
    for index in 0..AFTER {
        let report = quiet_tick(&mut driver)?;
        let phases = report.phases.unwrap_or_default();
        woke = woke.max(report.cells.active_tiles);
        check.metric(
            &format!("after_move_tick_{index}"),
            format!(
                "total {:.2} activity {:.2} material {:.2} ms, {} active tiles, scene {:.2} ms",
                ms(report.gpu_time),
                ms(Some(phases.activity)),
                ms(Some(phases.material)),
                report.cells.active_tiles,
                draw(&mut driver, &mut offscreen)?,
            ),
        );
    }
    check.that(woke < 64 * 65 * 64 / 10, || {
        format!("a tick after the move had {woke} active tiles")
    });
    let mut loading = Vec::new();
    load(&mut driver, &mut slots, &arriving, |driver| {
        let report = driver
            .outcomes
            .iter()
            .rev()
            .find_map(|outcome| match outcome {
                world_runtime::port::Outcome::TickCompleted(report) => Some(*report),
                _ => None,
            })
            .ok_or("no tick report")?;
        loading.push(ms(report.gpu_time));
        Ok(())
    })?;
    check.metric("loading_ticks_ms", format!("{loading:.2?}"));
    for _ in 0..4 {
        quiet_tick(&mut driver)?;
        draw(&mut driver, &mut offscreen)?;
    }
    stage(check, &driver, "settled");
    check.equal(
        "window buffers replaced by the move",
        driver.world.view().buffers_generation,
        generation,
    );
    check.equal(
        "world bind groups created by the move",
        offscreen.renderer.counters().world_bind_groups_created,
        groups,
    );
    Ok(())
}
