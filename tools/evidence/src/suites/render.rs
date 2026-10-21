//! The `render` GPU suite: the production renderer (`world-render`) on the GPU owner's device,
//! drawing synthetic worlds offscreen and read back, and its cost on generated terrain.

use super::Case;

mod cases;
mod measure;
pub mod offscreen;
mod visibility;

pub const CASES: &[Case] = &[
    Case {
        name: "formats",
        summary: "the renderer builds for every surface format the client may get, encodes sRGB exactly once whichever it gets, and refuses a format it cannot write",
        run: cases::formats,
    },
    Case {
        name: "zero-size",
        summary: "a zero-sized output allocates no targets and draws nothing; a later size draws again",
        run: cases::zero_size,
    },
    Case {
        name: "terrain-materials",
        summary: "stone, sand, a water pool and grass are drawn from the resident cells with their materials' looks",
        run: cases::terrain_materials,
    },
    Case {
        name: "missing-chunks",
        summary: "a chunk that is not resident draws nothing, at positive and negative keys; resident chunks meet without a gap",
        run: cases::missing_chunks,
    },
    Case {
        name: "edit-visible",
        summary: "a stroke applied by a tick is drawn by the next frame from the same buffers, with no pipeline, bind group or target created",
        run: cases::edit_visible,
    },
    Case {
        name: "sphere-at-gpu-pose",
        summary: "a sphere is drawn where the device holds it",
        run: cases::sphere_at_gpu_pose,
    },
    Case {
        name: "follow-camera",
        summary: "a followed, driven sphere stays at one place on screen from the device's pose while the CPU's sample is stale; a camera from the stale sample loses it",
        run: cases::follow_camera,
    },
    Case {
        name: "bind-groups-follow-buffers",
        summary: "frames of changing cells create no bind group; a window resize creates exactly one and the moved chunks draw",
        run: cases::bind_groups_follow_buffers,
    },
    Case {
        name: "activity-overlay",
        summary: "the activity overlay tints the tiles the last tick scheduled and leaves idle tiles unchanged",
        run: cases::activity_overlay,
    },
    Case {
        name: "device-loss",
        summary: "after a device loss, frames are refused before encoding and the renderer drops cleanly",
        run: cases::device_loss,
    },
    Case {
        name: "presentation-admission",
        summary: "the admission poll a client makes between frames frees completed presentation frames and ticks without decoding results, and keeps them while completions are held",
        run: cases::presentation_admission,
    },
    Case {
        name: "culling-matches-unculled",
        summary: "the culled frame equals the unculled one pixel for pixel from four poses, draws fewer chunks, and an edit made out of view shows when the camera turns to it",
        run: visibility::culling_matches_unculled,
    },
    Case {
        name: "frame-cost",
        summary: "device time of the scene and post passes, CPU encode time and renderer memory at radius 6 and 16 on generated terrain (measurement)",
        run: measure::frame_cost,
    },
];
