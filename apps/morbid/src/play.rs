//! What the person does in an open world, turned into the runtime's commands: brushes, sphere
//! presets, attract and lift, possession with driving and jumping, the observer's movement, and
//! the simulation, climate and window settings. Every physical effect is a command through
//! `Runtime::submit`, resolved on the GPU (targets are GPU-resolved rays), and every command's
//! outcome is read back from its receipt and shown as it arrives: nothing is displayed as
//! applied before its result was received.
//!
//! Commands that act while a button or key is held (strokes, drive, attract, lift) are issued
//! once per tick: before a frame, the client asks the runtime how many ticks the frame owes and
//! queues one command for each of those ticks (`submit_at`), so the effect does not depend on
//! the frame rate. Queries (the cursor ray, the floor under a walking observer, totals) are
//! issued at most one at a time per purpose.

use std::collections::{HashMap, VecDeque};

use world_model::{
    body::BodyPreset,
    command::{
        AmbientTemperature, BodySpawn, Command, Direction, Drive, EditTool, ElevationScale,
        MaterialEdit, Ray, Target,
    },
    config::{
        BRUSH_RADIUS_CELLS, BRUSH_STRENGTH, DEFAULT_BRUSH_RADIUS_CELLS, DEFAULT_BRUSH_STRENGTH,
        RAY_MAX_DISTANCE,
    },
    glam::{DVec3, IVec2, Vec2, Vec3},
    observation::{BodyHit, SurfaceHit, WorldTotals},
    rules::SimulationRules,
};
use world_render::frame::{eye_position, forward, view_ray};
use world_runtime::{
    bodies::BodyEvent,
    ids::{CommandId, Tick},
    port::CommandResult,
    receipts::{AdmissionError, ReceiptState},
    session::WorldSession,
};

use crate::{
    input::{Button, InputState, Key},
    observer::Observer,
};

/// A sphere dropped from the brush cursor lands from this height above the column.
pub const SPHERE_DROP_HEIGHT: f32 = 10.0;
/// Totals are measured this often while the statistics are shown.
pub const TOTALS_EVERY_TICKS: u64 = 30;
/// Outcomes kept for the outcome log.
const LOG_LENGTH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    None,
    Brush(EditTool),
    Sphere(BodyPreset),
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Brush(EditTool::Water) => "Spawn Water",
            Self::Brush(EditTool::Lava) => "Spawn Lava",
            Self::Brush(EditTool::Oil) => "Spawn Oil",
            Self::Brush(EditTool::Ice) => "Spawn Ice",
            Self::Brush(EditTool::Fire) => "Spawn Fire",
            Self::Brush(EditTool::Sand) => "Spawn Sand",
            Self::Brush(EditTool::Raise) => "Raise Terrain",
            Self::Brush(EditTool::Lower) => "Lower Terrain",
            Self::Sphere(BodyPreset::Metal) => "Spawn Metal Sphere",
            Self::Sphere(BodyPreset::Wood) => "Spawn Wooden Sphere",
        }
    }
}

/// The toolbar's order (the baseline's); keys 1-9 and 0 select the tools after `None`.
pub const TOOLS: [Tool; 11] = [
    Tool::None,
    Tool::Brush(EditTool::Water),
    Tool::Brush(EditTool::Lava),
    Tool::Brush(EditTool::Oil),
    Tool::Brush(EditTool::Ice),
    Tool::Brush(EditTool::Fire),
    Tool::Brush(EditTool::Sand),
    Tool::Brush(EditTool::Raise),
    Tool::Brush(EditTool::Lower),
    Tool::Sphere(BodyPreset::Metal),
    Tool::Sphere(BodyPreset::Wood),
];

/// The tool a digit key selects: 1-9, then 0 for the tenth.
pub fn tool_for_digit(digit: u8) -> Option<Tool> {
    let index = if digit == 0 { 10 } else { usize::from(digit) };
    TOOLS.get(index).copied()
}

/// What a command was issued for, which decides what its outcome updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Purpose {
    Stroke,
    Spawn,
    Remove,
    Drive,
    Jump,
    Attract,
    Lift,
    CursorRay,
    Floor,
    Totals,
    Rules,
    Climate,
    Scale,
}

