use bevy::camera::{Hdr, visibility::NoFrustumCulling};
use bevy::pbr::{DistanceFog, FogFalloff, MaterialPipeline, MaterialPipelineKey};
use bevy::post_process::bloom::Bloom;
use bevy::render::mesh::Indices;
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderDefVal;
use bevy::{
    asset::RenderAssetUsages,
    mesh::{MeshVertexBufferLayoutRef, PrimitiveTopology},
    prelude::*,
    render::render_resource::{
        AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
    },
    shader::ShaderRef,
};
use monarch_engine::prelude::*;
use rustc_hash::FxHashSet;
use spatial_lib::chunk::math::ChunkKey;

pub struct WorldRenderPlugin;

impl Plugin for WorldRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            MaterialPlugin::<TerrainMaterial>::default(),
            MaterialPlugin::<GranularMaterial>::default(),
            MaterialPlugin::<FluidMaterial>::default(),
            MaterialPlugin::<SurfaceMaterial>::default(),
        ))
        .init_resource::<WorldLighting>()
        .add_systems(Startup, (setup_rendering, spawn_sun))
        .add_systems(
            Update,
            (
                configure_world_cameras,
                apply_world_lighting,
                fit_fog_to_view,
                sync_grid_rendering,
            )
                .chain(),
        );
    }
}

/// Palette rows reserved per landscape layer. Materials occupy the lower half of a row and
/// their shading properties the upper half, at `PALETTE_PROPERTY_OFFSET`.
const PALETTE_LAYER_STRIDE: u32 = 32;
const PALETTE_PROPERTY_OFFSET: u32 = 16;
const PALETTE_LAYER_COUNT: u32 = 4;
const PALETTE_ENTRIES: usize = (PALETTE_LAYER_STRIDE * PALETTE_LAYER_COUNT) as usize;

const LAYER_TERRAIN: u32 = 0;
const LAYER_GRANULAR: u32 = 1;
const LAYER_FLUID: u32 = 2;
const LAYER_SURFACE: u32 = 3;

/// Single source of truth for the scene's light. Drives the landscape shaders, the PBR sun
/// and ambient light used by entities, the clear colour and the camera fog together, so
/// terrain and meshes are always lit consistently.
#[derive(Resource, Debug, Clone)]
pub struct WorldLighting {
    /// Compass bearing of the sun in degrees: 0 is north (-Z), 90 is east (+X).
    pub sun_azimuth_degrees: f32,
    /// Angle of the sun above the horizon in degrees.
    pub sun_elevation_degrees: f32,
    pub sun_illuminance_lux: f32,
    pub sun_color: Color,
    /// Hemispheric sky light: zenith colour, ground bounce colour and brightness, in the same
    /// units as `GlobalAmbientLight::brightness`.
    pub sky_color: Color,
    pub ground_color: Color,
    pub ambient_brightness: f32,
    /// Colour of the horizon haze; also used as the clear colour so the sky meets the fog.
    pub horizon_color: Color,
    /// Tint added to the fog when looking towards the sun.
    pub sun_glow_color: Color,
    pub sun_glow_exponent: f32,
    /// Fog fades in between these fractions of the distance from the camera to the edge of the
    /// loaded window, so nearby terrain stays clear while the window boundary is hidden.
    pub fog_start_fraction: f32,
    pub fog_end_fraction: f32,
    /// Opacity of heightfield-marched terrain shadows; zero skips the march entirely.
    pub terrain_shadow_strength: f32,
    pub ambient_occlusion_strength: f32,
}

impl Default for WorldLighting {
    fn default() -> Self {
        Self {
            sun_azimuth_degrees: 125.0,
            sun_elevation_degrees: 35.0,
            sun_illuminance_lux: light_consts::lux::AMBIENT_DAYLIGHT,
            sun_color: Color::srgb(1.0, 0.95, 0.86),
            sky_color: Color::srgb(0.55, 0.70, 0.95),
            ground_color: Color::srgb(0.42, 0.36, 0.28),
            ambient_brightness: 900.0,
            horizon_color: Color::srgb(0.68, 0.76, 0.86),
            sun_glow_color: Color::srgba(1.0, 0.85, 0.65, 0.5),
            sun_glow_exponent: 24.0,
            fog_start_fraction: 0.55,
            fog_end_fraction: 0.98,
            terrain_shadow_strength: 0.85,
            ambient_occlusion_strength: 0.6,
        }
    }
}

