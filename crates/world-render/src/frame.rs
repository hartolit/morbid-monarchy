//! What the client asks a frame to show, and its packing into the frame uniform.
//!
//! Positions the client gives are absolute render space (`f64`): x is the cell x, y is up and
//! z is minus the cell y, as `world_model::spatial` and the body samples use. The renderer works
//! in window-local render space (origin at the first cell of the window's minimum chunk), so a
//! far-away world keeps its precision in `f32`; the conversion happens here, once per frame,
//! against the window of the buffers the frame draws.

use gpu_world::world::WorldView;
use world_model::{
    body::BodyHandle,
    glam::{DVec3, IVec2, Mat4, Vec2, Vec3},
    spatial::CHUNK_SIZE,
};
use world_runtime::port::ResidentWindow;

use crate::lighting::{Lighting, linear};

/// `Frame::follow.w` bits: the activity tiles of the last tick, and the bodies' contact state.
pub const OVERLAY_ACTIVITY: u32 = 1;
pub const OVERLAY_BODIES: u32 = 2;

/// The eye sits this far above the observer or the tethered point (the baseline's lens offset;
/// camera.wgsl adds the same).
pub const LENS_OFFSET_Y: f32 = 0.7;

/// Where the camera is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CameraRig {
    /// An observer at `position` (absolute render space), looking along yaw and pitch.
    Free {
        position: DVec3,
        yaw: f32,
        pitch: f32,
    },
    /// Tethered `distance` behind a body, at the body's position on the device. While the body
    /// is not live (not yet spawned, removed, or a stale generation), the camera stays at
    /// `fallback`.
    Follow {
        body: BodyHandle,
        fallback: DVec3,
        yaw: f32,
        pitch: f32,
        distance: f32,
    },
}

impl CameraRig {
    pub fn angles(&self) -> (f32, f32) {
        match *self {
            Self::Free { yaw, pitch, .. } | Self::Follow { yaw, pitch, .. } => (yaw, pitch),
        }
    }
}

/// Which debug views draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Overlay {
    /// Tint terrain by the activity tiles the last tick scheduled (awake, active).
    pub activity: bool,
    /// Tint spheres by their contact state: grounded, suspended at the residency edge, in
    /// liquid.
    pub bodies: bool,
}

impl Overlay {
    fn bits(self) -> u32 {
        u32::from(self.activity) * OVERLAY_ACTIVITY + u32::from(self.bodies) * OVERLAY_BODIES
    }
}

/// The brush cursor: the target cell the last ray query reported and the stroke's radius.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cursor {
    pub cell: IVec2,
    pub radius: f32,
}

/// One frame's request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameRequest {
    pub camera: CameraRig,
    /// Vertical field of view, radians.
    pub fov_y: f32,
    pub near: f32,
    pub lighting: Lighting,
    pub cursor: Option<Cursor>,
    pub overlay: Overlay,
    /// Animation time (flames, ripples), seconds.
    pub seconds: f32,
    /// Where the sphere shadow map is centred (absolute render space); the camera's focus.
    pub shadow_focus: DVec3,
}

/// The baseline observer's vertical field of view.
pub const DEFAULT_FOV_Y_DEGREES: f32 = 85.0;
pub const DEFAULT_NEAR: f32 = 0.1;

/// The sphere shadow map covers this far around its focus on each side, in world units, and
/// this far along the sun.
const SHADOW_HALF_EXTENT: f32 = 96.0;
const SHADOW_DEPTH: f32 = 4096.0;
pub const SHADOW_MAP_SIZE: u32 = 2048;
const SHADOW_DEPTH_BIAS: f32 = 0.0005;

/// Window-local render position of an absolute render-space position.
pub fn window_local(window: ResidentWindow, position: DVec3) -> Vec3 {
    let min = window.min_key().to_ivec2();
    let size = CHUNK_SIZE as f64;
    Vec3::new(
        (position.x - f64::from(min.x) * size) as f32,
        position.y as f32,
        (position.z + f64::from(min.y) * size) as f32,
    )
}