impl Purpose {
    fn label(self) -> &'static str {
        match self {
            Self::Stroke => "brush stroke",
            Self::Spawn => "spawn sphere",
            Self::Remove => "remove sphere",
            Self::Drive => "drive",
            Self::Jump => "jump",
            Self::Attract => "attract",
            Self::Lift => "lift",
            Self::CursorRay => "cursor ray",
            Self::Floor => "floor query",
            Self::Totals => "measure totals",
            Self::Rules => "simulation rules",
            Self::Climate => "ambient temperature",
            Self::Scale => "elevation scale",
        }
    }

    /// The purpose a command serves when it is issued directly (a scripted run's timeline).
    pub fn of(command: &Command) -> Self {
        match command {
            Command::EditMaterial(_) => Self::Stroke,
            Command::SpawnBody(_) => Self::Spawn,
            Command::RemoveBody { .. } => Self::Remove,
            Command::DriveBody { .. } => Self::Drive,
            Command::JumpBody { .. } => Self::Jump,
            Command::AttractBodies { .. } => Self::Attract,
            Command::LiftBodies { .. } => Self::Lift,
            Command::SetAmbientTemperature(_) => Self::Climate,
            Command::SetSimulationRules(_) => Self::Rules,
            Command::SetElevationScale(_) => Self::Scale,
            Command::QueryColumn { .. } => Self::Floor,
            Command::QueryRay { .. } => Self::CursorRay,
            Command::MeasureTotals => Self::Totals,
        }
    }

    /// Frequent purposes are counted, not logged one by one.
    fn logged(self) -> bool {
        !matches!(
            self,
            Self::CursorRay | Self::Floor | Self::Totals | Self::Drive | Self::Attract | Self::Lift
        )
    }
}

/// How a command ended, for the log.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Admitted by the runtime; its result has not arrived.
    Accepted,
    /// Applied in a tick and observed.
    Applied(String),
    /// Refused at admission, refused by the world (a missed target, a stale body, capacity),
    /// or abandoned.
    Rejected(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    pub what: &'static str,
    pub tick: Option<Tick>,
    pub verdict: Verdict,
}

/// Counts of outcomes by state, for the overlay.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutcomeCounts {
    pub accepted: u64,
    pub applied: u64,
    pub rejected: u64,
    pub refused_at_admission: u64,
    /// Ticks between a command's admission and the tick it applied in, the latest.
    pub latency_ticks: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushSettings {
    pub radius: i32,
    pub strength: u8,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self {
            radius: DEFAULT_BRUSH_RADIUS_CELLS,
            strength: DEFAULT_BRUSH_STRENGTH,
        }
    }
}

/// Simulation settings the person asked for, and what the world reports applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub rules: SimulationRules,
    pub ambient_c: f32,
    pub elevation_scale: f32,
    pub tick_rate_hz: f64,
    pub max_ticks_per_frame: u32,
    pub radius: [u32; 2],
}

/// The cursor's last GPU-resolved target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorTarget {
    pub hit: SurfaceHit,
    pub body: Option<BodyHit>,
    pub tick: Tick,
}

/// What the frame's input may drive.
#[derive(Debug, Clone, Copy)]
pub struct Focus {
    /// The overlay does not want the pointer (it is over the world, not a panel).
    pub pointer: bool,
    /// No text field has the keyboard.
    pub keyboard: bool,
}

pub struct Play {
    pub observer: Observer,
    pub tool: Tool,
    pub brush: BrushSettings,
    pub requested: Settings,
    pub applied: Settings,
    pub cursor: Option<CursorTarget>,
    pub floor: Option<(IVec2, f32, Tick)>,
    pub totals: Option<(Tick, WorldTotals)>,
    pub log: VecDeque<LogEntry>,
    pub counts: OutcomeCounts,
    /// Outcomes by purpose, applied and rejected, and the latest of each purpose's results.
    pub applied_by: HashMap<Purpose, u64>,
    pub rejected_by: HashMap<Purpose, u64>,
    pub latest: HashMap<Purpose, (Tick, CommandResult)>,
    /// Cells brush strokes changed, as their results reported.
    pub changed_cells: u64,
    pub body_events: VecDeque<BodyEvent>,
    pub statistics_open: bool,
    pub viewport: [u32; 2],
    pub fov_y: f32,
    next_command: u64,
    /// Commands not yet final, oldest first.
    pending: VecDeque<(CommandId, Purpose)>,
    /// The newest unfinished query per purpose: a new one waits until it is answered.
    queries: HashMap<Purpose, CommandId>,
    /// The last tick a held command was queued for, per purpose.
    queued_through: HashMap<Purpose, Tick>,
    last_totals: Option<Tick>,
    looking: bool,
}

