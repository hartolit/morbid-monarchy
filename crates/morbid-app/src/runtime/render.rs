use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::{
        render_resource::{AsBindGroup, ShaderType},
        storage::ShaderStorageBuffer,
    },
    shader::ShaderRef,
    sprite_render::{Material2d, Material2dPlugin},
};
use monarch_engine::prelude::ActiveWorldGrid;

pub struct WorldRenderPlugin;

impl Plugin for WorldRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(Material2dPlugin::<WorldMaterial>::default())
            .add_systems(Startup, setup_rendering)
            .add_systems(Update, sync_grid_rendering);
    }
}

/// The binding structure mapping Bevy to the WGSL shader
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct WorldMaterial {
    #[storage(0, read_only)]
    pub grid_buffer: Handle<ShaderStorageBuffer>,
    #[storage(1, read_only)]
    pub palette_buffer: Handle<ShaderStorageBuffer>,
    #[uniform(2)]
    pub window: WorldWindowUniform,
}

impl Material2d for WorldMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/world.wgsl".into()
    }
}

#[derive(Clone, Default, ShaderType, Debug)]
pub struct WorldWindowUniform {
    pub origin: Vec2,
    pub size: Vec2,
    pub head: Vec2,
}

#[derive(Component)]
pub struct WorldQuadMarker;

fn setup_rendering(
    mut commands: Commands,
    mut materials: ResMut<Assets<WorldMaterial>>,
    mut buffers: ResMut<Assets<ShaderStorageBuffer>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    // Initialize Grid Buffer (Will be properly sized on first sync)
    let grid_buffer = buffers.add(ShaderStorageBuffer::new(
        &[0u8; 4],
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    ));

    // Initialize Palette (Maps MaterialId 0..255 to RGBA base colors)
    let mut palette = vec![[0.0f32; 4]; 256];

    // System
    palette[0] = [0.0, 0.0, 0.0, 0.0];       // EMPTY
    palette[255] = [0.0, 0.0, 0.0, 1.0];      // VOID

    // Liquids (1-31)
    palette[1] = [0.15, 0.35, 0.85, 1.0];     // LIQUID_WATER
    palette[2] = [0.85, 0.25, 0.05, 1.0];     // LIQUID_MAGMA
    palette[3] = [0.55, 0.02, 0.02, 1.0];     // LIQUID_BLOOD
    palette[4] = [0.30, 0.75, 0.10, 1.0];     // LIQUID_ACID
    palette[5] = [0.12, 0.08, 0.04, 1.0];     // LIQUID_OIL

    // Gases & Plasmas (32-63)
    palette[32] = [0.85, 0.85, 0.90, 0.6];    // GAS_STEAM
    palette[33] = [0.30, 0.30, 0.30, 0.7];    // GAS_SMOKE
    palette[34] = [0.40, 0.15, 0.50, 0.5];    // GAS_POISON
    palette[35] = [1.00, 0.60, 0.10, 1.0];    // FIRE

    // Organics (64-127)
    palette[64] = [0.45, 0.28, 0.12, 1.0];    // ORGANIC_WOOD
    palette[65] = [0.18, 0.45, 0.12, 1.0];    // ORGANIC_FOLIAGE
    palette[66] = [0.75, 0.50, 0.45, 1.0];    // ORGANIC_FLESH
    palette[67] = [0.88, 0.85, 0.75, 1.0];    // ORGANIC_BONE
    palette[68] = [0.30, 0.22, 0.10, 1.0];    // ORGANIC_ROT

    // Powders & Loose Solids (128-191)
    palette[128] = [0.82, 0.72, 0.48, 1.0];   // LOOSE_SAND
    palette[129] = [0.40, 0.28, 0.15, 1.0];   // LOOSE_DIRT
    palette[130] = [0.45, 0.42, 0.40, 1.0];   // LOOSE_ASH
    palette[131] = [0.92, 0.94, 0.96, 1.0];   // LOOSE_SNOW

    // Solids (192-254)
    palette[192] = [0.48, 0.46, 0.44, 1.0];   // SOLID_STONE
    palette[193] = [0.68, 0.42, 0.28, 1.0];   // SOLID_CLAY
    palette[194] = [0.70, 0.85, 0.95, 1.0];   // SOLID_ICE
    palette[195] = [0.60, 0.60, 0.65, 1.0];   // SOLID_METAL
    palette[196] = [0.75, 0.88, 0.92, 0.8];   // SOLID_GLASS

    let palette_buffer = buffers.add(ShaderStorageBuffer::new(
        bytemuck::cast_slice(&palette),
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    ));

    let material = materials.add(WorldMaterial {
        grid_buffer,
        palette_buffer,
        window: WorldWindowUniform::default(),
    });

    // Spawn the Canvas Quad
    // Note: The size matches the engine's active window size (1024x1024)
    commands.spawn((
        Mesh2d(meshes.add(Rectangle::new(768.0, 768.0))),
        MeshMaterial2d(material),
        Transform::from_translation(Vec3::new(384.0, 384.0, 0.0)),
        WorldQuadMarker,
    ));
}

/// Translates the pure engine grid state into the GPU-bound Storage Buffers,
/// and ensures the rendering Quad perfectly maps to the physical simulation grid.
fn sync_grid_rendering(
    grid: Res<ActiveWorldGrid>,
    mut materials: ResMut<Assets<WorldMaterial>>,
    mut buffers: ResMut<Assets<ShaderStorageBuffer>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut quad_query: Query<
        (&mut Transform, &mut Mesh2d, &MeshMaterial2d<WorldMaterial>),
        With<WorldQuadMarker>,
    >,
) {
    if !grid.is_changed() {
        return;
    }

    let Ok((mut transform, mut mesh2d, material_handle)) = quad_query.single_mut() else {
        return;
    };

    let Some(material) = materials.get_mut(&material_handle.0) else {
        return;
    };

    let grid_w = grid.width as f32;
    let grid_h = grid.height as f32;

    // Center the Quad perfectly over the Toroidal grid's mathematical bounds.
    // The window_origin is the bottom-left corner, so the center is origin + (size / 2.0)
    transform.translation.x = grid.window_origin.x as f32 + (grid_w / 2.0);
    transform.translation.y = grid.window_origin.y as f32 + (grid_h / 2.0);

    // Resize the actual Bevy Mesh so it never overhangs the buffer and creates Toroidal Seams
    mesh2d.0 = meshes.add(Rectangle::new(grid_w, grid_h));

    // Update Uniform offsets for the WGSL Shader
    material.window.origin = Vec2::new(grid.window_origin.x as f32, grid.window_origin.y as f32);
    material.window.size = Vec2::new(grid_w, grid_h);
    material.window.head = Vec2::new(grid.buffer_head.x as f32, grid.buffer_head.y as f32);

    // Sync Memory payload (Zero-cost safe cast from `WorldCell` Vec -> `u8` slice)
    if let Some(buffer) = buffers.get_mut(&material.grid_buffer) {
        let bytes: &[u8] = bytemuck::cast_slice(&grid.cells);

        *buffer = ShaderStorageBuffer::new(
            bytes,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        );
    }
}
