//! The renderer: pipelines and buffers built once per device, render targets per window size,
//! and one frame's passes encoded into the caller's encoder.
//!
//! ```text
//! place_camera (compute)  the view from the frame's rig and the live body state
//! shadow pass             spheres as depth from the sun
//! main pass               terrain, granular and surface layers; spheres; liquids (blended)
//! bloom                   13-tap downsamples, tent upsamples blended per mip, into the frame
//! tone map                AgX into the output texture
//! ```
//!
//! Every pass reads the GPU owner's buffers as the last submitted tick left them: the frame is
//! submitted on the same timeline after the ticks (`GpuWorld::submit_presentation`), so no copy
//! and no wait is involved. Pipelines are created once, bind groups over the world's buffers only
//! when the window's buffers are replaced (`WorldView::buffers_generation`), and render targets
//! when the output size changes; the per-frame CPU work is writing one uniform.

use std::num::NonZeroU64;

use gpu_world::{
    shader::{Assembly, ShaderDiagnostic},
    world::WorldView,
};
use world_runtime::ids::SessionId;

use crate::{
    frame::{FrameRequest, FrameUniform, SHADOW_MAP_SIZE, VIEW_RECORD_BYTES},
    geometry::{TERRAIN_INDEX_COUNT, icosphere, terrain_indices},
    lighting::landscape_palette,
    shaders,
};

const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// The sphere mesh's subdivisions: 642 vertices, 1,280 triangles.
const SPHERE_LEVELS: u32 = 3;
/// Bevy's `Bloom::NATURAL`: the largest mip's height, the low-frequency boost and its curvature,
/// and the high-pass frequency.
const BLOOM_MAX_MIP_DIMENSION: u32 = 512;
const BLOOM_LOW_FREQUENCY_BOOST: f32 = 0.7;
const BLOOM_LOW_FREQUENCY_CURVATURE: f32 = 0.95;
const BLOOM_HIGH_PASS_FREQUENCY: f32 = 1.0;
/// Byte stride of the post parameters: the uniform offset alignment WebGPU guarantees.
const POST_PARAMS_STRIDE: u64 = 256;

/// Why the renderer could not be built or draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    /// An assembled shader failed offline validation, at a module file and line.
    Source(ShaderDiagnostic),
    /// The device refused a module, pipeline or resource the offline validator accepted.
    Device {
        what: &'static str,
        description: String,
    },
    /// A window of zero width or height has no targets; nothing is drawn until it has a size.
    ZeroSize,
    /// The output format is not one the tone-map pass writes (8-bit RGBA/BGRA or 16-bit float).
    UnsupportedOutput(wgpu::TextureFormat),
    /// The frame belongs to another session than the one the renderer was built for: a new
    /// device needs a new renderer.
    WrongSession {
        built_for: SessionId,
        presented: SessionId,
    },
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Source(diagnostic) => write!(f, "render shader: {diagnostic}"),
            Self::Device { what, description } => write!(f, "{what}: {description}"),
            Self::ZeroSize => write!(f, "the output has no area"),
            Self::UnsupportedOutput(format) => write!(f, "unsupported output format {format:?}"),
            Self::WrongSession {
                built_for,
                presented,
            } => write!(
                f,
                "renderer built for {built_for:?}, frame of {presented:?}"
            ),
        }
    }
}

/// Work the renderer did, for the evidence that frames reuse what they can.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RenderCounters {
    pub frames: u64,
    pub pipelines_created: u32,
    /// Bind groups over the world's buffers: once per window buffer generation.
    pub world_bind_groups_created: u64,
    /// Render target sets: once per output size.
    pub target_sets_created: u64,
    pub uniform_bytes_written: u64,
    /// Device bytes of the renderer's own buffers and textures (not the world's).
    pub static_bytes: u64,
    pub target_bytes: u64,
    /// Instances the last frame drew: chunks per landscape layer, and body slots.
    pub chunk_instances: u32,
    pub body_instances: u32,
}

/// The output's format, chosen by the caller; whether the tone map encodes sRGB itself.
fn output_encoding(format: wgpu::TextureFormat) -> Result<bool, RenderError> {
    use wgpu::TextureFormat as F;
    match format {
        F::Rgba8UnormSrgb | F::Bgra8UnormSrgb | F::Rgba16Float => Ok(false),
        F::Rgba8Unorm | F::Bgra8Unorm => Ok(true),
        other => Err(RenderError::UnsupportedOutput(other)),
    }
}

