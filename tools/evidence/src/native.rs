//! Scripted runs of the native client (`morbid-app`) in its real window: the same composition,
//! event handling, renderer and overlay a person uses, driven through `morbid_app::driver`.
//! The `native-client` binary runs them; each writes a JSON result and, where asked, PNG frames
//! captured through the client. [`bench`](mod@bench) is the measured run of a frozen scenario.

pub mod bench;

use std::{
    fs::File,
    io::BufWriter,
    path::{Path, PathBuf},
};

use morbid_app::{
    client::{Capture, Running},
    driver::{Control, Driver, DriverFrame},
    input::InputState,
};
use world_model::glam::DVec3;

/// Writes a capture as an 8-bit RGBA PNG.
pub fn write_png(path: &Path, capture: &Capture) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let file = File::create(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), capture.size[0], capture.size[1]);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
    writer
        .write_image_data(&capture.rgba)
        .map_err(|error| error.to_string())
}

/// Where the observer stands for a capture: absolute render position, yaw and pitch (radians).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub position: DVec3,
    pub yaw: f32,
    pub pitch: f32,
}

/// Opens a world, holds a pose, waits for the residency to settle and captures frames.
pub struct CaptureDriver {
    pub name: String,
    pub world: PathBuf,
    pub pose: Option<Pose>,
    /// Frames to capture after the world settled, and where to write them.
    pub captures: Vec<(u64, PathBuf)>,
    pub settled_at: Option<u64>,
    pub frame: u64,
    pub written: Vec<PathBuf>,
    pub failures: Vec<String>,
    opened: bool,
}

impl CaptureDriver {
    pub fn new(
        name: String,
        world: PathBuf,
        pose: Option<Pose>,
        captures: Vec<(u64, PathBuf)>,
    ) -> Self {
        Self {
            name,
            world,
            pose,
            captures,
            settled_at: None,
            frame: 0,
            written: Vec::new(),
            failures: Vec::new(),
            opened: false,
        }
    }
}

impl Driver for CaptureDriver {
    fn before_frame(&mut self, frame: DriverFrame<'_>) -> Control {
        self.frame = frame.frame;
        let Some(running) = frame.running else {
            if self.opened {
                return Control::Quit;
            }
            self.opened = true;
            return Control::Open {
                name: self.name.clone(),
                path: self.world.clone(),
            };
        };
        if let Some(pose) = self.pose {
            let observer = &mut running.play_mut().observer;
            observer.position = pose.position;
            observer.yaw = pose.yaw;
            observer.pitch = pose.pitch;
        }
        if self.settled_at.is_none() && running.session().residency().settled() && frame.frame > 30
        {
            self.settled_at = Some(frame.frame);
        }
        if let Some(settled) = self.settled_at
            && self
                .captures
                .iter()
                .any(|(after, _)| settled + after == frame.frame)
        {
            running.request_capture();
        }
        Control::Continue
    }

    fn after_frame(&mut self, running: Option<&mut Running>, _input: &InputState) -> bool {
        let Some(running) = running else {
            return false;
        };
        if let Some(capture) = running.take_capture() {
            let settled = self.settled_at.unwrap_or(0);
            let path = self
                .captures
                .iter()
                .find(|(after, _)| settled + after == self.frame)
                .map(|(_, path)| path.clone());
            match (capture, path) {
                (Ok(capture), Some(path)) => match write_png(&path, &capture) {
                    Ok(()) => self.written.push(path),
                    Err(error) => self.failures.push(error),
                },
                (Err(error), _) => self.failures.push(error),
                (Ok(_), None) => {}
            }
        }
        let last = self
            .captures
            .iter()
            .map(|(after, _)| *after)
            .max()
            .unwrap_or(0);
        self.settled_at
            .is_some_and(|settled| self.frame > settled + last)
            || self.frame > 20_000
    }
}

use std::{cell::RefCell, rc::Rc};

use morbid_app::input::{Button, InputEvent, Key};
use serde::Serialize;

/// What a scripted step does on its frame.
pub enum Act {
    Input(InputEvent),
    Pose(Pose),
    /// Capture the frame as `<out>/<name>.png`.
    Capture(String),
    /// A named check of what the client and the world report now, with the values remembered
    /// earlier.
    Check(&'static str, Box<CheckFn>),
    /// Remember a value for later checks (a figure before an action).
    Remember(&'static str, Box<dyn Fn(&mut Running) -> f64>),
    Resize([u32; 2]),
    /// The window size the client's resize handling receives (see `Control::WindowSize`).
    WindowSize([u32; 2]),
    Minimize(bool),
    /// Inject a device loss through the session's fault controller.
    LoseDevice,
    Close,
}

/// Values remembered by [`Act::Remember`].
pub type Memory = std::collections::BTreeMap<String, f64>;
pub type CheckFn = dyn Fn(&mut Running, &Memory) -> Result<String, String>;

/// One cue of a script: its frame, counted from the frame the world first settled.
pub struct Cue {
    pub at: u64,
    pub act: Act,
}

pub fn at(at: u64, act: Act) -> Cue {
    Cue { at, act }
}

pub fn key(at_frame: u64, key: Key, pressed: bool) -> Cue {
    at(at_frame, Act::Input(InputEvent::Key { key, pressed }))
}

pub fn button(at_frame: u64, button: Button, pressed: bool) -> Cue {
    at(at_frame, Act::Input(InputEvent::Button { button, pressed }))
}

pub fn check(
    at_frame: u64,
    name: &'static str,
    test: impl Fn(&mut Running) -> Result<String, String> + 'static,
) -> Cue {
    at(
        at_frame,
        Act::Check(name, Box::new(move |running, _memory| test(running))),
    )
}

/// A check's outcome in the report.
#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub name: String,
    pub frame: u64,
    pub tick: Option<u64>,
    pub passed: bool,
    pub detail: String,
}

/// What a scripted run reports.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ScriptReport {
    pub script: String,
    pub checks: Vec<CheckResult>,
    pub captures: Vec<String>,
    pub failures: Vec<String>,
    pub frames: u64,
    pub settled_at: Option<u64>,
    pub remembered: std::collections::BTreeMap<String, f64>,
    pub opened_worlds: u32,
    /// The start screen's messages as the run ended (closes, losses, refusals).
    pub messages: Vec<String>,
}

