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
