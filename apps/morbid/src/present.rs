//! Presentation: the window's surface, the overlay (egui) and, while a world is open, the world
//! renderer, all on the device that owns the world.
//!
//! While no world is open (the start screen, after a close or a device loss), a menu device
//! draws the overlay alone. It holds no world and never exists beside a world's device: opening
//! a world drops it first, since a window has one surface at a time.
//!
//! Surface failures are handled per frame: an outdated or lost surface is reconfigured, a
//! timeout or an occluded window skips the frame, and a zero-sized (minimized) window is never
//! configured or drawn; the client waits for its next tick instead of spinning.

use std::sync::Arc;

use futures_lite::future::block_on;
use winit::window::Window;

/// The window's surface and its configuration.
pub struct Surface {
    surface: wgpu::Surface<'static>,
    config: Option<wgpu::SurfaceConfiguration>,
    format: wgpu::TextureFormat,
    present_mode: wgpu::PresentMode,
    alpha: wgpu::CompositeAlphaMode,
    /// The last frame was suboptimal: reconfigure before the next one, once its texture is gone.
    stale: bool,
    pub reconfigurations: u64,
    pub skipped_frames: u64,
    /// Frames handed to the presentation engine.
    pub presented_frames: u64,
}

/// A frame to draw into, or why there is none.
pub enum Acquired {
    Frame(wgpu::SurfaceTexture),
    /// No frame this time (minimized, occluded, timed out, or just reconfigured).
    Skip(&'static str),
    /// The surface is gone and must be created again on its instance ([`Surface::replace`]).
    Lost,
    /// The surface reported a validation failure: a defect, not a passing condition.
    Failed(String),
}

impl Surface {
    /// Chooses an sRGB format when the surface offers one, and FIFO (vsync) or, without it,
    /// mailbox before immediate: mailbox never tears, and on KWin's Wayland session with the
    /// NVIDIA driver acquiring an immediate-mode image waited 10 ms on every presented frame,
    /// where mailbox waited on about 15% of them (06 measurement).
    pub fn new(
        surface: wgpu::Surface<'static>,
        capabilities: &wgpu::SurfaceCapabilities,
        vsync: bool,
    ) -> Result<Self, String> {
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| {
                matches!(
                    format,
                    wgpu::TextureFormat::Bgra8UnormSrgb | wgpu::TextureFormat::Rgba8UnormSrgb
                )
            })
            .or_else(|| {
                capabilities.formats.iter().copied().find(|format| {
                    matches!(
                        format,
                        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
                    )
                })
            })
            .ok_or_else(|| format!("no supported surface format in {:?}", capabilities.formats))?;
        let modes = &capabilities.present_modes;
        let present_mode = if vsync {
            wgpu::PresentMode::Fifo
        } else if modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else if modes.contains(&wgpu::PresentMode::Immediate) {
            wgpu::PresentMode::Immediate
        } else {
            wgpu::PresentMode::Fifo
        };
        let alpha = capabilities
            .alpha_modes
            .first()
            .copied()
            .unwrap_or(wgpu::CompositeAlphaMode::Auto);
        Ok(Self {
            surface,
            config: None,
            format,
            present_mode,
            alpha,
            stale: false,
            reconfigurations: 0,
            skipped_frames: 0,
            presented_frames: 0,
        })
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    pub fn present_mode(&self) -> wgpu::PresentMode {
        self.present_mode
    }

    /// The configured size, or `None` while minimized.
    pub fn size(&self) -> Option<[u32; 2]> {
        self.config
            .as_ref()
            .map(|config| [config.width, config.height])
    }

    /// Configures the surface for `size`; a zero size unconfigures it (nothing is allocated).
    pub fn configure(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if size[0] == 0 || size[1] == 0 {
            self.config = None;
            return;
        }
        if self.size() == Some(size) {
            return;
        }
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: self.format,
            width: size[0],
            height: size[1],
            present_mode: self.present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: self.alpha,
            view_formats: vec![],
        };
        self.surface.configure(device, &config);
        self.config = Some(config);
        self.reconfigurations += 1;
    }

    /// A surface created again after [`Acquired::Lost`], configured as the old one was.
    pub fn replace(&mut self, device: &wgpu::Device, surface: wgpu::Surface<'static>) {
        self.surface = surface;
        let size = self.size();
        self.config = None;
        if let Some(size) = size {
            self.configure(device, size);
        }
    }

    /// The next frame's texture. An outdated surface is reconfigured for the next frame.
    pub fn acquire(&mut self, device: &wgpu::Device) -> Acquired {
        let Some(config) = self.config.clone() else {
            self.skipped_frames += 1;
            return Acquired::Skip("minimized");
        };
        if self.stale {
            self.stale = false;
            self.surface.configure(device, &config);
            self.reconfigurations += 1;
        }
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => Acquired::Frame(texture),
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => {
                // Present this one; the next frame gets a fresh configuration.
                self.stale = true;
                Acquired::Frame(texture)
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(device, &config);
                self.reconfigurations += 1;
                self.skipped_frames += 1;
                Acquired::Skip("surface reconfigured")
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.skipped_frames += 1;
                Acquired::Lost
            }
            wgpu::CurrentSurfaceTexture::Timeout => {
                self.skipped_frames += 1;
                Acquired::Skip("timed out")
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                self.skipped_frames += 1;
                Acquired::Skip("occluded")
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                Acquired::Failed("the surface reported a validation error".to_owned())
            }
        }
    }
}

/// The overlay: egui's context, its window input and its renderer on one device.
pub struct Overlay {
    pub ctx: egui::Context,
    pub state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    paint: Vec<egui::ClippedPrimitive>,
    free: Vec<egui::TextureId>,
    pixels_per_point: f32,
}