impl ScriptReport {
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
            && !self.checks.is_empty()
            && self.checks.iter().all(|check| check.passed)
    }
}

/// Runs cues against the real client: opens the world, waits until its window settled, then
/// plays the cues frame by frame and records checks and captures.
pub struct ScriptDriver {
    name: String,
    world: PathBuf,
    out: PathBuf,
    cues: Vec<Cue>,
    pub report: Rc<RefCell<ScriptReport>>,
    settled: Option<u64>,
    opened: bool,
    pending_capture: Option<String>,
    fault_injection: bool,
    recover: bool,
    /// The run ends this many frames after the last cue.
    tail: u64,
    /// Held until the world settles, so the window settles where the script starts.
    start_pose: Option<Pose>,
    /// The pointer position the script set last, applied again before every frame: the window
    /// also delivers the person's own pointer events (a mouse passing over it, or leaving it),
    /// which must not move the script's cursor.
    cursor: Option<InputEvent>,
}

impl ScriptDriver {
    pub fn new(script: &str, name: String, world: PathBuf, out: PathBuf, cues: Vec<Cue>) -> Self {
        Self {
            name,
            world,
            out,
            cues,
            report: Rc::new(RefCell::new(ScriptReport {
                script: script.to_owned(),
                ..ScriptReport::default()
            })),
            settled: None,
            opened: false,
            pending_capture: None,
            cursor: None,
            fault_injection: false,
            recover: false,
            tail: 30,
            start_pose: None,
        }
    }

    /// Holds the observer at `pose` until the resident window settled around it.
    pub fn starting_at(mut self, pose: Pose) -> Self {
        self.start_pose = Some(pose);
        self
    }

    /// Opens sessions with a fault controller (for the loss and recovery script).
    pub fn with_faults(mut self) -> Self {
        self.fault_injection = true;
        self.recover = true;
        self
    }

    fn last_cue(&self) -> u64 {
        self.cues.iter().map(|cue| cue.at).max().unwrap_or(0)
    }
}

impl Driver for ScriptDriver {
    fn fault_injection(&self) -> bool {
        self.fault_injection
    }

    fn recover_after_loss(&mut self) -> bool {
        self.recover
    }

    fn before_frame(&mut self, frame: DriverFrame<'_>) -> Control {
        let mut report = self.report.borrow_mut();
        report.frames = frame.frame;
        if report.messages != frame.start.messages {
            report.messages = frame.start.messages.clone();
        }
        let Some(running) = frame.running else {
            if !self.opened {
                self.opened = true;
                report.opened_worlds += 1;
                return Control::Open {
                    name: self.name.clone(),
                    path: self.world.clone(),
                };
            }
            // A world closed or lost: the cues that need one wait for the recovery.
            return Control::Continue;
        };
        if self.settled.is_none() {
            if let Some(pose) = self.start_pose {
                let observer = &mut running.play_mut().observer;
                observer.position = pose.position;
                observer.yaw = pose.yaw;
                observer.pitch = pose.pitch;
            }
            let residency = running.session().residency();
            let complete = residency.summary().resident == residency.window().chunk_count();
            if residency.settled() && complete && frame.frame > 20 {
                self.settled = Some(frame.frame);
                report.settled_at = Some(frame.frame);
            } else {
                return Control::Continue;
            }
        }
        let offset = frame.frame - self.settled.unwrap_or(frame.frame);
        let tick = running
            .session()
            .runtime()
            .last_submitted_tick()
            .map(|tick| tick.value());
        let mut control = Control::Continue;
        for cue in self.cues.iter().filter(|cue| cue.at == offset) {
            match &cue.act {
                Act::Input(event) => {
                    if matches!(event, InputEvent::Cursor { .. } | InputEvent::CursorLeft) {
                        self.cursor = Some(*event);
                    }
                    frame.events.push(*event);
                }
                Act::Pose(pose) => {
                    let observer = &mut running.play_mut().observer;
                    observer.position = pose.position;
                    observer.yaw = pose.yaw;
                    observer.pitch = pose.pitch;
                }
                Act::Capture(name) => {
                    running.request_capture();
                    self.pending_capture = Some(name.clone());
                }
                Act::Check(name, test) => {
                    let (passed, detail) = match test(running, &report.remembered) {
                        Ok(detail) => (true, detail),
                        Err(detail) => (false, detail),
                    };
                    report.checks.push(CheckResult {
                        name: (*name).to_owned(),
                        frame: frame.frame,
                        tick,
                        passed,
                        detail,
                    });
                }
                Act::Remember(name, value) => {
                    let value = value(running);
                    report.remembered.insert((*name).to_owned(), value);
                }
                Act::Resize(size) => control = Control::Resize(*size),
                Act::WindowSize(size) => control = Control::WindowSize(*size),
                Act::Minimize(minimized) => control = Control::Minimize(*minimized),
                Act::LoseDevice => match &running.live().fault_controller {
                    Some(faults) => faults.lose_device(),
                    None => report
                        .failures
                        .push("no fault controller: the session was opened without one".to_owned()),
                },
                Act::Close => control = Control::Close,
            }
        }
        if let Some(cursor) = self.cursor {
            frame.events.push(cursor);
        }
        control
    }