impl WorldLighting {
    /// Unit vector pointing from the ground towards the sun.
    pub fn sun_direction(&self) -> Vec3 {
        let azimuth = self.sun_azimuth_degrees.to_radians();
        let elevation = self.sun_elevation_degrees.to_radians();
        Vec3::new(
            elevation.cos() * azimuth.sin(),
            elevation.sin(),
            -elevation.cos() * azimuth.cos(),
        )
    }

    /// Fog for a camera `altitude` units above the ground plane. The distance to the window
    /// edge grows with altitude, so the fog band moves out as the camera climbs and a top-down
    /// view is not swallowed by haze meant for the horizon.
    fn fog_for_view(&self, window_half_extent: f32, altitude: f32) -> DistanceFog {
        let edge_distance = window_half_extent.hypot(altitude.max(0.0));
        DistanceFog {
            color: self.horizon_color,
            directional_light_color: self.sun_glow_color,
            directional_light_exponent: self.sun_glow_exponent,
            falloff: FogFalloff::Linear {
                start: edge_distance * self.fog_start_fraction,
                end: edge_distance * self.fog_end_fraction,
            },
        }
    }
}

/// GPU boundary struct reflecting the active projection window, brush cursor and lighting.
/// Field order and meaning must match `WorldWindow` in `common.wgsl`.
#[derive(Clone, Default, ShaderType, Debug)]
pub struct WorldWindowUniform {
    pub origin_size: Vec4,   // x: origin.x, y: origin.y, z: size.x, w: size.y
    pub head_cursor: Vec4,   // x: head.x,   y: head.y,   z: cursor.x, w: cursor.y
    pub config: Vec4, // x: elev_scale, y: cursor_radius, z: shadow strength, w: occlusion strength
    pub sun_direction: Vec4, // xyz: direction towards the sun, w: illuminance (lux)
    pub sun_color: Vec4, // rgb: linear sun colour, w: ambient brightness
    pub sky_color: Vec4, // rgb: linear zenith colour
    pub ground_color: Vec4, // rgb: linear ground bounce colour
}

impl WorldWindowUniform {
    fn apply_lighting(&mut self, lighting: &WorldLighting) {
        let linear = |color: Color| LinearRgba::from(color).to_vec4();
        self.config.z = lighting.terrain_shadow_strength;
        self.config.w = lighting.ambient_occlusion_strength;
        self.sun_direction = lighting
            .sun_direction()
            .extend(lighting.sun_illuminance_lux);
        self.sun_color = linear(lighting.sun_color)
            .truncate()
            .extend(lighting.ambient_brightness);
        self.sky_color = linear(lighting.sky_color);
        self.ground_color = linear(lighting.ground_color);
    }
}

/// Compile-time constants shared between the engine and the landscape shaders, injected as
/// shader defs so material ids and palette layout cannot drift apart.
fn landscape_shader_defs() -> [ShaderDefVal; 6] {
    [
        ShaderDefVal::UInt("PALETTE_LAYER_STRIDE".into(), PALETTE_LAYER_STRIDE),
        ShaderDefVal::UInt("PALETTE_PROPERTY_OFFSET".into(), PALETTE_PROPERTY_OFFSET),
        ShaderDefVal::UInt(
            "SURFACE_MAT_FOLIAGE".into(),
            SurfaceMat::SURFACE_FOLIAGE.0 as u32,
        ),
        ShaderDefVal::UInt("SURFACE_MAT_FIRE".into(), SurfaceMat::SURFACE_FIRE.0 as u32),
        ShaderDefVal::UInt("FOLIAGE_MATURE_STATE".into(), FOLIAGE_MATURE_STATE as u32),
        ShaderDefVal::UInt("FLUID_MAT_ICE".into(), FluidMat::FLUID_ICE.0 as u32),
    ]
}