/// The observer's forward direction for `yaw` and `pitch` (yaw about +Y, then pitch about the
/// lens's X; forward is -Z at zero).
pub fn forward(yaw: f32, pitch: f32) -> Vec3 {
    let (sin_pitch, cos_pitch) = pitch.sin_cos();
    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    // rotate_x(0, 0, -1) = (0, sin p, -cos p); then rotate_y.
    let x = Vec3::new(0.0, sin_pitch, -cos_pitch);
    Vec3::new(
        cos_yaw * x.x + sin_yaw * x.z,
        x.y,
        -sin_yaw * x.x + cos_yaw * x.z,
    )
}

/// The eye of a camera rig for a body at `body_position` (absolute render space), as
/// camera.wgsl places it: the CPU's estimate, for the cursor ray and the residency focus.
pub fn eye_position(rig: &CameraRig, body_position: Option<DVec3>) -> DVec3 {
    let lens = DVec3::new(0.0, f64::from(LENS_OFFSET_Y), 0.0);
    match *rig {
        CameraRig::Free { position, .. } => position + lens,
        CameraRig::Follow {
            fallback,
            yaw,
            pitch,
            distance,
            ..
        } => match body_position {
            Some(body) => body + (-forward(yaw, pitch) * distance).as_dvec3() + lens,
            None => fallback + lens,
        },
    }
}

/// The world ray through a point of the screen, `ndc` in -1..1 (x right, y up), from `eye`.
pub fn view_ray(yaw: f32, pitch: f32, fov_y: f32, aspect: f32, ndc: Vec2) -> Vec3 {
    let forward = forward(yaw, pitch);
    let right = forward.cross(Vec3::Y).normalize_or_zero();
    let up = right.cross(forward);
    let half_height = (fov_y * 0.5).tan();
    (forward + right * (ndc.x * half_height * aspect) + up * (ndc.y * half_height)).normalize()
}

/// The frame uniform; must match `Frame` in shaders/types.wgsl.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct FrameUniform {
    window: [i32; 4],
    window_cells: [f32; 4],
    cursor: [f32; 4],
    sun_direction: [f32; 4],
    sun_color: [f32; 4],
    sky_color: [f32; 4],
    ground_color: [f32; 4],
    horizon_color: [f32; 4],
    sun_glow: [f32; 4],
    fog: [f32; 4],
    eye: [f32; 4],
    angles: [f32; 4],
    projection: [f32; 4],
    follow: [u32; 4],
    offsets: [u32; 4],
    shadow_view_proj: [[f32; 4]; 4],
    shadow: [f32; 4],
}

/// Bytes of the `View` record camera.wgsl writes: a matrix and three vectors.
pub(crate) const VIEW_RECORD_BYTES: u64 = 16 * 4 + 3 * 16;

