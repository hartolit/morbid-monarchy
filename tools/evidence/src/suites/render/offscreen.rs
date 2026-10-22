//! The production renderer drawing into an offscreen texture on the GPU owner's device, read
//! back after the device finished: the render suite's view of what a window would show. The
//! readback is a maintenance wait of the harness, never part of a client frame.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

use gpu_world::world::GpuWorld;
use world_model::glam::{DVec3, Vec2};
use world_render::{
    CameraRig, FrameRequest, Lighting, Overlay, Renderer,
    frame::{DEFAULT_FOV_Y_DEGREES, DEFAULT_NEAR, LENS_OFFSET_Y, project},
};

/// One read-back frame, RGBA8 as the output format holds it.
pub struct Image {
    pub size: [u32; 2],
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let x = x.min(self.size[0] - 1);
        let y = y.min(self.size[1] - 1);
        let at = ((y * self.size[0] + x) * 4) as usize;
        [
            self.rgba[at],
            self.rgba[at + 1],
            self.rgba[at + 2],
            self.rgba[at + 3],
        ]
    }

    /// The pixel at normalized device coordinates (x right, y up).
    pub fn at_ndc(&self, ndc: Vec2) -> [u8; 4] {
        let x = ((ndc.x * 0.5 + 0.5) * self.size[0] as f32).floor().max(0.0) as u32;
        let y = ((0.5 - ndc.y * 0.5) * self.size[1] as f32).floor().max(0.0) as u32;
        self.pixel(x, y)
    }

    /// The mean colour of a square of `half` pixels around `ndc`.
    pub fn mean_at(&self, ndc: Vec2, half: i32) -> [f32; 3] {
        let cx = ((ndc.x * 0.5 + 0.5) * self.size[0] as f32) as i32;
        let cy = ((0.5 - ndc.y * 0.5) * self.size[1] as f32) as i32;
        let mut sum = [0.0f32; 3];
        let mut count: f32 = 0.0;
        for y in cy - half..=cy + half {
            for x in cx - half..=cx + half {
                if x < 0 || y < 0 || x >= self.size[0] as i32 || y >= self.size[1] as i32 {
                    continue;
                }
                let [r, g, b, _] = self.pixel(x as u32, y as u32);
                sum[0] += f32::from(r);
                sum[1] += f32::from(g);
                sum[2] += f32::from(b);
                count += 1.0;
            }
        }
        sum.map(|value| value / count.max(1.0))
    }

    /// Writes the frame as a PNG, for the case's record.
    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        crate::native::write_png(
            path,
            &morbid_app::client::Capture {
                size: self.size,
                rgba: self.rgba.clone(),
                tick: None,
            },
        )
    }
}

/// What a colour looks like, for the cases' predicates (8-bit sRGB after tone mapping).
pub mod colour {
    pub fn is_water(c: [f32; 3]) -> bool {
        c[2] > c[0] + 20.0 && c[2] >= c[1]
    }

    pub fn is_sand(c: [f32; 3]) -> bool {
        c[0] > c[2] + 35.0 && c[1] > c[2] + 20.0 && c[0] > 120.0
    }

    pub fn is_grass(c: [f32; 3]) -> bool {
        c[1] > c[0] + 15.0 && c[1] > c[2] + 15.0
    }

    /// Low saturation: stone, metal, the horizon haze.
    pub fn is_grey(c: [f32; 3]) -> bool {
        let max = c[0].max(c[1]).max(c[2]);
        let min = c[0].min(c[1]).min(c[2]);
        max - min < 32.0
    }

    pub fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    }
}

/// A camera straight down onto `(x, z)` from `height` world units, north up.
pub fn top_down(x: f64, z: f64, height: f64) -> CameraRig {
    CameraRig::Free {
        position: DVec3::new(x, height, z),
        yaw: 0.0,
        pitch: -1.54,
    }
}

/// The default lighting with the fog pushed beyond the scene, so colours are the materials'.
pub fn clear_lighting() -> Lighting {
    Lighting {
        fog_start_fraction: 50.0,
        fog_end_fraction: 60.0,
        ..Lighting::default()
    }
}

pub fn request(camera: CameraRig) -> FrameRequest {
    FrameRequest {
        camera,
        fov_y: DEFAULT_FOV_Y_DEGREES.to_radians(),
        near: DEFAULT_NEAR,
        lighting: clear_lighting(),
        cursor: None,
        overlay: Overlay::default(),
        seconds: 0.0,
        shadow_focus: match camera {
            CameraRig::Free { position, .. } => position,
            CameraRig::Follow { fallback, .. } => fallback,
        },
    }
}

