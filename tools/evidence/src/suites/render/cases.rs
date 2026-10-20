//! The render suite's correctness cases: the production renderer on the GPU owner's device,
//! drawing synthetic worlds whose every column is known, read back offscreen.

use std::time::Duration;

use gpu_world::world::PresentRefusal;
use world_model::{
    body::{BodyHandle, BodyPreset},
    cell::WorldCell,
    command::{Command, Direction, Drive, EditTool, MaterialEdit},
    glam::{DVec3, IVec2},
    material::{FluidMat, GranularMat, SurfaceMat, TerrainMat},
    rules::FOLIAGE_MATURE_STATE,
    spatial::ChunkKey,
};
use world_render::{
    CameraRig, RenderError,
    frame::{LENS_OFFSET_Y, forward, project},
};
use world_runtime::port::{PhysicalWorld, Refusal, ResidentWindow, ResizeRequest};

use super::{
    super::{
        Check,
        bodies::scene::{Scene, Setup},
        driver,
    },
    offscreen::{Image, Offscreen, colour, map_read, ndc_of, request, top_down},
};

const SIZE: [u32; 2] = [480, 360];
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Ground elevation of the synthetic worlds: 100 units, 50 world units at the default scale.
const GROUND: u16 = 100;
const GROUND_Y: f64 = 50.0;

fn grass(cell: &mut WorldCell) {
    cell.set_elevation(GROUND);
    cell.set_surface_mat(SurfaceMat::SURFACE_FOLIAGE);
    cell.set_surface_state(FOLIAGE_MATURE_STATE);
}

/// The origin chunk in four quadrants (stone, sand, a water pool, grass), grass elsewhere.
fn quadrants(cell: IVec2, column: &mut WorldCell) {
    grass(column);
    if !(0..64).contains(&cell.x) || !(0..64).contains(&cell.y) {
        return;
    }
    column.set_surface_mat(SurfaceMat::EMPTY);
    column.set_surface_state(0);
    match (cell.x < 32, cell.y < 32) {
        (true, true) => column.set_terrain_mat(TerrainMat::TERRAIN_STONE),
        (false, true) => {
            column.set_granular_mat(GranularMat::GRANULAR_SAND);
            column.set_granular_vol(4);
        }
        (true, false) => {
            column.set_elevation(GROUND - 30);
            column.set_fluid_mat(FluidMat::FLUID_WATER);
            column.set_fluid_vol(40);
        }
        (false, false) => grass(column),
    }
}

/// The render-space point of a cell's vertex at the ground.
fn ground(cell: IVec2) -> DVec3 {
    DVec3::new(f64::from(cell.x), GROUND_Y, -f64::from(cell.y))
}

fn quadrant_scene(adapter: &str) -> Result<Scene, String> {
    Scene::open(adapter, Setup::default(), quadrants)
}

fn sample(
    image: &Image,
    request: &world_render::FrameRequest,
    point: DVec3,
) -> Result<[f32; 3], String> {
    let ndc = ndc_of(request, SIZE[0] as f32 / SIZE[1] as f32, point)
        .ok_or_else(|| format!("{point} is behind the camera"))?;
    Ok(image.mean_at(ndc, 2))
}

fn save(check: &mut Check, image: &Image, name: &str) {
    let path =
        std::env::temp_dir().join(format!("morbid-render-{name}-{}.png", std::process::id()));
    if image.save(&path).is_ok() {
        check.note(format!("frame written to {}", path.display()));
    }
}

