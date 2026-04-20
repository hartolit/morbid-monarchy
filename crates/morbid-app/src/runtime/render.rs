use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, MeshVertexAttribute, MeshVertexBufferLayoutRef, PrimitiveTopology},
    prelude::*,
    render::{
        render_resource::{
            AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
            VertexFormat,
        },
        storage::ShaderStorageBuffer,
    },
    shader::ShaderRef,
};
use monarch_engine::prelude::ActiveWorldGrid;

pub struct WorldRenderPlugin;

/// Tracks the dimensions of the last-built voxel grid mesh so we can detect
/// when the grid is resized and the mesh needs to be rebuilt.
#[derive(Resource, Default)]
struct GridMeshSize {
    width: i32,
    height: i32,
}

impl Plugin for WorldRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<WorldMaterial>::default())
            .init_resource::<GridMeshSize>()
            .add_systems(Startup, setup_rendering)
            .add_systems(Update, sync_grid_rendering);
    }
}

pub const ATTRIBUTE_CELL_INDEX: MeshVertexAttribute =
    MeshVertexAttribute::new("Vertex_Cell_Index", 10, VertexFormat::Uint32);
pub const ATTRIBUTE_LAYER: MeshVertexAttribute =
    MeshVertexAttribute::new("Vertex_Layer", 11, VertexFormat::Uint32);

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

    // The custom vertex shader displaces geometry in ways the standard prepass
    // shaders cannot replicate — disabling prevents the pipeline validator from
    // matching our sparse VertexOutput against the default prepass fragment
    // interface which expects a full UV output at @location(2).
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
        // Declare exactly the four attributes the shader reads. Without this,
        // Bevy's default layout omits the custom attributes and the pipeline
        // validator rejects Location[10] / Location[11] as unsatisfied inputs.
        let vertex_layout = layout.0.get_layout(&[
            Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
            Mesh::ATTRIBUTE_NORMAL.at_shader_location(1),
            ATTRIBUTE_CELL_INDEX.at_shader_location(10),
            ATTRIBUTE_LAYER.at_shader_location(11),
        ])?;
        descriptor.vertex.buffers = vec![vertex_layout];
        Ok(())
    }
}

#[derive(Clone, Default, ShaderType, Debug)]
pub struct WorldWindowUniform {
    pub origin: Vec2,
    pub size: Vec2,
    pub head: Vec2,
    pub h_max: f32,
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
        &[0u8; 4],
        RenderAssetUsages::all(),
    ));

    // Palette: maps MaterialId (0..255) to a linear RGBA base colour.
    let mut palette = vec![[0.0f32; 4]; 256];

    palette[0] = [0.0, 0.0, 0.0, 0.0]; // EMPTY
    palette[255] = [0.0, 0.0, 0.0, 1.0]; // VOID

    // Liquids (1-31)
    palette[1] = [0.15, 0.35, 0.85, 1.0]; // LIQUID_WATER
    palette[2] = [0.85, 0.25, 0.05, 1.0]; // LIQUID_MAGMA
    palette[3] = [0.55, 0.02, 0.02, 1.0]; // LIQUID_BLOOD
    palette[4] = [0.30, 0.75, 0.10, 1.0]; // LIQUID_ACID
    palette[5] = [0.12, 0.08, 0.04, 1.0]; // LIQUID_OIL

    // Gases & Plasmas (32-63)
    palette[32] = [0.85, 0.85, 0.90, 0.6]; // GAS_STEAM
    palette[33] = [0.30, 0.30, 0.30, 0.7]; // GAS_SMOKE
    palette[34] = [0.40, 0.15, 0.50, 0.5]; // GAS_POISON
    palette[35] = [1.00, 0.60, 0.10, 1.0]; // FIRE

    // Organics (64-127)
    palette[64] = [0.45, 0.28, 0.12, 1.0]; // ORGANIC_WOOD
    palette[65] = [0.18, 0.45, 0.12, 1.0]; // ORGANIC_FOLIAGE
    palette[66] = [0.75, 0.50, 0.45, 1.0]; // ORGANIC_FLESH
    palette[67] = [0.88, 0.85, 0.75, 1.0]; // ORGANIC_BONE
    palette[68] = [0.30, 0.22, 0.10, 1.0]; // ORGANIC_ROT

    // Powders & Loose Solids (128-191)
    palette[128] = [0.82, 0.72, 0.48, 1.0]; // LOOSE_SAND
    palette[129] = [0.40, 0.28, 0.15, 1.0]; // LOOSE_DIRT
    palette[130] = [0.45, 0.42, 0.40, 1.0]; // LOOSE_ASH
    palette[131] = [0.92, 0.94, 0.96, 1.0]; // LOOSE_SNOW

    // Solids (192-254)
    palette[192] = [0.48, 0.46, 0.44, 1.0]; // SOLID_STONE
    palette[193] = [0.68, 0.42, 0.28, 1.0]; // SOLID_CLAY
    palette[194] = [0.70, 0.85, 0.95, 1.0]; // SOLID_ICE
    palette[195] = [0.60, 0.60, 0.65, 1.0]; // SOLID_METAL
    palette[196] = [0.75, 0.88, 0.92, 0.8]; // SOLID_GLASS

    let palette_buffer = buffers.add(ShaderStorageBuffer::new(
        bytemuck::cast_slice(&palette),
        RenderAssetUsages::all(),
    ));

    let material = materials.add(WorldMaterial {
        grid_buffer,
        palette_buffer,
        window: WorldWindowUniform {
            h_max: 50.0,
            elevation_scale: 0.15,
            ..default()
        },
    });

    commands.spawn((
        Mesh3d(meshes.add(Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::all(),
        ))),
        MeshMaterial3d(material),
        Transform::from_translation(Vec3::ZERO),
        WorldGridMarker,
    ));
}

