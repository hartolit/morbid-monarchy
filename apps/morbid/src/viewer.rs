//! A window that shows a session someone else drives: the scripted scenarios of the evidence
//! runner (`resident-world --native`) open and close their own sessions over their own stores,
//! and the viewer draws each one with the client's renderer and overlay, on that session's
//! device and timeline. It is the client's presentation without its tools.
//!
//! ```text
//! Viewer::open                      the window (an explicitly pumped event loop)
//! viewer.target()                   the window for GpuWorld::open_presenting
//! viewer.attach(gpu, surface)       renderer and overlay on the session's device
//! viewer.present(session)           at most one frame per display interval; never blocks
//! viewer.detach()                   before the session goes (its surface first)
//! ```

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use gpu_world::world::GpuWorld;
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    platform::pump_events::{EventLoopExtPumpEvents, PumpStatus},
    window::{Window, WindowId},
};
use world_model::{glam::DVec3, spatial::CHUNK_SIZE};
use world_render::{CameraRig, FrameRequest, Lighting, Overlay as DebugOverlay, Renderer};
use world_runtime::session::WorldSession;

use crate::{
    client::Capture,
    present::{Acquired, Overlay, Surface},
};

/// Frames are presented no faster than this, so a scenario running ticks as fast as the device
/// takes them is not paced by the display.
const PRESENT_INTERVAL: Duration = Duration::from_millis(16);

struct ViewerWindow {
    window: Option<Arc<Window>>,
    size: PhysicalSize<u32>,
    resized: Option<[u32; 2]>,
    close_requested: bool,
    failure: Option<String>,
}

impl ApplicationHandler for ViewerWindow {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Morbid Monarchy — scenario")
            .with_inner_size(self.size);
        match event_loop.create_window(attributes) {
            Ok(window) => self.window = Some(Arc::new(window)),
            Err(error) => self.failure = Some(format!("the window could not be created: {error}")),
        }
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::Resized(size) => self.resized = Some([size.width, size.height]),
            WindowEvent::CloseRequested => self.close_requested = true,
            _ => {}
        }
    }
}

struct Gfx {
    surface: Surface,
    renderer: Renderer,
    overlay: Overlay,
    device: wgpu::Device,
}

pub struct Viewer {
    event_loop: EventLoop<()>,
    window: ViewerWindow,
    gfx: Option<Gfx>,
    caption: String,
    last_present: Option<Instant>,
    started: Instant,
    pub presented: u64,
    pub skipped: u64,
}

impl Viewer {
    /// Opens the window (pumping the event loop until it exists).
    pub fn open(size: [u32; 2]) -> Result<Self, String> {
        let event_loop = EventLoop::new().map_err(|error| format!("no event loop: {error}"))?;
        let mut viewer = Self {
            event_loop,
            window: ViewerWindow {
                window: None,
                size: PhysicalSize::new(size[0], size[1]),
                resized: None,
                close_requested: false,
                failure: None,
            },
            gfx: None,
            caption: String::new(),
            last_present: None,
            started: Instant::now(),
            presented: 0,
            skipped: 0,
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while viewer.window.window.is_none() && Instant::now() < deadline {
            viewer.pump(Duration::from_millis(10))?;
            if let Some(failure) = viewer.window.failure.take() {
                return Err(failure);
            }
        }
        viewer
            .window
            .window
            .as_ref()
            .map(|_| ())
            .ok_or_else(|| "the window did not appear".to_owned())?;
        Ok(viewer)
    }

    fn pump(&mut self, wait: Duration) -> Result<(), String> {
        match self
            .event_loop
            .pump_app_events(Some(wait), &mut self.window)
        {
            PumpStatus::Continue => Ok(()),
            PumpStatus::Exit(code) => Err(format!("the event loop exited ({code})")),
        }
    }

    fn arc(&self) -> Result<Arc<Window>, String> {
        self.window
            .window
            .clone()
            .ok_or_else(|| "no window".to_owned())
    }

    /// The window, for `GpuWorld::open_presenting`. Detach the previous session first.
    pub fn target(&self) -> Result<wgpu::SurfaceTarget<'static>, String> {
        Ok(wgpu::SurfaceTarget::from(self.arc()?))
    }

