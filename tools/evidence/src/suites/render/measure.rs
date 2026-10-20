//! The renderer's cost on generated terrain at the qualification and scale radii: device time of
//! the scene passes (camera, sphere shadows, the four layers and the spheres) and the post passes
//! (bloom and tone map) from pass timestamps, the CPU time of encoding a frame, and the
//! renderer's own device memory. Frames are drawn one at a time with the device idle between
//! them, so each pass's time is its own; this is the renderer alone, not a client frame. The
//! unculled comparison path is measured in the same run.

use std::time::Instant;

use gpu_world::world::{GpuWorldConfig, TimestampSupport};
use world_model::{
    generation::WorldGenerator, glam::DVec3, rules::SimulationRules, spatial::ChunkKey,
};
use world_render::CameraRig;
use world_runtime::{
    ids::WorldId,
    port::{PhysicalWorld, ResidentWindow},
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
    let window = ResidentWindow::new(ChunkKey::new(3, 3, 0), radius, radius)
        .map_err(|error| format!("{error:?}"))?;
    let mut config = GpuWorldConfig::new(WorldId::new(0x0600), adapter, window);
    config.allow_software = allow_software();
    config.rules = SimulationRules::NONE;
    config.timestamps = true;
    let mut driver = Driver::open(config)?;
    let generator = WorldGenerator::new(69);
    let view = window.view();
    let batch = driver.world.capabilities().chunk_upload_slots as usize;
    let mut pending = 0;
    for y in view.min.key.y..=view.max.key.y {
        for x in view.min.key.x..=view.max.key.x {
            let key = ChunkKey::new(x, y, 0);
            driver.stage_upload(key, &generator.generate_chunk(key).cells)?;
            pending += 1;
            if pending == batch {
                driver
                    .submit(&[])
                    .map_err(|refusal| format!("{refusal:?}"))?;
                driver.settle()?;
                pending = 0;
            }
        }
    }
    driver
        .submit(&[])
        .map_err(|refusal| format!("{refusal:?}"))?;
    driver.settle()?;
    Ok(driver)
}

pub fn frame_cost(adapter: &str, check: &mut Check) -> Result<(), String> {
    for radius in [6u32, 16] {
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
        // The unculled comparison path first: every resident chunk of every layer.
        offscreen.renderer.set_culling(false);
        let mut unculled = Vec::new();
        for index in 0..WARMUP + FRAMES {
            offscreen.draw(&mut driver.world, &frame)?;
            if let Some(period) = period_ns {
                let (scene_ns, _) = pass_times(&mut driver.world, &offscreen, period)?;
                if index >= WARMUP {
                    unculled.push(scene_ns / 1.0e6);
                }
            } else {
                driver.world.wait_idle(std::time::Duration::from_secs(10));
            }
        }
        if !unculled.is_empty() {
            check.metric(
                &format!("r{radius}_unculled_scene_ms_median"),
                percentile(&mut unculled, 0.5),
            );
        }
        offscreen.renderer.set_culling(true);
        let (mut scene, mut post, mut encode) = (Vec::new(), Vec::new(), Vec::new());
        for index in 0..WARMUP + FRAMES {
            let started = Instant::now();
            offscreen.draw(&mut driver.world, &frame)?;
            let cpu = started.elapsed().as_secs_f64() * 1000.0;
            let times = match period_ns {
                Some(period) => Some(pass_times(&mut driver.world, &offscreen, period)?),
                None => {
                    driver.world.wait_idle(std::time::Duration::from_secs(10));
                    None
                }
            };
            if index >= WARMUP {
                encode.push(cpu);
                if let Some((scene_ns, post_ns)) = times {
                    scene.push(scene_ns / 1.0e6);
                    post.push(post_ns / 1.0e6);
                }
            }
        }
        let drawn = super::cases::draw_counts(&mut driver.world, &offscreen)?;
        check.metric(&format!("r{radius}_drawn_per_layer"), format!("{drawn:?}"));
        let counters = offscreen.renderer.counters();
        let label = |name: &str| format!("r{radius}_{name}");
        if !scene.is_empty() {
            check.metric(&label("scene_ms_median"), percentile(&mut scene, 0.5));
            check.metric(&label("scene_ms_p95"), percentile(&mut scene, 0.95));
            check.metric(&label("post_ms_median"), percentile(&mut post, 0.5));
            check.metric(&label("post_ms_p95"), percentile(&mut post, 0.95));
        }
        check.metric(&label("encode_cpu_ms_median"), percentile(&mut encode, 0.5));
        check.metric(&label("encode_cpu_ms_p95"), percentile(&mut encode, 0.95));
        check.metric(&label("chunk_instances"), counters.chunk_instances);
        check.metric(&label("renderer_static_bytes"), counters.static_bytes);
        check.metric(&label("renderer_target_bytes"), counters.target_bytes);
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