impl Play {
    pub fn new(observer: Observer, settings: Settings) -> Self {
        Self {
            observer,
            tool: Tool::None,
            brush: BrushSettings::default(),
            requested: settings,
            applied: settings,
            cursor: None,
            floor: None,
            totals: None,
            log: VecDeque::with_capacity(LOG_LENGTH),
            counts: OutcomeCounts::default(),
            applied_by: HashMap::new(),
            rejected_by: HashMap::new(),
            latest: HashMap::new(),
            changed_cells: 0,
            body_events: VecDeque::with_capacity(LOG_LENGTH),
            statistics_open: false,
            viewport: [1, 1],
            fov_y: world_render::frame::DEFAULT_FOV_Y_DEGREES.to_radians(),
            next_command: 1,
            pending: VecDeque::new(),
            queries: HashMap::new(),
            queued_through: HashMap::new(),
            last_totals: None,
            looking: false,
        }
    }

    pub fn looking(&self) -> bool {
        self.looking
    }

    fn push_log(&mut self, entry: LogEntry) {
        if self.log.len() == LOG_LENGTH {
            self.log.pop_back();
        }
        self.log.push_front(entry);
    }

    /// Submits `command` for the tick `at` (or the next one), recording why.
    fn submit(
        &mut self,
        session: &mut WorldSession,
        purpose: Purpose,
        command: Command,
        at: Option<Tick>,
    ) -> Option<CommandId> {
        let id = CommandId::new(self.next_command);
        let runtime = session.runtime_mut();
        let admitted = match at {
            Some(tick) => runtime.submit_at(id, command, tick).map(|_| ()),
            None => runtime.submit(id, command).map(|_| ()),
        };
        match admitted {
            Ok(()) => {
                self.next_command += 1;
                self.pending.push_back((id, purpose));
                self.counts.accepted += 1;
                if purpose.logged() {
                    self.push_log(LogEntry {
                        what: purpose.label(),
                        tick: None,
                        verdict: Verdict::Accepted,
                    });
                }
                Some(id)
            }
            Err(error) => {
                self.counts.refused_at_admission += 1;
                // Pressure on a frequent query is expected backpressure, not news.
                if purpose.logged() || !matches!(error, AdmissionError::Pressure(_)) {
                    self.push_log(LogEntry {
                        what: purpose.label(),
                        tick: None,
                        verdict: Verdict::Rejected(format!("refused at admission: {error:?}")),
                    });
                }
                None
            }
        }
    }

    /// Submits a scripted run's command for tick `at` under the client's own identities, so its
    /// outcome is read, counted and released like any other; the admission error when refused.
    pub fn submit_scripted(
        &mut self,
        session: &mut WorldSession,
        command: Command,
        at: Tick,
    ) -> Result<CommandId, AdmissionError> {
        let id = CommandId::new(self.next_command);
        session.runtime_mut().submit_at(id, command, at)?;
        self.next_command += 1;
        self.pending.push_back((id, Purpose::of(&command)));
        self.counts.accepted += 1;
        Ok(id)
    }

    /// Submits a query unless the previous one of its purpose is unanswered.
    fn query(&mut self, session: &mut WorldSession, purpose: Purpose, command: Command) {
        if let Some(previous) = self.queries.get(&purpose)
            && session
                .runtime()
                .receipt(*previous)
                .is_some_and(|receipt| !receipt.state.is_final())
        {
            return;
        }
        if let Some(id) = self.submit(session, purpose, command, None) {
            self.queries.insert(purpose, id);
        }
    }

    /// Queues one `command` for each of the frame's `due` ticks not yet covered for `purpose`.
    fn per_tick(
        &mut self,
        session: &mut WorldSession,
        purpose: Purpose,
        due: u32,
        command: Command,
    ) {
        let next = session
            .runtime()
            .last_submitted_tick()
            .map_or(Tick::ZERO, |tick| Tick::new(tick.value() + 1));
        let from = match self.queued_through.get(&purpose) {
            Some(through) if *through >= next => Tick::new(through.value() + 1),
            _ => next,
        };
        let until = next.value() + u64::from(due);
        let mut tick = from.value();
        while tick < until {
            if self
                .submit(session, purpose, command, Some(Tick::new(tick)))
                .is_none()
            {
                break;
            }
            self.queued_through.insert(purpose, Tick::new(tick));
            tick += 1;
        }
    }