impl Overlay {
    /// A fresh overlay for `device`. egui's textures live on one device, so a new device gets a
    /// new context; the client's own state (panels, tools) is kept elsewhere.
    pub fn new(window: &Window, device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let ctx = egui::Context::default();
        let state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        let renderer =
            egui_wgpu::Renderer::new(device, format, egui_wgpu::RendererOptions::default());
        Self {
            ctx,
            state,
            renderer,
            paint: Vec::new(),
            free: Vec::new(),
            pixels_per_point: window.scale_factor() as f32,
        }
    }

    /// Runs the overlay's interface for this frame and prepares its triangles and textures.
    pub fn run(
        &mut self,
        window: &Window,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        build: impl FnMut(&mut egui::Ui),
    ) {
        let input = self.state.take_egui_input(window);
        let output = self.ctx.run_ui(input, build);
        self.state
            .handle_platform_output(window, output.platform_output);
        self.pixels_per_point = output.pixels_per_point;
        self.paint = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        for (id, delta) in &output.textures_delta.set {
            self.renderer.update_texture(device, queue, *id, delta);
        }
        self.free = output.textures_delta.free;
    }

    /// Draws the prepared overlay over `target`.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: [u32; 2],
    ) {
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: size,
            pixels_per_point: self.pixels_per_point,
        };
        let extra = self
            .renderer
            .update_buffers(device, queue, encoder, &self.paint, &screen);
        debug_assert!(extra.is_empty(), "the overlay registers no paint callbacks");
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("overlay"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        self.renderer
            .render(&mut pass.forget_lifetime(), &self.paint, &screen);
    }

    /// Frees the textures egui released this frame, after the frame that last used them.
    pub fn finish(&mut self) {
        for id in self.free.drain(..) {
            self.renderer.free_texture(&id);
        }
    }
}

/// The device that draws the overlay while no world is open.
pub struct MenuGfx {
    instance: wgpu::Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pub surface: Surface,
    pub overlay: Overlay,
    pub adapter_name: String,
}

impl Drop for MenuGfx {
    /// Lets the device finish its last frames before the surface goes, so the window's next
    /// surface (a world's) does not meet a swapchain that still exists.
    fn drop(&mut self) {
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(2)),
        });
    }
}

impl MenuGfx {
    /// Opens the first hardware adapter that can present to `window`: any will do for menus,
    /// which hold no world. The world's adapter is the user's explicit choice instead.
    pub fn open(
        window: &Arc<Window>,
        preferred: Option<&str>,
        vsync: bool,
    ) -> Result<Self, String> {
        // The primary backends only (as the world's adapters are offered): a GL surface of the
        // window would outlive this one and collide with the world's.
        let backends = wgpu::Backends::PRIMARY;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = instance
            .create_surface(Arc::clone(window))
            .map_err(|error| format!("the window has no surface: {error}"))?;
        let adapters = block_on(instance.enumerate_adapters(backends));
        let presentable = |adapter: &wgpu::Adapter| {
            adapter.get_info().device_type != wgpu::DeviceType::Cpu
                && adapter.is_surface_supported(&surface)
        };
        let identifier = |adapter: &wgpu::Adapter| {
            let info = adapter.get_info();
            format!(
                "{}-{:04x}-{:04x}",
                format!("{:?}", info.backend).to_lowercase(),
                info.vendor,
                info.device
            )
        };
        let adapter = adapters
            .iter()
            .find(|adapter| {
                preferred.is_some_and(|id| identifier(adapter) == id) && presentable(adapter)
            })
            .or_else(|| adapters.iter().find(|adapter| presentable(adapter)))
            .or_else(|| {
                adapters
                    .iter()
                    .find(|adapter| adapter.is_surface_supported(&surface))
            })
            .ok_or("no adapter can present to this window")?;
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("menu"),
            ..wgpu::DeviceDescriptor::default()
        }))
        .map_err(|error| format!("the menu device was refused: {error}"))?;
        let capabilities = surface.get_capabilities(adapter);
        let mut surface = Surface::new(surface, &capabilities, vsync)?;
        let size = window.inner_size();
        surface.configure(&device, [size.width, size.height]);
        let overlay = Overlay::new(window, &device, surface.format());
        Ok(Self {
            adapter_name: adapter.get_info().name,
            instance,
            device,
            queue,
            surface,
            overlay,
        })
    }

    pub fn resize(&mut self, size: [u32; 2]) {
        self.surface.configure(&self.device, size);
    }

    /// Draws one overlay frame; `build` lays out the interface.
    pub fn frame(
        &mut self,
        window_handle: &Arc<Window>,
        build: impl FnMut(&mut egui::Ui),
    ) -> Result<(), String> {
        let window: &Window = window_handle;
        let _ = self.device.poll(wgpu::PollType::Poll);
        self.overlay.run(window, &self.device, &self.queue, build);
        let texture = match self.surface.acquire(&self.device) {
            Acquired::Frame(texture) => texture,
            Acquired::Skip(_) => {
                self.overlay.finish();
                return Ok(());
            }
            Acquired::Lost => {
                let surface = self
                    .instance
                    .create_surface(Arc::clone(window_handle))
                    .map_err(|error| {
                        format!("the window's surface could not be recreated: {error}")
                    })?;
                self.surface.replace(&self.device, surface);
                self.overlay.finish();
                return Ok(());
            }
            Acquired::Failed(reason) => return Err(reason),
        };
        let view = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("menu"),
            });
        {
            let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("menu background"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.04,
                            g: 0.045,
                            b: 0.06,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        let size = self.surface.size().unwrap_or([1, 1]);
        self.overlay
            .encode(&self.device, &self.queue, &mut encoder, &view, size);
        self.queue.submit([encoder.finish()]);
        texture.present();
        self.overlay.finish();
        Ok(())
    }
}