macro_rules! define_landscape_material {
    ($struct_name:ident, $shader_path:expr, $alpha_mode:expr, $writes_depth:expr) => {
        #[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
        pub struct $struct_name {
            #[storage(10, read_only, visibility(vertex))]
            pub grid_buffer: Handle<ShaderBuffer>,

            #[storage(11, read_only, visibility(vertex))]
            pub palette_buffer: Handle<ShaderBuffer>,

            #[uniform(12, visibility(vertex, fragment))]
            pub window: WorldWindowUniform,
        }

        impl Material for $struct_name {
            fn vertex_shader() -> ShaderRef {
                $shader_path.into()
            }
            fn fragment_shader() -> ShaderRef {
                $shader_path.into()
            }

            fn alpha_mode(&self) -> AlphaMode {
                $alpha_mode
            }

            fn enable_prepass() -> bool {
                false
            }
            fn enable_shadows() -> bool {
                false
            }

            fn specialize(
                _pipeline: &MaterialPipeline,
                descriptor: &mut RenderPipelineDescriptor,
                layout: &MeshVertexBufferLayoutRef,
                _key: MaterialPipelineKey<Self>,
            ) -> Result<(), SpecializedMeshPipelineError> {
                let vertex_layout = layout
                    .0
                    .get_layout(&[Mesh::ATTRIBUTE_POSITION.at_shader_location(0)])?;
                descriptor.vertex.buffers = vec![vertex_layout];
                descriptor.primitive.cull_mode = None;

                if $writes_depth {
                    if let Some(depth_stencil) = descriptor.depth_stencil.as_mut() {
                        depth_stencil.depth_write_enabled = Some(true);
                    }
                }

                let shader_defs = landscape_shader_defs();
                descriptor.vertex.shader_defs.extend(shader_defs.clone());
                if let Some(fragment) = descriptor.fragment.as_mut() {
                    fragment.shader_defs.extend(shader_defs);
                }
                Ok(())
            }
        }
    };
}

// Opaque layers already write depth. The liquid layer is blended but writes depth as well:
// one chunk mesh holds every face of a liquid body, and without depth writes the far side of
// a lava lobe drawn later in index order paints over its near side from some view directions.
// The shader discards nearly invisible fragments so faded shorelines never occlude.
define_landscape_material!(
    TerrainMaterial,
    "shaders/landscape/terrain.wgsl",
    AlphaMode::Opaque,
    false
);
define_landscape_material!(
    GranularMaterial,
    "shaders/landscape/granular.wgsl",
    AlphaMode::Opaque,
    false
);
define_landscape_material!(
    FluidMaterial,
    "shaders/landscape/fluid.wgsl",
    AlphaMode::Blend,
    true
);
define_landscape_material!(
    SurfaceMaterial,
    "shaders/landscape/surface.wgsl",
    AlphaMode::Opaque,
    false
);

#[derive(Resource)]
pub struct ChunkMeshHandle(pub Handle<Mesh>);

#[derive(Resource)]
pub struct GlobalWorldMaterials {
    pub grid_buffer: Handle<ShaderBuffer>,
    pub terrain: Handle<TerrainMaterial>,
    pub granular: Handle<GranularMaterial>,
    pub fluid: Handle<FluidMaterial>,
    pub surface: Handle<SurfaceMaterial>,
}

#[derive(Component)]
pub struct ChunkRenderMarker(pub ChunkKey);

/// Marks the directional light that `WorldLighting` steers.
#[derive(Component)]
pub struct WorldSun;

/// Shading response of one palette material, packed into the property half of its row.
#[derive(Clone, Copy)]
struct MaterialLook {
    srgb: [f32; 3],
    /// Peak sun specular reflectance.
    specular: f32,
    /// Blinn-Phong exponent; higher is glossier.
    shininess: f32,
    /// Self-illumination as a multiple of albedo, added after lighting (feeds bloom).
    emission: f32,
    /// Light absorption per world unit of depth (liquids only); zero is perfectly clear.
    optical_density: f32,
    /// Solid phase of the liquid layer: no ripples or foam, and a crystalline surface instead.
    /// Carried to the shader in the albedo alpha, which is otherwise unused.
    frozen: bool,
}

impl MaterialLook {
    const fn matte(srgb: [f32; 3]) -> Self {
        Self {
            srgb,
            specular: 0.02,
            shininess: 8.0,
            emission: 0.0,
            optical_density: 0.0,
            frozen: false,
        }
    }

