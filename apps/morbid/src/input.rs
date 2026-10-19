//! Keyboard and mouse input as the client uses it: the window's events translated once into
//! [`InputEvent`]s, held state, and what changed this frame. Scripted runs (evidence drivers)
//! inject the same events, so they exercise the same handling as a person's keys and buttons.
//!
//! Keys are physical positions (WASD stays WASD on any layout). Losing focus releases
//! everything held, so no key sticks while another window has the keyboard.

use std::collections::HashSet;

use winit::{
    event::{DeviceEvent, ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    keyboard::{KeyCode, PhysicalKey},
};

/// The keys the client binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    W,
    A,
    S,
    D,
    Space,
    ControlLeft,
    ShiftLeft,
    /// Possess or release the nearest sphere.
    F,
    /// Attach or detach the residency focus.
    G,
    /// Toggle noclip flight.
    N,
    /// Toggle the overlay (also F12).
    Backquote,
    F12,
    /// Toggle the debug overlay of activity and contacts.
    F3,
    /// Save now.
    F5,
    Escape,
    /// Tool selection: 1-9 and 0, in the toolbar's order.
    Digit(u8),
    /// Brush radius down / up.
    BracketLeft,
    BracketRight,
    /// Brush strength down / up.
    Minus,
    Equal,
}

/// The mouse buttons the client binds: left uses the tool, right looks around, back attracts,
/// forward lifts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Button {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputEvent {
    Key {
        key: Key,
        pressed: bool,
    },
    Button {
        button: Button,
        pressed: bool,
    },
    /// The cursor's position in physical pixels from the window's top left.
    Cursor {
        x: f32,
        y: f32,
    },
    CursorLeft,
    /// Wheel movement in lines (up positive).
    Wheel {
        lines: f32,
    },
    /// Raw pointer motion in device units, for looking around.
    Motion {
        dx: f32,
        dy: f32,
    },
    /// The window lost the keyboard: everything held is released.
    FocusLost,
}

fn key(code: KeyCode) -> Option<Key> {
    Some(match code {
        KeyCode::KeyW => Key::W,
        KeyCode::KeyA => Key::A,
        KeyCode::KeyS => Key::S,
        KeyCode::KeyD => Key::D,
        KeyCode::Space => Key::Space,
        KeyCode::ControlLeft => Key::ControlLeft,
        KeyCode::ShiftLeft => Key::ShiftLeft,
        KeyCode::KeyF => Key::F,
        KeyCode::KeyG => Key::G,
        KeyCode::KeyN => Key::N,
        KeyCode::Backquote => Key::Backquote,
        KeyCode::F12 => Key::F12,
        KeyCode::F3 => Key::F3,
        KeyCode::F5 => Key::F5,
        KeyCode::Escape => Key::Escape,
        KeyCode::Digit1 => Key::Digit(1),
        KeyCode::Digit2 => Key::Digit(2),
        KeyCode::Digit3 => Key::Digit(3),
        KeyCode::Digit4 => Key::Digit(4),
        KeyCode::Digit5 => Key::Digit(5),
        KeyCode::Digit6 => Key::Digit(6),
        KeyCode::Digit7 => Key::Digit(7),
        KeyCode::Digit8 => Key::Digit(8),
        KeyCode::Digit9 => Key::Digit(9),
        KeyCode::Digit0 => Key::Digit(0),
        KeyCode::BracketLeft => Key::BracketLeft,
        KeyCode::BracketRight => Key::BracketRight,
        KeyCode::Minus => Key::Minus,
        KeyCode::Equal => Key::Equal,
        _ => return None,
    })
}

/// The client's reading of a window event, if it binds it.
pub fn from_window_event(event: &WindowEvent) -> Option<InputEvent> {
    match event {
        WindowEvent::KeyboardInput { event, .. } => {
            let PhysicalKey::Code(code) = event.physical_key else {
                return None;
            };
            Some(InputEvent::Key {
                key: key(code)?,
                pressed: event.state == ElementState::Pressed,
            })
        }
        WindowEvent::MouseInput { state, button, .. } => {
            let button = match button {
                MouseButton::Left => Button::Left,
                MouseButton::Right => Button::Right,
                MouseButton::Middle => Button::Middle,
                MouseButton::Back | MouseButton::Other(4) => Button::Back,
                MouseButton::Forward | MouseButton::Other(5) => Button::Forward,
                MouseButton::Other(_) => return None,
            };
            Some(InputEvent::Button {
                button,
                pressed: *state == ElementState::Pressed,
            })
        }
        WindowEvent::CursorMoved { position, .. } => Some(InputEvent::Cursor {
            x: position.x as f32,
            y: position.y as f32,
        }),
        WindowEvent::CursorLeft { .. } => Some(InputEvent::CursorLeft),
        WindowEvent::MouseWheel { delta, .. } => Some(InputEvent::Wheel {
            lines: match delta {
                MouseScrollDelta::LineDelta(_, y) => *y,
                // Pixel deltas (touchpads) at the platform's usual 40 pixels per line.
                MouseScrollDelta::PixelDelta(position) => position.y as f32 / 40.0,
            },
        }),
        WindowEvent::Focused(false) => Some(InputEvent::FocusLost),
        _ => None,
    }
}

