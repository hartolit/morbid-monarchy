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
use monarch_engine::prelude::ActiveWorldGrid;

pub struct WorldRenderPlugin;

impl Plugin for WorldRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<WorldMaterial>::default())
            .init_resource::<GridMeshSize>()
            .add_systems(Startup, setup_rendering)
            .add_systems(Update, sync_grid_rendering);
    }
}

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

/// Tracks the cell dimensions of the last-built procedural mesh so we only
/// rebuild it when the grid is actually resized (not on every chunk load).
#[derive(Resource, Default)]
struct GridMeshSize {
    width: i32,
    height: i32,
}

// ---------------------------------------------------------------------------
// Material
// ---------------------------------------------------------------------------

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

    // Disable prepasses: the custom vertex shader is incompatible with Bevy's
    // standard prepass fragment interface (no UVs, non-standard clip-space math).
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
        // The procedural shader derives all geometry from @builtin(vertex_index).
        // No custom vertex attributes are needed. We only bind POSITION so Bevy's
        // pipeline validator sees a non-empty layout — the values are never read.
        let vertex_layout = layout
            .0
            .get_layout(&[Mesh::ATTRIBUTE_POSITION.at_shader_location(0)])?;
        descriptor.vertex.buffers = vec![vertex_layout];
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Uniform
// ---------------------------------------------------------------------------

#[derive(Clone, Default, ShaderType, Debug)]
pub struct WorldWindowUniform {
    /// Bottom-left world coordinate of the active window.
    pub origin: Vec2,
    /// Width and height of the active window in cells.
    pub size: Vec2,
    /// Toroidal buffer head offset (in cells).
    pub head: Vec2,
    /// Maximum terrain height in world units (maps to atmosphere.state == 0).
    pub h_max: f32,
    /// World units of height per unit of atmosphere/fluid state.
    pub elevation_scale: f32,
}

// ---------------------------------------------------------------------------
// Marker
// ---------------------------------------------------------------------------

#[derive(Component)]
pub struct WorldGridMarker;

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

fn setup_rendering(
    mut commands: Commands,
    mut materials: ResMut<Assets<WorldMaterial>>,
    mut buffers: ResMut<Assets<ShaderStorageBuffer>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    // Allocate a minimal placeholder buffer — actual data is written on the
    // first Update tick once the grid resource is ready.
    let grid_buffer = buffers.add(ShaderStorageBuffer::new(
        &[0u8; 4],
        RenderAssetUsages::all(),
    ));

    // Static colour palette: MaterialId (0-255) → linear RGBA.
    let mut palette = vec![[0.0f32; 4]; 256];

    palette[0] = [0.00, 0.00, 0.00, 0.0]; // EMPTY   (transparent / invisible)
    palette[255] = [0.00, 0.00, 0.00, 1.0]; // VOID

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

    // Spawn a single dummy mesh entity.  The mesh itself only needs to carry
    // enough vertices for Bevy to issue the right draw-call count.  The vertex
    // shader ignores the actual position values and reconstructs world-space
    // geometry from @builtin(vertex_index) alone.  We start with a 1-cell
    // placeholder; sync_grid_rendering will replace it once the grid is live.
    commands.spawn((
        Mesh3d(meshes.add(build_procedural_dummy(1, 1))),
        MeshMaterial3d(material),
        // Anchor at the world origin; the shader adds world-space offsets itself.
        Transform::from_translation(Vec3::ZERO),
        // The shader places all geometry from @builtin(vertex_index) — Bevy
        // sees only a zero-size dummy AABB and would cull this entity the
        // moment the camera moves away from the origin.  Disable CPU-side
        // frustum culling entirely; the GPU discards degenerate triangles.
        NoFrustumCulling,
        WorldGridMarker,
    ));
}

// ---------------------------------------------------------------------------
// Per-frame sync
// ---------------------------------------------------------------------------