/// Where `point` lands for a free camera rig of `request`, as `world_render` places the eye.
pub fn ndc_of(request: &FrameRequest, aspect: f32, point: DVec3) -> Option<Vec2> {
    let CameraRig::Free {
        position,
        yaw,
        pitch,
    } = request.camera
    else {
        return None;
    };
    let eye = position + DVec3::new(0.0, f64::from(LENS_OFFSET_Y), 0.0);
    project(eye, yaw, pitch, request.fov_y, aspect, point)
}

pub struct Offscreen {
    pub renderer: Renderer,
    pub size: [u32; 2],
    pub format: wgpu::TextureFormat,
    texture: wgpu::Texture,
    target: wgpu::TextureView,
    readback: wgpu::Buffer,
    row: u32,
}

impl Offscreen {
    pub fn new(
        world: &mut GpuWorld,
        size: [u32; 2],
        format: wgpu::TextureFormat,
    ) -> Result<Self, String> {
        let view = world.view();
        let mut renderer = Renderer::new(&view, format).map_err(|error| error.to_string())?;
        renderer
            .resize(view.device, view.queue, size)
            .map_err(|error| error.to_string())?;
        let texture = view.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("render suite target"),
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
        let texel = format.block_copy_size(None).unwrap_or(4);
        let row = (size[0] * texel).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = view.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("render suite readback"),
            size: u64::from(row) * u64::from(size[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Ok(Self {
            renderer,
            size,
            format,
            texture,
            target,
            readback,
            row,
        })
    }

    pub fn aspect(&self) -> f32 {
        self.size[0] as f32 / self.size[1] as f32
    }

    /// Draws a frame on the world's timeline without reading it back.
    pub fn draw(&mut self, world: &mut GpuWorld, request: &FrameRequest) -> Result<(), String> {
        let renderer = &mut self.renderer;
        let target = &self.target;
        let mut failure = None;
        world
            .submit_presentation("render suite", |view, encoder| {
                if let Err(error) = renderer.encode(view, request, encoder, target) {
                    failure = Some(error.to_string());
                }
            })
            .map_err(|refusal| format!("presentation refused: {refusal:?}"))?;
        failure.map_or(Ok(()), Err)
    }

    /// Draws a frame and reads it back (a maintenance wait).
    pub fn image(&mut self, world: &mut GpuWorld, request: &FrameRequest) -> Result<Image, String> {
        let renderer = &mut self.renderer;
        let (target, texture, readback, row, size) = (
            &self.target,
            &self.texture,
            &self.readback,
            self.row,
            self.size,
        );
        let mut failure = None;
        world
            .submit_presentation("render suite readback", |view, encoder| {
                if let Err(error) = renderer.encode(view, request, encoder, target) {
                    failure = Some(error.to_string());
                }
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: readback,
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
            .map_err(|refusal| format!("presentation refused: {refusal:?}"))?;
        if let Some(failure) = failure {
            return Err(failure);
        }
        map_read(world, &self.readback, |bytes| {
            let bgra = matches!(
                self.format,
                wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
            );
            let mut rgba = Vec::with_capacity((size[0] * size[1] * 4) as usize);
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
            Image { size, rgba }
        })
    }
}

/// Maps `buffer` after the device finished everything submitted, reads it and unmaps it.
pub fn map_read<T>(
    world: &mut GpuWorld,
    buffer: &wgpu::Buffer,
    read: impl FnOnce(&[u8]) -> T,
) -> Result<T, String> {
    world.wait_idle(Duration::from_secs(10));
    let mapped = Arc::new(AtomicU8::new(0));
    let flag = Arc::clone(&mapped);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            flag.store(if result.is_ok() { 1 } else { 2 }, Ordering::Release);
        });
    let deadline = Instant::now() + Duration::from_secs(10);
    while mapped.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
        world.wait_idle(Duration::from_millis(200));
        std::thread::sleep(Duration::from_micros(200));
    }
    if mapped.load(Ordering::Acquire) != 1 {
        return Err("the readback did not map".to_owned());
    }
    let value = {
        let bytes = buffer
            .slice(..)
            .get_mapped_range()
            .map_err(|error| format!("the readback could not be read: {error}"))?;
        read(&bytes)
    };
    buffer.unmap();
    Ok(value)
}
