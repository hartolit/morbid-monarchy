//! The observer: the person's viewpoint, flying or walking, and the camera that either rides
//! with it or follows a possessed sphere. Ported from the baseline's `KinematicObserver` with
//! the same speeds, look sensitivity and pitch limit.
//!
//! The observer is not a physical body and never touches the live world. Walking keeps it a
//! fixed height above the ground under it, read from a column query's answer (an observation of
//! a completed tick, at most a few ticks old), never from a CPU copy of the terrain. While a
//! sphere is possessed, the camera is placed on the device from the sphere's live pose
//! (`world_render::CameraRig::Follow`); the observer itself tracks the sphere's latest sample so
//! the residency focus and the cursor ray follow it.

use world_model::{
    body::BodyHandle,
    glam::{DVec3, Vec3},
    spatial::{CHUNK_SIZE, ChunkKey},
    units::GRAVITY_Y,
};
use world_render::{CameraRig, frame::forward};

use crate::input::{InputState, Key};

pub const BASE_SPEED: f32 = 50.0;
pub const SPRINT_SPEED: f32 = 200.0;
pub const JUMP_IMPULSE: f32 = 20.0;
pub const LOOK_SENSITIVITY: f32 = 0.002;
pub const MAX_PITCH: f32 = 1.54;
/// A walking observer's eye stays this far above the ground.
pub const WALK_HEIGHT: f32 = 10.0;
pub const START_HEIGHT: f64 = 150.0;
pub const DEFAULT_TETHER: f32 = 15.0;
pub const TETHER_RANGE: std::ops::RangeInclusive<f32> = 5.0..=100.0;
pub const ZOOM_PER_LINE: f32 = 3.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Observer {
    /// Absolute render space: x is the cell x, y is up, z is minus the cell y.
    pub position: DVec3,
    pub velocity: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub noclip: bool,
    /// Whether the resident window follows the observer.
    pub attached: bool,
    pub tether: f32,
    /// The possessed sphere, which the camera follows and the movement keys drive.
    pub possessed: Option<BodyHandle>,
}

impl Observer {
    /// An observer above the centre of `chunk`, flying, attached, looking north.
    pub fn above(chunk: ChunkKey) -> Self {
        let origin = chunk.origin_cell();
        let half = CHUNK_SIZE as f64 / 2.0;
        Self {
            position: DVec3::new(
                f64::from(origin.x) + half,
                START_HEIGHT,
                -(f64::from(origin.y) + half),
            ),
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            noclip: true,
            attached: true,
            tether: DEFAULT_TETHER,
            possessed: None,
        }
    }

    /// The chunk under the observer, which the resident window centres on while attached.
    pub fn chunk(&self) -> ChunkKey {
        ChunkKey::new(
            (self.position.x / CHUNK_SIZE as f64).floor() as i32,
            (-self.position.z / CHUNK_SIZE as f64).floor() as i32,
            0,
        )
    }

    /// The cell under the observer.
    pub fn cell(&self) -> world_model::glam::IVec2 {
        world_model::glam::IVec2::new(
            self.position.x.floor() as i32,
            (-self.position.z).floor() as i32,
        )
    }

    /// Mouse look while the right button is held, toggles, and the wheel's zoom.
    pub fn steer(&mut self, input: &InputState, looking: bool, keyboard: bool) {
        if keyboard {
            if input.pressed(Key::N) {
                self.noclip = !self.noclip;
            }
            if input.pressed(Key::G) {
                self.attached = !self.attached;
            }
        }
        if looking {
            self.yaw -= input.motion[0] * LOOK_SENSITIVITY;
            self.pitch =
                (self.pitch - input.motion[1] * LOOK_SENSITIVITY).clamp(-MAX_PITCH, MAX_PITCH);
        }
        self.tether = (self.tether - input.wheel_lines * ZOOM_PER_LINE)
            .clamp(*TETHER_RANGE.start(), *TETHER_RANGE.end());
    }