fn sync_grid_rendering(
    mut grid: ResMut<ActiveWorldGrid>,
    mut materials: ResMut<Assets<WorldMaterial>>,
    mut buffers: ResMut<Assets<ShaderStorageBuffer>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mesh_size: ResMut<GridMeshSize>,
    mut grid_query: Query<
        (&mut Transform, &mut Mesh3d, &MeshMaterial3d<WorldMaterial>),
        With<WorldGridMarker>,
    >,
) {
    // Use bypass_change_detection for all reads so that merely observing the
    // grid fields does not mark the resource as changed and re-trigger Bevy's
    // RenderAsset extraction pipeline on frames where nothing actually mutated.
    // We only call DerefMut (via the raw ResMut) at the very end when we
    // explicitly clear cells_dirty — that is the one intentional mutation.
    let grid_ref = grid.bypass_change_detection();

    let Ok((mut transform, mut mesh3d, material_handle)) = grid_query.single_mut() else {
        return;
    };

    let Some(material) = materials.get_mut(&material_handle.0) else {
        return;
    };

    // --- Cheap uniform update (always) ---
    // buffer_head and window_origin change on every camera movement tick
    // (toroidal shift).  Pushing them costs ~32 bytes per frame — essentially
    // free.  We read through grid_ref so no change-detection flag is set.
    material.window.origin = Vec2::new(
        grid_ref.window_origin.x as f32,
        grid_ref.window_origin.y as f32,
    );
    material.window.size = Vec2::new(grid_ref.width as f32, grid_ref.height as f32);
    material.window.head = Vec2::new(grid_ref.buffer_head.x as f32, grid_ref.buffer_head.y as f32);
    // TODO: expose h_max / elevation_scale to a typed tuning Resource.
    material.window.h_max = 50.0;
    material.window.elevation_scale = 0.15;

    // -----------------------------------------------------------------------
    // Expensive path: only runs when actual cell data changed.
    // -----------------------------------------------------------------------
    // cells_dirty is set by load_chunk(), set_cell(), and resize_in_place().
    // It is NOT set by shift_window() — moving the camera never triggers this.
    if !grid_ref.cells_dirty {
        return;
    }

    // --- Rebuild the procedural dummy mesh when grid dimensions change ---
    // The dummy mesh only carries vertex count information (POSITION attribute
    // with the right number of entries).  The shader ignores the values.
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

    // --- Upload cell buffer in-place ---
    // Write directly into buffer.data to reuse the existing GPU allocation.
    // This avoids creating a new ShaderStorageBuffer asset (and thus a new
    // wgpu buffer object) on every update — only the bytes are transferred.
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

    // Snap the mesh to the world origin
    // Engine (+Y) renders at Bevy (-Z)
    transform.translation.x = grid_ref.window_origin.x as f32;
    transform.translation.z = -(grid_ref.window_origin.y as f32) - (grid_ref.height as f32) + 1.0;

    // Acknowledge the upload: clear the dirty flag.
    // This is the only place we take a true &mut — it goes through DerefMut
    // and intentionally marks the resource changed so Bevy knows the flag
    // write happened, but by this point the expensive work is already done.
    grid.cells_dirty = false;
}

// ---------------------------------------------------------------------------
// Procedural dummy mesh
// ---------------------------------------------------------------------------

/// Builds a minimal mesh whose only purpose is to tell Bevy's draw-call
/// machinery how many vertices to emit.  The vertex shader ignores all
/// attribute values and reconstructs world-space geometry purely from
/// `@builtin(vertex_index)`.
///
/// Layout driven by the shader — 7 face-slots × 6 verts per cell:
///   • Slot 0   — Terrain top cap        (1 face  × 6 verts)
///   • Slots 1-4 — Terrain side walls    (4 faces × 6 verts)
///   • Slot 5   — Fluid top cap          (1 face  × 6 verts, degenerate if no fluid)
///   • Slot 6   — Surface top cap        (1 face  × 6 verts, degenerate if no surface)
///
/// Total vertices = width × height × 7 × 6 = width × height × 42
///
/// We store a dummy POSITION attribute (all zeros) because Bevy requires at
/// least one vertex attribute to build a valid pipeline layout.  The shader
/// reads `@location(0) position` but immediately discards it.
fn build_procedural_dummy(width: u32, height: u32) -> Mesh {
    // 7 face-slots × 6 verts (non-indexed triangles)
    let vertex_count = (width * height * 7 * 6) as usize;

    // All-zero positions: the shader never reads these values.
    let positions: Vec<[f32; 3]> = vec![[0.0, 0.0, 0.0]; vertex_count];

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::all());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    // No index buffer — vertices are emitted in order, shader uses vertex_index.
    mesh
}