    fn after_frame(&mut self, running: Option<&mut Running>, _input: &InputState) -> bool {
        let mut report = self.report.borrow_mut();
        if let Some(running) = running
            && let Some(capture) = running.take_capture()
        {
            let name = self
                .pending_capture
                .take()
                .unwrap_or_else(|| "capture".to_owned());
            match capture {
                Ok(capture) => {
                    let path = self.out.join(format!("{name}.png"));
                    match write_png(&path, &capture) {
                        Ok(()) => report.captures.push(path.display().to_string()),
                        Err(error) => report.failures.push(error),
                    }
                }
                Err(error) => report.failures.push(format!("capture {name}: {error}")),
            }
        }
        let done = self
            .settled
            .is_some_and(|settled| report.frames > settled + self.last_cue() + self.tail);
        if report.frames > 60_000 {
            report
                .failures
                .push("the script did not finish in 60,000 frames".to_owned());
            return true;
        }
        done
    }
}

use morbid_app::play::{Purpose, Tool};
use world_model::{body::BodyPreset, command::EditTool, material::FluidMat};

/// Builds a script by appending cues at a running frame offset.
pub struct Script {
    pub cues: Vec<Cue>,
    pub t: u64,
}

impl Script {
    pub fn new() -> Self {
        Self {
            cues: Vec::new(),
            t: 0,
        }
    }

    pub fn wait(&mut self, frames: u64) -> &mut Self {
        self.t += frames;
        self
    }

    pub fn act(&mut self, act: Act) -> &mut Self {
        self.cues.push(at(self.t, act));
        self
    }

    pub fn input(&mut self, event: InputEvent) -> &mut Self {
        self.act(Act::Input(event))
    }

    /// A key pressed and released one frame later.
    pub fn tap(&mut self, key: Key) -> &mut Self {
        self.input(InputEvent::Key { key, pressed: true });
        self.t += 1;
        self.input(InputEvent::Key {
            key,
            pressed: false,
        });
        self.t += 1;
        self
    }

    /// A button held for `frames` frames.
    pub fn hold(&mut self, button: Button, frames: u64) -> &mut Self {
        self.input(InputEvent::Button {
            button,
            pressed: true,
        });
        self.t += frames;
        self.input(InputEvent::Button {
            button,
            pressed: false,
        });
        self.t += 1;
        self
    }

    /// A key held for `frames` frames.
    pub fn hold_key(&mut self, key: Key, frames: u64) -> &mut Self {
        self.input(InputEvent::Key { key, pressed: true });
        self.t += frames;
        self.input(InputEvent::Key {
            key,
            pressed: false,
        });
        self.t += 1;
        self
    }

    pub fn check(
        &mut self,
        name: &'static str,
        test: impl Fn(&mut Running) -> Result<String, String> + 'static,
    ) -> &mut Self {
        self.act(Act::Check(
            name,
            Box::new(move |running, _memory| test(running)),
        ))
    }

    pub fn check_with(
        &mut self,
        name: &'static str,
        test: impl Fn(&mut Running, &Memory) -> Result<String, String> + 'static,
    ) -> &mut Self {
        self.act(Act::Check(name, Box::new(test)))
    }

    pub fn remember(
        &mut self,
        name: &'static str,
        value: impl Fn(&mut Running) -> f64 + 'static,
    ) -> &mut Self {
        self.act(Act::Remember(name, Box::new(value)))
    }

    pub fn capture(&mut self, name: &str) -> &mut Self {
        self.act(Act::Capture(name.to_owned()))
    }
}

impl Default for Script {
    fn default() -> Self {
        Self::new()
    }
}

fn digit_for(tool: Tool) -> u8 {
    let index = morbid_app::play::TOOLS
        .iter()
        .position(|candidate| *candidate == tool)
        .unwrap_or(0);
    if index == 10 { 0 } else { index as u8 }
}

fn totals(running: &Running) -> Option<world_model::observation::WorldTotals> {
    running.play().totals.map(|(_, totals)| totals)
}