    /// The camera eye and the ray through the cursor, from the observer and, while possessing,
    /// the sphere's latest sample (the camera itself is placed on the device).
    pub fn cursor_ray(&self, session: &WorldSession, cursor: Option<[f32; 2]>) -> Option<Ray> {
        let [x, y] = cursor?;
        let [width, height] = self.viewport;
        let ndc = Vec2::new(
            x / width.max(1) as f32 * 2.0 - 1.0,
            1.0 - y / height.max(1) as f32 * 2.0,
        );
        let eye = eye_position(&self.observer.rig(), self.possessed_position(session));
        let direction = view_ray(
            self.observer.yaw,
            self.observer.pitch,
            self.fov_y,
            width.max(1) as f32 / height.max(1) as f32,
            ndc,
        );
        Ray::new(eye, direction, RAY_MAX_DISTANCE).ok()
    }

    fn possessed_position(&self, session: &WorldSession) -> Option<DVec3> {
        let body = self.observer.possessed?;
        session
            .runtime()
            .bodies()
            .body(body)
            .and_then(|tracked| tracked.last.map(|sample| sample.position))
    }

    /// Changes the tool, keeping the brush settings.
    pub fn select_tool(&mut self, tool: Tool) {
        self.tool = tool;
    }

    /// Possesses the nearest observed sphere, or releases the possessed one.
    pub fn toggle_possession(&mut self, session: &mut WorldSession) {
        if self.observer.possessed.take().is_some() {
            session.runtime_mut().set_controlled(None);
            self.push_log(LogEntry {
                what: "release sphere",
                tick: None,
                verdict: Verdict::Applied("released".to_owned()),
            });
            return;
        }
        let eye = self.observer.position;
        let nearest = session
            .runtime()
            .bodies()
            .bodies()
            .filter_map(|tracked| tracked.last.map(|sample| (tracked.body, sample.position)))
            .min_by(|a, b| {
                a.1.distance_squared(eye)
                    .total_cmp(&b.1.distance_squared(eye))
            });
        match nearest {
            Some((body, _)) => {
                self.observer.possessed = Some(body);
                session.runtime_mut().set_controlled(Some(body));
                self.push_log(LogEntry {
                    what: "possess sphere",
                    tick: None,
                    verdict: Verdict::Applied(format!(
                        "slot {} gen {}",
                        body.index(),
                        body.generation()
                    )),
                });
            }
            None => self.push_log(LogEntry {
                what: "possess sphere",
                tick: None,
                verdict: Verdict::Rejected("no sphere has been observed".to_owned()),
            }),
        }
    }

    /// Requests the settings changes the panels made: rules, climate and scale as commands, the
    /// clock and the window directly on their owners.
    pub fn apply_settings(&mut self, session: &mut WorldSession) {
        let requested = self.requested;
        if requested.rules != self.applied.rules {
            self.applied.rules = requested.rules;
            self.submit(
                session,
                Purpose::Rules,
                Command::SetSimulationRules(requested.rules),
                None,
            );
        }
        if requested.ambient_c != self.applied.ambient_c
            && let Ok(temperature) = AmbientTemperature::celsius(requested.ambient_c)
        {
            {
                self.applied.ambient_c = requested.ambient_c;
                self.submit(
                    session,
                    Purpose::Climate,
                    Command::SetAmbientTemperature(temperature),
                    None,
                );
            }
        }
        if requested.elevation_scale != self.applied.elevation_scale
            && let Ok(scale) = ElevationScale::new(requested.elevation_scale)
        {
            {
                self.applied.elevation_scale = requested.elevation_scale;
                self.submit(
                    session,
                    Purpose::Scale,
                    Command::SetElevationScale(scale),
                    None,
                );
            }
        }
        if (requested.tick_rate_hz, requested.max_ticks_per_frame)
            != (self.applied.tick_rate_hz, self.applied.max_ticks_per_frame)
        {
            let verdict = match session
                .runtime_mut()
                .set_clock(requested.tick_rate_hz, requested.max_ticks_per_frame)
            {
                Ok(()) => {
                    self.applied.tick_rate_hz = requested.tick_rate_hz;
                    self.applied.max_ticks_per_frame = requested.max_ticks_per_frame;
                    Verdict::Applied(format!(
                        "{} Hz, {} ticks per frame",
                        requested.tick_rate_hz, requested.max_ticks_per_frame
                    ))
                }
                Err(error) => {
                    self.requested.tick_rate_hz = self.applied.tick_rate_hz;
                    self.requested.max_ticks_per_frame = self.applied.max_ticks_per_frame;
                    Verdict::Rejected(format!("{error:?}"))
                }
            };
            self.push_log(LogEntry {
                what: "clock",
                tick: None,
                verdict,
            });
        }
    }

