//! Scripted runs of the real client, for evidence: a [`Driver`] sees every frame, may inject
//! input events (handled exactly as the window's own), open and close worlds, resize or minimize
//! the window, and read what the open world reports. The interactive client runs without one.

use std::path::PathBuf;

use crate::{
    client::Running,
    input::{InputEvent, InputState},
    ui::{Hud, StartScreen},
};

/// What a driver asks of the client before a frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    Continue,
    Open {
        name: String,
        path: PathBuf,
    },
    /// Close the open world in order and return to the start screen.
    Close,
    /// Close any open world in order and end the run.
    Quit,
    /// Ask the window system for a new inner size.
    Resize([u32; 2]),
    /// A start-screen action, handled exactly as the screen's own control would be.
    Start(crate::ui::StartAction),
    /// Deliver a window size to the client's resize handling as the window system would (a
    /// zero size is what a minimized window reports on platforms that report it).
    WindowSize([u32; 2]),
    Minimize(bool),
}

/// Which phase the client is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseKind {
    Start,
    Running,
    Closing,
    Lost,
}

/// One frame, as a driver sees it before the client runs it.
pub struct DriverFrame<'a> {
    pub frame: u64,
    pub phase: PhaseKind,
    pub running: Option<&'a mut Running>,
    /// Events to apply before the frame, as if the window had delivered them.
    pub events: &'a mut Vec<InputEvent>,
    pub start: &'a mut StartScreen,
    pub hud: &'a mut Hud,
}

pub trait Driver {
    /// Whether sessions this driver opens hand out a fault controller (conformance only).
    fn fault_injection(&self) -> bool {
        false
    }

    fn before_frame(&mut self, frame: DriverFrame<'_>) -> Control;

    /// After the frame; `true` ends the run (an open world is closed in order first).
    fn after_frame(&mut self, running: Option<&mut Running>, input: &InputState) -> bool;

    /// Whether to recover a lost world without waiting for the person.
    fn recover_after_loss(&mut self) -> bool {
        false
    }
}