/// The pose the tool steps aim from: above the sand bank of seed 69's lake shore, looking down
/// at it (the frozen coupled scenario's camera start).
pub const SHORE_POSE: Pose = Pose {
    position: DVec3::new(200.0, 150.0, -230.0),
    yaw: std::f32::consts::FRAC_PI_6,
    // 40 degrees down.
    pitch: -0.698_131_7,
};

/// Every tool and control of the frozen feature inventory, driven through the client's input
/// handling (keys, buttons, the cursor) or the functions its panels call, each followed by a
/// check of the outcome the world reported.
pub fn features(size: [u32; 2]) -> Vec<Cue> {
    let mut s = Script::new();
    let centre = InputEvent::Cursor {
        x: size[0] as f32 / 2.0,
        y: size[1] as f32 / 2.0,
    };
    s.act(Act::Pose(SHORE_POSE)).input(centre);
    s.wait(2)
        .check("world drawn from the resident window", |running| {
            let counters = running.render_counters();
            let resident = running.session().residency().summary().resident;
            if counters.frames > 0 && counters.chunk_instances == 169 && resident == 169 {
                Ok(format!(
                    "{} frames, {} chunk instances, {resident} resident",
                    counters.frames, counters.chunk_instances
                ))
            } else {
                Err(format!("{counters:?}, {resident} resident"))
            }
        });
    // Statistics on: totals come from the GPU every 30 ticks.
    s.check("statistics measure totals on the GPU", |running| {
        running.play_mut().statistics_open = true;
        Ok("statistics opened".to_owned())
    });
    s.wait(150)
        .check("totals arrived", |running| match running.play().totals {
            Some((tick, totals)) => Ok(format!(
                "tick {}: {} cells, liquid {}",
                tick.value(),
                totals.cells,
                totals.fluid_volume()
            )),
            None => Err("no totals".to_owned()),
        });

    // Each material brush: selected by its key, stroked with the left button at the cursor.
    let brushes: [(EditTool, f32, f32); 8] = [
        (EditTool::Water, 0.5, 0.5),
        (EditTool::Sand, 0.42, 0.62),
        (EditTool::Raise, 0.58, 0.62),
        (EditTool::Lower, 0.66, 0.5),
        (EditTool::Lava, 0.3, 0.75),
        (EditTool::Oil, 0.72, 0.75),
        (EditTool::Ice, 0.36, 0.45),
        (EditTool::Fire, 0.2, 0.85),
    ];
    for (tool, fx, fy) in brushes {
        let name: &'static str = Box::leak(format!("{tool:?} brush").into_boxed_str());
        s.input(InputEvent::Cursor {
            x: size[0] as f32 * fx,
            y: size[1] as f32 * fy,
        });
        s.tap(Key::Digit(digit_for(Tool::Brush(tool))));
        s.check(name, move |running| {
            if running.play().tool == Tool::Brush(tool) {
                Ok("selected by its key".to_owned())
            } else {
                Err(format!("tool is {:?}", running.play().tool))
            }
        });
        s.remember("strokes", |running| {
            running.play().applied(Purpose::Stroke) as f64
        });
        s.remember("changed", |running| running.play().changed_cells as f64);
        let before = totals_marker(tool);
        s.remember("material", move |running| {
            totals(running).map_or(-1.0, |t| before(&t))
        });
        s.wait(20).hold(Button::Left, 30).wait(150);
        let after = totals_marker(tool);
        s.check_with(name, move |running, memory| {
            let play = running.play();
            let strokes = play.applied(Purpose::Stroke) as f64 - memory["strokes"];
            let changed = play.changed_cells as f64 - memory["changed"];
            let before = memory["material"];
            let now = totals(running).map_or(-1.0, |t| after(&t));
            let moved = match tool {
                EditTool::Fire => true,
                EditTool::Lower => now < before,
                _ => now > before,
            };
            let text = format!(
                "{strokes} strokes applied, {changed} cells changed; measured total {before} -> {now}"
            );
            let measured = tool == EditTool::Fire || before >= 0.0;
            if strokes > 0.0 && changed > 0.0 && moved && measured {
                Ok(text)
            } else {
                Err(text)
            }
        });
    }
    s.wait(1).capture("brushes");

    // Spheres: both presets at the cursor.
    // On the flat sand east of the lake, clear of the water a wooden sphere would float in, a
    // few cells apart so none lands on another.
    for (index, preset) in [
        BodyPreset::Wood,
        BodyPreset::Metal,
        BodyPreset::Wood,
        BodyPreset::Metal,
    ]
    .into_iter()
    .enumerate()
    {
        s.input(InputEvent::Cursor {
            x: size[0] as f32 * (0.6 + 0.04 * index as f32),
            y: size[1] as f32 * 0.5,
        });
        s.tap(Key::Digit(digit_for(Tool::Sphere(preset))));
        s.wait(5).hold(Button::Left, 2).wait(40);
    }
    s.wait(120)
        .check("spheres spawned at the GPU-resolved cursor", |running| {
            let spawned = running.play().applied(Purpose::Spawn);
            let tracked = running.session().runtime().bodies().summary().tracked;
            let drawn = running.render_counters().body_instances;
            if spawned >= 4 && tracked >= 4 && drawn >= 4 {
                Ok(format!(
                    "{spawned} spawned, {tracked} tracked, {drawn} drawn"
                ))
            } else {
                Err(format!(
                    "{spawned} spawned, {tracked} tracked, {drawn} drawn"
                ))
            }
        });
    s.capture("spheres");

    // Possession, driving, jumping, zoom and release.
    s.tap(Key::Escape);
    s.tap(Key::F)
        .wait(5)
        .check("possess the nearest sphere", |running| {
            match running.play().observer.possessed {
                Some(body) => Ok(format!("slot {} gen {}", body.index(), body.generation())),
                None => Err("nothing possessed".to_owned()),
            }
        });
    s.wait(60)
        .check("the possessed sphere rests on the ground", |running| {
            let body = running
                .play()
                .observer
                .possessed
                .ok_or("nothing possessed")?;
            let sample = running
                .session()
                .runtime()
                .bodies()
                .body(body)
                .and_then(|tracked| tracked.last)
                .ok_or("no sample")?;
            if sample.grounded {
                Ok(format!("grounded at tick {}", sample.tick))
            } else {
                Err(format!("{sample:?}"))
            }
        });
    s.tap(Key::Space)
        .wait(40)
        .check("jump when grounded", |running| {
            match running.play().latest.get(&Purpose::Jump) {
                Some((
                    tick,
                    result @ world_runtime::port::CommandResult::Jumped { jumped: true, .. },
                )) => Ok(format!("tick {}: {result:?}", tick.value())),
                other => Err(format!("{other:?}")),
            }
        });
    s.wait(60)
        .hold_key(Key::S, 120)
        .wait(30)
        .check("drive the possessed sphere", |running| {
            let drives = running.play().applied(Purpose::Drive);
            match possessed_position(running) {
                Some(position) if drives > 0 => Ok(format!(
                    "{drives} drive commands applied; sphere at ({:.1}, {:.1}, {:.1})",
                    position.x, position.y, position.z
                )),
                other => Err(format!("{drives} drives, position {other:?}")),
            }
        });
    s.capture("possessed");
    s.input(InputEvent::Wheel { lines: -3.0 })
        .wait(2)
        .check("zoom the tether", |running| {
            let tether = running.play().observer.tether;
            if (tether - 24.0).abs() < 1e-3 {
                Ok(format!("tether {tether}"))
            } else {
                Err(format!("tether {tether}"))
            }
        });
    s.tap(Key::F)
        .wait(3)
        .check("release the sphere", |running| {
            if running.play().observer.possessed.is_none() {
                Ok("released".to_owned())
            } else {
                Err("still possessed".to_owned())
            }
        });

    // Attract and lift at the cursor (mouse back and forward buttons).
    s.act(Act::Pose(SHORE_POSE)).wait(2);
    s.input(InputEvent::Cursor {
        x: size[0] as f32 * 0.5,
        y: size[1] as f32 * 0.5,
    });
    s.hold(Button::Back, 90).wait(30).check(
        "attract spheres to the cursor",
        |running| match running.play().latest.get(&Purpose::Attract) {
            Some((tick, result)) if running.play().applied(Purpose::Attract) > 0 => Ok(format!(
                "{} applied; tick {}: {result:?}",
                running.play().applied(Purpose::Attract),
                tick.value()
            )),
            other => Err(format!("{other:?}")),
        },
    );
    s.hold(Button::Forward, 90).wait(30).check(
        "lift spheres at the cursor",
        |running| match running.play().latest.get(&Purpose::Lift) {
            Some((tick, result)) if running.play().applied(Purpose::Lift) > 0 => Ok(format!(
                "{} applied; tick {}: {result:?}",
                running.play().applied(Purpose::Lift),
                tick.value()
            )),
            other => Err(format!("{other:?}")),
        },
    );

    // Looking around with the right button.
    s.remember("yaw", |running| f64::from(running.play().observer.yaw));
    s.input(InputEvent::Button {
        button: Button::Right,
        pressed: true,
    })
    .wait(1)
    .input(InputEvent::Motion { dx: 200.0, dy: 0.0 })
    .wait(1)
    .input(InputEvent::Button {
        button: Button::Right,
        pressed: false,
    })
    .wait(2)
    .check("look around with the right button", |running| {
        let yaw = running.play().observer.yaw;
        Ok(format!("yaw {yaw:.3}"))
    });

    // Walking on the GPU-queried floor, then flying again.
    s.act(Act::Pose(Pose {
        position: DVec3::new(SHORE_POSE.position.x, 100.0, SHORE_POSE.position.z),
        ..SHORE_POSE
    }))
    .wait(1);
    s.tap(Key::N)
        .wait(400)
        .check("walk on the queried floor", |running| {
            let play = running.play();
            match play.floor {
                Some((cell, height, tick)) if !play.observer.noclip => {
                    let eye = play.observer.position.y;
                    let expected = f64::from(height + morbid_app::observer::WALK_HEIGHT);
                    if (eye - expected).abs() < 1.0 {
                        Ok(format!(
                            "floor {height:.2} at {cell} (tick {}), eye {eye:.2}",
                            tick.value()
                        ))
                    } else {
                        Err(format!("floor {height:.2}, eye {eye:.2}"))
                    }
                }
                other => Err(format!("noclip {}, floor {other:?}", play.observer.noclip)),
            }
        });
    s.hold_key(Key::W, 60)
        .tap(Key::N)
        .wait(2)
        .check("fly again", |running| {
            if running.play().observer.noclip {
                Ok("noclip".to_owned())
            } else {
                Err("still walking".to_owned())
            }
        });
    s.tap(Key::G)
        .wait(2)
        .check("detach the residency focus", |running| {
            if running.play().observer.attached {
                Err("still attached".to_owned())
            } else {
                Ok("detached".to_owned())
            }
        });
    s.tap(Key::G);

    // Simulation, climate and scale through the panel's own path.
    s.check("simulation toggles and climate", |running| {
        let (session, play) = running.session_and_play();
        play.requested.rules.biology = false;
        play.requested.ambient_c = -10.0;
        play.requested.elevation_scale = 0.6;
        play.requested.tick_rate_hz = 30.0;
        play.apply_settings(session);
        Ok("requested".to_owned())
    });
    s.wait(120)
        .check("settings applied by the world", |running| {
            let play = running.play();
            let configured = play.applied(Purpose::Rules)
                + play.applied(Purpose::Climate)
                + play.applied(Purpose::Scale);
            let gpu = running.gpu();
            let (rules, ambient, scale) = (
                gpu.rules(),
                gpu.ambient_temperature().degrees(),
                gpu.elevation_scale().value(),
            );
            let rate = running.session().runtime().clock_settings().0;
            if configured == 3
                && !rules.biology
                && ambient == -10.0
                && (scale - 0.6).abs() < 1e-6
                && rate == 30.0
            {
                Ok(format!(
                    "rules {:#x}, {ambient} °C, scale {scale}, {rate} Hz",
                    rules.bits()
                ))
            } else {
                Err(format!(
                    "{configured} configured; rules {:#x}, {ambient} °C, scale {scale}, {rate} Hz",
                    rules.bits()
                ))
            }
        });
    s.capture("frozen");
    s.check("restore the defaults", |running| {
        let (session, play) = running.session_and_play();
        play.requested.rules.biology = true;
        play.requested.ambient_c = world_model::config::DEFAULT_AMBIENT_TEMPERATURE_C;
        play.requested.elevation_scale = 0.5;
        play.requested.tick_rate_hz = 60.0;
        play.apply_settings(session);
        Ok("requested".to_owned())
    });

    // Overlay and debug views.
    s.wait(30)
        .tap(Key::F3)
        .wait(10)
        .capture("debug-overlay")
        .tap(Key::F3);
    s.tap(Key::Backquote)
        .wait(5)
        .capture("overlay-hidden")
        .tap(Key::Backquote);

    // Saving from the keyboard.
    s.tap(Key::F5)
        .wait(240)
        .check("save from the keyboard", |running| {
            match running.session().checkpoints().receipt() {
                Some(receipt) if receipt.committed().is_some() => Ok(format!("{receipt:?}")),
                other => Err(format!("{other:?}")),
            }
        });

    // Active-world resize, through the residency owner.
    s.check("resize the active world", |running| {
        let (session, play) = running.session_and_play();
        play.requested.radius = [4, 4];
        play.resize_window(session);
        Ok("requested 4 x 4".to_owned())
    });
    s.wait(400).check("the window was resized", |running| {
        let window = running.session().residency().window();
        let resident = running.session().residency().summary().resident;
        let drawn = running.render_counters().chunk_instances;
        if window.radius_x() == 4 && resident == 81 && drawn == 81 {
            Ok(format!("{resident} resident, {drawn} drawn"))
        } else {
            Err(format!(
                "radius {}x{}, {resident} resident, {drawn} drawn",
                window.radius_x(),
                window.radius_y()
            ))
        }
    });

    // The real window: resize, minimize and restore.
    s.act(Act::Resize([1280, 720])).wait(60).check(
        "the surface follows a window resize",
        |running| {
            let surface = running.surface();
            let targets = running.render_counters().target_sets_created;
            match surface.size {
                Some([1280, 720]) if targets >= 2 => Ok(format!(
                    "{:?}, {targets} target sets, {} reconfigurations",
                    surface.size, surface.reconfigurations
                )),
                other => Err(format!("{other:?}, {targets} target sets")),
            }
        },
    );
    s.capture("resized");
    s.remember("ticks", |running| {
        running.session().runtime().counters().ticks_submitted as f64
    });
    // A minimized window reports a zero size (KWin's Wayland reports nothing, so the size is
    // delivered to the client's resize handling as other platforms deliver it).
    s.act(Act::WindowSize([0, 0])).wait(90).check_with("zero size: nothing drawn or allocated, ticks continue", |running, memory| {
        let surface = running.surface();
        let ticks = running.session().runtime().counters().ticks_submitted as f64 - memory["ticks"];
        let frames = running.render_counters().frames;
        if surface.size.is_none() && ticks > 0.0 {
            Ok(format!("no surface size, {} skipped frames, {ticks} ticks, renderer at {frames} frames", surface.skipped_frames))
        } else {
            Err(format!("surface {:?}, {ticks} ticks", surface.size))
        }
    });
    s.act(Act::WindowSize([1280, 720])).wait(30).check(
        "drawing resumes at the restored size",
        |running| {
            let surface = running.surface();
            let counters = running.render_counters();
            match surface.size {
                Some([1280, 720]) => Ok(format!(
                    "{:?}, {} target sets",
                    surface.size, counters.target_sets_created
                )),
                other => Err(format!("{other:?}")),
            }
        },
    );
    s.capture("restored");
    s.wait(10).act(Act::Close);
    s.cues
}