    const fn glossy(srgb: [f32; 3], specular: f32, shininess: f32) -> Self {
        Self {
            srgb,
            specular,
            shininess,
            emission: 0.0,
            optical_density: 0.0,
            frozen: false,
        }
    }

    const fn glowing(self, emission: f32) -> Self {
        Self { emission, ..self }
    }

    const fn solid(self) -> Self {
        Self {
            frozen: true,
            ..self
        }
    }

    const fn absorbing(self, optical_density: f32) -> Self {
        Self {
            optical_density,
            ..self
        }
    }
}

fn landscape_palette() -> Vec<[f32; 4]> {
    let entries: [(u32, u8, MaterialLook); 32] = [
        (
            LAYER_TERRAIN,
            TerrainMat::TERRAIN_STONE.0,
            MaterialLook::glossy([0.48, 0.46, 0.44], 0.08, 16.0),
        ),
        (
            LAYER_TERRAIN,
            TerrainMat::TERRAIN_DIRT.0,
            MaterialLook::matte([0.40, 0.28, 0.15]),
        ),
        (
            LAYER_TERRAIN,
            TerrainMat::TERRAIN_SANDSTONE.0,
            MaterialLook::glossy([0.65, 0.55, 0.35], 0.04, 12.0),
        ),
        (
            LAYER_TERRAIN,
            TerrainMat::TERRAIN_DENSE_SNOW.0,
            MaterialLook::glossy([0.70, 0.85, 0.95], 0.25, 32.0),
        ),
        (
            LAYER_TERRAIN,
            TerrainMat::TERRAIN_METAL.0,
            MaterialLook::glossy([0.60, 0.60, 0.65], 0.6, 64.0),
        ),
        (
            LAYER_TERRAIN,
            TerrainMat::TERRAIN_CORRUPTION.0,
            MaterialLook::glossy([0.35, 0.15, 0.40], 0.1, 16.0).glowing(0.05),
        ),
        (
            LAYER_GRANULAR,
            GranularMat::GRANULAR_DIRT.0,
            MaterialLook::matte([0.45, 0.32, 0.18]),
        ),
        (
            LAYER_GRANULAR,
            GranularMat::GRANULAR_SAND.0,
            MaterialLook::glossy([0.82, 0.72, 0.48], 0.05, 16.0),
        ),
        (
            LAYER_GRANULAR,
            GranularMat::GRANULAR_MUD.0,
            MaterialLook::glossy([0.25, 0.18, 0.10], 0.3, 48.0),
        ),
        (
            LAYER_GRANULAR,
            GranularMat::GRANULAR_GRAVEL.0,
            MaterialLook::glossy([0.40, 0.40, 0.42], 0.06, 16.0),
        ),
        (
            LAYER_GRANULAR,
            GranularMat::GRANULAR_SNOW.0,
            MaterialLook::glossy([0.92, 0.94, 0.96], 0.25, 32.0),
        ),
        (
            LAYER_GRANULAR,
            GranularMat::GRANULAR_LIQUID_METAL.0,
            MaterialLook::glossy([0.75, 0.75, 0.80], 0.8, 96.0),
        ),
        (
            LAYER_GRANULAR,
            GranularMat::GRANULAR_CORRUPTION.0,
            MaterialLook::glossy([0.45, 0.20, 0.50], 0.1, 16.0).glowing(0.1),
        ),
        (
            LAYER_FLUID,
            FluidMat::FLUID_WATER.0,
            MaterialLook::glossy([0.10, 0.30, 0.45], 1.0, 256.0).absorbing(0.12),
        ),
        (
            LAYER_FLUID,
            FluidMat::FLUID_MAGMA.0,
            MaterialLook::glossy([0.85, 0.25, 0.05], 0.2, 24.0)
                .glowing(3.0)
                .absorbing(50.0),
        ),
        (
            LAYER_FLUID,
            FluidMat::FLUID_BLOOD.0,
            MaterialLook::glossy([0.55, 0.02, 0.02], 0.6, 96.0).absorbing(1.5),
        ),
        (
            LAYER_FLUID,
            FluidMat::FLUID_ACID.0,
            MaterialLook::glossy([0.30, 0.75, 0.10], 0.8, 128.0)
                .glowing(0.4)
                .absorbing(0.35),
        ),
        (
            LAYER_FLUID,
            FluidMat::FLUID_OIL.0,
            MaterialLook::glossy([0.12, 0.08, 0.04], 1.0, 128.0).absorbing(3.0),
        ),
        (
            LAYER_FLUID,
            FluidMat::FLUID_CORRUPTION.0,
            MaterialLook::glossy([0.25, 0.05, 0.30], 0.6, 64.0)
                .glowing(0.5)
                .absorbing(1.0),
        ),
        (
            LAYER_FLUID,
            FluidMat::FLUID_ICE.0,
            MaterialLook::glossy([0.70, 0.86, 0.97], 0.9, 160.0)
                .absorbing(0.3)
                .solid(),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_FIRE.0,
            MaterialLook::matte([1.00, 0.60, 0.10]).glowing(4.0),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_FOLIAGE.0,
            MaterialLook::glossy([0.18, 0.45, 0.12], 0.05, 8.0),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_WOOD.0,
            MaterialLook::matte([0.45, 0.28, 0.12]),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_FLESH.0,
            MaterialLook::glossy([0.75, 0.50, 0.45], 0.2, 24.0),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_BONE.0,
            MaterialLook::glossy([0.88, 0.85, 0.75], 0.1, 16.0),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_ROT.0,
            MaterialLook::glossy([0.30, 0.22, 0.10], 0.15, 24.0),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_ASH.0,
            MaterialLook::matte([0.45, 0.42, 0.40]),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_SNOW.0,
            MaterialLook::glossy([0.95, 0.95, 0.98], 0.25, 32.0),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_CLAY.0,
            MaterialLook::matte([0.68, 0.42, 0.28]),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_METAL.0,
            MaterialLook::glossy([0.55, 0.55, 0.60], 0.7, 64.0),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_GLASS.0,
            MaterialLook::glossy([0.75, 0.88, 0.92], 0.9, 192.0),
        ),
        (
            LAYER_SURFACE,
            SurfaceMat::SURFACE_CORRUPTION.0,
            MaterialLook::glossy([0.50, 0.10, 0.60], 0.2, 24.0).glowing(0.4),
        ),
    ];

    let mut palette = vec![[0.0f32; 4]; PALETTE_ENTRIES];
    for (layer, material, look) in entries {
        let color_slot = (layer * PALETTE_LAYER_STRIDE + material as u32) as usize;
        let property_slot = color_slot + PALETTE_PROPERTY_OFFSET as usize;
        // Palette colours are authored in sRGB and shaded in linear space.
        let linear = LinearRgba::from(Color::srgb(look.srgb[0], look.srgb[1], look.srgb[2]));
        let mobility = if look.frozen { 0.0 } else { 1.0 };
        palette[color_slot] = [linear.red, linear.green, linear.blue, mobility];
        palette[property_slot] = [
            look.specular,
            look.shininess,
            look.emission,
            look.optical_density,
        ];
    }
    palette
}