pub fn from_device_event(event: &DeviceEvent) -> Option<InputEvent> {
    match event {
        DeviceEvent::MouseMotion { delta } => Some(InputEvent::Motion {
            dx: delta.0 as f32,
            dy: delta.1 as f32,
        }),
        _ => None,
    }
}

/// Held keys and buttons, and this frame's changes.
#[derive(Debug, Default)]
pub struct InputState {
    keys: HashSet<Key>,
    pressed_keys: HashSet<Key>,
    buttons: HashSet<Button>,
    pressed_buttons: HashSet<Button>,
    released_buttons: HashSet<Button>,
    pub cursor: Option<[f32; 2]>,
    pub wheel_lines: f32,
    pub motion: [f32; 2],
    /// Events applied, for scripted runs' accounting.
    pub events: u64,
}

impl InputState {
    pub fn apply(&mut self, event: InputEvent) {
        self.events += 1;
        match event {
            InputEvent::Key { key, pressed } => {
                if pressed {
                    if self.keys.insert(key) {
                        self.pressed_keys.insert(key);
                    }
                } else {
                    self.keys.remove(&key);
                }
            }
            InputEvent::Button { button, pressed } => {
                if pressed {
                    if self.buttons.insert(button) {
                        self.pressed_buttons.insert(button);
                    }
                } else if self.buttons.remove(&button) {
                    self.released_buttons.insert(button);
                }
            }
            InputEvent::Cursor { x, y } => self.cursor = Some([x, y]),
            InputEvent::CursorLeft => self.cursor = None,
            InputEvent::Wheel { lines } => self.wheel_lines += lines,
            InputEvent::Motion { dx, dy } => {
                self.motion[0] += dx;
                self.motion[1] += dy;
            }
            InputEvent::FocusLost => {
                self.released_buttons.extend(self.buttons.drain());
                self.keys.clear();
            }
        }
    }

    pub fn held(&self, key: Key) -> bool {
        self.keys.contains(&key)
    }

    pub fn pressed(&self, key: Key) -> bool {
        self.pressed_keys.contains(&key)
    }

    pub fn button(&self, button: Button) -> bool {
        self.buttons.contains(&button)
    }

    pub fn button_pressed(&self, button: Button) -> bool {
        self.pressed_buttons.contains(&button)
    }

    pub fn button_released(&self, button: Button) -> bool {
        self.released_buttons.contains(&button)
    }

    /// The digit key pressed this frame, if any.
    pub fn digit_pressed(&self) -> Option<u8> {
        self.pressed_keys.iter().find_map(|key| match key {
            Key::Digit(digit) => Some(*digit),
            _ => None,
        })
    }

    /// Forgets this frame's changes; held state stays.
    pub fn end_frame(&mut self) {
        self.pressed_keys.clear();
        self.pressed_buttons.clear();
        self.released_buttons.clear();
        self.wheel_lines = 0.0;
        self.motion = [0.0, 0.0];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_last_one_frame_and_focus_loss_releases_everything() {
        let mut input = InputState::default();
        input.apply(InputEvent::Key {
            key: Key::W,
            pressed: true,
        });
        input.apply(InputEvent::Button {
            button: Button::Left,
            pressed: true,
        });
        assert!(input.pressed(Key::W) && input.held(Key::W));
        assert!(input.button_pressed(Button::Left));
        input.end_frame();
        assert!(!input.pressed(Key::W) && input.held(Key::W));
        // A repeated press while held is not a new press.
        input.apply(InputEvent::Key {
            key: Key::W,
            pressed: true,
        });
        assert!(!input.pressed(Key::W));
        input.apply(InputEvent::FocusLost);
        assert!(!input.held(Key::W));
        assert!(!input.button(Button::Left) && input.button_released(Button::Left));
    }
}