/// The renderer builds for every output format the client may get from a surface, encodes sRGB
/// exactly once whichever it gets, and refuses a format it cannot write.
pub fn formats(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut scene = quadrant_scene(adapter)?;
    let camera = top_down(32.0, -32.0, GROUND_Y + 100.0);
    let frame = request(camera);
    let mut images = Vec::new();
    for format in [
        wgpu::TextureFormat::Bgra8UnormSrgb,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        wgpu::TextureFormat::Bgra8Unorm,
        wgpu::TextureFormat::Rgba8Unorm,
    ] {
        let mut offscreen = Offscreen::new(&mut scene.driver.world, SIZE, format)?;
        let image = offscreen.image(&mut scene.driver.world, &frame)?;
        check.equal(
            &format!("{format:?} pipelines"),
            offscreen.renderer.counters().pipelines_created,
            world_render::renderer::PIPELINES,
        );
        images.push((format, image));
    }
    let mut fp16 = Offscreen::new(
        &mut scene.driver.world,
        SIZE,
        wgpu::TextureFormat::Rgba16Float,
    )?;
    fp16.draw(&mut scene.driver.world, &frame)?;
    scene.driver.world.wait_idle(Duration::from_secs(10));
    let reference = &images[0].1;
    for (format, image) in &images[1..] {
        let worst = reference
            .rgba
            .iter()
            .zip(&image.rgba)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        // The shader's encoding and the hardware's agree to rounding; a double or missing
        // encoding would differ by tens of levels.
        check.that(worst <= 3, || {
            format!("{format:?} differs from Bgra8UnormSrgb by up to {worst} levels")
        });
        check.metric(&format!("{format:?}_max_level_difference"), worst);
    }
    let view = scene.driver.world.view();
    match world_render::Renderer::new(&view, wgpu::TextureFormat::R8Unorm) {
        Err(RenderError::UnsupportedOutput(_)) => {}
        Err(other) => check.fail(format!("R8Unorm refused for another reason: {other}")),
        Ok(_) => check.fail("R8Unorm was accepted"),
    }
    check.that(
        scene
            .driver
            .world
            .view()
            .device
            .limits()
            .max_storage_buffers_per_shader_stage
            >= 6,
        || "the device grants fewer than six storage buffers per stage".to_owned(),
    );
    Ok(())
}

/// A zero-sized output (a minimized window) is refused without allocating targets, nothing is
/// drawn, and a later size draws again.
pub fn zero_size(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut scene = quadrant_scene(adapter)?;
    let mut offscreen = Offscreen::new(&mut scene.driver.world, SIZE, FORMAT)?;
    let created = offscreen.renderer.counters().target_sets_created;
    let view = scene.driver.world.view();
    match offscreen.renderer.resize(view.device, view.queue, [0, 600]) {
        Err(RenderError::ZeroSize) => {}
        other => check.fail(format!("a zero width was not refused: {other:?}")),
    }
    check.equal("targets after a zero size", offscreen.renderer.size(), None);
    check.equal(
        "target bytes after a zero size",
        offscreen.renderer.target_bytes(),
        0,
    );
    check.equal(
        "target sets created for a zero size",
        offscreen.renderer.counters().target_sets_created,
        created,
    );
    let frame = request(top_down(32.0, -32.0, GROUND_Y + 100.0));
    match offscreen.draw(&mut scene.driver.world, &frame) {
        Err(error) if error.contains("no area") => {}
        other => check.fail(format!("a frame without targets drew: {other:?}")),
    }
    let view = scene.driver.world.view();
    offscreen
        .renderer
        .resize(view.device, view.queue, SIZE)
        .map_err(|error| error.to_string())?;
    let image = offscreen.image(&mut scene.driver.world, &frame)?;
    let stone = sample(&image, &frame, ground(IVec2::new(16, 16)))?;
    check.that(colour::is_grey(stone), || {
        format!("after the restored size, stone is {stone:?}")
    });
    Ok(())
}

/// Every layer is drawn from the resident cells with its material's look: stone, sand, a water
/// pool and grass, each in its quadrant of one chunk.
pub fn terrain_materials(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut scene = quadrant_scene(adapter)?;
    let mut offscreen = Offscreen::new(&mut scene.driver.world, SIZE, FORMAT)?;
    let frame = request(top_down(32.0, -32.0, GROUND_Y + 100.0));
    let image = offscreen.image(&mut scene.driver.world, &frame)?;
    save(check, &image, "materials");
    let quadrant = |x, y| ground(IVec2::new(x, y));
    let stone = sample(&image, &frame, quadrant(16, 16))?;
    let sand = sample(&image, &frame, quadrant(48, 16))?;
    let water = sample(
        &image,
        &frame,
        quadrant(16, 48) - DVec3::new(0.0, 15.0, 0.0),
    )?;
    let grass = sample(&image, &frame, quadrant(48, 48))?;
    check.that(colour::is_grey(stone), || format!("stone is {stone:?}"));
    check.that(colour::is_sand(sand), || format!("sand is {sand:?}"));
    check.that(colour::is_water(water), || format!("water is {water:?}"));
    check.that(colour::is_grass(grass), || format!("grass is {grass:?}"));
    for (name, value) in [
        ("stone", stone),
        ("sand", sand),
        ("water", water),
        ("grass", grass),
    ] {
        check.metric(name, format!("{value:?}"));
    }
    Ok(())
}

