//! The client: one window, its event loop driven explicitly (`pump_app_events`), and the phases
//! an open world goes through.
//!
//! ```text
//! Start    world list, adapter choice, create and import; the menu device draws it
//! Running  the live world: input -> intent and commands -> session frame -> outcomes -> render
//! Closing  one frame saying so, then the session's orderly close (bounded), then Start
//! Lost     the device was lost: what the CPU held is written, the loss is shown, Recover opens
//!          a new session from the latest complete checkpoint on a new device
//! ```
//!
//! A frame never waits for the device: the session polls, the renderer submits on the world's
//! timeline when a presentation slot is free, and FIFO presentation paces a visible window.
//! Without vsync nothing paces it, and a frame that found both slots busy and no tick it could
//! submit would change nothing; the loop runs a frame only when one has work (see
//! `Client::has_work`) and otherwise waits in the event wait for `IDLE_WAIT` at a time. A
//! minimized or occluded window draws nothing and the loop sleeps in the event wait until the
//! next tick is due, so the world keeps its clock without a busy loop.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use gpu_world::world::PresentRefusal;
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{DeviceEvent, DeviceId, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    platform::pump_events::{EventLoopExtPumpEvents, PumpStatus},
    window::{CursorGrabMode, Window, WindowId},
};
use world_render::{FrameRequest, Renderer, frame::DEFAULT_NEAR};

use crate::{
    catalog,
    driver::{Driver, DriverFrame},
    input::{self, InputEvent, InputState, Key},
    live::{self, LiveWorld, LostWorld, SessionRequest},
    observer::Observer,
    options::{self, ClientOptions},
    play::{Focus, Play, Settings},
    present::{Acquired, MenuGfx, Overlay, Surface},
    ui::{self, Hud, HudAction, LossAction, StartAction, StartScreen, WorldStatus},
};

/// Frame durations kept for the frame-rate and tail figures.
const FRAME_HISTORY: usize = 600;
/// How long menus wait for events between frames.
const MENU_FRAME: Duration = Duration::from_millis(33);
/// How long the loop waits at a time while an open world has nothing for a frame to do. A
/// presentation slot frees when the older of two frames completes, while the device renders
/// the newer; this wait plus the next frame's CPU work must stay below that frame's device
/// time, or the device idles. The tightest case measured, radius 6 without vsync, has frames
/// of about 1.1 ms and 0.45 ms of CPU work in the frame that presents (07).
const IDLE_WAIT: Duration = Duration::from_micros(250);

/// A world on its device: the session, the window's surface, the renderer and the overlay.
pub struct Running {
    live: LiveWorld,
    surface: Surface,
    renderer: Renderer,
    overlay: Overlay,
    play: Play,
    /// The chunk the window was last centred on.
    focus: world_model::spatial::ChunkKey,
    /// Frames the renderer did not draw because two were still on the device.
    busy_frames: u64,
    /// Times the loop found nothing for a frame to do and waited instead.
    idle_waits: u64,
    /// A driver asked for the next frame's pixels.
    capture_requested: bool,
    capture: Option<Result<Capture, String>>,
}

/// One frame's pixels, as presented (world and overlay), in RGBA8 rows of `size[0]` pixels.
#[derive(Debug, Clone)]
pub struct Capture {
    pub size: [u32; 2],
    pub rgba: Vec<u8>,
    /// The bytes are sRGB-encoded (the surface format was an sRGB or encoded one).
    pub tick: Option<world_runtime::ids::Tick>,
}

/// The surface as presented, for reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceReport {
    pub format: wgpu::TextureFormat,
    pub present_mode: wgpu::PresentMode,
    pub size: Option<[u32; 2]>,
    pub reconfigurations: u64,
    pub skipped_frames: u64,
    pub presented_frames: u64,
    pub busy_frames: u64,
    /// Times the loop found nothing for a frame to do and waited instead, each wait at most a
    /// quarter of a millisecond.
    pub idle_waits: u64,
    /// The last swapchain image acquisition's wait.
    pub last_acquire: std::time::Duration,
}

impl Running {
    /// Takes the session out, releasing the window's surface, the renderer and the overlay
    /// first: the window can have one surface at a time, and the next one (the menu's, or a
    /// recovered session's) must not be created while this one still exists.
    fn into_live(self: Box<Self>) -> LiveWorld {
        let Running {
            mut live,
            surface,
            renderer,
            overlay,
            ..
        } = *self;
        // The last frames finish before the surface goes (a maintenance wait).
        live.gpu().wait_idle(Duration::from_secs(2));
        drop(overlay);
        drop(renderer);
        drop(surface);
        live
    }

    pub fn live(&self) -> &LiveWorld {
        &self.live
    }

    pub fn session(&self) -> &world_runtime::session::WorldSession {
        &self.live.session
    }

    pub fn session_mut(&mut self) -> &mut world_runtime::session::WorldSession {
        &mut self.live.session
    }

