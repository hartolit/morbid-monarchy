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
use monarch_engine::world::grid::ActiveWorldGrid;

pub struct WorldRenderPlugin;

impl Plugin for WorldRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(Material2dPlugin::<WorldMaterial>::default())
            .add_systems(Startup, setup_rendering)
            .add_systems(Update, (sync_grid_rendering, sync_quad_to_camera))
            .add_systems(PostUpdate, sync_quad_to_camera);
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

    // Initialize Palette (Maps MaterialId 0..255 to RGBA)
    let mut palette = vec![[0.0f32; 4]; 256];
    palette[0] = [0.0, 0.0, 0.0, 0.0]; // Empty
    palette[1] = [0.15, 0.35, 0.85, 1.0]; // Water
    palette[2] = [0.65, 0.05, 0.05, 1.0]; // Blood
    palette[3] = [0.15, 0.40, 0.10, 1.0]; // Grass
    palette[4] = [0.80, 0.70, 0.45, 1.0]; // Sand

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
        Mesh2d(meshes.add(Rectangle::new(1024.0, 1024.0))),
        MeshMaterial2d(material), // <-- Modern Bevy uses the MeshMaterial2d component wrapper
        Transform::from_translation(Vec3::new(512.0, 512.0, 0.0)),
        WorldQuadMarker,
    ));
}

/// Translates the pure engine grid state into the GPU-bound Storage Buffers
fn sync_grid_rendering(
    grid: Res<ActiveWorldGrid>,
    mut materials: ResMut<Assets<WorldMaterial>>,
    mut buffers: ResMut<Assets<ShaderStorageBuffer>>,
    material_handles: Query<&MeshMaterial2d<WorldMaterial>, With<WorldQuadMarker>>,
) {
    if !grid.is_changed() {
        return;
    }

    let Ok(material_handle) = material_handles.single() else {
        return;
    };

    let Some(material) = materials.get_mut(&material_handle.0) else {
        return;
    };

    // Update Uniform offsets
    material.window.origin = Vec2::new(grid.window_origin.x as f32, grid.window_origin.y as f32);
    material.window.size = Vec2::new(grid.width as f32, grid.height as f32);

    // Sync Memory payload (Zero-cost safe cast from `WorldCell` -> `u8` slice)
    if let Some(buffer) = buffers.get_mut(&material.grid_buffer) {
        let bytes: &[u8] = bytemuck::cast_slice(&grid.cells);

        *buffer = ShaderStorageBuffer::new(
            bytes,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        );
    }
}

/// Keeps the rendering canvas perfectly centered over the camera to prevent edge clipping
fn sync_quad_to_camera(
    camera_query: Query<&Transform, (With<Camera2d>, Without<WorldQuadMarker>)>,
    mut quad_query: Query<&mut Transform, With<WorldQuadMarker>>,
) {
    if let Ok(camera_transform) = camera_query.single() {
        if let Ok(mut quad_transform) = quad_query.single_mut() {
            quad_transform.translation.x = camera_transform.translation.x;
            quad_transform.translation.y = camera_transform.translation.y;
        }
    }
}