struct Pipelines {
    terrain: wgpu::RenderPipeline,
    granular: wgpu::RenderPipeline,
    surface: wgpu::RenderPipeline,
    fluid: wgpu::RenderPipeline,
    spheres: wgpu::RenderPipeline,
    sphere_shadows: wgpu::RenderPipeline,
    camera: wgpu::ComputePipeline,
    downsample_first: wgpu::RenderPipeline,
    downsample: wgpu::RenderPipeline,
    upsample: wgpu::RenderPipeline,
    tonemap: wgpu::RenderPipeline,
}

struct Layouts {
    world: wgpu::BindGroupLayout,
    shadow: wgpu::BindGroupLayout,
    camera: wgpu::BindGroupLayout,
    post: wgpu::BindGroupLayout,
}

/// Bind groups over the world's buffers, for one buffer generation of one session.
struct WorldGroups {
    generation: u64,
    scene: wgpu::BindGroup,
    camera: wgpu::BindGroup,
}

/// Targets for one output size.
struct Targets {
    size: [u32; 2],
    hdr: wgpu::TextureView,
    depth: wgpu::TextureView,
    bloom_mips: Vec<wgpu::TextureView>,
    /// Post bind groups: the HDR frame's first, then one per bloom mip.
    post_hdr: wgpu::BindGroup,
    post_mips: Vec<wgpu::BindGroup>,
    bytes: u64,
}

/// Device timestamps around the scene passes (camera, shadows, main) and the post passes (bloom,
/// tone map), resolved into a buffer the caller copies out; for measurement only.
struct Timestamps {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
}

/// Queries per frame: scene begin and end, post begin and end.
pub const TIMESTAMP_QUERIES: u32 = 4;

pub struct Renderer {
    session: SessionId,
    timestamps: Option<Timestamps>,
    encode_srgb: bool,
    layouts: Layouts,
    pipelines: Pipelines,
    terrain_indices: wgpu::Buffer,
    sphere_vertices: wgpu::Buffer,
    sphere_indices: wgpu::Buffer,
    sphere_index_count: u32,
    palette: wgpu::Buffer,
    frame_uniform: wgpu::Buffer,
    view_record: wgpu::Buffer,
    post_params: wgpu::Buffer,
    linear_sampler: wgpu::Sampler,
    shadow_view: wgpu::TextureView,
    shadow_group: wgpu::BindGroup,
    world_groups: Option<WorldGroups>,
    targets: Option<Targets>,
    counters: RenderCounters,
}

fn device_error(what: &'static str, error: Option<wgpu::Error>) -> Result<(), RenderError> {
    match error {
        Some(error) => Err(RenderError::Device {
            what,
            description: error.to_string(),
        }),
        None => Ok(()),
    }
}

fn compile(device: &wgpu::Device, assembly: &Assembly) -> Result<wgpu::ShaderModule, RenderError> {
    assembly.validate().map_err(RenderError::Source)?;
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(assembly.name()),
        source: wgpu::ShaderSource::Wgsl(assembly.source().into()),
    });
    device_error(assembly.name(), futures_lite::future::block_on(scope.pop()))?;
    Ok(module)
}

fn buffer_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    ty: wgpu::BufferBindingType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn read_only(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    buffer_entry(
        binding,
        visibility,
        wgpu::BufferBindingType::Storage { read_only: true },
    )
}

fn layouts(device: &wgpu::Device) -> Layouts {
    let draw = wgpu::ShaderStages::VERTEX_FRAGMENT;
    let world = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("render world"),
        entries: &[
            buffer_entry(0, draw, wgpu::BufferBindingType::Uniform),
            read_only(1, draw),
            read_only(2, draw),
            read_only(3, draw),
            read_only(4, draw),
            read_only(5, draw),
            read_only(6, draw),
        ],
    });
    let shadow = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("render sphere shadows"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
        ],
    });
    let compute = wgpu::ShaderStages::COMPUTE;
    let camera = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("render camera"),
        entries: &[
            buffer_entry(0, compute, wgpu::BufferBindingType::Uniform),
            buffer_entry(
                1,
                compute,
                wgpu::BufferBindingType::Storage { read_only: false },
            ),
            read_only(2, compute),
        ],
    });
    let fragment = wgpu::ShaderStages::FRAGMENT;
    let post = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("render post"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: fragment,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: fragment,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: fragment,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(16),
                },
                count: None,
            },
        ],
    });
    Layouts {
        world,
        shadow,
        camera,
        post,
    }
}

