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
use rustc_hash::FxHashSet;
use spatial_lib::chunk::math::ChunkKey;

pub struct WorldRenderPlugin;

impl Plugin for WorldRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<WorldMaterial>::default())
            .init_resource::<WorldTuningConfig>()
            .add_systems(Startup, setup_rendering)
            .add_systems(Update, sync_grid_rendering);
    }
}

#[derive(Resource)]
pub struct WorldTuningConfig {
    pub elevation_scale: f32,
}

impl Default for WorldTuningConfig {
    fn default() -> Self {
        Self {
            elevation_scale: 0.50,
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

/// GPU boundary struct reflecting the active projection window and brush tuning.
#[derive(Clone, Default, ShaderType, Debug)]
pub struct WorldWindowUniform {
    pub origin_size: Vec4, // x: origin.x, y: origin.y, z: size.x, w: size.y
    pub head_cursor: Vec4, // x: head.x,   y: head.y,   z: cursor.x, w: cursor.y
    pub config: Vec4,      // x: elev_scale, y: cursor_radius, z: (pad), w: (pad)
}

#[derive(Resource)]
pub struct ChunkMeshHandle(pub Handle<Mesh>);

#[derive(Resource)]
pub struct GlobalWorldMaterial(pub Handle<WorldMaterial>);

#[derive(Component)]
pub struct ChunkRenderMarker(pub ChunkKey);

/// Allocates an immutable, contiguous dummy mesh mapping exactly to a single physical chunk.
/// Prevents catastrophic VRAM scaling by locking the footprint to ~5MB per instance.
fn build_chunk_dummy(size: u32) -> Mesh {
    let vertex_count = (size * size * 120) as usize;
    let positions: Vec<[f32; 3]> = vec![[0.0, 0.0, 0.0]; vertex_count];

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::all());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh
}

fn setup_rendering(
    mut commands: Commands,
    mut materials: ResMut<Assets<WorldMaterial>>,
    mut buffers: ResMut<Assets<ShaderStorageBuffer>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let grid_buffer = buffers.add(ShaderStorageBuffer::new(
        &[0u8; 8],
        RenderAssetUsages::all(),
    ));

    let mut palette = vec![[0.0f32; 4]; 256];
    palette[0] = [0.00, 0.00, 0.00, 0.0];

    // Terrain Offset (0)
    palette[TerrainMat::TERRAIN_STONE.0 as usize] = [0.48, 0.46, 0.44, 1.0];
    palette[TerrainMat::TERRAIN_DIRT.0 as usize] = [0.40, 0.28, 0.15, 1.0];
    palette[TerrainMat::TERRAIN_SANDSTONE.0 as usize] = [0.65, 0.55, 0.35, 1.0];
    palette[TerrainMat::TERRAIN_ICE.0 as usize] = [0.70, 0.85, 0.95, 1.0];
    palette[TerrainMat::TERRAIN_METAL.0 as usize] = [0.60, 0.60, 0.65, 1.0];
    palette[TerrainMat::TERRAIN_CORRUPTION.0 as usize] = [0.35, 0.15, 0.40, 1.0];

    // Granular Offset (32)
    palette[(32 + GranularMat::GRANULAR_DIRT.0) as usize] = [0.45, 0.32, 0.18, 1.0];
    palette[(32 + GranularMat::GRANULAR_SAND.0) as usize] = [0.82, 0.72, 0.48, 1.0];
    palette[(32 + GranularMat::GRANULAR_MUD.0) as usize] = [0.25, 0.18, 0.10, 1.0];
    palette[(32 + GranularMat::GRANULAR_GRAVEL.0) as usize] = [0.40, 0.40, 0.42, 1.0];
    palette[(32 + GranularMat::GRANULAR_SNOW.0) as usize] = [0.92, 0.94, 0.96, 1.0];
    palette[(32 + GranularMat::GRANULAR_LIQUID_METAL.0) as usize] = [0.75, 0.75, 0.80, 1.0];
    palette[(32 + GranularMat::GRANULAR_CORRUPTION.0) as usize] = [0.45, 0.20, 0.50, 1.0];

    // Fluid Offset (64)
    palette[(64 + FluidMat::FLUID_WATER.0) as usize] = [0.15, 0.35, 0.85, 1.0];
    palette[(64 + FluidMat::FLUID_MAGMA.0) as usize] = [0.85, 0.25, 0.05, 1.0];
    palette[(64 + FluidMat::FLUID_BLOOD.0) as usize] = [0.55, 0.02, 0.02, 1.0];
    palette[(64 + FluidMat::FLUID_ACID.0) as usize] = [0.30, 0.75, 0.10, 1.0];
    palette[(64 + FluidMat::FLUID_OIL.0) as usize] = [0.12, 0.08, 0.04, 1.0];
    palette[(64 + FluidMat::FLUID_CORRUPTION.0) as usize] = [0.25, 0.05, 0.30, 1.0];

    // Surface Offset (96)
    palette[(96 + SurfaceMat::SURFACE_FIRE.0) as usize] = [1.00, 0.60, 0.10, 1.0];
    palette[(96 + SurfaceMat::SURFACE_FOLIAGE.0) as usize] = [0.18, 0.45, 0.12, 1.0];
    palette[(96 + SurfaceMat::SURFACE_WOOD.0) as usize] = [0.45, 0.28, 0.12, 1.0];
    palette[(96 + SurfaceMat::SURFACE_FLESH.0) as usize] = [0.75, 0.50, 0.45, 1.0];
    palette[(96 + SurfaceMat::SURFACE_BONE.0) as usize] = [0.88, 0.85, 0.75, 1.0];
    palette[(96 + SurfaceMat::SURFACE_ROT.0) as usize] = [0.30, 0.22, 0.10, 1.0];
    palette[(96 + SurfaceMat::SURFACE_ASH.0) as usize] = [0.45, 0.42, 0.40, 1.0];
    palette[(96 + SurfaceMat::SURFACE_SNOW.0) as usize] = [0.95, 0.95, 0.98, 1.0];
    palette[(96 + SurfaceMat::SURFACE_CLAY.0) as usize] = [0.68, 0.42, 0.28, 1.0];
    palette[(96 + SurfaceMat::SURFACE_ICE.0) as usize] = [0.80, 0.90, 0.95, 0.8];
    palette[(96 + SurfaceMat::SURFACE_METAL.0) as usize] = [0.55, 0.55, 0.60, 1.0];
    palette[(96 + SurfaceMat::SURFACE_GLASS.0) as usize] = [0.75, 0.88, 0.92, 0.6];
    palette[(96 + SurfaceMat::SURFACE_CORRUPTION.0) as usize] = [0.50, 0.10, 0.60, 1.0];

    let palette_buffer = buffers.add(ShaderStorageBuffer::new(
        bytemuck::cast_slice(&palette),
        RenderAssetUsages::all(),
    ));

    let material = materials.add(WorldMaterial {
        grid_buffer,
        palette_buffer,
        window: WorldWindowUniform {
            config: Vec4::new(0.15, -1.0, 0.0, 0.0), // elev_scale, cursor_radius (hidden)
            ..default()
        },
    });

    let chunk_mesh = meshes.add(build_chunk_dummy(CHUNK_SIZE as u32));

    commands.insert_resource(GlobalWorldMaterial(material));
    commands.insert_resource(ChunkMeshHandle(chunk_mesh));
}

/// Enforces spatial synchronization between the ECS hierarchy and the active SSBO matrix.
fn sync_grid_rendering(
    mut commands: Commands,
    mut grid: ResMut<ActiveWorldGrid>,
    manager: Res<WorldManager>,
    mut materials: ResMut<Assets<WorldMaterial>>,
    mut buffers: ResMut<Assets<ShaderStorageBuffer>>,
    chunk_mesh: Res<ChunkMeshHandle>,
    global_material: Res<GlobalWorldMaterial>,
    tuning: Res<WorldTuningConfig>,
    chunk_query: Query<(Entity, &ChunkRenderMarker)>,
) {
    let grid_ref = grid.bypass_change_detection();

    // Maintain ToroidalGrid SSBO memory projection
    if grid_ref.cells_dirty {
        if let Some(material) = materials.get_mut(&global_material.0) {
            material.window.origin_size = Vec4::new(
                grid_ref.spatial.window_origin.x as f32,
                grid_ref.spatial.window_origin.y as f32,
                grid_ref.spatial.width as f32,
                grid_ref.spatial.height as f32,
            );

            // Overwrite head positions; brush cursor coordinates (Z/W) are injected asynchronously
            material.window.head_cursor.x = grid_ref.spatial.buffer_head.x as f32;
            material.window.head_cursor.y = grid_ref.spatial.buffer_head.y as f32;
            material.window.config.x = tuning.elevation_scale;

            if let Some(buffer) = buffers.get_mut(&material.grid_buffer) {
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

            commands.spawn((
                Mesh3d(chunk_mesh.0.clone()),
                MeshMaterial3d(global_material.0.clone()),
                // Translate the chunk instance mapping mathematical X/Y to physical X/-Z
                Transform::from_xyz(chunk_x as f32, 0.0, -(chunk_y as f32)),
                ChunkRenderMarker(key),
                NoFrustumCulling,
            ));
        }
    }
}
