use bevy::input::mouse::{MouseMotion, MouseScrollUnit, MouseWheel};
use bevy::prelude::MessageReader;
use bevy::prelude::*;
use monarch_engine::prelude::*;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const PAN_SPEED: f32 = 250.0;
const ORBIT_SENSITIVITY: f32 = 0.005; // radians per pixel
const ZOOM_SENSITIVITY: f32 = 15.0; // world-units per scroll line
const ZOOM_SENSITIVITY_PIXELS: f32 = 0.5; // world-units per pixel (trackpad)
const MIN_DIST: f32 = 20.0;
const MAX_DIST: f32 = 800.0;
const MIN_PITCH: f32 = 0.1745; // ~10 degrees
const MAX_PITCH: f32 = 1.5533; // ~89 degrees

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// Attached to the camera entity. Owns all orbital state; `Transform` is a
/// pure derived output — never write to it directly from other systems.
#[derive(Component)]
pub struct FocalPoint {
    /// World-space XZ anchor the camera orbits around (Y is always 0).
    pub anchor: Vec3,
    /// Horizontal rotation around the world Y axis (radians, wraps freely).
    pub yaw: f32,
    /// Vertical elevation above the XZ plane (radians, clamped).
    pub pitch: f32,
    /// Distance from anchor to camera eye (world units, clamped).
    pub distance: f32,
}

impl Default for FocalPoint {
    fn default() -> Self {
        // Matches the initial Transform::from_xyz(0.0, 150.0, 150.0).looking_at(Vec3::ZERO)
        // offset = (0, 150, 150), distance = 150√2, yaw = 0, pitch = 45°
        let distance = (150.0_f32 * 150.0 + 150.0 * 150.0).sqrt();
        let pitch = (150.0_f32 / distance).asin();
        Self {
            anchor: Vec3::ZERO,
            yaw: 0.0,
            pitch,
            distance,
        }
    }
}

impl FocalPoint {
    /// Computes the camera eye position from spherical coordinates.
    fn eye(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        self.anchor
            + Vec3::new(
                self.distance * cos_pitch * sin_yaw,
                self.distance * sin_pitch,
                self.distance * cos_pitch * cos_yaw,
            )
    }

    /// Flat (XZ-projected) forward direction, derived from yaw only.
    fn flat_forward(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        Vec3::new(-sin_yaw, 0.0, -cos_yaw)
    }

    /// Flat (XZ-projected) right direction, 90° CW from flat_forward.
    fn flat_right(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        Vec3::new(cos_yaw, 0.0, -sin_yaw)
    }
}

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