fn sync_grid_rendering(
    grid: Res<ActiveWorldGrid>,
    mut materials: ResMut<Assets<WorldMaterial>>,
    mut buffers: ResMut<Assets<ShaderStorageBuffer>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mesh_size: ResMut<GridMeshSize>,
    mut grid_query: Query<
        (&mut Transform, &mut Mesh3d, &MeshMaterial3d<WorldMaterial>),
        With<WorldGridMarker>,
    >,
) {
    if !grid.is_changed() {
        return;
    }

    let Ok((mut transform, mut mesh3d, material_handle)) = grid_query.single_mut() else {
        return;
    };
    let Some(material) = materials.get_mut(&material_handle.0) else {
        return;
    };

    material.window.origin = Vec2::new(grid.window_origin.x as f32, grid.window_origin.y as f32);
    material.window.size = Vec2::new(grid.width as f32, grid.height as f32);
    material.window.head = Vec2::new(grid.buffer_head.x as f32, grid.buffer_head.y as f32);
    // TODO: expose h_max / elevation_scale to a typed tuning Resource.
    material.window.h_max = 50.0;
    material.window.elevation_scale = 0.15;

    transform.translation.x = grid.window_origin.x as f32;
    transform.translation.z = grid.window_origin.y as f32;

    // Rebuild the static geometry when the grid dimensions change or the mesh
    // is missing / empty (e.g. first frame or after a resize event).
    let dims_changed = mesh_size.width != grid.width || mesh_size.height != grid.height;
    if dims_changed
        || mesh3d.0.id() == Handle::<Mesh>::default().id()
        || meshes
            .get(&mesh3d.0)
            .map_or(true, |m| m.count_vertices() == 0)
    {
        mesh3d.0 = meshes.add(build_voxel_grid(grid.width as u32, grid.height as u32));
        mesh_size.width = grid.width;
        mesh_size.height = grid.height;
    }

    // Update cell data in-place by writing directly into the buffer's byte vec.
    // This avoids allocating a new GPU buffer object every frame, which was the
    // dominant cause of per-frame lag.
    if let Some(buffer) = buffers.get_mut(&material.grid_buffer) {
        let src: &[u8] = bytemuck::cast_slice(&grid.cells);
        match &mut buffer.data {
            Some(dst) => {
                dst.resize(src.len(), 0);
                dst.copy_from_slice(src);
            }
            slot => *slot = Some(src.to_vec()),
        }
    }
}