fn possessed_position(running: &Running) -> Option<DVec3> {
    let body = running.play().observer.possessed?;
    running
        .session()
        .runtime()
        .bodies()
        .body(body)
        .and_then(|tracked| tracked.last.map(|sample| sample.position))
}

/// The totals figure a brush changes: its liquid's volume, loose material or bedrock.
fn totals_marker(tool: EditTool) -> impl Fn(&world_model::observation::WorldTotals) -> f64 {
    move |totals| match tool {
        EditTool::Water => totals.fluid[FluidMat::FLUID_WATER.0 as usize] as f64,
        EditTool::Lava => totals.fluid[FluidMat::FLUID_MAGMA.0 as usize] as f64,
        EditTool::Oil => totals.fluid[FluidMat::FLUID_OIL.0 as usize] as f64,
        EditTool::Ice => totals.fluid[FluidMat::FLUID_ICE.0 as usize] as f64,
        EditTool::Sand => totals.granular as f64,
        EditTool::Raise | EditTool::Lower => totals.elevation as f64,
        EditTool::Fire => -1.0,
    }
}

/// Device loss and recovery in the real window: a sphere and a save, a change after the save,
/// an injected loss, then the client's recovery from the latest complete checkpoint under a
/// later epoch (the driver answers the loss dialog's Recover).
pub fn loss_and_recovery(size: [u32; 2]) -> Vec<Cue> {
    let mut s = Script::new();
    s.act(Act::Pose(SHORE_POSE)).input(InputEvent::Cursor {
        x: size[0] as f32 * 0.66,
        y: size[1] as f32 * 0.5,
    });
    s.wait(2)
        .tap(Key::Digit(digit_for(Tool::Sphere(BodyPreset::Metal))));
    s.wait(5).hold(Button::Left, 2).wait(150);
    s.check("a sphere before the save", |running| {
        let tracked = running.session().runtime().bodies().summary().tracked;
        if tracked == 1 {
            Ok("1 tracked".to_owned())
        } else {
            Err(format!("{tracked} tracked"))
        }
    });
    s.tap(Key::F5).wait(240);
    s.check("the save committed", |running| {
        match running.session().checkpoints().receipt() {
            Some(receipt) if receipt.committed().is_some() => Ok(format!("{:?}", receipt.state)),
            other => Err(format!("{other:?}")),
        }
    });
    s.remember("saved_tick", |running| {
        running
            .session()
            .checkpoints()
            .latest()
            .tick
            .map_or(-1.0, |tick| tick.value() as f64)
    });
    s.remember("saved_checkpoint", |running| {
        running.session().checkpoints().latest().id.value() as f64
    });
    s.remember("epoch", |running| {
        f64::from(running.live().opened.epoch.value())
    });
    // A change after the save, which the loss takes with it.
    s.input(InputEvent::Cursor {
        x: size[0] as f32 * 0.5,
        y: size[1] as f32 * 0.5,
    });
    s.tap(Key::Digit(digit_for(Tool::Brush(EditTool::Water))));
    s.hold(Button::Left, 30).wait(60);
    s.check("a stroke after the save", |running| {
        let strokes = running.play().applied(Purpose::Stroke);
        if strokes > 0 {
            Ok(format!("{strokes} strokes"))
        } else {
            Err("no stroke applied".to_owned())
        }
    });
    s.tap(Key::Escape);
    s.act(Act::LoseDevice);
    s.wait(400).check_with("recovered from the latest complete checkpoint under a later epoch", |running, memory| {
        let opened = &running.live().opened;
        let epoch = f64::from(opened.epoch.value());
        let tick = opened.checkpoint.tick.map_or(-1.0, |tick| tick.value() as f64);
        let checkpoint = opened.checkpoint.id.value() as f64;
        let text = format!(
            "epoch {} -> {epoch}; reopened checkpoint {checkpoint} at tick {tick} (saved {} at {})",
            memory["epoch"], memory["saved_checkpoint"], memory["saved_tick"]
        );
        if epoch > memory["epoch"] && tick == memory["saved_tick"] && checkpoint == memory["saved_checkpoint"] {
            Ok(text)
        } else {
            Err(text)
        }
    });
    s.wait(120).check(
        "the saved sphere is back, the later stroke is not",
        |running| {
            let tracked = running.session().runtime().bodies().summary().tracked;
            let strokes = running.play().applied(Purpose::Stroke);
            if tracked == 1 && strokes == 0 {
                Ok(format!(
                    "{tracked} sphere restored; the recovered session has applied {strokes} strokes"
                ))
            } else {
                Err(format!("{tracked} tracked, {strokes} strokes"))
            }
        },
    );
    s.capture("recovered");
    s.wait(5).act(Act::Close);
    s.cues
}