    pub fn play(&self) -> &Play {
        &self.play
    }

    pub fn play_mut(&mut self) -> &mut Play {
        &mut self.play
    }

    /// The session and the play state together, for drivers that act as the person would.
    pub fn session_and_play(&mut self) -> (&mut world_runtime::session::WorldSession, &mut Play) {
        (&mut self.live.session, &mut self.play)
    }

    pub fn gpu(&mut self) -> &mut gpu_world::world::GpuWorld {
        self.live.gpu()
    }

    pub fn render_counters(&self) -> world_render::RenderCounters {
        self.renderer.counters()
    }

    /// Asks for the next drawn frame's pixels; [`take_capture`](Self::take_capture) returns
    /// them once that frame was drawn. A capture is a maintenance readback that waits for the
    /// device: it is for evidence, never part of an ordinary frame.
    pub fn request_capture(&mut self) {
        self.capture_requested = true;
    }

    pub fn take_capture(&mut self) -> Option<Result<Capture, String>> {
        self.capture.take()
    }

    pub fn surface(&self) -> SurfaceReport {
        SurfaceReport {
            format: self.surface.format(),
            present_mode: self.surface.present_mode(),
            size: self.surface.size(),
            reconfigurations: self.surface.reconfigurations,
            skipped_frames: self.surface.skipped_frames,
            presented_frames: self.surface.presented_frames,
            busy_frames: self.busy_frames,
            idle_waits: self.idle_waits,
            last_acquire: self.surface.last_acquire,
        }
    }
}

enum Phase {
    Start,
    Running(Box<Running>),
    /// The notice is drawn on the first frame; the close runs on the next.
    Closing {
        running: Box<Running>,
        exit_after: bool,
        notice_drawn: bool,
    },
    Lost(Box<LostWorld>),
}

/// What a run of the client did, for its caller.
#[derive(Debug, Clone, Default)]
pub struct RunSummary {
    pub frames: u64,
    pub closes: Vec<String>,
    /// Device losses, each with what it cost (reported, not a failure of the client).
    pub losses: Vec<String>,
    pub errors: Vec<String>,
}

pub struct Client {
    options: ClientOptions,
    window: Option<Arc<Window>>,
    input: InputState,
    phase: Phase,
    menu: Option<MenuGfx>,
    start: StartScreen,
    hud: Hud,
    quit: bool,
    last_frame: Instant,
    started: Instant,
    frame_times: VecDeque<f64>,
    pending_size: Option<[u32; 2]>,
    occluded: bool,
    /// The last check found nothing for a frame of the open world to do.
    idle: bool,
    grabbed: bool,
    driver: Option<Box<dyn Driver>>,
    /// A start-screen action a driver submitted, handled on the next start-screen frame.
    driven_start: Option<StartAction>,
    summary: RunSummary,
    /// Set by scripted runs: conformance fault injection on the sessions they open.
    fault_injection: bool,
    /// Set by measurement runs: tick timestamps on the sessions they open.
    timestamps: bool,
}

impl Client {
    pub fn new(options: ClientOptions, driver: Option<Box<dyn Driver>>) -> Self {
        let fault_injection = driver
            .as_ref()
            .is_some_and(|driver| driver.fault_injection());
        let timestamps = driver.as_ref().is_some_and(|driver| driver.timestamps());
        let start = StartScreen {
            worlds: Vec::new(),
            worlds_dir: options.worlds.clone(),
            adapters: Vec::new(),
            adapter: options.adapter.clone(),
            new_name: "New World".to_owned(),
            new_seed: catalog::fresh_seed().to_string(),
            import_path: String::new(),
            import_name: "Imported".to_owned(),
            import_seed: "69".to_owned(),
            import_scale: "0.5".to_owned(),
            messages: Vec::new(),
        };
        Self {
            options,
            window: None,
            input: InputState::default(),
            phase: Phase::Start,
            menu: None,
            start,
            hud: Hud::default(),
            quit: false,
            last_frame: Instant::now(),
            started: Instant::now(),
            frame_times: VecDeque::with_capacity(FRAME_HISTORY),
            pending_size: None,
            occluded: false,
            idle: false,
            grabbed: false,
            driver,
            driven_start: None,
            summary: RunSummary::default(),
            fault_injection,
            timestamps,
        }
    }

    fn window(&self) -> Option<Arc<Window>> {
        self.window.clone()
    }

    fn window_size(&self) -> [u32; 2] {
        self.window.as_ref().map_or([0, 0], |window| {
            let size = window.inner_size();
            [size.width, size.height]
        })
    }

    /// How long the event wait may sleep before the next frame.
    fn wait_hint(&self) -> Duration {
        match &self.phase {
            Phase::Running(running) => {
                if self.occluded || running.surface.size().is_none() {
                    running.live.session.runtime().tick_duration()
                } else if self.idle {
                    IDLE_WAIT
                } else {
                    Duration::ZERO
                }
            }
            Phase::Closing { .. } => Duration::ZERO,
            Phase::Start | Phase::Lost(_) => MENU_FRAME,
        }
    }