struct DrawState<'a> {
    module: &'a wgpu::ShaderModule,
    vertex: &'static str,
    fragment: Option<&'static str>,
    buffers: &'a [wgpu::VertexBufferLayout<'a>],
    target: Option<wgpu::ColorTargetState>,
    depth: Option<wgpu::DepthStencilState>,
    cull: Option<wgpu::Face>,
}

fn render_pipeline(
    device: &wgpu::Device,
    label: &'static str,
    layout: &wgpu::PipelineLayout,
    state: DrawState<'_>,
) -> wgpu::RenderPipeline {
    let targets = [state.target];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: state.module,
            entry_point: Some(state.vertex),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: state.buffers,
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: state.cull,
            ..wgpu::PrimitiveState::default()
        },
        depth_stencil: state.depth,
        multisample: wgpu::MultisampleState::default(),
        fragment: state.fragment.map(|entry| wgpu::FragmentState {
            module: state.module,
            entry_point: Some(entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &targets,
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// Reverse-Z depth: nearer is greater.
fn scene_depth(write: bool) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(write),
        depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    }
}

fn pipelines(
    device: &wgpu::Device,
    layouts: &Layouts,
    output: wgpu::TextureFormat,
) -> Result<Pipelines, RenderError> {
    let scene = compile(device, &shaders::scene_assembly())?;
    let camera = compile(device, &shaders::camera_assembly())?;
    let post = compile(device, &shaders::post_assembly())?;
    let layout = |label, groups: &[Option<&wgpu::BindGroupLayout>]| {
        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: groups,
            immediate_size: 0,
        })
    };
    let scene_layout = layout(
        "render scene",
        &[Some(&layouts.world), Some(&layouts.shadow)],
    );
    let shadow_layout = layout("render sphere shadows", &[Some(&layouts.world)]);
    let camera_layout = layout("render camera", &[Some(&layouts.camera)]);
    let post_layout = layout("render post", &[Some(&layouts.post)]);

    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let opaque = Some(wgpu::ColorTargetState {
        format: HDR_FORMAT,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    });
    let layer = |label, vertex, fragment, blend: Option<wgpu::BlendState>| {
        render_pipeline(
            device,
            label,
            &scene_layout,
            DrawState {
                module: &scene,
                vertex,
                fragment: Some(fragment),
                buffers: &[],
                target: Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                }),
                // Liquids write depth too: one chunk instance holds every face of a liquid
                // body, and without depth the far side of a lava lobe drawn later paints over
                // its near side (the baseline's choice, kept).
                depth: Some(scene_depth(true)),
                cull: None,
            },
        )
    };
    let terrain = layer("terrain layer", "terrain_vertex", "solid_fragment", None);
    let granular = layer("granular layer", "granular_vertex", "solid_fragment", None);
    let surface = layer("surface layer", "surface_vertex", "solid_fragment", None);
    let fluid = layer(
        "fluid layer",
        "fluid_vertex",
        "fluid_fragment",
        Some(wgpu::BlendState::ALPHA_BLENDING),
    );
    let sphere_buffers = [wgpu::VertexBufferLayout {
        array_stride: 12,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        }],
    }];
    let spheres = render_pipeline(
        device,
        "spheres",
        &scene_layout,
        DrawState {
            module: &scene,
            vertex: "sphere_vertex",
            fragment: Some("sphere_fragment"),
            buffers: &sphere_buffers,
            target: opaque,
            depth: Some(scene_depth(true)),
            cull: Some(wgpu::Face::Back),
        },
    );
    let sphere_shadows = render_pipeline(
        device,
        "sphere shadows",
        &shadow_layout,
        DrawState {
            module: &scene,
            vertex: "sphere_shadow_vertex",
            fragment: None,
            buffers: &sphere_buffers,
            target: None,
            depth: Some(wgpu::DepthStencilState {
                format: SHADOW_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            cull: Some(wgpu::Face::Back),
        },
    );
    let camera = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("place camera"),
        layout: Some(&camera_layout),
        module: &camera,
        entry_point: Some("place_camera"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });
    let fullscreen = |label, fragment, format, blend| {
        render_pipeline(
            device,
            label,
            &post_layout,
            DrawState {
                module: &post,
                vertex: "fullscreen_vertex",
                fragment: Some(fragment),
                buffers: &[],
                target: Some(wgpu::ColorTargetState {
                    format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                }),
                depth: None,
                cull: None,
            },
        )
    };
    let energy_conserving = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Constant,
        dst_factor: wgpu::BlendFactor::OneMinusConstant,
        operation: wgpu::BlendOperation::Add,
    };
    let downsample_first = fullscreen(
        "bloom first downsample",
        "downsample_first",
        HDR_FORMAT,
        None,
    );
    let downsample = fullscreen("bloom downsample", "downsample", HDR_FORMAT, None);
    let upsample = fullscreen(
        "bloom upsample",
        "upsample",
        HDR_FORMAT,
        Some(wgpu::BlendState {
            color: energy_conserving,
            alpha: wgpu::BlendComponent::REPLACE,
        }),
    );
    let tonemap = fullscreen("tone map", "tonemap", output, None);
    device_error(
        "render pipelines",
        futures_lite::future::block_on(scope.pop()),
    )?;
    Ok(Pipelines {
        terrain,
        granular,
        surface,
        fluid,
        spheres,
        sphere_shadows,
        camera,
        downsample_first,
        downsample,
        upsample,
        tonemap,
    })
}