/// The start screen through the client's own handlers: create a world, import a baseline save
/// (from a copy), open the new world, close it with its save, and reopen it at the saved tick.
pub struct WorldsDriver {
    pub report: Rc<RefCell<ScriptReport>>,
    legacy: PathBuf,
    step: u32,
    wait_until: u64,
    saved_tick: Option<u64>,
    opened_once: bool,
}

impl WorldsDriver {
    pub fn new(legacy: PathBuf) -> Self {
        Self {
            report: Rc::new(RefCell::new(ScriptReport {
                script: "worlds".to_owned(),
                ..ScriptReport::default()
            })),
            legacy,
            step: 0,
            wait_until: 10,
            saved_tick: None,
            opened_once: false,
        }
    }

    fn record(&self, frame: u64, name: &str, result: Result<String, String>) {
        let (passed, detail) = match result {
            Ok(detail) => (true, detail),
            Err(detail) => (false, detail),
        };
        self.report.borrow_mut().checks.push(CheckResult {
            name: name.to_owned(),
            frame,
            tick: None,
            passed,
            detail,
        });
    }
}

impl Driver for WorldsDriver {
    fn before_frame(&mut self, frame: DriverFrame<'_>) -> Control {
        use morbid_app::{catalog::EntryState, ui::StartAction};
        {
            let mut report = self.report.borrow_mut();
            report.frames = frame.frame;
            report.messages = frame.start.messages.clone();
        }
        if frame.frame < self.wait_until {
            return Control::Continue;
        }
        let find = |name: &str| {
            frame
                .start
                .worlds
                .iter()
                .find(|entry| entry.name == name)
                .cloned()
        };
        let now = frame.frame;
        match self.step {
            0 => {
                self.step = 1;
                self.wait_until = now + 5;
                Control::Start(StartAction::Create {
                    name: "Scripted".to_owned(),
                    seed: 69,
                })
            }
            1 => {
                let entry = find("Scripted");
                self.record(
                    now,
                    "create a world from the start screen",
                    match &entry {
                        Some(entry) => match &entry.state {
                            EntryState::Ready {
                                seed: 69,
                                tick: None,
                                imported: false,
                                ..
                            } => Ok(format!("{} listed, never run", entry.path.display())),
                            other => Err(format!("{other:?}")),
                        },
                        None => Err(format!("not listed: {:?}", frame.start.messages.first())),
                    },
                );
                self.step = 2;
                self.wait_until = now + 5;
                Control::Start(StartAction::Import {
                    name: "Imported".to_owned(),
                    legacy: self.legacy.clone(),
                    seed: 69,
                    scale: 0.5,
                })
            }
            2 => {
                let entry = find("Imported");
                self.record(
                    now,
                    "import a baseline save from the start screen",
                    match &entry {
                        Some(entry) => match &entry.state {
                            EntryState::Ready {
                                imported: true,
                                chunks,
                                ..
                            } if *chunks > 0 => Ok(format!(
                                "{chunks} chunks imported; {:?}",
                                frame.start.messages.first()
                            )),
                            other => Err(format!("{other:?}")),
                        },
                        None => Err(format!("not listed: {:?}", frame.start.messages.first())),
                    },
                );
                self.step = 3;
                let path = find("Scripted").map(|entry| entry.path).unwrap_or_default();
                Control::Open {
                    name: "Scripted".to_owned(),
                    path,
                }
            }
            3 => {
                let Some(running) = frame.running else {
                    return Control::Continue;
                };
                if !running.session().residency().settled()
                    || running
                        .session()
                        .runtime()
                        .last_submitted_tick()
                        .is_none_or(|tick| tick.value() < 120)
                {
                    return Control::Continue;
                }
                self.saved_tick = running
                    .session()
                    .runtime()
                    .last_submitted_tick()
                    .map(|tick| tick.value());
                self.opened_once = true;
                self.step = 4;
                Control::Close
            }
            4 => {
                if frame.phase != morbid_app::driver::PhaseKind::Start {
                    return Control::Continue;
                }
                let message = frame.start.messages.first().cloned().unwrap_or_default();
                self.record(
                    now,
                    "close with its save",
                    if message.contains("closed and saved") {
                        Ok(message)
                    } else {
                        Err(message)
                    },
                );
                self.step = 5;
                let path = find("Scripted").map(|entry| entry.path).unwrap_or_default();
                Control::Open {
                    name: "Scripted".to_owned(),
                    path,
                }
            }
            5 => {
                let Some(running) = frame.running else {
                    return Control::Continue;
                };
                let opened = running
                    .live()
                    .opened
                    .checkpoint
                    .tick
                    .map(|tick| tick.value());
                let at_close = self.saved_tick;
                self.record(now, "reopen at the closed tick", match (opened, at_close) {
                    (Some(opened), Some(closed)) if opened >= closed => Ok(format!(
                        "reopened at tick {opened}; {closed} was the last submitted tick when the close began"
                    )),
                    other => Err(format!("{other:?}")),
                });
                self.step = 6;
                Control::Quit
            }
            _ => Control::Continue,
        }
    }

    fn after_frame(&mut self, _running: Option<&mut Running>, _input: &InputState) -> bool {
        let report = self.report.borrow();
        report.frames > 20_000
    }
}