    fn refresh_worlds(&mut self) {
        match catalog::list(&self.options.worlds) {
            Ok(worlds) => self.start.worlds = worlds,
            Err(error) => self
                .start
                .message(format!("The worlds directory could not be read: {error}")),
        }
    }

    fn open_menu(&mut self) {
        let Some(window) = self.window() else {
            return;
        };
        if self.menu.is_some() {
            return;
        }
        match MenuGfx::open(&window, self.options.adapter.as_deref(), self.options.vsync) {
            Ok(menu) => self.menu = Some(menu),
            Err(error) => {
                eprintln!("morbid: {error}");
                self.summary.errors.push(error);
                self.quit = true;
            }
        }
    }

    fn request(&self, adapter: String) -> SessionRequest {
        SessionRequest {
            adapter,
            autosave: self.options.autosave.clone(),
            fault_injection: self.fault_injection,
            timestamps: self.timestamps,
        }
    }

    /// Opens a world on the chosen adapter: the menu device goes first, since the window has one
    /// surface at a time. A failure brings the menu back with the reason.
    fn open_world(
        &mut self,
        name: String,
        path: PathBuf,
        store: Option<Arc<dyn world_runtime::store::WorldStore>>,
        after: Option<world_runtime::ids::SessionEpoch>,
    ) {
        let Some(window) = self.window() else {
            return;
        };
        let Some(adapter) = self.options.adapter.clone() else {
            self.start.message("Choose an adapter first.");
            return;
        };
        if !matches!(self.phase, Phase::Start | Phase::Lost(_)) {
            // One world at a time: the open one is closed first (its surface must go before
            // another is made for the window).
            self.start
                .message(format!("{name}: close the open world first."));
            return;
        }
        self.menu = None;
        let store = match store.map_or_else(|| live::open_store(&path), Ok) {
            Ok(store) => store,
            Err(error) => {
                self.start.message(format!("{name}: {error}"));
                self.open_menu();
                return;
            }
        };
        let request = self.request(adapter);
        let opened = LiveWorld::open(
            name.clone(),
            path,
            store,
            &request,
            wgpu::SurfaceTarget::from(Arc::clone(&window)),
            after,
        );
        let (mut live, surface) = match opened {
            Ok(opened) => opened,
            Err(error) => {
                eprintln!("morbid: {name} could not be opened: {error}");
                self.start.message(format!("{name}: {error}"));
                self.summary.errors.push(error.to_string());
                self.open_menu();
                return;
            }
        };
        match Self::compose(
            &window,
            &mut live,
            surface,
            self.options.vsync,
            self.window_size(),
        ) {
            Ok((surface, renderer, overlay)) => {
                let play = Self::play_for(&live);
                let focus = live.opened.checkpoint.window.center();
                window.set_title(&format!("Morbid Monarchy — {name}"));
                if live.opened.previous_unclean {
                    self.start.message(format!(
                        "{name}: the previous session did not close; the world resumed at checkpoint {} (tick {}), and what came after it was lost. Its unpublished rows were reclaimed.",
                        live.opened.checkpoint.id.value(),
                        live.opened.checkpoint.tick.map_or("none".to_owned(), |tick| tick.value().to_string()),
                    ));
                }
                // Opening took a while; that time is not owed to the new session's clock.
                self.last_frame = Instant::now();
                self.phase = Phase::Running(Box::new(Running {
                    live,
                    surface,
                    renderer,
                    overlay,
                    play,
                    focus,
                    busy_frames: 0,
                    idle_waits: 0,
                    capture_requested: false,
                    capture: None,
                }));
            }
            Err(error) => {
                self.start.message(format!("{name}: {error}"));
                let report = live.close();
                self.start.message(ui::close_summary(&name, &report));
                self.open_menu();
            }
        }
    }

    fn compose(
        window: &Arc<Window>,
        live: &mut LiveWorld,
        surface: wgpu::Surface<'static>,
        vsync: bool,
        size: [u32; 2],
    ) -> Result<(Surface, Renderer, Overlay), String> {
        let gpu = live.gpu();
        let capabilities = gpu.surface_capabilities(&surface);
        let mut surface = Surface::new(surface, &capabilities, vsync)?;
        let view = gpu.view();
        surface.configure(view.device, size);
        let mut renderer =
            Renderer::new(&view, surface.format()).map_err(|error| error.to_string())?;
        if size[0] > 0 && size[1] > 0 {
            renderer
                .resize(view.device, view.queue, size)
                .map_err(|error| error.to_string())?;
        }
        let overlay = Overlay::new(window, view.device, surface.format());
        Ok((surface, renderer, overlay))
    }