pub fn setup_focal_point(mut commands: Commands) {
    let focal = FocalPoint::default();
    let eye = focal.eye();
    commands.spawn((
        focal,
        Camera3d::default(),
        Transform::from_translation(eye).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// Runs after `setup_focal_point` to reposition the camera anchor at the
/// centre of the focal point.
pub fn center_camera_on_grid(mut query: Query<&mut FocalPoint>) {
    let Ok(mut focal) = query.single_mut() else {
        return;
    };

    let half_chunk = CHUNK_SIZE as f32 / 2.0;
    focal.anchor = Vec3::new(half_chunk, 0.0, -half_chunk);
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// WASD translates the anchor across the XZ plane. Camera pose is unchanged.
pub fn player_movement(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut query: Query<&mut FocalPoint>,
    time: Res<Time>,
) {
    let Ok(mut focal) = query.single_mut() else {
        return;
    };

    let mut delta = Vec3::ZERO;

    if keyboard.pressed(KeyCode::KeyW) {
        delta += focal.flat_forward();
    }
    if keyboard.pressed(KeyCode::KeyS) {
        delta -= focal.flat_forward();
    }
    if keyboard.pressed(KeyCode::KeyA) {
        delta -= focal.flat_right();
    }
    if keyboard.pressed(KeyCode::KeyD) {
        delta += focal.flat_right();
    }

    if delta.length_squared() > 0.0 {
        focal.anchor += delta.normalize() * PAN_SPEED * time.delta_secs();
        focal.anchor.y = 0.0; // keep anchor pinned to ground plane
    }
}

/// Right-mouse-button drag: orbit (yaw + pitch) around the anchor.
pub fn orbit_camera(
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mut motion: MessageReader<MouseMotion>,
    mut query: Query<&mut FocalPoint>,
) {
    if !mouse_buttons.pressed(MouseButton::Right) {
        // Drain events so they don't accumulate while RMB is released.
        for _ in motion.read() {}
        return;
    }

    let Ok(mut focal) = query.single_mut() else {
        for _ in motion.read() {}
        return;
    };

    for ev in motion.read() {
        // Horizontal drag rotates around Y; vertical drag changes elevation.
        focal.yaw -= ev.delta.x * ORBIT_SENSITIVITY;
        focal.pitch += ev.delta.y * ORBIT_SENSITIVITY;
        focal.pitch = focal.pitch.clamp(MIN_PITCH, MAX_PITCH);
    }
}

/// Scroll wheel: dolly (zoom) along the camera's radial axis.
pub fn zoom_camera(mut scroll: MessageReader<MouseWheel>, mut query: Query<&mut FocalPoint>) {
    let Ok(mut focal) = query.single_mut() else {
        for _ in scroll.read() {}
        return;
    };

    for ev in scroll.read() {
        let delta = match ev.unit {
            MouseScrollUnit::Line => ev.y * ZOOM_SENSITIVITY,
            MouseScrollUnit::Pixel => ev.y * ZOOM_SENSITIVITY_PIXELS,
        };
        // Positive scroll (toward user) zooms in (reduces distance).
        focal.distance -= delta;
        focal.distance = focal.distance.clamp(MIN_DIST, MAX_DIST);
    }
}

/// Derives the camera `Transform` from `FocalPoint` state. Must run after all
/// systems that mutate `FocalPoint` so the camera is always one frame fresh.
pub fn apply_camera_transform(mut query: Query<(&FocalPoint, &mut Transform)>) {
    let Ok((focal, mut transform)) = query.single_mut() else {
        return;
    };

    let eye = focal.eye();
    let forward = (focal.anchor - eye).normalize();

    transform.translation = eye;
    transform.rotation = Transform::from_translation(eye)
        .looking_to(forward, Vec3::Y)
        .rotation;
}

// ---------------------------------------------------------------------------
// Resize input
// ---------------------------------------------------------------------------

pub fn handle_resize_input(
    keyboard: Res<ButtonInput<KeyCode>>,
    manager: Res<ChunkManager>,
    mut writer: MessageWriter<ResizeSimulationEvent>,
) {
    let mut new_radius_x = manager.active_radius_x;
    let mut new_radius_y = manager.active_radius_y;
    let mut changed = false;

    if keyboard.just_pressed(KeyCode::ArrowLeft) {
        new_radius_x += 4;
        changed = true;
    }

    if keyboard.just_pressed(KeyCode::ArrowRight) && new_radius_x > 0 {
        new_radius_x -= 4;
        changed = true;
    }

    if keyboard.just_pressed(KeyCode::ArrowUp) {
        new_radius_y += 4;
        changed = true;
    }

    if keyboard.just_pressed(KeyCode::ArrowDown) && new_radius_y > 0 {
        new_radius_y -= 4;
        changed = true;
    }

    if changed {
        info!(
            "Resizing Simulation: radius_x {} radius_y {}",
            new_radius_x, new_radius_y
        );

        writer.write(ResizeSimulationEvent {
            new_active_radius_x: new_radius_x,
            new_active_radius_y: new_radius_y,
        });
    }
}

// ---------------------------------------------------------------------------
// Engine sync
// ---------------------------------------------------------------------------

pub fn sync_world_focus(target: Single<&FocalPoint>, mut focus: ResMut<WorldFocus>) {
    // Map the Camera's 3D XZ plane to the Engine's 2D XY grid
    // Bevy's -Z direction is Engine's +Y direction (North)
    focus.position = bevy::math::DVec3::new(target.anchor.x as f64, -target.anchor.z as f64, 0.0);
}