    /// Requests a resize of the resident window around its current centre. The residency owner
    /// checks it against the device and its budgets before evicting anything; a refusal shows
    /// in the session panel and leaves the window as it was.
    pub fn resize_window(&mut self, session: &mut WorldSession) {
        let [x, y] = self.requested.radius;
        let center = session.residency().target().center();
        match world_runtime::port::ResidentWindow::new(center, x, y) {
            Ok(window) => {
                session.set_window(window);
                self.applied.radius = [x, y];
                self.push_log(LogEntry {
                    what: "resize window",
                    tick: None,
                    verdict: Verdict::Accepted,
                });
            }
            Err(error) => self.push_log(LogEntry {
                what: "resize window",
                tick: None,
                verdict: Verdict::Rejected(format!("{error:?}")),
            }),
        }
    }

    /// The frame's intent: observer, possession and tools, as commands for the frame's ticks.
    pub fn intend(
        &mut self,
        session: &mut WorldSession,
        input: &InputState,
        focus: Focus,
        elapsed: std::time::Duration,
    ) {
        let dt = elapsed.as_secs_f32().min(0.25);
        let keys = focus.keyboard;
        // Looking around holds the right button; it started over the world.
        if input.button_pressed(Button::Right) && focus.pointer {
            self.looking = true;
        }
        if !input.button(Button::Right) {
            self.looking = false;
        }
        self.observer.steer(input, self.looking, keys);
        if keys {
            if input.pressed(Key::F) {
                self.toggle_possession(session);
            }
            if let Some(tool) = input.digit_pressed().and_then(tool_for_digit) {
                self.select_tool(tool);
            }
            if input.pressed(Key::Escape) {
                self.select_tool(Tool::None);
            }
            let step = |value: i32, by: i32, range: &std::ops::RangeInclusive<i32>| {
                (value + by).clamp(*range.start(), *range.end())
            };
            if input.pressed(Key::BracketLeft) {
                self.brush.radius = step(self.brush.radius, -1, &BRUSH_RADIUS_CELLS);
            }
            if input.pressed(Key::BracketRight) {
                self.brush.radius = step(self.brush.radius, 1, &BRUSH_RADIUS_CELLS);
            }
            let strength = i32::from(self.brush.strength);
            let range = i32::from(*BRUSH_STRENGTH.start())..=i32::from(*BRUSH_STRENGTH.end());
            if input.pressed(Key::Minus) {
                self.brush.strength = step(strength, -1, &range) as u8;
            }
            if input.pressed(Key::Equal) {
                self.brush.strength = step(strength, 1, &range) as u8;
            }
        }
        let due = session.runtime().ticks_due(elapsed);

        // A possessed sphere is driven; the observer tracks its latest sample.
        if let Some(body) = self.observer.possessed {
            if !session.runtime().bodies().body(body).is_some() {
                // Removed, or never confirmed: release it.
                self.observer.possessed = None;
                session.runtime_mut().set_controlled(None);
            } else {
                let held = |key| keys && input.held(key);
                let heading = forward(self.observer.yaw, 0.0);
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
                if let Some(direction) = wish
                    .try_normalize()
                    .and_then(|wish| Direction::new(wish.x, -wish.z).ok())
                {
                    self.per_tick(
                        session,
                        Purpose::Drive,
                        due,
                        Command::DriveBody {
                            body,
                            drive: Drive::controls(direction),
                        },
                    );
                }
                if keys && input.pressed(Key::Space) {
                    self.submit(session, Purpose::Jump, Command::JumpBody { body }, None);
                }
                if let Some(position) = self.possessed_position(session) {
                    self.observer.track(position);
                }
            }
        }
        if self.observer.possessed.is_none() {
            let floor = self
                .floor
                .filter(|(cell, ..)| *cell == self.observer.cell())
                .map(|(_, height, _)| height);
            self.observer.travel(input, keys, dt, floor);
            if !self.observer.noclip {
                let cell = self.observer.cell();
                self.query(session, Purpose::Floor, Command::QueryColumn { cell });
            }
        }

        // Tools act at the GPU-resolved cursor ray.
        let ray = if focus.pointer && !self.looking {
            self.cursor_ray(session, input.cursor)
        } else {
            None
        };
        if let Some(ray) = ray {
            if self.tool != Tool::None {
                self.query(session, Purpose::CursorRay, Command::QueryRay { ray });
            }
            match self.tool {
                Tool::Brush(tool) if input.button(Button::Left) => {
                    if let Ok(edit) = MaterialEdit::at(
                        tool,
                        Target::Ray(ray),
                        self.brush.radius,
                        self.brush.strength,
                    ) {
                        self.per_tick(session, Purpose::Stroke, due, Command::EditMaterial(edit));
                    }
                }
                Tool::Sphere(preset) if input.button_pressed(Button::Left) => {
                    if let Ok(spawn) =
                        BodySpawn::with(preset.description(), Target::Ray(ray), SPHERE_DROP_HEIGHT)
                    {
                        self.submit(session, Purpose::Spawn, Command::SpawnBody(spawn), None);
                    }
                }
                _ => {}
            }
            if input.button(Button::Back) {
                self.per_tick(
                    session,
                    Purpose::Attract,
                    due,
                    Command::AttractBodies {
                        target: Target::Ray(ray),
                    },
                );
            }
            if input.button(Button::Forward) {
                self.per_tick(
                    session,
                    Purpose::Lift,
                    due,
                    Command::LiftBodies {
                        target: Target::Ray(ray),
                    },
                );
            }
        } else {
            self.cursor = None;
        }

        if self.statistics_open {
            let next = session
                .runtime()
                .last_submitted_tick()
                .map_or(0, |tick| tick.value() + 1);
            if self
                .last_totals
                .is_none_or(|last| next >= last.value() + TOTALS_EVERY_TICKS)
            {
                self.last_totals = Some(Tick::new(next));
                self.query(session, Purpose::Totals, Command::MeasureTotals);
            }
        }
    }