    fn play_for(live: &LiveWorld) -> Play {
        let gpu = live.gpu_ref();
        let (tick_rate_hz, max_ticks_per_frame) = live.session.runtime().clock_settings();
        let window = live.opened.checkpoint.window;
        let settings = Settings {
            rules: gpu.rules(),
            ambient_c: gpu.ambient_temperature().degrees(),
            elevation_scale: gpu.elevation_scale().value(),
            tick_rate_hz,
            max_ticks_per_frame,
            radius: [window.radius_x(), window.radius_y()],
        };
        Play::new(Observer::above(window.center()), settings)
    }

    fn set_grab(&mut self, grab: bool) {
        if grab == self.grabbed {
            return;
        }
        let Some(window) = self.window.as_ref() else {
            return;
        };
        if grab {
            let locked = window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
            if locked.is_ok() {
                window.set_cursor_visible(false);
            }
        } else {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
        }
        self.grabbed = grab;
    }

    /// Whether a frame now would do anything. In an open world in a visible window, a frame
    /// that finds both presentation slots busy and no tick it could submit changes nothing: it
    /// would lay out the overlay and run the session's frame for a picture that is never drawn.
    /// Without vsync that is most loop iterations, so the loop waits instead; input, storage
    /// results and outcomes wait with it for the next frame, at most until the device finishes a
    /// frame or a tick falls due. Every frame that does run is a whole frame, input to
    /// presentation, so a driver sees the same frames a person's window runs. A closed or lost
    /// session, a hidden window and every other phase always have a frame to run.
    fn has_work(&mut self) -> bool {
        self.idle = false;
        let Phase::Running(running) = &mut self.phase else {
            return true;
        };
        if self.occluded || running.surface.size().is_none() {
            return true;
        }
        let ticks_due = running
            .live
            .session
            .runtime()
            .ticks_due(self.last_frame.elapsed());
        let admissible = running.live.gpu().poll_admission();
        let work = match admissible.presentation {
            Ok(()) | Err(PresentRefusal::Closed(_)) => true,
            Err(PresentRefusal::Busy) => admissible.tick && ticks_due > 0,
        };
        if !work {
            running.idle_waits += 1;
            self.idle = true;
        }
        work
    }

    /// One frame of the current phase.
    fn frame(&mut self) {
        let now = Instant::now();
        let elapsed = now - self.last_frame;
        self.last_frame = now;
        if self.frame_times.len() == FRAME_HISTORY {
            self.frame_times.pop_front();
        }
        self.frame_times.push_back(elapsed.as_secs_f64() * 1000.0);
        self.summary.frames += 1;
        if let Some(driver) = self.driver.as_mut() {
            let mut events = Vec::new();
            let phase = match &self.phase {
                Phase::Start => crate::driver::PhaseKind::Start,
                Phase::Running(_) => crate::driver::PhaseKind::Running,
                Phase::Closing { .. } => crate::driver::PhaseKind::Closing,
                Phase::Lost(_) => crate::driver::PhaseKind::Lost,
            };
            let running = match &mut self.phase {
                Phase::Running(running) => Some(running.as_mut()),
                _ => None,
            };
            let control = driver.before_frame(DriverFrame {
                frame: self.summary.frames,
                phase,
                running,
                events: &mut events,
                start: &mut self.start,
                hud: &mut self.hud,
            });
            for event in events {
                self.input.apply(event);
            }
            match control {
                crate::driver::Control::Continue => {}
                crate::driver::Control::Open { name, path } => {
                    self.open_world(name, path, None, None)
                }
                crate::driver::Control::Close => self.begin_close(false),
                crate::driver::Control::Quit => self.begin_close(true),
                crate::driver::Control::Resize(size) => {
                    // A request the window system applies at once (Wayland) sends no Resized
                    // event: the returned size is the new one.
                    if let Some(window) = &self.window
                        && let Some(applied) =
                            window.request_inner_size(PhysicalSize::new(size[0], size[1]))
                    {
                        self.pending_size = Some([applied.width, applied.height]);
                    }
                }
                crate::driver::Control::WindowSize(size) => self.pending_size = Some(size),
                crate::driver::Control::Start(action) => self.driven_start = Some(action),
                crate::driver::Control::Minimize(minimized) => {
                    if let Some(window) = &self.window {
                        window.set_minimized(minimized);
                    }
                }
            }
        }
        if let Some(size) = self.pending_size.take() {
            self.apply_size(size);
        }
        let phase = std::mem::replace(&mut self.phase, Phase::Start);
        self.phase = match phase {
            Phase::Start => {
                self.start_frame();
                std::mem::replace(&mut self.phase, Phase::Start)
            }
            Phase::Running(running) => self.running_frame(running, elapsed),
            Phase::Closing {
                running,
                exit_after,
                notice_drawn,
            } => self.closing_frame(running, exit_after, notice_drawn),
            Phase::Lost(lost) => self.lost_frame(lost),
        };
        if let Some(driver) = self.driver.as_mut() {
            let running = match &mut self.phase {
                Phase::Running(running) => Some(running.as_mut()),
                _ => None,
            };
            if driver.after_frame(running, &self.input) {
                self.begin_close(true);
            }
        }
        self.input.end_frame();
    }

