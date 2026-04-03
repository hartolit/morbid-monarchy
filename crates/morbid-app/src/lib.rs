mod streaming;

use bevy::{
    asset::RenderAssetUsages,
    image::ImagePlugin,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
    window::{Window, WindowPlugin, WindowResolution},
};
use monarch_engine::world::{MaterialId, WorldState};
use streaming::ChunkStreamingState;

const WINDOW_WIDTH: u32 = 1024;
const WINDOW_HEIGHT: u32 = 1024;
const PIXEL_SCALE: f32 = 1.5;
const PLAYER_SPEED_PIXELS_PER_SECOND: f32 = 160.0;
const PLAYER_MARKER_SIZE: f32 = 6.0;

#[derive(Resource)]
struct ActiveGridImage {
    handle: Handle<Image>,
}

#[derive(Resource, Default)]
struct MovementCarry {
    pixels: Vec2,
}

#[derive(Component)]
struct PlayerMarker;

pub fn run() {
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(ImagePlugin::default_nearest())
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Morbid Monarchy Toroidal Grid".into(),
                        resolution: WindowResolution::new(WINDOW_WIDTH, WINDOW_HEIGHT),
                        resizable: false,
                        ..default()
                    }),
                    ..default()
                }),
        )
        .init_resource::<WorldState>()
        .init_resource::<ChunkStreamingState>()
        .init_resource::<MovementCarry>()
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                move_player,
                queue_chunk_streaming,
                poll_chunk_streaming,
                update_active_grid_texture,
                update_player_marker,
            )
            .chain(),
        )
        .run();
}

fn setup(
    mut commands: Commands,
    world: Res<WorldState>,
    mut images: ResMut<Assets<Image>>,
) {
    let pixel_dimensions = world.active_grid.visible_pixel_dimensions();
    let image = Image::new_fill(
        Extent3d {
            width: pixel_dimensions.x,
            height: pixel_dimensions.y,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    let image_handle = images.add(image);

    if let Some(image) = images.get_mut(&image_handle) {
        write_world_to_image(&world, image);
    }

    commands.insert_resource(ActiveGridImage {
        handle: image_handle.clone(),
    });
    commands.spawn(Camera2d);
    commands.spawn((
        Sprite::from_image(image_handle),
        Transform::from_scale(Vec3::splat(PIXEL_SCALE)),
    ));
    commands.spawn((
        Sprite::from_color(
            Color::srgb(0.95, 0.15, 0.15),
            Vec2::splat(PLAYER_MARKER_SIZE),
        ),
        Transform::from_translation(player_marker_translation(&world)),
        PlayerMarker,
    ));
}

fn move_player(
    keyboard: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut carry: ResMut<MovementCarry>,
    mut world: ResMut<WorldState>,
) {
    let mut axis = Vec2::ZERO;

    if keyboard.pressed(KeyCode::KeyA) || keyboard.pressed(KeyCode::ArrowLeft) {
        axis.x -= 1.0;
    }
    if keyboard.pressed(KeyCode::KeyD) || keyboard.pressed(KeyCode::ArrowRight) {
        axis.x += 1.0;
    }
    if keyboard.pressed(KeyCode::KeyW) || keyboard.pressed(KeyCode::ArrowUp) {
        axis.y -= 1.0;
    }
    if keyboard.pressed(KeyCode::KeyS) || keyboard.pressed(KeyCode::ArrowDown) {
        axis.y += 1.0;
    }

    if axis == Vec2::ZERO {
        return;
    }

    carry.pixels += axis.normalize() * PLAYER_SPEED_PIXELS_PER_SECOND * time.delta_secs();

    let delta = IVec2::new(carry.pixels.x.trunc() as i32, carry.pixels.y.trunc() as i32);
    if delta == IVec2::ZERO {
        return;
    }

    carry.pixels -= Vec2::new(delta.x as f32, delta.y as f32);
    world.move_player_by(delta);
}

fn queue_chunk_streaming(
    mut world: ResMut<WorldState>,
    mut streaming: ResMut<ChunkStreamingState>,
) {
    let delta = world.take_chunk_window_delta();
    if delta.is_empty() {
        return;
    }

    streaming.schedule_window_delta(world.as_mut(), delta);
}

fn poll_chunk_streaming(
    mut world: ResMut<WorldState>,
    mut streaming: ResMut<ChunkStreamingState>,
) {
    streaming.poll(world.as_mut());
}

fn update_active_grid_texture(
    world: Res<WorldState>,
    active_grid_image: Res<ActiveGridImage>,
    mut images: ResMut<Assets<Image>>,
) {
    if !world.is_changed() {
        return;
    }

    let Some(image) = images.get_mut(&active_grid_image.handle) else {
        return;
    };

    write_world_to_image(&world, image);
}

fn update_player_marker(
    world: Res<WorldState>,
    mut markers: Query<&mut Transform, With<PlayerMarker>>,
) {
    if !world.is_changed() {
        return;
    }

    let Some(mut transform) = markers.iter_mut().next() else {
        return;
    };

    transform.translation = player_marker_translation(&world);
}

fn write_world_to_image(world: &WorldState, image: &mut Image) {
    let pixels = world
        .active_grid
        .copy_visible_window_pixels(world.player_world_pixel);
    let mut rgba = vec![0_u8; pixels.len() * 4];

    for (pixel, chunk) in pixels.into_iter().zip(rgba.chunks_exact_mut(4)) {
        chunk.copy_from_slice(&material_color(pixel.material));
    }

    image.data = Some(rgba);
}

fn material_color(material: MaterialId) -> [u8; 4] {
    match material {
        id if id == MaterialId::EMPTY => [16, 18, 24, 255],
        id if id == MaterialId::DIRT => [123, 88, 60, 255],
        id if id == MaterialId::ROCK => [90, 98, 110, 255],
        id if id == MaterialId::WATER => [55, 116, 204, 255],
        _ => [255, 0, 255, 255],
    }
}

fn player_marker_translation(world: &WorldState) -> Vec3 {
    let pixel_dimensions = world.active_grid.visible_pixel_dimensions();
    let player_view_pixel = world.player_view_pixel();
    let centered_x = (player_view_pixel.x as f32 + 0.5) - pixel_dimensions.x as f32 * 0.5;
    let centered_y = pixel_dimensions.y as f32 * 0.5 - (player_view_pixel.y as f32 + 0.5);

    Vec3::new(centered_x * PIXEL_SCALE, centered_y * PIXEL_SCALE, 1.0)
}