/// A chunk that is not resident draws nothing (the clear colour shows through, never another
/// chunk's cells), at a positive and a negative key; resident chunks meet without a gap.
pub fn missing_chunks(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut scene = quadrant_scene(adapter)?;
    let mut offscreen = Offscreen::new(&mut scene.driver.world, SIZE, FORMAT)?;
    let frame = request(top_down(0.0, 0.0, GROUND_Y + 110.0));
    let before = offscreen.image(&mut scene.driver.world, &frame)?;
    // A line across the seams between the origin chunk and its east and west neighbours.
    let seam: Vec<[f32; 3]> = (-4..4)
        .chain(60..68)
        .map(|x| sample(&before, &frame, ground(IVec2::new(x, 48))))
        .collect::<Result<_, _>>()?;
    for key in [ChunkKey::new(1, 0, 0), ChunkKey::new(-1, -1, 0)] {
        scene.retire(key)?;
    }
    scene.step(&[])?;
    let after = offscreen.image(&mut scene.driver.world, &frame)?;
    save(check, &after, "missing");
    let gap = |key: ChunkKey, offsets: &[(i32, i32)]| -> Result<Vec<[f32; 3]>, String> {
        let origin = key.origin_cell();
        offsets
            .iter()
            .map(|(dx, dy)| sample(&after, &frame, ground(origin + IVec2::new(*dx, *dy))))
            .collect()
    };
    let points = [(10, 10), (32, 32), (50, 20), (20, 50)];
    let east = gap(ChunkKey::new(1, 0, 0), &points)?;
    let south_west = gap(ChunkKey::new(-1, -1, 0), &points)?;
    let clear = east[0];
    for value in east.iter().chain(&south_west) {
        check.that(colour::distance(*value, clear) < 1.5, || {
            format!("a missing chunk shows {value:?}, not the uniform clear colour {clear:?}")
        });
    }
    check.that(!colour::is_grass(clear), || {
        format!("the gap shows grass: {clear:?}")
    });
    for value in &seam {
        check.that(colour::distance(*value, clear) > 20.0, || {
            format!("a seam between resident chunks shows the clear colour: {value:?}")
        });
    }
    let kept = sample(&after, &frame, ground(IVec2::new(48, 48)))?;
    check.that(colour::is_grass(kept), || {
        format!("a resident neighbour of the gap is {kept:?}")
    });
    check.metric("clear_colour", format!("{clear:?}"));
    Ok(())
}

/// A brush stroke applied by a tick is drawn by the next frame from the same buffers: nothing is
/// uploaded, and no pipeline, bind group or target is created for it.
pub fn edit_visible(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut scene = quadrant_scene(adapter)?;
    let mut offscreen = Offscreen::new(&mut scene.driver.world, SIZE, FORMAT)?;
    let frame = request(top_down(16.0, -16.0, GROUND_Y + 80.0));
    let target = ground(IVec2::new(16, 16));
    let before = offscreen.image(&mut scene.driver.world, &frame)?;
    let stone = sample(&before, &frame, target)?;
    let counters = offscreen.renderer.counters();
    let edit = MaterialEdit::new(EditTool::Water, IVec2::new(16, 16), 6, 40)
        .map_err(|error| format!("{error:?}"))?;
    scene.step(&[Command::EditMaterial(edit)])?;
    let after = offscreen.image(&mut scene.driver.world, &frame)?;
    let water = sample(&after, &frame, target + DVec3::new(0.0, 20.0, 0.0))?;
    check.that(colour::is_grey(stone), || {
        format!("before the stroke: {stone:?}")
    });
    check.that(colour::is_water(water), || {
        format!("after the stroke: {water:?}")
    });
    let now = offscreen.renderer.counters();
    check.equal(
        "pipelines",
        now.pipelines_created,
        counters.pipelines_created,
    );
    check.equal(
        "world bind groups",
        now.world_bind_groups_created,
        counters.world_bind_groups_created,
    );
    check.equal(
        "target sets",
        now.target_sets_created,
        counters.target_sets_created,
    );
    check.metric(
        "uniform_bytes_per_frame",
        (now.uniform_bytes_written - counters.uniform_bytes_written) as f64,
    );
    Ok(())
}