    /// Reads the outcomes of this session's commands, releases their receipts and collects the
    /// body monitor's events.
    pub fn observe(&mut self, session: &mut WorldSession) {
        let mut acknowledged = None;
        while let Some(&(id, purpose)) = self.pending.front() {
            let Some(receipt) = session.runtime().receipt(id) else {
                // Already released (a new session): nothing to read.
                self.pending.pop_front();
                continue;
            };
            match receipt.state {
                ReceiptState::Completed { tick, result } => {
                    let latency = receipt
                        .admitted_after
                        .map_or(tick.value() + 1, |after| tick.since(after));
                    self.counts.latency_ticks = latency;
                    self.completed(session, purpose, tick, result);
                }
                ReceiptState::Abandoned { reason } => {
                    self.counts.rejected += 1;
                    self.push_log(LogEntry {
                        what: purpose.label(),
                        tick: None,
                        verdict: Verdict::Rejected(format!("abandoned: {reason:?}")),
                    });
                }
                ReceiptState::Pending | ReceiptState::Submitted { .. } => break,
            }
            self.pending.pop_front();
            acknowledged = Some(id);
        }
        if let Some(through) = acknowledged {
            // Every receipt up to `through` is final: the loop stopped at the first that was not.
            let _ = session.runtime_mut().acknowledge(through);
        }
        let mut events = Vec::new();
        session.runtime_mut().bodies_mut().drain_events(&mut events);
        for event in events {
            if self.body_events.len() == LOG_LENGTH {
                self.body_events.pop_back();
            }
            self.body_events.push_front(event);
            if let BodyEvent::Removed { body, .. } = event
                && self.observer.possessed == Some(body)
            {
                self.observer.possessed = None;
                session.runtime_mut().set_controlled(None);
            }
        }
    }