    fn apply_size(&mut self, size: [u32; 2]) {
        match &mut self.phase {
            Phase::Running(running) | Phase::Closing { running, .. } => {
                let gpu = running.live.gpu();
                let view = gpu.view();
                running.surface.configure(view.device, size);
                if size[0] > 0
                    && size[1] > 0
                    && let Err(error) = running.renderer.resize(view.device, view.queue, size)
                {
                    self.summary.errors.push(error.to_string());
                }
            }
            Phase::Start | Phase::Lost(_) => {
                if let Some(menu) = self.menu.as_mut() {
                    menu.resize(size);
                }
            }
        }
    }

    fn start_frame(&mut self) {
        let Some(window) = self.window() else {
            return;
        };
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        let mut action = None;
        let start = &mut self.start;
        if let Err(error) = menu.frame(&window, |ui| action = ui::start_screen(ui, start)) {
            self.summary.errors.push(error);
        }
        if let Some(driven) = self.driven_start.take() {
            action = Some(driven);
        }
        match action {
            None => {}
            Some(StartAction::Quit) => self.quit = true,
            Some(StartAction::Refresh) => self.refresh_worlds(),
            Some(StartAction::ChooseAdapter(id)) => {
                if let Err(error) = options::remember_adapter(&self.options.settings, &id) {
                    self.start
                        .message(format!("The choice could not be saved: {error}"));
                }
                self.options.adapter = Some(id.clone());
                self.start.adapter = Some(id);
            }
            Some(StartAction::Create { name, seed }) => {
                match catalog::create(&self.options.worlds, &name, seed) {
                    Ok(path) => {
                        self.start
                            .message(format!("Created {} (seed {seed}).", path.display()));
                        self.refresh_worlds();
                        self.start.new_seed = catalog::fresh_seed().to_string();
                    }
                    Err(error) => self.start.message(error),
                }
            }
            Some(StartAction::Import {
                name,
                legacy,
                seed,
                scale,
            }) => {
                match catalog::import_legacy(&self.options.worlds, &name, &legacy, seed, scale) {
                    Ok(report) => self.start.message(format!(
                        "Imported {} rows from {} into {}{}; {} rows with z != 0 were left out.",
                        report.interpretation.rows_imported,
                        legacy.display(),
                        report.destination.display(),
                        report
                            .read_repaired_copy
                            .map_or(String::new(), |copy| format!(
                                " (read from a repaired copy, {})",
                                copy.display()
                            )),
                        report.refused_keys.len()
                    )),
                    Err(error) => self.start.message(format!("Import refused: {error}")),
                }
                self.refresh_worlds();
            }
            Some(StartAction::Open { name, path }) => self.open_world(name, path, None, None),
        }
    }