    /// Whether the person closed the window.
    pub fn close_requested(&self) -> bool {
        self.window.close_requested
    }

    /// Text drawn over the scene: what the driver is doing.
    pub fn set_caption(&mut self, caption: impl Into<String>) {
        self.caption = caption.into();
    }

    /// Builds the renderer and overlay on `gpu`'s device, presenting into `surface`.
    pub fn attach(
        &mut self,
        gpu: &mut GpuWorld,
        surface: wgpu::Surface<'static>,
    ) -> Result<(), String> {
        self.detach();
        let window = self.arc()?;
        let capabilities = gpu.surface_capabilities(&surface);
        // Unpaced: the scenario's ticks set the pace, and frames are skipped instead.
        let mut surface = Surface::new(surface, &capabilities, false)?;
        let view = gpu.view();
        let size = window.inner_size();
        let size = [size.width, size.height];
        surface.configure(view.device, size);
        let mut renderer =
            Renderer::new(&view, surface.format()).map_err(|error| error.to_string())?;
        if size[0] > 0 && size[1] > 0 {
            renderer
                .resize(view.device, view.queue, size)
                .map_err(|error| error.to_string())?;
        }
        let overlay = Overlay::new(&window, view.device, surface.format());
        self.gfx = Some(Gfx {
            surface,
            renderer,
            overlay,
            device: view.device.clone(),
        });
        Ok(())
    }