/// Bevy's per-mip bloom blend (`compute_blend_factor`, energy-conserving composite).
fn bloom_blend(intensity: f32, mip: f32, max_mip: f32) -> f32 {
    let position = if max_mip > 0.0 { mip / max_mip } else { 0.0 };
    let mut boost = (1.0 - (1.0 - position).powf(1.0 / (1.0 - BLOOM_LOW_FREQUENCY_CURVATURE)))
        * BLOOM_LOW_FREQUENCY_BOOST;
    let high_pass =
        1.0 - ((position - BLOOM_HIGH_PASS_FREQUENCY) / BLOOM_HIGH_PASS_FREQUENCY).clamp(0.0, 1.0);
    boost *= 1.0 - intensity;
    (intensity + boost) * high_pass
}

/// The bloom chain's first mip size and mip count for an output size (Bevy's: the first mip is
/// 512 high, each later one half the size, down to a 4-pixel mip).
fn bloom_extent(size: [u32; 2]) -> ([u32; 2], u32) {
    let ratio = BLOOM_MAX_MIP_DIMENSION as f32 / size[1].max(1) as f32;
    let first = [
        ((size[0] as f32 * ratio).round() as u32).max(1),
        ((size[1] as f32 * ratio).round() as u32).max(1),
    ];
    let largest_count = BLOOM_MAX_MIP_DIMENSION.ilog2().max(2) - 1;
    let fit = first[0].min(first[1]).max(1).ilog2().max(1);
    (first, largest_count.min(fit))
}