    fn status(&self, running: &Running) -> WorldStatus {
        let session = &running.live.session;
        let runtime = session.runtime();
        let mut sorted: Vec<f64> = self.frame_times.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);
        let p95 = sorted
            .get(sorted.len().saturating_sub(1) * 95 / 100)
            .copied()
            .unwrap_or(0.0);
        let mean = if sorted.is_empty() {
            0.0
        } else {
            sorted.iter().sum::<f64>() / sorted.len() as f64
        };
        let ages = &runtime.bodies().summary().sample_age_ticks;
        WorldStatus {
            name: running.live.name.clone(),
            epoch: u64::from(running.live.opened.epoch.value()),
            last_submitted: runtime.last_submitted_tick(),
            ledger: *runtime.ledger().summary(),
            counters: runtime.counters(),
            residency: *session.residency().summary(),
            refused_move: session.residency().refused_move(),
            failed_chunks: session.residency().failed().count(),
            latest: *session.checkpoints().latest(),
            receipt: session.checkpoints().receipt().cloned(),
            autosave: session.checkpoints().autosave_ticks(),
            previous_unclean: running.live.opened.previous_unclean,
            fps: if mean > 0.0 { 1000.0 / mean } else { 0.0 },
            frame_ms: self.frame_times.back().copied().unwrap_or(0.0),
            frame_p95_ms: p95,
            render: running.renderer.counters(),
            presentations_in_flight: running.live.gpu_ref().presentations_in_flight(),
            sample_age_ticks: ages.mean().unwrap_or(0.0),
            discarded_ticks: runtime.discarded_ticks(),
            pending_commands: runtime.pending_commands(),
            body_capacity: runtime.capabilities().body_capacity,
            bodies_tracked: runtime.bodies().summary().tracked,
        }
    }

    fn running_frame(&mut self, mut running: Box<Running>, elapsed: Duration) -> Phase {
        if running.live.session.loss().is_some() {
            return self.lose(running);
        }
        let Some(window) = self.window() else {
            return Phase::Running(running);
        };
        let status = self.status(&running);
        let mut actions = Vec::new();
        {
            let Running {
                live,
                overlay,
                play,
                ..
            } = running.as_mut();
            let gpu = live.gpu();
            let view = gpu.view();
            let hud = &mut self.hud;
            overlay.run(&window, view.device, view.queue, |ui| {
                actions = ui::world_overlay(ui, hud, play, &status);
            });
        }
        let ctx = running.overlay.ctx.clone();
        let focus = Focus {
            pointer: running.play.looking() || !ctx.egui_wants_pointer_input(),
            keyboard: !ctx.egui_wants_keyboard_input(),
        };
        if focus.keyboard {
            if self.input.pressed(Key::Backquote) || self.input.pressed(Key::F12) {
                self.hud.visible = !self.hud.visible;
            }
            if self.input.pressed(Key::F3) {
                self.hud.debug.activity = !self.hud.debug.activity;
                self.hud.debug.bodies = self.hud.debug.activity;
            }
            if self.input.pressed(Key::F5) {
                actions.push(HudAction::Save);
            }
        }
        let mut close = false;
        for action in actions {
            let session = &mut running.live.session;
            match action {
                HudAction::Save => {
                    session.save();
                }
                HudAction::Close => close = true,
                HudAction::ApplySettings => running.play.apply_settings(session),
                HudAction::Resize => running.play.resize_window(session),
                HudAction::SetAutosave(ticks) => session.set_autosave(Some(ticks)),
            }
        }
        running
            .play
            .intend(&mut running.live.session, &self.input, focus, elapsed);
        let looking = running.play.looking();
        self.set_grab(looking);
        if running.play.observer.attached {
            let chunk = running.play.observer.chunk();
            if chunk != running.focus && running.live.session.set_focus(chunk).is_ok() {
                running.focus = chunk;
            }
        }
        running.live.session.frame(elapsed);
        running.play.observe(&mut running.live.session);
        self.render(&mut running, &window);
        if close {
            self.set_grab(false);
            return Phase::Closing {
                running,
                exit_after: false,
                notice_drawn: false,
            };
        }
        if self.quit {
            self.quit = false;
            self.set_grab(false);
            return Phase::Closing {
                running,
                exit_after: true,
                notice_drawn: false,
            };
        }
        Phase::Running(running)
    }

    fn render(&mut self, running: &mut Running, window: &Arc<Window>) {
        let size = match running.surface.size() {
            Some(size) if !self.occluded => size,
            _ => {
                running.overlay.finish();
                return;
            }
        };
        let gpu = running.live.gpu();
        match gpu.presentation_ready() {
            Ok(()) => {}
            Err(PresentRefusal::Busy) => {
                running.busy_frames += 1;
                running.overlay.finish();
                return;
            }
            Err(PresentRefusal::Closed(_)) => {
                running.overlay.finish();
                return;
            }
        }
        let texture = match running.surface.acquire(gpu.view().device) {
            Acquired::Frame(texture) => texture,
            Acquired::Skip(_) => {
                running.overlay.finish();
                return;
            }
            Acquired::Lost => {
                match gpu.create_surface(wgpu::SurfaceTarget::from(Arc::clone(window))) {
                    Ok(surface) => running.surface.replace(gpu.view().device, surface),
                    Err(error) => self.summary.errors.push(format!("{error:?}")),
                }
                running.overlay.finish();
                return;
            }
            Acquired::Failed(reason) => {
                self.summary.errors.push(reason);
                running.overlay.finish();
                return;
            }
        };
        let target = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let play = &running.play;
        let shadow_focus = play.observer.position;
        let request = FrameRequest {
            camera: play.observer.rig(),
            fov_y: play.fov_y,
            near: DEFAULT_NEAR,
            lighting: self.hud.lighting,
            cursor: play.cursor_preview(),
            overlay: self.hud.debug,
            seconds: self.started.elapsed().as_secs_f32(),
            shadow_focus,
        };
        running.play.viewport = size;
        if running.capture_requested {
            running.capture_requested = false;
            let format = running.surface.format();
            let tick = running.live.session.runtime().last_submitted_tick();
            let gpu = running.live.gpu();
            let captured = capture_frame(
                gpu,
                &mut running.renderer,
                &mut running.overlay,
                &request,
                format,
                size,
            )
            .map(|(size, rgba)| Capture { size, rgba, tick });
            running.capture = Some(captured);
            // The readback waited for the device: not frame time the clock owes ticks for.
            self.last_frame = Instant::now();
        }
        let gpu = running.live.gpu();
        let renderer = &mut running.renderer;
        let overlay = &mut running.overlay;
        let mut failure = None;
        let submitted = gpu.submit_presentation("frame", |view, encoder| {
            if let Err(error) = renderer.encode(view, &request, encoder, &target) {
                failure = Some(error.to_string());
            }
            overlay.encode(view.device, view.queue, encoder, &target, size);
        });
        if let Some(failure) = failure {
            self.summary.errors.push(failure);
        }
        if submitted.is_ok() {
            window.pre_present_notify();
            texture.present();
            running.surface.presented_frames += 1;
        }
        running.overlay.finish();
    }

    fn begin_close(&mut self, exit_after: bool) {
        self.set_grab(false);
        let phase = std::mem::replace(&mut self.phase, Phase::Start);
        self.phase = match phase {
            Phase::Running(running) => Phase::Closing {
                running,
                exit_after,
                notice_drawn: false,
            },
            other => {
                if exit_after {
                    self.quit = true;
                }
                other
            }
        };
    }

    fn closing_frame(
        &mut self,
        mut running: Box<Running>,
        exit_after: bool,
        notice_drawn: bool,
    ) -> Phase {
        if !notice_drawn {
            if let Some(window) = self.window() {
                let Running {
                    live,
                    overlay,
                    surface,
                    ..
                } = running.as_mut();
                let gpu = live.gpu();
                {
                    let view = gpu.view();
                    overlay.run(&window, view.device, view.queue, |ui| {
                        ui::notice(ui, "Saving and closing the world…");
                    });
                }
                if gpu.presentation_ready().is_ok()
                    && let Acquired::Frame(texture) = surface.acquire(gpu.view().device)
                {
                    let target = texture
                        .texture
                        .create_view(&wgpu::TextureViewDescriptor::default());
                    let size = surface.size().unwrap_or([1, 1]);
                    if gpu
                        .submit_presentation("closing", |view, encoder| {
                            overlay.encode(view.device, view.queue, encoder, &target, size)
                        })
                        .is_ok()
                    {
                        texture.present();
                    }
                }
                overlay.finish();
            }
            return Phase::Closing {
                running,
                exit_after,
                notice_drawn: true,
            };
        }
        let live = running.into_live();
        let name = live.name.clone();
        let report = live.close();
        let summary = ui::close_summary(&name, &report);
        eprintln!("morbid: {summary}");
        self.summary.closes.push(summary.clone());
        self.start.message(summary);
        if let Some(window) = &self.window {
            window.set_title("Morbid Monarchy");
        }
        if exit_after {
            self.quit = true;
        } else {
            self.refresh_worlds();
            self.open_menu();
        }
        Phase::Start
    }

    fn lose(&mut self, running: Box<Running>) -> Phase {
        self.set_grab(false);
        let live = running.into_live();
        let lost = live.finish_after_loss();
        let text = format!(
            "{}: the device was lost; the latest complete checkpoint is {} at tick {:?}, {} ticks lost{}.",
            lost.name,
            lost.report.last_complete.value(),
            lost.report.last_complete_tick.map(|tick| tick.value()),
            lost.report.ticks_lost,
            lost.report
                .autosave_ticks
                .map_or(String::new(), |every| format!(
                    " (autosave every {every} ticks)"
                )),
        );
        eprintln!("morbid: {text}");
        self.summary.losses.push(text.clone());
        self.start.message(text);
        self.open_menu();
        Phase::Lost(Box::new(lost))
    }

    fn lost_frame(&mut self, lost: Box<LostWorld>) -> Phase {
        let Some(window) = self.window() else {
            return Phase::Lost(lost);
        };
        let mut action = None;
        if let Some(menu) = self.menu.as_mut()
            && let Err(error) = menu.frame(&window, |ui| {
                action = ui::loss_dialog(ui, &lost.name, &lost.report);
            })
        {
            self.summary.errors.push(error);
        }
        if let Some(driver) = self.driver.as_mut()
            && driver.recover_after_loss()
        {
            action = Some(LossAction::Recover);
        }
        match action {
            None => Phase::Lost(lost),
            Some(LossAction::Close) => {
                self.refresh_worlds();
                Phase::Start
            }
            Some(LossAction::Recover) => {
                let LostWorld {
                    name,
                    path,
                    store,
                    epoch,
                    ..
                } = *lost;
                self.open_world(name, path, Some(store), Some(epoch));
                std::mem::replace(&mut self.phase, Phase::Start)
            }
        }
    }

    /// Closes an open world in order before the process exits.
    fn shutdown(mut self) -> RunSummary {
        self.set_grab(false);
        let phase = std::mem::replace(&mut self.phase, Phase::Start);
        if let Phase::Running(running) | Phase::Closing { running, .. } = phase {
            let live = running.into_live();
            let name = live.name.clone();
            let summary = ui::close_summary(&name, &live.close());
            eprintln!("morbid: {summary}");
            self.summary.closes.push(summary);
        }
        self.summary
    }
}

