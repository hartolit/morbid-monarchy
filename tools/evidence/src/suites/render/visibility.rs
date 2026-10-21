//! Visibility: the culled frame equals the unculled one, and nothing it derives is stale.

use world_model::{
    command::{Command, EditTool, MaterialEdit},
    glam::{DVec3, IVec2},
};
use world_render::CameraRig;

use super::{
    super::Check,
    cases::draw_counts,
    measure::generated,
    offscreen::{Offscreen, colour, ndc_of, request},
};

const SIZE: [u32; 2] = [640, 360];

/// Poses over generated terrain around chunk (3, 3): the qualification camera, a top-down view
/// onto a chunk corner, a low view along the window's edge, and one looking out of the window.
fn poses() -> [(&'static str, CameraRig); 4] {
    [
        (
            "qualification",
            CameraRig::Free {
                position: DVec3::new(200.0, 150.0, -230.0),
                yaw: 30f32.to_radians(),
                pitch: (-40f32).to_radians(),
            },
        ),
        (
            "corner-top-down",
            CameraRig::Free {
                position: DVec3::new(256.0, 140.0, -256.0),
                yaw: 0.0,
                pitch: -1.54,
            },
        ),
        (
            "window-edge-low",
            CameraRig::Free {
                position: DVec3::new(-170.0, 90.0, -40.0),
                yaw: (-90f32).to_radians(),
                pitch: (-8f32).to_radians(),
            },
        ),
        (
            "looking-out",
            CameraRig::Free {
                position: DVec3::new(600.0, 120.0, -230.0),
                yaw: (-90f32).to_radians(),
                pitch: (-20f32).to_radians(),
            },
        ),
    ]
}

pub fn culling_matches_unculled(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = generated(adapter, 6)?;
    // Water across the corner of four chunks, so a layer reaches chunks from a neighbour.
    let edit = MaterialEdit::new(EditTool::Water, IVec2::new(256, 256), 6, 30)
        .map_err(|error| format!("{error:?}"))?;
    driver
        .submit(&[Command::EditMaterial(edit)])
        .map_err(|refusal| format!("{refusal:?}"))?;
    driver.settle()?;
    let mut offscreen =
        Offscreen::new(&mut driver.world, SIZE, wgpu::TextureFormat::Rgba8UnormSrgb)?;
    let candidates = driver.world.view().window.chunk_count();
    for (name, camera) in poses() {
        let mut frame = request(camera);
        frame.lighting = world_render::Lighting::default();
        offscreen.renderer.set_culling(true);
        let culled = offscreen.image(&mut driver.world, &frame)?;
        let drawn = draw_counts(&mut driver.world, &offscreen)?;
        offscreen.renderer.set_culling(false);
        let all = offscreen.image(&mut driver.world, &frame)?;
        let differing = culled
            .rgba
            .iter()
            .zip(&all.rgba)
            .filter(|(a, b)| a != b)
            .count();
        let worst = culled
            .rgba
            .iter()
            .zip(&all.rgba)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        check.that(differing == 0, || {
            format!("{name}: {differing} channel values differ from the unculled frame (up to {worst} levels)")
        });
        check.metric(
            &format!("{name}_drawn_per_layer"),
            format!("{drawn:?} of {candidates}"),
        );
        if name == "qualification" {
            check.that(drawn[0] < candidates, || {
                format!("{name}: the terrain layer drew all {candidates} chunks")
            });
        }
    }
    offscreen.renderer.set_culling(true);

    // An edit made while its chunk is out of view shows the moment the camera turns to it: the
    // bounds and layers are derived from the cells every frame.
    let away = CameraRig::Free {
        position: DVec3::new(200.0, 150.0, -230.0),
        yaw: 0.0,
        pitch: (-30f32).to_radians(),
    };
    let target = IVec2::new(200, 150);
    offscreen.image(&mut driver.world, &request(away))?;
    let edit =
        MaterialEdit::new(EditTool::Water, target, 5, 60).map_err(|error| format!("{error:?}"))?;
    driver
        .submit(&[Command::EditMaterial(edit)])
        .map_err(|refusal| format!("{refusal:?}"))?;
    driver.settle()?;
    let back = request(CameraRig::Free {
        position: DVec3::new(200.0, 160.0, -150.0),
        yaw: 0.0,
        pitch: -1.54,
    });
    let image = offscreen.image(&mut driver.world, &back)?;
    let ndc = ndc_of(&back, offscreen.aspect(), DVec3::new(200.0, 60.0, -150.0))
        .ok_or("the edit is behind the camera")?;
    let seen = image.mean_at(ndc, 2);
    check.that(colour::is_water(seen), || {
        format!("the out-of-view edit is not drawn after turning to it: {seen:?}")
    });
    Ok(())
}

/// How many chunks the last frame's relight pass refreshed.
fn relit(world: &mut gpu_world::world::GpuWorld, offscreen: &Offscreen) -> Result<u32, String> {
    let args = offscreen.renderer.relight_args().clone();
    let copy = world.view().device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("relight readback"),
        size: args.size(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    world
        .submit_presentation("relight readback", |_, encoder| {
            encoder.copy_buffer_to_buffer(&args, 0, &copy, 0, args.size());
        })
        .map_err(|refusal| format!("{refusal:?}"))?;
    super::offscreen::map_read(world, &copy, |bytes| {
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    })
}

/// The largest difference of any channel between two images, and how many channel values
/// differ at all.
fn difference(a: &super::offscreen::Image, b: &super::offscreen::Image) -> (u8, usize) {
    let worst = a
        .rgba
        .iter()
        .zip(&b.rgba)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0);
    let differing = a.rgba.iter().zip(&b.rgba).filter(|(x, y)| x != y).count();
    (worst, differing)
}

/// The terrain lighting cache draws what per-vertex lighting draws, to within one level of the
/// 8-bit output (the cached terms are rounded to 1/255, which moves the shaded colour by well under
/// a level), and refreshes only what it must: nothing on a frame without change, the
/// neighbourhood of an edit, every drawn chunk when the sun moves or the window does, and a chunk
/// edited out of view once it is drawn again. Flames, lit per vertex, are among the edits.
pub fn lighting_cache_matches(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = generated(adapter, 6)?;
    // Water across the corner of four chunks and a burning patch.
    for edit in [
        MaterialEdit::new(EditTool::Water, IVec2::new(256, 256), 6, 30),
        MaterialEdit::new(EditTool::Fire, IVec2::new(230, 240), 3, 1),
    ] {
        let edit = edit.map_err(|error| format!("{error:?}"))?;
        driver
            .submit(&[Command::EditMaterial(edit)])
            .map_err(|refusal| format!("{refusal:?}"))?;
    }
    driver.settle()?;
    let mut offscreen =
        Offscreen::new(&mut driver.world, SIZE, wgpu::TextureFormat::Rgba8UnormSrgb)?;
    let mut worst_all = 0;
    for (name, camera) in poses() {
        let mut frame = request(camera);
        frame.lighting = world_render::Lighting::default();
        offscreen.renderer.set_lighting_cache(true);
        let cached = offscreen.image(&mut driver.world, &frame)?;
        offscreen.renderer.set_lighting_cache(false);
        let live = offscreen.image(&mut driver.world, &frame)?;
        offscreen.renderer.set_lighting_cache(true);
        let (worst, differing) = difference(&cached, &live);
        worst_all = worst_all.max(worst);
        check.that(worst <= 1, || {
            format!("{name}: the cached frame differs by up to {worst} levels ({differing} values)")
        });
        check.metric(&format!("{name}_values_differing"), differing);
        check.metric(&format!("{name}_worst_level"), u32::from(worst));
    }
    check.metric("worst_level", u32::from(worst_all));

    // Relighting: the cache was just switched back on, so this frame relights what it draws;
    // the next one, with nothing changed, relights nothing.
    let qualification = poses()[0].1;
    let mut frame = request(qualification);
    frame.lighting = world_render::Lighting::default();
    offscreen.image(&mut driver.world, &frame)?;
    let drawn = draw_counts(&mut driver.world, &offscreen)?[0];
    offscreen.image(&mut driver.world, &frame)?;
    check.equal(
        "chunks relit with nothing changed",
        relit(&mut driver.world, &offscreen)?,
        0,
    );

    // An edit relights its neighbourhood and the chunks whose shadows reach it, no more.
    let edit = MaterialEdit::new(EditTool::Raise, IVec2::new(225, 225), 2, 20)
        .map_err(|error| format!("{error:?}"))?;
    driver
        .submit(&[Command::EditMaterial(edit)])
        .map_err(|refusal| format!("{refusal:?}"))?;
    driver.settle()?;
    let cached = offscreen.image(&mut driver.world, &frame)?;
    let after_edit = relit(&mut driver.world, &offscreen)?;
    check.that((1..=27).contains(&after_edit), || {
        format!("an edit in one chunk relit {after_edit} chunks")
    });
    check.metric("chunks_relit_by_one_edit", after_edit);
    offscreen.renderer.set_lighting_cache(false);
    let live = offscreen.image(&mut driver.world, &frame)?;
    offscreen.renderer.set_lighting_cache(true);
    let (worst, differing) = difference(&cached, &live);
    check.that(worst <= 1, || {
        format!(
            "after an edit the cached frame differs by up to {worst} levels ({differing} values)"
        )
    });
    offscreen.image(&mut driver.world, &frame)?;

    // The sun moves: every drawn chunk is relit.
    frame.lighting.sun_azimuth_degrees += 40.0;
    let cached = offscreen.image(&mut driver.world, &frame)?;
    let after_sun = relit(&mut driver.world, &offscreen)?;
    let drawn_now = draw_counts(&mut driver.world, &offscreen)?[0];
    check.equal("chunks relit when the sun moves", after_sun, drawn_now);
    offscreen.renderer.set_lighting_cache(false);
    let live = offscreen.image(&mut driver.world, &frame)?;
    offscreen.renderer.set_lighting_cache(true);
    let (worst, _) = difference(&cached, &live);
    check.that(worst <= 1, || {
        format!("after the sun moved the cached frame differs by up to {worst} levels")
    });
    check.metric("chunks_drawn", drawn);

    // A chunk edited while out of view is relit when it is drawn again.
    let away = request(CameraRig::Free {
        position: DVec3::new(200.0, 150.0, -230.0),
        yaw: 0.0,
        pitch: (-30f32).to_radians(),
    });
    offscreen.image(&mut driver.world, &away)?;
    let edit = MaterialEdit::new(EditTool::Lower, IVec2::new(200, 150), 5, 30)
        .map_err(|error| format!("{error:?}"))?;
    driver
        .submit(&[Command::EditMaterial(edit)])
        .map_err(|refusal| format!("{refusal:?}"))?;
    driver.settle()?;
    offscreen.image(&mut driver.world, &away)?;
    let back = request(CameraRig::Free {
        position: DVec3::new(200.0, 160.0, -150.0),
        yaw: 0.0,
        pitch: -1.54,
    });
    let cached = offscreen.image(&mut driver.world, &back)?;
    offscreen.renderer.set_lighting_cache(false);
    let live = offscreen.image(&mut driver.world, &back)?;
    offscreen.renderer.set_lighting_cache(true);
    let (worst, differing) = difference(&cached, &live);
    check.that(worst <= 1, || {
        format!("an edit made out of view is drawn with stale lighting: up to {worst} levels ({differing} values)")
    });
    check.metric(
        "lighting_cache_bytes",
        offscreen.renderer.counters().lighting_bytes,
    );
    Ok(())
}