/// Allocates an immutable, continuous dummy mesh mapping exactly to a single physical chunk.
fn build_chunk_dummy(size: u32) -> Mesh {
    let verts_across = size + 1;
    let num_verts = (verts_across * verts_across) as usize;
    let mut positions = Vec::with_capacity(num_verts);
    let mut indices = Vec::with_capacity((size * size * 6) as usize);

    // Generate grid mapping Y to -Z to match the engine's original layout
    for z in 0..verts_across {
        for x in 0..verts_across {
            positions.push([x as f32, 0.0, -(z as f32)]);
        }
    }

    // Connect triangles with a reversed winding order to fix backface culling
    for z in 0..size {
        for x in 0..size {
            let i = z * verts_across + x;

            // Triangle 1
            indices.push(i);
            indices.push(i + 1);
            indices.push(i + verts_across);

            // Triangle 2
            indices.push(i + 1);
            indices.push(i + verts_across + 1);
            indices.push(i + verts_across);
        }
    }

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::all());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

fn setup_rendering(
    mut commands: Commands,
    mut terrain_mats: ResMut<Assets<TerrainMaterial>>,
    mut granular_mats: ResMut<Assets<GranularMaterial>>,
    mut fluid_mats: ResMut<Assets<FluidMaterial>>,
    mut surface_mats: ResMut<Assets<SurfaceMaterial>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    mut meshes: ResMut<Assets<Mesh>>,
    lighting: Res<WorldLighting>,
    physics: Res<GlobalPhysicsConfig>,
) {
    let grid_buffer = buffers.add(ShaderBuffer::new(&[0u8; 8], RenderAssetUsages::all()));
    let palette_buffer = buffers.add(ShaderBuffer::new(
        bytemuck::cast_slice(&landscape_palette()),
        RenderAssetUsages::all(),
    ));

    let mut window = WorldWindowUniform {
        config: Vec4::new(physics.elevation_scale, -1.0, 0.0, 0.0),
        ..default()
    };
    window.apply_lighting(&lighting);

    let mat_terrain = terrain_mats.add(TerrainMaterial {
        grid_buffer: grid_buffer.clone(),
        palette_buffer: palette_buffer.clone(),
        window: window.clone(),
    });
    let mat_granular = granular_mats.add(GranularMaterial {
        grid_buffer: grid_buffer.clone(),
        palette_buffer: palette_buffer.clone(),
        window: window.clone(),
    });
    let mat_fluid = fluid_mats.add(FluidMaterial {
        grid_buffer: grid_buffer.clone(),
        palette_buffer: palette_buffer.clone(),
        window: window.clone(),
    });
    let mat_surface = surface_mats.add(SurfaceMaterial {
        grid_buffer: grid_buffer.clone(),
        palette_buffer,
        window,
    });

    let chunk_mesh = meshes.add(build_chunk_dummy(CHUNK_SIZE as u32));

    commands.insert_resource(GlobalWorldMaterials {
        grid_buffer,
        terrain: mat_terrain,
        granular: mat_granular,
        fluid: mat_fluid,
        surface: mat_surface,
    });
    commands.insert_resource(ChunkMeshHandle(chunk_mesh));
}