impl ApplicationHandler for Client {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attributes = Window::default_attributes().with_title("Morbid Monarchy");
        if let Some([width, height]) = self.options.window_size {
            attributes = attributes.with_inner_size(PhysicalSize::new(width, height));
        }
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                let text = format!("the window could not be created: {error}");
                eprintln!("morbid: {text}");
                self.summary.errors.push(text);
                self.quit = true;
                return;
            }
        };
        self.start.adapters = gpu_world::context::presentable_adapters(wgpu::SurfaceTarget::from(
            Arc::clone(&window),
        ));
        if let Some(id) = &self.options.adapter
            && !self.start.adapters.iter().any(|record| &record.id == id)
        {
            self.start.message(format!(
                "Adapter {id} cannot present to this window or does not exist; choose another."
            ));
            self.options.adapter = None;
            self.start.adapter = None;
        }
        self.window = Some(window);
        self.refresh_worlds();
        match self.options.open.clone() {
            Some(open) if self.options.adapter.is_some() => {
                let path = if open.components().count() == 1 && open.extension().is_none() {
                    catalog::world_path(&self.options.worlds, &open.to_string_lossy())
                } else {
                    open
                };
                let name = path.file_stem().map_or_else(
                    || "world".to_owned(),
                    |stem| stem.to_string_lossy().into_owned(),
                );
                self.open_world(name, path, None, None);
            }
            _ => self.open_menu(),
        }
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(window) = self.window() else {
            return;
        };
        let overlay = match &mut self.phase {
            Phase::Running(running) | Phase::Closing { running, .. } => Some(&mut running.overlay),
            Phase::Start | Phase::Lost(_) => self.menu.as_mut().map(|menu| &mut menu.overlay),
        };
        if let Some(overlay) = overlay {
            let _ = overlay.state.on_window_event(&window, &event);
        }
        if let Some(event) = input::from_window_event(&event) {
            self.input.apply(event);
        }
        match event {
            WindowEvent::CloseRequested => self.begin_close(true),
            WindowEvent::Resized(size) => self.pending_size = Some([size.width, size.height]),
            WindowEvent::Occluded(occluded) => self.occluded = occluded,
            WindowEvent::Focused(false) => {
                self.input.apply(InputEvent::FocusLost);
                self.set_grab(false);
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let Some(event) = input::from_device_event(&event) {
            self.input.apply(event);
        }
    }
}