impl Renderer {
    /// Builds the renderer for the session `view` belongs to and an output of `format`. The
    /// renderer is tied to that device: a recovered session (a new device) needs a new one.
    pub fn new(view: &WorldView<'_>, format: wgpu::TextureFormat) -> Result<Self, RenderError> {
        let encode_srgb = output_encoding(format)?;
        let device = view.device;
        let layouts = layouts(device);
        let pipelines = pipelines(device, &layouts, format)?;
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let indices = terrain_indices();
        let terrain_indices = create_init(
            device,
            view.queue,
            "terrain indices",
            bytemuck::cast_slice(&indices),
            wgpu::BufferUsages::INDEX,
        );
        let (sphere_positions, sphere_index_data) = icosphere(SPHERE_LEVELS);
        let sphere_vertices = create_init(
            device,
            view.queue,
            "sphere vertices",
            bytemuck::cast_slice(&sphere_positions),
            wgpu::BufferUsages::VERTEX,
        );
        let sphere_indices = create_init(
            device,
            view.queue,
            "sphere indices",
            bytemuck::cast_slice(&sphere_index_data),
            wgpu::BufferUsages::INDEX,
        );
        let palette_data = landscape_palette();
        let palette = create_init(
            device,
            view.queue,
            "landscape palette",
            bytemuck::cast_slice(&palette_data),
            wgpu::BufferUsages::STORAGE,
        );
        let frame_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame"),
            size: std::mem::size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_record = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view"),
            size: VIEW_RECORD_BYTES,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        // One parameter block per post source: the HDR frame and up to the largest mip count.
        let post_sources = 1 + u64::from(BLOOM_MAX_MIP_DIMENSION.ilog2());
        let post_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post parameters"),
            size: post_sources * POST_PARAMS_STRIDE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sphere shadows"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..wgpu::SamplerDescriptor::default()
        });
        let shadow_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sphere shadow map"),
            size: wgpu::Extent3d {
                width: SHADOW_MAP_SIZE,
                height: SHADOW_MAP_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_view = shadow_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let shadow_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sphere shadows"),
            layout: &layouts.shadow,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&shadow_sampler),
                },
            ],
        });
        device_error(
            "render resources",
            futures_lite::future::block_on(scope.pop()),
        )?;
        let static_bytes = terrain_indices.size()
            + sphere_vertices.size()
            + sphere_indices.size()
            + palette.size()
            + frame_uniform.size()
            + view_record.size()
            + post_params.size()
            + u64::from(SHADOW_MAP_SIZE) * u64::from(SHADOW_MAP_SIZE) * 4;
        Ok(Self {
            session: view.session,
            timestamps: None,
            encode_srgb,
            layouts,
            pipelines,
            terrain_indices,
            sphere_vertices,
            sphere_indices,
            sphere_index_count: sphere_index_data.len() as u32,
            palette,
            frame_uniform,
            view_record,
            post_params,
            linear_sampler,
            shadow_view,
            shadow_group,
            world_groups: None,
            targets: None,
            counters: RenderCounters {
                pipelines_created: 11,
                static_bytes,
                ..RenderCounters::default()
            },
        })
    }

    pub fn session(&self) -> SessionId {
        self.session
    }

    /// Writes device timestamps around the scene and post passes from now on (the device must
    /// have `TIMESTAMP_QUERY`). After each [`encode`](Self::encode), the resolved ticks are in
    /// [`timestamp_buffer`](Self::timestamp_buffer): [`TIMESTAMP_QUERIES`] `u64`s.
    pub fn enable_timestamps(&mut self, device: &wgpu::Device) -> Result<(), RenderError> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return Err(RenderError::Device {
                what: "render timestamps",
                description: "the device has no TIMESTAMP_QUERY".to_owned(),
            });
        }
        let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("render timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: TIMESTAMP_QUERIES,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("render timestamps"),
            size: u64::from(TIMESTAMP_QUERIES) * 8,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        self.timestamps = Some(Timestamps { queries, resolve });
        Ok(())
    }

    pub fn timestamp_buffer(&self) -> Option<&wgpu::Buffer> {
        self.timestamps
            .as_ref()
            .map(|timestamps| &timestamps.resolve)
    }

    pub fn counters(&self) -> RenderCounters {
        self.counters
    }

    /// The output size the targets were built for, if any.
    pub fn size(&self) -> Option<[u32; 2]> {
        self.targets.as_ref().map(|targets| targets.size)
    }

    /// Builds the render targets for an output of `size`. A zero-sized output (a minimized
    /// window) is refused and leaves the renderer without targets; nothing is allocated for it.
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: [u32; 2],
    ) -> Result<(), RenderError> {
        if size[0] == 0 || size[1] == 0 {
            self.targets = None;
            return Err(RenderError::ZeroSize);
        }
        if self
            .targets
            .as_ref()
            .is_some_and(|targets| targets.size == size)
        {
            return Ok(());
        }
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let texture = |label, size: [u32; 2], format, mips: u32, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: mips,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let attachment =
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let hdr_texture = texture("hdr frame", size, HDR_FORMAT, 1, attachment);
        let depth_texture = texture(
            "depth",
            size,
            DEPTH_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let (first, mips) = bloom_extent(size);
        let bloom_texture = texture("bloom", first, HDR_FORMAT, mips, attachment);
        let hdr = hdr_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bloom_mips: Vec<wgpu::TextureView> = (0..mips)
            .map(|mip| {
                bloom_texture.create_view(&wgpu::TextureViewDescriptor {
                    base_mip_level: mip,
                    mip_level_count: Some(1),
                    ..wgpu::TextureViewDescriptor::default()
                })
            })
            .collect();
        let mip_size = |mip: u32| [(first[0] >> mip).max(1), (first[1] >> mip).max(1)];
        let mut params = vec![0u8; (POST_PARAMS_STRIDE * (1 + u64::from(mips))) as usize];
        let mut write = |index: usize, texel: [f32; 4]| {
            let at = index * POST_PARAMS_STRIDE as usize;
            params[at..at + 16].copy_from_slice(bytemuck::cast_slice(&texel));
        };
        let encode = if self.encode_srgb { 1.0 } else { 0.0 };
        write(0, [1.0 / size[0] as f32, 1.0 / size[1] as f32, 0.0, encode]);
        for mip in 0..mips {
            let [w, h] = mip_size(mip);
            write(1 + mip as usize, [1.0 / w as f32, 1.0 / h as f32, 0.0, 0.0]);
        }
        queue.write_buffer(&self.post_params, 0, &params);
        let post_group = |label, source: &wgpu::TextureView, index: u64| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &self.layouts.post,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.post_params,
                            offset: index * POST_PARAMS_STRIDE,
                            size: NonZeroU64::new(16),
                        }),
                    },
                ],
            })
        };
        let post_hdr = post_group("post hdr", &hdr, 0);
        let post_mips = bloom_mips
            .iter()
            .enumerate()
            .map(|(mip, view)| post_group("post bloom mip", view, 1 + mip as u64))
            .collect();
        device_error(
            "render targets",
            futures_lite::future::block_on(scope.pop()),
        )?;
        let pixels = u64::from(size[0]) * u64::from(size[1]);
        let bloom_bytes: u64 = (0..mips)
            .map(|mip| {
                let [w, h] = mip_size(mip);
                u64::from(w) * u64::from(h) * 8
            })
            .sum();
        let bytes = pixels * 8 + pixels * 4 + bloom_bytes;
        self.counters.target_sets_created += 1;
        self.counters.target_bytes = bytes;
        self.targets = Some(Targets {
            size,
            hdr,
            depth,
            bloom_mips,
            post_hdr,
            post_mips,
            bytes,
        });
        Ok(())
    }

    /// Bind groups over the world's buffers, rebuilt only when they were replaced.
    fn bind_world(&mut self, view: &WorldView<'_>) {
        if self
            .world_groups
            .as_ref()
            .is_some_and(|groups| groups.generation == view.buffers_generation)
        {
            return;
        }
        fn whole(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: buffer.as_entire_binding(),
            }
        }
        let scene = view.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("render world"),
            layout: &self.layouts.world,
            entries: &[
                whole(0, &self.frame_uniform),
                whole(1, &self.view_record),
                whole(2, view.cells),
                whole(3, view.window_map),
                whole(4, &self.palette),
                whole(5, view.bodies),
                whole(6, view.activity),
            ],
        });
        let camera = view.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("render camera"),
            layout: &self.layouts.camera,
            entries: &[
                whole(0, &self.frame_uniform),
                whole(1, &self.view_record),
                whole(2, view.bodies),
            ],
        });
        self.counters.world_bind_groups_created += 1;
        self.world_groups = Some(WorldGroups {
            generation: view.buffers_generation,
            scene,
            camera,
        });
    }

    /// Encodes one frame into `encoder`, drawing into `output` (a texture of the format the
    /// renderer was built for, at the size of the last [`resize`](Self::resize)).
    pub fn encode(
        &mut self,
        view: &WorldView<'_>,
        request: &FrameRequest,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
    ) -> Result<(), RenderError> {
        if view.session != self.session {
            return Err(RenderError::WrongSession {
                built_for: self.session,
                presented: view.session,
            });
        }
        let Some(size) = self.size() else {
            return Err(RenderError::ZeroSize);
        };
        let aspect = size[0] as f32 / size[1] as f32;
        let uniform = FrameUniform::pack(view, request, aspect);
        view.queue
            .write_buffer(&self.frame_uniform, 0, bytemuck::bytes_of(&uniform));
        self.counters.uniform_bytes_written += std::mem::size_of::<FrameUniform>() as u64;
        self.bind_world(view);
        let (Some(groups), Some(targets)) = (self.world_groups.as_ref(), self.targets.as_ref())
        else {
            return Err(RenderError::ZeroSize);
        };
        let chunks = view.window.chunk_count();
        let bodies = view.body_extent;

        let stamp = |begin: Option<u32>, end: Option<u32>| {
            self.timestamps
                .as_ref()
                .map(|timestamps| (&timestamps.queries, begin, end))
        };
        let scene_start = stamp(Some(0), None);
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("place camera"),
                timestamp_writes: scene_start.map(|(query_set, begin, end)| {
                    wgpu::ComputePassTimestampWrites {
                        query_set,
                        beginning_of_pass_write_index: begin,
                        end_of_pass_write_index: end,
                    }
                }),
            });
            pass.set_pipeline(&self.pipelines.camera);
            pass.set_bind_group(0, &groups.camera, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sphere shadows"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if bodies > 0 {
                pass.set_pipeline(&self.pipelines.sphere_shadows);
                pass.set_bind_group(0, &groups.scene, &[]);
                pass.set_vertex_buffer(0, self.sphere_vertices.slice(..));
                pass.set_index_buffer(self.sphere_indices.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(0..self.sphere_index_count, 0, 0..bodies);
            }
        }
        let [r, g, b] = uniform.horizon();
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.hdr,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(r),
                            g: f64::from(g),
                            b: f64::from(b),
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: stamp(None, Some(1)).map(|(query_set, begin, end)| {
                    wgpu::RenderPassTimestampWrites {
                        query_set,
                        beginning_of_pass_write_index: begin,
                        end_of_pass_write_index: end,
                    }
                }),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &groups.scene, &[]);
            pass.set_bind_group(1, &self.shadow_group, &[]);
            pass.set_index_buffer(self.terrain_indices.slice(..), wgpu::IndexFormat::Uint16);
            for pipeline in [
                &self.pipelines.terrain,
                &self.pipelines.granular,
                &self.pipelines.surface,
            ] {
                pass.set_pipeline(pipeline);
                pass.draw_indexed(0..TERRAIN_INDEX_COUNT, 0, 0..chunks);
            }
            if bodies > 0 {
                pass.set_pipeline(&self.pipelines.spheres);
                pass.set_vertex_buffer(0, self.sphere_vertices.slice(..));
                pass.set_index_buffer(self.sphere_indices.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(0..self.sphere_index_count, 0, 0..bodies);
                pass.set_index_buffer(self.terrain_indices.slice(..), wgpu::IndexFormat::Uint16);
            }
            pass.set_pipeline(&self.pipelines.fluid);
            pass.draw_indexed(0..TERRAIN_INDEX_COUNT, 0, 0..chunks);
        }
        let post_start = stamp(Some(2), None);
        let bloomed = self.encode_bloom(
            encoder,
            targets,
            request.lighting.bloom_intensity,
            post_start,
        );
        {
            let start = if bloomed { None } else { Some(2) };
            let mut pass =
                fullscreen_pass_stamped(encoder, "tone map", output, true, stamp(start, Some(3)));
            pass.set_pipeline(&self.pipelines.tonemap);
            pass.set_bind_group(0, &targets.post_hdr, &[]);
            pass.draw(0..3, 0..1);
        }
        if let Some(timestamps) = &self.timestamps {
            encoder.resolve_query_set(
                &timestamps.queries,
                0..TIMESTAMP_QUERIES,
                &timestamps.resolve,
                0,
            );
        }
        self.counters.frames += 1;
        self.counters.chunk_instances = chunks;
        self.counters.body_instances = bodies;
        Ok(())
    }

    /// Encodes the bloom chain; returns whether it ran (and wrote `start`'s timestamp).
    fn encode_bloom(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        targets: &Targets,
        intensity: f32,
        start: Option<Stamp<'_>>,
    ) -> bool {
        let mips = targets.bloom_mips.len();
        if mips == 0 || intensity <= 0.0 {
            return false;
        }
        {
            let mut pass =
                fullscreen_pass_stamped(encoder, "bloom", &targets.bloom_mips[0], true, start);
            pass.set_pipeline(&self.pipelines.downsample_first);
            pass.set_bind_group(0, &targets.post_hdr, &[]);
            pass.draw(0..3, 0..1);
        }
        for mip in 1..mips {
            let mut pass = fullscreen_pass(encoder, "bloom", &targets.bloom_mips[mip], true);
            pass.set_pipeline(&self.pipelines.downsample);
            pass.set_bind_group(0, &targets.post_mips[mip - 1], &[]);
            pass.draw(0..3, 0..1);
        }
        let max_mip = (mips - 1) as f32;
        for mip in (1..mips).rev() {
            let mut pass = fullscreen_pass(encoder, "bloom", &targets.bloom_mips[mip - 1], false);
            pass.set_pipeline(&self.pipelines.upsample);
            pass.set_bind_group(0, &targets.post_mips[mip], &[]);
            let blend = f64::from(bloom_blend(intensity, mip as f32, max_mip));
            pass.set_blend_constant(wgpu::Color {
                r: blend,
                g: blend,
                b: blend,
                a: blend,
            });
            pass.draw(0..3, 0..1);
        }
        let mut pass = fullscreen_pass(encoder, "bloom", &targets.hdr, false);
        pass.set_pipeline(&self.pipelines.upsample);
        pass.set_bind_group(0, &targets.post_mips[0], &[]);
        let blend = f64::from(bloom_blend(intensity, 0.0, max_mip));
        pass.set_blend_constant(wgpu::Color {
            r: blend,
            g: blend,
            b: blend,
            a: blend,
        });
        pass.draw(0..3, 0..1);
        true
    }

    /// Device bytes of the current render targets.
    pub fn target_bytes(&self) -> u64 {
        self.targets.as_ref().map_or(0, |targets| targets.bytes)
    }
}