fn flat_grass(_cell: IVec2, column: &mut WorldCell) {
    grass(column);
}

fn spawn_metal(scene: &mut Scene, x: f32, z: f32) -> Result<BodyHandle, String> {
    let body = scene.spawn(BodyPreset::Metal.description(), x, z, 6.0)?;
    scene.run(90)?;
    Ok(body)
}

/// A sphere is drawn where the device holds it: its observed position projects onto metal, and
/// the ground beside it stays grass.
pub fn sphere_at_gpu_pose(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut scene = Scene::open(adapter, Setup::default(), flat_grass)?;
    let body = spawn_metal(&mut scene, 30.0, -30.0)?;
    let position = scene.sample(body)?.position;
    let mut offscreen = Offscreen::new(&mut scene.driver.world, SIZE, FORMAT)?;
    let frame = request(top_down(
        position.x + 6.0,
        position.z + 4.0,
        position.y + 40.0,
    ));
    let image = offscreen.image(&mut scene.driver.world, &frame)?;
    save(check, &image, "sphere");
    let sphere = sample(&image, &frame, position + DVec3::new(0.0, 4.0, 0.0))?;
    let beside = sample(
        &image,
        &frame,
        DVec3::new(position.x + 12.0, GROUND_Y, position.z),
    )?;
    check.that(colour::is_grey(sphere) && !colour::is_grass(sphere), || {
        format!("the sphere's position shows {sphere:?}")
    });
    check.that(colour::is_grass(beside), || {
        format!("beside the sphere: {beside:?}")
    });
    check.equal(
        "body instances drawn",
        offscreen.renderer.counters().body_instances,
        1,
    );
    check.metric("position", format!("{position}"));
    Ok(())
}

/// The following camera is placed from the body's state on the device: while the sphere is
/// driven and the CPU's last sample is many ticks old, the sphere stays at the same place on
/// screen in every frame, and a camera placed from that stale sample loses it.
pub fn follow_camera(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut scene = Scene::open(adapter, Setup::default(), flat_grass)?;
    let body = spawn_metal(&mut scene, 30.0, -30.0)?;
    let stale = scene.sample(body)?.position;
    let mut offscreen = Offscreen::new(&mut scene.driver.world, SIZE, FORMAT)?;
    let (yaw, pitch, distance) = (0.3f32, -0.45f32, 18.0f32);
    let fov = world_render::frame::DEFAULT_FOV_Y_DEGREES.to_radians();
    let aspect = offscreen.aspect();
    // For any body position B, the follow camera's eye is B behind the tether plus the lens.
    let eye_of = |body: DVec3| {
        body - (forward(yaw, pitch) * distance).as_dvec3()
            + DVec3::new(0.0, f64::from(LENS_OFFSET_Y), 0.0)
    };
    // The upper part of the sphere: above the ground even while it sits in its own rut.
    let top = DVec3::new(0.0, 2.5, 0.0);
    let fixed = project(eye_of(DVec3::ZERO), yaw, pitch, fov, aspect, top)
        .ok_or("the followed body is behind its camera")?;
    let follow = CameraRig::Follow {
        body,
        fallback: stale,
        yaw,
        pitch,
        distance,
    };
    let direction = Direction::new(1.0, 0.0).map_err(|error| format!("{error:?}"))?;
    let drive = Command::DriveBody {
        body,
        drive: Drive::controls(direction),
    };
    let mut held = 0;
    for frame_index in 0..40 {
        // Ticks are submitted and the frame drawn without the CPU observing them first.
        scene
            .driver
            .submit(&[drive])
            .map_err(|refusal| format!("submit: {refusal:?}"))?;
        let mut frame = request(follow);
        frame.shadow_focus = stale;
        let image = offscreen.image(&mut scene.driver.world, &frame)?;
        let centre = image.mean_at(fixed, 1);
        if colour::is_grey(centre) && !colour::is_grass(centre) {
            held += 1;
        } else {
            check.fail(format!(
                "frame {frame_index}: the followed sphere left its place on screen ({centre:?})"
            ));
        }
        if frame_index == 39 {
            save(check, &image, "follow");
        }
        scene.driver.poll();
    }
    scene.driver.settle()?;
    let moved = scene
        .driver
        .outcomes
        .iter()
        .rev()
        .find_map(|outcome| match outcome {
            world_runtime::port::Outcome::Body(sample) if sample.body == body => {
                Some(sample.position)
            }
            _ => None,
        });
    // The same frame from a free camera placed from the stale sample, as a CPU-side follow
    // would place it.
    let free = CameraRig::Free {
        position: eye_of(stale) - DVec3::new(0.0, f64::from(LENS_OFFSET_Y), 0.0),
        yaw,
        pitch,
    };
    let image = offscreen.image(&mut scene.driver.world, &request(free))?;
    let stale_centre = image.mean_at(fixed, 1);
    check.that(colour::is_grass(stale_centre), || {
        format!("a camera placed from the stale sample still shows the sphere at the follow point ({stale_centre:?})")
    });
    check.metric("frames_holding_the_sphere", held);
    check.metric("stale_sample", format!("{stale}"));
    check.metric("moved_to", format!("{moved:?}"));
    Ok(())
}