/// Draws the frame into an offscreen texture of the surface's format and reads it back: a
/// maintenance wait on the world's timeline, for evidence only.
pub(crate) fn capture_frame(
    gpu: &mut gpu_world::world::GpuWorld,
    renderer: &mut Renderer,
    overlay: &mut Overlay,
    request: &FrameRequest,
    format: wgpu::TextureFormat,
    size: [u32; 2],
) -> Result<([u32; 2], Vec<u8>), String> {
    let device = gpu.view().device.clone();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("capture"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let row = (size[0] * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("capture readback"),
        size: u64::from(row) * u64::from(size[1]),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut failure = None;
    gpu.submit_presentation("capture", |view, encoder| {
        if let Err(error) = renderer.encode(view, request, encoder, &target) {
            failure = Some(error.to_string());
        }
        overlay.encode(view.device, view.queue, encoder, &target, size);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(size[1]),
                },
            },
            wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
        );
    })
    .map_err(|refusal| format!("capture refused: {refusal:?}"))?;
    if let Some(failure) = failure {
        return Err(failure);
    }
    let mapped = Arc::new(std::sync::atomic::AtomicU8::new(0));
    let flag = Arc::clone(&mapped);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            flag.store(
                if result.is_ok() { 1 } else { 2 },
                std::sync::atomic::Ordering::Release,
            );
        });
    let deadline = Instant::now() + Duration::from_secs(10);
    while mapped.load(std::sync::atomic::Ordering::Acquire) == 0 && Instant::now() < deadline {
        gpu.wait_idle(Duration::from_millis(500));
    }
    if mapped.load(std::sync::atomic::Ordering::Acquire) != 1 {
        return Err("the capture could not be read back".to_owned());
    }
    let bgra = matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    );
    let mut rgba = Vec::with_capacity((size[0] * size[1] * 4) as usize);
    {
        let bytes = buffer.slice(..).get_mapped_range();
        for y in 0..size[1] as usize {
            let line = &bytes[y * row as usize..][..size[0] as usize * 4];
            for pixel in line.as_chunks::<4>().0 {
                if bgra {
                    rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
                } else {
                    rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
                }
            }
        }
    }
    buffer.unmap();
    Ok((size, rgba))
}

/// Runs the client until the person quits or the driver says so; closes any open world in
/// order first.
pub fn run(options: ClientOptions, driver: Option<Box<dyn Driver>>) -> Result<RunSummary, String> {
    let mut event_loop = EventLoop::new().map_err(|error| format!("no event loop: {error}"))?;
    let mut client = Client::new(options, driver);
    loop {
        let wait = client.wait_hint();
        if let PumpStatus::Exit(_) = event_loop.pump_app_events(Some(wait), &mut client) {
            break;
        }
        if client.quit {
            break;
        }
        if client.window.is_some() && client.has_work() {
            client.frame();
        }
        if client.quit {
            break;
        }
    }
    Ok(client.shutdown())
}
