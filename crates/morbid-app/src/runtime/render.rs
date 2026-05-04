use bevy::camera::visibility::NoFrustumCulling;
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::{
    asset::RenderAssetUsages,
    mesh::{MeshVertexBufferLayoutRef, PrimitiveTopology},
    prelude::*,
    render::{
        render_resource::{
            AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
        },
        storage::ShaderStorageBuffer,
    },
    shader::ShaderRef,
};
use monarch_engine::prelude::*;

pub struct WorldRenderPlugin;

impl Plugin for WorldRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<WorldMaterial>::default())
            .init_resource::<GridMeshSize>()
            .init_resource::<WorldTuningConfig>()
            .add_systems(Startup, setup_rendering)
            .add_systems(Update, sync_grid_rendering);
    }
}

#[derive(Resource, Default)]
struct GridMeshSize {
    width: i32,
    height: i32,
}

#[derive(Resource)]
pub struct WorldTuningConfig {
    pub elevation_scale: f32,
}

impl Default for WorldTuningConfig {
    fn default() -> Self {
        Self {
            elevation_scale: 0.15,
        }
    }
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct WorldMaterial {
    #[storage(10, read_only, visibility(vertex))]
    pub grid_buffer: Handle<ShaderStorageBuffer>,

    #[storage(11, read_only, visibility(vertex))]
    pub palette_buffer: Handle<ShaderStorageBuffer>,

    #[uniform(12, visibility(vertex))]
    pub window: WorldWindowUniform,
}

impl Material for WorldMaterial {
    fn vertex_shader() -> ShaderRef {
        "shaders/world.wgsl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "shaders/world.wgsl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Opaque
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
        Ok(())
    }
}

#[derive(Clone, Default, ShaderType, Debug)]
pub struct WorldWindowUniform {
    pub origin: Vec2,
    pub size: Vec2,
    pub head: Vec2,
    pub elevation_scale: f32,
}

#[derive(Component)]
pub struct WorldGridMarker;

fn setup_rendering(
    mut commands: Commands,
    mut materials: ResMut<Assets<WorldMaterial>>,
    mut buffers: ResMut<Assets<ShaderStorageBuffer>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let grid_buffer = buffers.add(ShaderStorageBuffer::new(
        &[0u8; 8], // 8 bytes per cell
        RenderAssetUsages::all(),
    ));

    let mut palette = vec![[0.0f32; 4]; 256];

    palette[0] = [0.00, 0.00, 0.00, 0.0];

    // --- Terrain (0-31) ---
    palette[TerrainMat::STONE.0 as usize] = [0.48, 0.46, 0.44, 1.0];
    palette[TerrainMat::DIRT.0 as usize] = [0.40, 0.28, 0.15, 1.0];
    palette[TerrainMat::SAND.0 as usize] = [0.82, 0.72, 0.48, 1.0];
    palette[TerrainMat::FOLIAGE.0 as usize] = [0.18, 0.45, 0.12, 1.0];
    palette[TerrainMat::WOOD.0 as usize] = [0.45, 0.28, 0.12, 1.0];
    palette[TerrainMat::FLESH.0 as usize] = [0.75, 0.50, 0.45, 1.0];
    palette[TerrainMat::BONE.0 as usize] = [0.88, 0.85, 0.75, 1.0];
    palette[TerrainMat::ROT.0 as usize] = [0.30, 0.22, 0.10, 1.0];
    palette[TerrainMat::ASH.0 as usize] = [0.45, 0.42, 0.40, 1.0];
    palette[TerrainMat::SNOW.0 as usize] = [0.92, 0.94, 0.96, 1.0];
    palette[TerrainMat::CLAY.0 as usize] = [0.68, 0.42, 0.28, 1.0];
    palette[TerrainMat::ICE.0 as usize] = [0.70, 0.85, 0.95, 1.0];
    palette[TerrainMat::METAL.0 as usize] = [0.60, 0.60, 0.65, 1.0];
    palette[TerrainMat::GLASS.0 as usize] = [0.75, 0.88, 0.92, 0.8];

    // --- Fluid (32-63) ---
    palette[(32 + FluidMat::WATER.0) as usize] = [0.15, 0.35, 0.85, 1.0];
    palette[(32 + FluidMat::MAGMA.0) as usize] = [0.85, 0.25, 0.05, 1.0];
    palette[(32 + FluidMat::BLOOD.0) as usize] = [0.55, 0.02, 0.02, 1.0];
    palette[(32 + FluidMat::ACID.0) as usize] = [0.30, 0.75, 0.10, 1.0];
    palette[(32 + FluidMat::OIL.0) as usize] = [0.12, 0.08, 0.04, 1.0];

    // --- Surface (64-95) ---
    palette[(64 + SurfaceMat::FIRE.0) as usize] = [1.00, 0.60, 0.10, 1.0];
    palette[(64 + SurfaceMat::STEAM.0) as usize] = [0.85, 0.85, 0.90, 0.6];
    palette[(64 + SurfaceMat::SMOKE.0) as usize] = [0.30, 0.30, 0.30, 0.7];
    palette[(64 + SurfaceMat::POISON.0) as usize] = [0.40, 0.15, 0.50, 0.5];

    let palette_buffer = buffers.add(ShaderStorageBuffer::new(
        bytemuck::cast_slice(&palette),
        RenderAssetUsages::all(),
    ));

    let material = materials.add(WorldMaterial {
        grid_buffer,
        palette_buffer,
        window: WorldWindowUniform {
            elevation_scale: 0.15,
            ..default()
        },
    });

    commands.spawn((
        Mesh3d(meshes.add(build_procedural_dummy(1, 1))),
        MeshMaterial3d(material),
        Transform::from_translation(Vec3::ZERO),
        NoFrustumCulling,
        WorldGridMarker,
    ));
}

fn sync_grid_rendering(
    mut grid: ResMut<ActiveWorldGrid>,
    mut materials: ResMut<Assets<WorldMaterial>>,
    mut buffers: ResMut<Assets<ShaderStorageBuffer>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mesh_size: ResMut<GridMeshSize>,
    tuning: Res<WorldTuningConfig>,
    mut grid_query: Query<
        (&mut Transform, &mut Mesh3d, &MeshMaterial3d<WorldMaterial>),
        With<WorldGridMarker>,
    >,
) {
    let grid_ref = grid.bypass_change_detection();

    let Ok((mut transform, mut mesh3d, material_handle)) = grid_query.single_mut() else {
        return;
    };

    let Some(material) = materials.get_mut(&material_handle.0) else {
        return;
    };

    material.window.origin = Vec2::new(
        grid_ref.window_origin.x as f32,
        grid_ref.window_origin.y as f32,
    );
    material.window.size = Vec2::new(grid_ref.width as f32, grid_ref.height as f32);
    material.window.head = Vec2::new(grid_ref.buffer_head.x as f32, grid_ref.buffer_head.y as f32);
    material.window.elevation_scale = tuning.elevation_scale;

    if !grid_ref.cells_dirty {
        return;
    }

    let dims_changed = mesh_size.width != grid_ref.width || mesh_size.height != grid_ref.height;
    if dims_changed
        || mesh3d.0.id() == Handle::<Mesh>::default().id()
        || meshes
            .get(&mesh3d.0)
            .map_or(true, |m| m.count_vertices() == 0)
    {
        mesh3d.0 = meshes.add(build_procedural_dummy(
            grid_ref.width as u32,
            grid_ref.height as u32,
        ));
        mesh_size.width = grid_ref.width;
        mesh_size.height = grid_ref.height;
    }

    if let Some(buffer) = buffers.get_mut(&material.grid_buffer) {
        let src: &[u8] = bytemuck::cast_slice(&grid_ref.cells);
        match &mut buffer.data {
            Some(dst) => {
                dst.resize(src.len(), 0);
                dst.copy_from_slice(src);
            }
            slot => *slot = Some(src.to_vec()),
        }
    }

    transform.translation.x = grid_ref.window_origin.x as f32;
    transform.translation.z = -(grid_ref.window_origin.y as f32) - (grid_ref.height as f32) + 1.0;

    grid.cells_dirty = false;
}

fn build_procedural_dummy(width: u32, height: u32) -> Mesh {
    let vertex_count = (width * height * 11 * 6) as usize;
    let positions: Vec<[f32; 3]> = vec![[0.0, 0.0, 0.0]; vertex_count];

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::all());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh
}