    /// Moves a free observer for `dt` seconds: flying along the view, or walking on `floor` (the
    /// ground height under it, when known).
    pub fn travel(&mut self, input: &InputState, keyboard: bool, dt: f32, floor: Option<f32>) {
        let held = |key| keyboard && input.held(key);
        let speed = if held(Key::ShiftLeft) {
            SPRINT_SPEED
        } else {
            BASE_SPEED
        };
        let heading = forward(self.yaw, 0.0);
        let right = Vec3::new(-heading.z, 0.0, heading.x);
        let mut wish = Vec3::ZERO;
        if held(Key::W) {
            wish += heading;
        }
        if held(Key::S) {
            wish -= heading;
        }
        if held(Key::A) {
            wish -= right;
        }
        if held(Key::D) {
            wish += right;
        }
        if self.noclip {
            if held(Key::Space) {
                wish += Vec3::Y;
            }
            if held(Key::ControlLeft) {
                wish -= Vec3::Y;
            }
            self.velocity = wish.normalize_or_zero() * speed;
            self.position += (self.velocity * dt).as_dvec3();
            return;
        }
        let ground = wish.normalize_or_zero();
        self.velocity.x = ground.x * speed;
        self.velocity.z = ground.z * speed;
        self.velocity.y += GRAVITY_Y * dt;
        if held(Key::Space) && self.velocity.y <= 0.0 {
            self.velocity.y = JUMP_IMPULSE;
        }
        self.position += (self.velocity * dt).as_dvec3();
        if let Some(floor) = floor {
            let contact = f64::from(floor + WALK_HEIGHT);
            if self.position.y < contact {
                self.position.y = contact;
                self.velocity.y = self.velocity.y.max(0.0);
            }
        }
    }

    /// Keeps the observer at the tether behind a possessed sphere's latest observed position.
    pub fn track(&mut self, body: DVec3) {
        self.position = body - (forward(self.yaw, self.pitch) * self.tether).as_dvec3();
        self.velocity = Vec3::ZERO;
    }

    pub fn rig(&self) -> CameraRig {
        match self.possessed {
            Some(body) => CameraRig::Follow {
                body,
                fallback: self.position,
                yaw: self.yaw,
                pitch: self.pitch,
                distance: self.tether,
            },
            None => CameraRig::Free {
                position: self.position,
                yaw: self.yaw,
                pitch: self.pitch,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::InputEvent;

    #[test]
    fn flying_follows_the_view_and_walking_stays_above_the_floor() {
        let mut observer = Observer::above(ChunkKey::new(-1, 2, 0));
        assert_eq!(observer.chunk(), ChunkKey::new(-1, 2, 0));
        let mut input = InputState::default();
        input.apply(InputEvent::Key {
            key: Key::W,
            pressed: true,
        });
        let start = observer.position;
        observer.travel(&input, true, 0.5, None);
        // North is -z.
        assert!((observer.position.z - (start.z - 25.0)).abs() < 1e-3);
        observer.noclip = false;
        observer.position.y = 5.0;
        observer.travel(&input, true, 0.1, Some(20.0));
        assert_eq!(observer.position.y, 30.0);
        // Keys are ignored while the overlay has the keyboard.
        let before = observer.position;
        observer.noclip = true;
        observer.travel(&input, false, 0.5, None);
        assert_eq!(observer.position, before);
    }

    #[test]
    fn the_wheel_zooms_within_the_tether_range() {
        let mut observer = Observer::above(ChunkKey::new(0, 0, 0));
        let mut input = InputState::default();
        input.apply(InputEvent::Wheel { lines: 100.0 });
        observer.steer(&input, false, true);
        assert_eq!(observer.tether, 5.0);
        input.end_frame();
        input.apply(InputEvent::Wheel { lines: -100.0 });
        observer.steer(&input, false, true);
        assert_eq!(observer.tether, 100.0);
    }
}