/// Builds a static mesh of `width × height` cells, each with 3 layer-slabs
/// (terrain, fluid, surface). Every slab is a unit cube; the vertex shader
/// stretches and culls them at runtime from storage-buffer data.
///
/// Z is flipped relative to the grid row index so that row 0 (the engine's
/// southern / minimum-Y edge) maps to the largest Z offset. This matches
/// Bevy's right-hand Y-up convention where a camera positioned at positive Z
/// looking toward the origin sees the southern edge at the bottom of the screen.
fn build_voxel_grid(width: u32, height: u32) -> Mesh {
    let cell_count = (width * height) as usize;
    let verts_per_face = 4usize;
    let faces_per_cube = 6usize;
    let layers = 3usize;
    let total_verts = cell_count * faces_per_cube * verts_per_face * layers;
    let total_indices = cell_count * faces_per_cube * 6 * layers;

    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(total_verts);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity(total_verts);
    let mut indices: Vec<u32> = Vec::with_capacity(total_indices);
    let mut cell_indices: Vec<u32> = Vec::with_capacity(total_verts);
    let mut layer_ids: Vec<u32> = Vec::with_capacity(total_verts);

    let mut index_offset: u32 = 0;

    // Unit-cube corner positions, indexed 0..7.
    let v_pos: [[f32; 3]; 8] = [
        [0., 0., 0.], // 0 front-bottom-left
        [1., 0., 0.], // 1 front-bottom-right
        [1., 1., 0.], // 2 front-top-right
        [0., 1., 0.], // 3 front-top-left
        [0., 0., 1.], // 4 back-bottom-left
        [1., 0., 1.], // 5 back-bottom-right
        [1., 1., 1.], // 6 back-top-right
        [0., 1., 1.], // 7 back-top-left
    ];

    // Each face: corner indices in CCW winding order + outward normal.
    let face_defs: [(usize, usize, usize, usize, [f32; 3]); 6] = [
        (0, 1, 2, 3, [0., 0., -1.]), // Front  (-Z)
        (5, 4, 7, 6, [0., 0., 1.]),  // Back   (+Z)
        (3, 2, 6, 7, [0., 1., 0.]),  // Top    (+Y)
        (4, 5, 1, 0, [0., -1., 0.]), // Bottom (-Y)
        (1, 5, 6, 2, [1., 0., 0.]),  // Right  (+X)
        (4, 0, 3, 7, [-1., 0., 0.]), // Left   (-X)
    ];

    for layer in 0..3u32 {
        for y in 0..height {
            for x in 0..width {
                // cell_index encodes the logical (x, y) grid position for the shader.
                let cell_idx: u32 = y * width + x;
                let offset_x = x as f32;
                // Flip Z: engine row 0 is the southern (minimum-Y) edge.
                // The camera sits at positive Z looking toward -Z, so row 0
                // must occupy the largest Z to appear at the screen bottom.
                let offset_z = (height - 1 - y) as f32;

                for (a, b, c, d, n) in face_defs {
                    positions.push([v_pos[a][0] + offset_x, v_pos[a][1], v_pos[a][2] + offset_z]);
                    positions.push([v_pos[b][0] + offset_x, v_pos[b][1], v_pos[b][2] + offset_z]);
                    positions.push([v_pos[c][0] + offset_x, v_pos[c][1], v_pos[c][2] + offset_z]);
                    positions.push([v_pos[d][0] + offset_x, v_pos[d][1], v_pos[d][2] + offset_z]);

                    for _ in 0..4 {
                        normals.push(n);
                        cell_indices.push(cell_idx);
                        layer_ids.push(layer);
                    }

                    // CCW winding from outside the cube (Bevy/wgpu default: CCW = front face).
                    // Previous order (0,1,2, 0,2,3) was CW from outside, causing all faces to
                    // be back-facing and culled, producing the "open cube" pyramid artifact.
                    indices.extend_from_slice(&[
                        index_offset,
                        index_offset + 2,
                        index_offset + 1,
                        index_offset,
                        index_offset + 3,
                        index_offset + 2,
                    ]);
                    index_offset += 4;
                }
            }
        }
    }

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::all());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(ATTRIBUTE_CELL_INDEX, cell_indices);
    mesh.insert_attribute(ATTRIBUTE_LAYER, layer_ids);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}