    /// Releases the session's surface, renderer and overlay (after its last frame finished).
    pub fn detach(&mut self) {
        if let Some(gfx) = self.gfx.take() {
            let _ = gfx.device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(2)),
            });
            let Gfx {
                surface,
                renderer,
                overlay,
                ..
            } = gfx;
            drop(overlay);
            drop(renderer);
            drop(surface);
        }
    }

    /// The camera over the resident window: north up, looking down at its centre chunk from
    /// close enough that the fog (fitted to the window's size) leaves it clear.
    fn camera(session: &WorldSession) -> CameraRig {
        let window = session.residency().window();
        let centre = window.center().origin_cell();
        let half = CHUNK_SIZE as f64 / 2.0;
        CameraRig::Free {
            position: DVec3::new(
                f64::from(centre.x) + half,
                95.0,
                -(f64::from(centre.y) + half) + 80.0,
            ),
            yaw: 0.0,
            pitch: (-45f32).to_radians(),
        }
    }

    fn request(&self, session: &WorldSession) -> FrameRequest {
        let camera = Self::camera(session);
        let CameraRig::Free { position, .. } = camera else {
            unreachable!("the viewer's camera is free")
        };
        FrameRequest {
            camera,
            fov_y: world_render::frame::DEFAULT_FOV_Y_DEGREES.to_radians(),
            near: world_render::frame::DEFAULT_NEAR,
            lighting: Lighting::default(),
            cursor: None,
            overlay: DebugOverlay::default(),
            seconds: self.started.elapsed().as_secs_f32(),
            shadow_focus: position - DVec3::new(0.0, 80.0, 80.0),
        }
    }

    /// Draws `session` if a display interval passed and the device has a presentation slot;
    /// otherwise counts a skipped frame. Never waits for the device.
    pub fn present(&mut self, session: &mut WorldSession) -> Result<(), String> {
        self.pump(Duration::ZERO)?;
        if self
            .last_present
            .is_some_and(|last| last.elapsed() < PRESENT_INTERVAL)
        {
            return Ok(());
        }
        let window = self.arc()?;
        if let Some(size) = self.window.resized.take()
            && let Some(gfx) = self.gfx.as_mut()
        {
            let gpu = session
                .runtime_mut()
                .port_as::<GpuWorld>()
                .ok_or("the session's world is not a GpuWorld")?;
            let view = gpu.view();
            gfx.surface.configure(view.device, size);
            if size[0] > 0 && size[1] > 0 {
                gfx.renderer
                    .resize(view.device, view.queue, size)
                    .map_err(|error| error.to_string())?;
            }
        }
        let request = self.request(session);
        let status = status_text(session, &self.caption);
        let Some(gfx) = self.gfx.as_mut() else {
            return Ok(());
        };
        let Some(size) = gfx.surface.size() else {
            self.skipped += 1;
            return Ok(());
        };
        let gpu = session
            .runtime_mut()
            .port_as::<GpuWorld>()
            .ok_or("the session's world is not a GpuWorld")?;
        if gpu.presentation_ready().is_err() {
            self.skipped += 1;
            return Ok(());
        }
        {
            let view = gpu.view();
            gfx.overlay.run(&window, view.device, view.queue, |ui| {
                egui::Area::new(egui::Id::new("scenario caption"))
                    .fixed_pos(egui::pos2(12.0, 12.0))
                    .show(ui.ctx(), |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label(egui::RichText::new(&status).monospace());
                        });
                    });
            });
        }
        let texture = match gfx.surface.acquire(gpu.view().device) {
            Acquired::Frame(texture) => texture,
            Acquired::Skip(_) | Acquired::Lost | Acquired::Failed(_) => {
                gfx.overlay.finish();
                self.skipped += 1;
                return Ok(());
            }
        };
        let target = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let (renderer, overlay) = (&mut gfx.renderer, &mut gfx.overlay);
        let mut failure = None;
        let submitted = gpu.submit_presentation("scenario frame", |view, encoder| {
            if let Err(error) = renderer.encode(view, &request, encoder, &target) {
                failure = Some(error.to_string());
            }
            overlay.encode(view.device, view.queue, encoder, &target, size);
        });
        if submitted.is_ok() {
            window.pre_present_notify();
            texture.present();
            self.presented += 1;
            self.last_present = Some(Instant::now());
        }
        gfx.overlay.finish();
        failure.map_or(Ok(()), Err)
    }

    /// The next frame's pixels (a maintenance readback), with the caption.
    pub fn capture(&mut self, session: &mut WorldSession) -> Result<Capture, String> {
        let request = self.request(session);
        let tick = session.runtime().last_submitted_tick();
        let window = self.arc()?;
        let status = status_text(session, &self.caption);
        let gfx = self.gfx.as_mut().ok_or("no session attached")?;
        let size = gfx.surface.size().ok_or("the window has no size")?;
        let format = gfx.surface.format();
        let gpu = session
            .runtime_mut()
            .port_as::<GpuWorld>()
            .ok_or("the session's world is not a GpuWorld")?;
        {
            let view = gpu.view();
            gfx.overlay.run(&window, view.device, view.queue, |ui| {
                egui::Area::new(egui::Id::new("scenario caption"))
                    .fixed_pos(egui::pos2(12.0, 12.0))
                    .show(ui.ctx(), |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label(egui::RichText::new(&status).monospace());
                        });
                    });
            });
        }
        let captured = crate::client::capture_frame(
            gpu,
            &mut gfx.renderer,
            &mut gfx.overlay,
            &request,
            format,
            size,
        );
        gfx.overlay.finish();
        captured.map(|(size, rgba)| Capture { size, rgba, tick })
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.detach();
    }
}

fn status_text(session: &WorldSession, caption: &str) -> String {
    let runtime = session.runtime();
    let residency = session.residency().summary();
    let window = session.residency().window();
    let latest = session.checkpoints().latest();
    format!(
        "{caption}\nsession {:?}  tick {}\nwindow centre {:?} radius {}x{}  resident {} (loading {}, evicting {})\nwrites {} committed, {} pending, {} failed\ncheckpoint {} at tick {:?}  spheres {}",
        runtime.session().epoch,
        runtime
            .last_submitted_tick()
            .map_or_else(|| "none".to_owned(), |tick| tick.value().to_string()),
        window.center().to_ivec2(),
        window.radius_x(),
        window.radius_y(),
        residency.resident,
        residency.loading,
        residency.evicting,
        residency.writes_committed,
        residency.writes_pending,
        residency.write_failures,
        latest.id.value(),
        latest.tick.map(|tick| tick.value()),
        runtime.bodies().summary().tracked,
    )
}