impl FrameUniform {
    pub(crate) fn pack(view: &WorldView<'_>, request: &FrameRequest, aspect: f32) -> Self {
        let window = view.window;
        let min = window.min_key().to_ivec2();
        let chunks = [window.width() as i32, window.height() as i32];
        let size = CHUNK_SIZE as f32;
        let lighting = request.lighting.sanitized();
        let [sun_x, sun_y, sun_z] = lighting.sun_direction();
        let sun = linear(lighting.sun_color);
        let sky = linear(lighting.sky_color);
        let ground = linear(lighting.ground_color);
        let horizon = linear(lighting.horizon_color);
        let glow = linear([
            lighting.sun_glow_color[0],
            lighting.sun_glow_color[1],
            lighting.sun_glow_color[2],
        ]);
        let cursor = match request.cursor {
            Some(cursor) => {
                let local = cursor.cell - min * CHUNK_SIZE as i32;
                [local.x as f32, local.y as f32, cursor.radius, 0.0]
            }
            None => [0.0, 0.0, -1.0, 0.0],
        };
        let (eye, follow_slot, generation, following, distance) = match request.camera {
            CameraRig::Free { position, .. } => (position, 0, 0, 0.0, 0.0),
            CameraRig::Follow {
                body,
                fallback,
                distance,
                ..
            } => (fallback, body.index(), body.generation(), 1.0, distance),
        };
        let eye = window_local(window, eye);
        let (yaw, pitch) = request.camera.angles();
        let half_extent = chunks[0].min(chunks[1]) as f32 * size / 2.0;
        let focus = window_local(window, request.shadow_focus);
        let sun = [sun[0], sun[1], sun[2]];
        let sun_direction = Vec3::new(sun_x, sun_y, sun_z);
        let shadow = Mat4::orthographic_rh(
            -SHADOW_HALF_EXTENT,
            SHADOW_HALF_EXTENT,
            -SHADOW_HALF_EXTENT,
            SHADOW_HALF_EXTENT,
            0.0,
            SHADOW_DEPTH,
        ) * Mat4::look_to_rh(
            focus + sun_direction * (SHADOW_DEPTH * 0.5),
            -sun_direction,
            if sun_direction.y.abs() > 0.999 {
                Vec3::Z
            } else {
                Vec3::Y
            },
        );
        Self {
            window: [min.x, min.y, chunks[0], chunks[1]],
            window_cells: [
                chunks[0] as f32 * size,
                chunks[1] as f32 * size,
                view.elevation_scale.value(),
                request.seconds,
            ],
            cursor,
            sun_direction: [sun_x, sun_y, sun_z, lighting.sun_illuminance_lux],
            sun_color: [sun[0], sun[1], sun[2], lighting.ambient_brightness],
            sky_color: [sky[0], sky[1], sky[2], lighting.exposure],
            ground_color: [
                ground[0],
                ground[1],
                ground[2],
                lighting.terrain_shadow_strength,
            ],
            horizon_color: [
                horizon[0],
                horizon[1],
                horizon[2],
                lighting.ambient_occlusion_strength,
            ],
            sun_glow: [glow[0], glow[1], glow[2], lighting.sun_glow_color[3]],
            fog: [
                lighting.fog_start_fraction,
                lighting.fog_end_fraction,
                lighting.sun_glow_exponent,
                half_extent,
            ],
            eye: [eye.x, eye.y, eye.z, following],
            angles: [yaw, pitch, distance, request.fov_y],
            projection: [aspect, request.near, 0.0, 0.0],
            follow: [
                follow_slot,
                generation,
                view.body_extent,
                request.overlay.bits(),
            ],
            offsets: [view.plan.bodies.config, view.plan.activity.records, 0, 0],
            shadow_view_proj: shadow.to_cols_array_2d(),
            shadow: [
                1.0 / SHADOW_MAP_SIZE as f32,
                SHADOW_DEPTH_BIAS,
                if view.body_extent > 0 { 1.0 } else { 0.0 },
                0.0,
            ],
        }
    }

    /// The linear horizon colour, which the frame clears to.
    pub(crate) fn horizon(&self) -> [f32; 3] {
        [
            self.horizon_color[0],
            self.horizon_color[1],
            self.horizon_color[2],
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_uniform_matches_the_wgsl_struct_size() {
        // Fifteen vectors, the shadow matrix and one more vector.
        assert_eq!(std::mem::size_of::<FrameUniform>(), 16 * 16 + 64);
    }

    #[test]
    fn forward_follows_the_compass_convention() {
        let north = forward(0.0, 0.0);
        assert!((north - Vec3::NEG_Z).length() < 1e-6);
        // Turning left (positive yaw) faces west (-x).
        let west = forward(std::f32::consts::FRAC_PI_2, 0.0);
        assert!((west - Vec3::NEG_X).length() < 1e-6);
        let up = forward(0.0, std::f32::consts::FRAC_PI_2);
        assert!((up - Vec3::Y).length() < 1e-6);
        // The centre of the screen is straight ahead.
        let ray = view_ray(0.3, -0.2, 1.4, 1.7, Vec2::ZERO);
        assert!((ray - forward(0.3, -0.2)).length() < 1e-6);
    }

    #[test]
    fn window_local_positions_are_relative_to_the_minimum_chunk() {
        let window = ResidentWindow::new(world_model::spatial::ChunkKey::new(-3, 5, 0), 2, 1)
            .expect("window");
        // Minimum chunk (-5, 4): first cell (-320, 256), render z -256.
        let local = window_local(window, DVec3::new(-320.0, 7.0, -256.0));
        assert_eq!(local, Vec3::new(0.0, 7.0, 0.0));
        let local = window_local(window, DVec3::new(-300.5, 1.0, -300.0));
        assert_eq!(local, Vec3::new(19.5, 1.0, -44.0));
    }
}