/// A timestamp query set with the begin and end indices a pass writes.
type Stamp<'a> = (&'a wgpu::QuerySet, Option<u32>, Option<u32>);

fn fullscreen_pass<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    label: &'static str,
    target: &'a wgpu::TextureView,
    clear: bool,
) -> wgpu::RenderPass<'a> {
    fullscreen_pass_stamped(encoder, label, target, clear, None)
}

fn fullscreen_pass_stamped<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    label: &'static str,
    target: &'a wgpu::TextureView,
    clear: bool,
    stamp: Option<Stamp<'a>>,
) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: if clear {
                    wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                } else {
                    wgpu::LoadOp::Load
                },
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: stamp.map(|(query_set, begin, end)| wgpu::RenderPassTimestampWrites {
            query_set,
            beginning_of_pass_write_index: begin,
            end_of_pass_write_index: end,
        }),
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

fn create_init(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &'static str,
    contents: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    let size = (contents.len() as u64).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut padded = contents.to_vec();
    padded.resize(size as usize, 0);
    queue.write_buffer(&buffer, 0, &padded);
    buffer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bloom_chain_follows_bevys_natural_settings() {
        let (first, mips) = bloom_extent([1920, 1080]);
        assert_eq!(first, [910, 512]);
        assert_eq!(mips, 8);
        // A tiny window still gets at least one mip and never a zero-sized one.
        let (first, mips) = bloom_extent([3, 2]);
        assert!(first[0] >= 1 && first[1] >= 1 && mips >= 1);
        // The full-resolution blend is the intensity; deeper mips get the low-frequency boost.
        assert!((bloom_blend(0.15, 0.0, 7.0) - 0.15).abs() < 1e-6);
        assert!(bloom_blend(0.15, 3.0, 7.0) > 0.15);
        assert!(bloom_blend(0.15, 7.0, 7.0) <= 1.0);
    }

    #[test]
    fn outputs_are_encoded_once() {
        assert_eq!(
            output_encoding(wgpu::TextureFormat::Bgra8UnormSrgb),
            Ok(false)
        );
        assert_eq!(output_encoding(wgpu::TextureFormat::Bgra8Unorm), Ok(true));
        assert!(output_encoding(wgpu::TextureFormat::R8Unorm).is_err());
    }
}