fn spawn_sun(mut commands: Commands, lighting: Res<WorldLighting>) {
    commands.spawn((
        WorldSun,
        DirectionalLight {
            color: lighting.sun_color,
            illuminance: lighting.sun_illuminance_lux,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::default().looking_to(-lighting.sun_direction(), Vec3::Y),
    ));
}

fn window_half_extent(grid: &ActiveWorldGrid) -> f32 {
    grid.spatial.width.min(grid.spatial.height) as f32 / 2.0
}

/// Gives every 3D camera the HDR target, bloom and fog the landscape shading expects.
fn configure_world_cameras(
    mut commands: Commands,
    cameras: Query<Entity, Added<Camera3d>>,
    lighting: Res<WorldLighting>,
    grid: Res<ActiveWorldGrid>,
) {
    for camera in &cameras {
        commands.entity(camera).insert((
            Hdr,
            Bloom::NATURAL,
            lighting.fog_for_view(window_half_extent(&grid), 0.0),
        ));
    }
}

/// Re-fits each camera's fog band to its altitude and the loaded window every frame.
fn fit_fog_to_view(
    lighting: Res<WorldLighting>,
    grid: Res<ActiveWorldGrid>,
    mut cameras: Query<(&GlobalTransform, &mut DistanceFog)>,
) {
    let half_extent = window_half_extent(&grid);
    for (transform, mut fog) in &mut cameras {
        *fog = lighting.fog_for_view(half_extent, transform.translation().y);
    }
}

/// Pushes `WorldLighting` into the PBR sun, ambient light and clear colour.
fn apply_world_lighting(
    lighting: Res<WorldLighting>,
    mut clear_color: ResMut<ClearColor>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut suns: Query<(&mut DirectionalLight, &mut Transform), With<WorldSun>>,
) {
    if !lighting.is_changed() {
        return;
    }

    clear_color.0 = lighting.horizon_color;
    ambient.color = lighting.sky_color;
    ambient.brightness = lighting.ambient_brightness;

    for (mut light, mut transform) in &mut suns {
        light.color = lighting.sun_color;
        light.illuminance = lighting.sun_illuminance_lux;
        *transform = Transform::default().looking_to(-lighting.sun_direction(), Vec3::Y);
    }
}

/// Enforces spatial synchronization between the ECS hierarchy and the active SSBO matrix.
fn sync_grid_rendering(
    mut commands: Commands,
    mut grid: ResMut<ActiveWorldGrid>,
    manager: Res<WorldManager>,
    mut terrain_mats: ResMut<Assets<TerrainMaterial>>,
    mut granular_mats: ResMut<Assets<GranularMaterial>>,
    mut fluid_mats: ResMut<Assets<FluidMaterial>>,
    mut surface_mats: ResMut<Assets<SurfaceMaterial>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    chunk_mesh: Res<ChunkMeshHandle>,
    global_mats: Res<GlobalWorldMaterials>,
    physics: Res<GlobalPhysicsConfig>,
    lighting: Res<WorldLighting>,
    chunk_query: Query<(Entity, &ChunkRenderMarker)>,
) {
    let grid_ref = grid.bypass_change_detection();
    let cells_dirty = grid_ref.cells_dirty;

    if cells_dirty {
        // All four layer materials share one storage buffer, so it is copied once.
        if let Some(mut buffer) = buffers.get_mut(&global_mats.grid_buffer) {
            let src: &[u8] = bytemuck::cast_slice(&grid_ref.spatial.cells);
            match &mut buffer.data {
                Some(dst) => {
                    dst.resize(src.len(), 0);
                    dst.copy_from_slice(src);
                }
                slot => *slot = Some(src.to_vec()),
            }
        }
    }

    // Touching a material also rebuilds its bind group, which is what makes it pick up the
    // re-uploaded grid buffer.
    if cells_dirty || lighting.is_changed() || physics.is_changed() {
        let update_window = |window: &mut WorldWindowUniform| {
            window.origin_size = Vec4::new(
                grid_ref.spatial.window_origin.x as f32,
                grid_ref.spatial.window_origin.y as f32,
                grid_ref.spatial.width as f32,
                grid_ref.spatial.height as f32,
            );
            window.head_cursor.x = grid_ref.spatial.buffer_head.x as f32;
            window.head_cursor.y = grid_ref.spatial.buffer_head.y as f32;
            window.config.x = physics.elevation_scale;
            window.apply_lighting(&lighting);
        };

        if let Some(mut material) = terrain_mats.get_mut(&global_mats.terrain) {
            update_window(&mut material.window);
        }
        if let Some(mut material) = granular_mats.get_mut(&global_mats.granular) {
            update_window(&mut material.window);
        }
        if let Some(mut material) = fluid_mats.get_mut(&global_mats.fluid) {
            update_window(&mut material.window);
        }
        if let Some(mut material) = surface_mats.get_mut(&global_mats.surface) {
            update_window(&mut material.window);
        }

        grid.cells_dirty = false;
    }

    let Some(active_view) = manager.inner.active_view else {
        return;
    };

    // Eradicate ECS instances outside the strict topological boundary
    let mut existing_chunks = FxHashSet::default();
    for (entity, marker) in chunk_query.iter() {
        if active_view.contains(&marker.0) {
            existing_chunks.insert(marker.0);
        } else {
            commands.entity(entity).despawn();
        }
    }

    // Spawns missing instances locked to their physical coordinate origins
    for key in active_view.iter() {
        if !existing_chunks.contains(&key) {
            let chunk_x = key.key.x * (CHUNK_SIZE as i32);
            let chunk_y = key.key.y * (CHUNK_SIZE as i32);
            let pos = Transform::from_xyz(chunk_x as f32, 0.0, -(chunk_y as f32));

            commands.spawn((
                Mesh3d(chunk_mesh.0.clone()),
                MeshMaterial3d(global_mats.terrain.clone()),
                pos,
                ChunkRenderMarker(key),
                NoFrustumCulling,
            ));
            commands.spawn((
                Mesh3d(chunk_mesh.0.clone()),
                MeshMaterial3d(global_mats.granular.clone()),
                pos,
                ChunkRenderMarker(key),
                NoFrustumCulling,
            ));
            commands.spawn((
                Mesh3d(chunk_mesh.0.clone()),
                MeshMaterial3d(global_mats.fluid.clone()),
                pos,
                ChunkRenderMarker(key),
                NoFrustumCulling,
            ));
            commands.spawn((
                Mesh3d(chunk_mesh.0.clone()),
                MeshMaterial3d(global_mats.surface.clone()),
                pos,
                ChunkRenderMarker(key),
                NoFrustumCulling,
            ));
        }
    }
}