/// Bind groups over the world's buffers are made once per buffer generation: many frames of
/// changing cells create none, a window resize (new buffers) creates exactly one more, and the
/// moved chunks still draw.
pub fn bind_groups_follow_buffers(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut scene = quadrant_scene(adapter)?;
    let mut offscreen = Offscreen::new(&mut scene.driver.world, SIZE, FORMAT)?;
    let frame = request(top_down(32.0, -32.0, GROUND_Y + 100.0));
    for index in 0..30 {
        let edit = MaterialEdit::new(EditTool::Sand, IVec2::new(40 + index % 8, 40), 2, 1)
            .map_err(|error| format!("{error:?}"))?;
        scene.step(&[Command::EditMaterial(edit)])?;
        offscreen.draw(&mut scene.driver.world, &frame)?;
    }
    let counters = offscreen.renderer.counters();
    check.equal(
        "bind groups after 30 frames of edits",
        counters.world_bind_groups_created,
        1,
    );
    let generation = scene.driver.world.view().buffers_generation;
    let session = scene.driver.session();
    let window =
        ResidentWindow::new(ChunkKey::new(0, 0, 0), 2, 2).map_err(|error| format!("{error:?}"))?;
    scene
        .driver
        .world
        .resize(&ResizeRequest { session, window })
        .map_err(|refusal| format!("resize: {refusal:?}"))?;
    scene.step(&[])?;
    check.that(
        scene.driver.world.view().buffers_generation != generation,
        || "the resize did not replace the window's buffers".to_owned(),
    );
    let image = offscreen.image(&mut scene.driver.world, &frame)?;
    let counters = offscreen.renderer.counters();
    check.equal(
        "bind groups after the resize",
        counters.world_bind_groups_created,
        2,
    );
    check.equal(
        "pipelines",
        counters.pipelines_created,
        world_render::renderer::PIPELINES,
    );
    let stone = sample(&image, &frame, ground(IVec2::new(16, 16)))?;
    check.that(colour::is_grey(stone), || {
        format!("the moved stone chunk draws {stone:?}")
    });
    check.equal(
        "chunk instances after the resize",
        counters.chunk_instances,
        25,
    );
    Ok(())
}