    fn completed(
        &mut self,
        session: &WorldSession,
        purpose: Purpose,
        tick: Tick,
        result: CommandResult,
    ) {
        let scale = self.applied.elevation_scale;
        let (applied, text) = match result {
            CommandResult::Edited {
                center,
                changed,
                not_resident,
            } => (
                true,
                format!("{changed} cells changed at {center} ({not_resident} not resident)"),
            ),
            CommandResult::TargetMissed(hit) => {
                if purpose == Purpose::CursorRay {
                    self.cursor = None;
                }
                (false, format!("the ray found no surface: {hit:?}"))
            }
            CommandResult::TargetNotResident { cell } => (false, format!("{cell} is not resident")),
            CommandResult::Column { cell, column } => {
                if purpose == Purpose::Floor {
                    self.floor = Some((cell, column.solid_top() as f32 * scale, tick));
                }
                (true, format!("column at {cell}"))
            }
            CommandResult::ColumnNotResident { cell } => (false, format!("{cell} is not resident")),
            CommandResult::Configured => (true, "configured".to_owned()),
            CommandResult::Totals(totals) => {
                self.totals = Some((tick, totals));
                (true, "measured".to_owned())
            }
            CommandResult::BodySpawned { body, position } => (
                true,
                format!(
                    "slot {} gen {} at ({:.1}, {:.1}, {:.1})",
                    body.index(),
                    body.generation(),
                    position.x,
                    position.y,
                    position.z
                ),
            ),
            CommandResult::BodyCapacity { capacity } => {
                (false, format!("all {capacity} body slots are taken"))
            }
            CommandResult::BodyRemoved { body, .. } => {
                (true, format!("slot {} removed", body.index()))
            }
            CommandResult::StaleBody { body } => {
                if self.observer.possessed == Some(body) {
                    self.observer.possessed = None;
                }
                (
                    false,
                    format!("slot {} gen {} is gone", body.index(), body.generation()),
                )
            }
            CommandResult::Driven { .. } => (true, "driven".to_owned()),
            CommandResult::Jumped { jumped, .. } => {
                if jumped {
                    (true, "jumped".to_owned())
                } else {
                    (false, "not grounded: no jump".to_owned())
                }
            }
            CommandResult::Attracted { bodies, .. } => (true, format!("{bodies} attracted")),
            CommandResult::Lifted { bodies, .. } => (true, format!("{bodies} lifted")),
            CommandResult::Ray { surface, body } => {
                if purpose == Purpose::CursorRay {
                    self.cursor = Some(CursorTarget {
                        hit: surface,
                        body,
                        tick,
                    });
                }
                (true, format!("{surface:?}"))
            }
        };
        if applied {
            self.counts.applied += 1;
            *self.applied_by.entry(purpose).or_default() += 1;
        } else {
            self.counts.rejected += 1;
            *self.rejected_by.entry(purpose).or_default() += 1;
        }
        if let CommandResult::Edited { changed, .. } = result {
            self.changed_cells += u64::from(changed);
        }
        self.latest.insert(purpose, (tick, result));
        if purpose.logged() || !applied && purpose != Purpose::CursorRay {
            let _ = session;
            self.push_log(LogEntry {
                what: purpose.label(),
                tick: Some(tick),
                verdict: if applied {
                    Verdict::Applied(text)
                } else {
                    Verdict::Rejected(text)
                },
            });
        }
    }

    /// The brush cursor to draw: the cell the last ray hit, with the stroke's radius (a point
    /// for a sphere preset).
    pub fn cursor_preview(&self) -> Option<world_render::Cursor> {
        if self.tool == Tool::None {
            return None;
        }
        let cell = self.cursor?.hit.cell()?;
        Some(world_render::Cursor {
            cell,
            radius: match self.tool {
                Tool::Sphere(_) => 0.0,
                _ => self.brush.radius as f32,
            },
        })
    }

    /// Commands admitted and not yet final.
    pub fn outstanding(&self) -> usize {
        self.pending.len()
    }

    pub fn applied(&self, purpose: Purpose) -> u64 {
        self.applied_by.get(&purpose).copied().unwrap_or(0)
    }

    pub fn rejected(&self, purpose: Purpose) -> u64 {
        self.rejected_by.get(&purpose).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_select_the_toolbar_in_order() {
        assert_eq!(tool_for_digit(1), Some(Tool::Brush(EditTool::Water)));
        assert_eq!(tool_for_digit(8), Some(Tool::Brush(EditTool::Lower)));
        assert_eq!(tool_for_digit(9), Some(Tool::Sphere(BodyPreset::Metal)));
        assert_eq!(tool_for_digit(0), Some(Tool::Sphere(BodyPreset::Wood)));
        // Every tool the baseline offered is in the toolbar exactly once.
        for tool in EditTool::ALL {
            assert_eq!(TOOLS.iter().filter(|t| **t == Tool::Brush(tool)).count(), 1);
        }
    }
}