/// The activity overlay draws the tiles the last tick scheduled: a stroke's tiles are tinted
/// with the overlay on, and an idle area is identical with it on or off.
pub fn activity_overlay(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut scene = Scene::open(adapter, Setup::default(), flat_grass)?;
    let mut offscreen = Offscreen::new(&mut scene.driver.world, SIZE, FORMAT)?;
    let edit = MaterialEdit::new(EditTool::Sand, IVec2::new(20, 20), 4, 2)
        .map_err(|error| format!("{error:?}"))?;
    scene.step(&[Command::EditMaterial(edit)])?;
    scene.step(&[])?;
    let mut frame = request(top_down(32.0, -32.0, GROUND_Y + 100.0));
    let off = offscreen.image(&mut scene.driver.world, &frame)?;
    frame.overlay.activity = true;
    let on = offscreen.image(&mut scene.driver.world, &frame)?;
    save(check, &on, "activity");
    let edited = ground(IVec2::new(20, 20)) + DVec3::new(0.0, 2.0, 0.0);
    let idle = ground(IVec2::new(52, 52));
    let edited_off = sample(&off, &frame, edited)?;
    let edited_on = sample(&on, &frame, edited)?;
    let idle_off = sample(&off, &frame, idle)?;
    let idle_on = sample(&on, &frame, idle)?;
    check.that(colour::distance(edited_off, edited_on) > 15.0, || {
        format!("the stroke's tile is not tinted: {edited_off:?} against {edited_on:?}")
    });
    check.that(colour::distance(idle_off, idle_on) < 1.0, || {
        format!("an idle tile changed with the overlay: {idle_off:?} against {idle_on:?}")
    });
    Ok(())
}

/// After the device is lost, the renderer's frames are refused before anything is encoded, and
/// dropping it does not touch the lost device.
pub fn device_loss(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut config = driver::config(adapter, 1);
    config.fault_injection = true;
    let mut world = driver::Driver::open(config)?;
    world.upload(ChunkKey::new(0, 0, 0), &driver::flat(GROUND))?;
    let mut offscreen = Offscreen::new(&mut world.world, SIZE, FORMAT)?;
    let frame = request(top_down(32.0, -32.0, GROUND_Y + 100.0));
    offscreen.image(&mut world.world, &frame)?;
    let faults = world
        .world
        .fault_controller()
        .ok_or("no fault controller")?;
    faults.lose_device();
    world.poll();
    world.poll();
    match world.world.presentation_ready() {
        Err(PresentRefusal::Closed(Refusal::DeviceLost)) => {}
        other => check.fail(format!("presentation after the loss: {other:?}")),
    }
    match offscreen.draw(&mut world.world, &frame) {
        Err(error) if error.contains("DeviceLost") => {}
        other => check.fail(format!("a frame after the loss: {other:?}")),
    }
    drop(offscreen);
    check.note("the renderer was dropped after the loss without a panic");
    Ok(())
}

/// The readback's timestamps of the scene and post passes, in nanoseconds.
pub fn pass_times(
    world: &mut gpu_world::world::GpuWorld,
    offscreen: &Offscreen,
    period_ns: f32,
) -> Result<(f64, f64), String> {
    let resolve = offscreen
        .renderer
        .timestamp_buffer()
        .ok_or("timestamps are not enabled")?
        .clone();
    let copy = world.view().device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("render timestamps readback"),
        size: resolve.size(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    world
        .submit_presentation("timestamps", |_, encoder| {
            encoder.copy_buffer_to_buffer(&resolve, 0, &copy, 0, resolve.size());
        })
        .map_err(|refusal| format!("{refusal:?}"))?;
    let ticks = map_read(world, &copy, |bytes| {
        bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|word| u64::from_le_bytes(*word))
            .collect::<Vec<u64>>()
    })?;
    let span = |begin: usize, end: usize| {
        ticks[end].saturating_sub(ticks[begin]) as f64 * f64::from(period_ns)
    };
    Ok((span(0, 1), span(2, 3)))
}

/// The instance counts the last frame's layer draws hold (terrain, granular, fluid, surface).
pub fn draw_counts(
    world: &mut gpu_world::world::GpuWorld,
    offscreen: &Offscreen,
) -> Result<[u32; 4], String> {
    let args = offscreen.renderer.draw_args().clone();
    let copy = world.view().device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("layer draws readback"),
        size: args.size(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    world
        .submit_presentation("layer draws", |_, encoder| {
            encoder.copy_buffer_to_buffer(&args, 0, &copy, 0, args.size());
        })
        .map_err(|refusal| format!("{refusal:?}"))?;
    map_read(world, &copy, |bytes| {
        let words: Vec<u32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| u32::from_le_bytes(*word))
            .collect();
        [words[1], words[6], words[11], words[16]]
    })
}
