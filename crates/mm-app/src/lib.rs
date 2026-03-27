mod world_persistence;
mod world_runtime;

use bevy::prelude::*;
use mm_core::{
    MorbidMonarchyCorePlugin, MorbidMonarchyCoreSystems, MovementIntent, Player, PlayerBundle,
    SimulationStep,
};
use world_persistence::ChunkPersistence;
use world_runtime::{
    ChunkRenderState, Enemy, EntitySpatialIndex, PendingWorldSplatters, PreviousTranslation,
    PrimaryWorldCamera, RuntimeWorldState, apply_pending_world_splatters,
    camera_follow_player, capture_previous_player_positions, draw_world_debug,
    handle_enemy_interaction, handle_world_interaction, resolve_player_world_collision,
    stream_world_around_player, sync_chunk_renders, sync_player_surface_state,
    update_entity_spatial_index,
};

const PLAYER_SPRITE_PATH: &str = "player-48x48-sprite.png";
const PLAYER_FRAME_SIZE: UVec2 = UVec2::new(192, 192);
const PLAYER_RENDER_SCALE: f32 = 0.25;
const PLAYER_Z_INDEX: f32 = 10.0;
const ENEMY_Z_INDEX: f32 = 8.0;
const ENEMY_SIZE: Vec2 = Vec2::new(18.0, 18.0);
const PLAYER_ANIMATION_FRAMES: usize = 4;
const PLAYER_ROW_UP: usize = 0;
const PLAYER_ROW_RIGHT: usize = 1;
const PLAYER_ROW_DOWN: usize = 2;
const PLAYER_ROW_LEFT: usize = 3;

#[derive(Component, Debug)]
struct WalkAnimation {
    facing: Facing,
    frame: usize,
    timer: Timer,
}

impl Default for WalkAnimation {
    fn default() -> Self {
        Self {
            facing: Facing::Down,
            frame: 0,
            timer: Timer::from_seconds(0.12, TimerMode::Repeating),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Facing {
    Up,
    Right,
    Down,
    Left,
}

impl Facing {
    fn row(self) -> usize {
        match self {
            Self::Up => PLAYER_ROW_UP,
            Self::Right => PLAYER_ROW_RIGHT,
            Self::Down => PLAYER_ROW_DOWN,
            Self::Left => PLAYER_ROW_LEFT,
        }
    }
}

pub fn run() {
    App::new()
        .add_plugins(DefaultPlugins.set(ImagePlugin::default_nearest()))
        .add_plugins(MorbidMonarchyCorePlugin)
        .init_resource::<RuntimeWorldState>()
        .init_resource::<ChunkRenderState>()
        .init_resource::<EntitySpatialIndex>()
        .init_resource::<PendingWorldSplatters>()
        .init_resource::<ChunkPersistence>()
        .add_systems(Startup, setup_scene)
        .add_systems(
            Update,
            (
                update_simulation_step,
                translate_player_input,
                capture_previous_player_positions,
            )
                .chain()
                .before(MorbidMonarchyCoreSystems::Movement),
        )
        .add_systems(
            Update,
            (
                stream_world_around_player,
                resolve_player_world_collision,
                update_entity_spatial_index,
                handle_world_interaction,
                handle_enemy_interaction,
                apply_pending_world_splatters,
                sync_chunk_renders,
                sync_player_surface_state,
                camera_follow_player,
                animate_player_sprite,
                draw_world_debug,
            )
                .chain()
                .after(MorbidMonarchyCoreSystems::Movement),
        )
        .run();
}

fn setup_scene(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut texture_atlas_layouts: ResMut<Assets<TextureAtlasLayout>>,
) {
    commands.spawn((Camera2d, PrimaryWorldCamera));

    let player_image = asset_server.load(PLAYER_SPRITE_PATH);
    let player_layout = texture_atlas_layouts.add(TextureAtlasLayout::from_grid(
        PLAYER_FRAME_SIZE,
        4,
        4,
        None,
        None,
    ));

    commands.spawn((
        PlayerBundle {
            transform: Transform {
                translation: Vec3::new(0.0, 0.0, PLAYER_Z_INDEX),
                scale: Vec3::splat(PLAYER_RENDER_SCALE),
                ..Default::default()
            },
            ..Default::default()
        },
        Sprite::from_atlas_image(
            player_image,
            TextureAtlas {
                layout: player_layout,
                index: Facing::Down.row() * PLAYER_ANIMATION_FRAMES,
            },
        ),
        PreviousTranslation::default(),
        WalkAnimation::default(),
    ));

    commands.spawn((
        Enemy,
        Sprite::from_color(Color::srgb(0.75, 0.15, 0.18), ENEMY_SIZE),
        Transform::from_xyz(72.0, 24.0, ENEMY_Z_INDEX),
    ));
}

fn update_simulation_step(time: Res<Time>, mut step: ResMut<SimulationStep>) {
    step.delta_seconds = time.delta_secs();
}

fn translate_player_input(
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut players: Query<&mut MovementIntent, With<Player>>,
) {
    let horizontal = axis_pressed(
        &keyboard_input,
        KeyCode::KeyA,
        KeyCode::KeyD,
        KeyCode::ArrowLeft,
        KeyCode::ArrowRight,
    );
    let vertical = axis_pressed(
        &keyboard_input,
        KeyCode::KeyS,
        KeyCode::KeyW,
        KeyCode::ArrowDown,
        KeyCode::ArrowUp,
    );
    let intent = Vec3::new(horizontal, vertical, 0.0);

    for mut movement_intent in &mut players {
        movement_intent.0 = intent;
    }
}

fn animate_player_sprite(
    time: Res<Time>,
    mut players: Query<(&MovementIntent, &mut WalkAnimation, &mut Sprite), With<Player>>,
) {
    for (intent, mut animation, mut sprite) in &mut players {
        let planar = Vec2::new(intent.0.x, intent.0.y);
        if planar == Vec2::ZERO {
            animation.frame = 0;
            if let Some(atlas) = &mut sprite.texture_atlas {
                atlas.index = animation.facing.row() * PLAYER_ANIMATION_FRAMES;
            }
            continue;
        }

        animation.facing = facing_from_planar(planar);
        animation.timer.tick(time.delta());
        if animation.timer.just_finished() {
            animation.frame = (animation.frame + 1) % PLAYER_ANIMATION_FRAMES;
        }

        if let Some(atlas) = &mut sprite.texture_atlas {
            atlas.index = animation.facing.row() * PLAYER_ANIMATION_FRAMES + animation.frame;
        }
    }
}

fn facing_from_planar(planar: Vec2) -> Facing {
    if planar.x.abs() > planar.y.abs() {
        if planar.x > 0.0 {
            Facing::Right
        } else {
            Facing::Left
        }
    } else if planar.y > 0.0 {
        Facing::Up
    } else {
        Facing::Down
    }
}

fn axis_pressed(
    keyboard_input: &ButtonInput<KeyCode>,
    negative_primary: KeyCode,
    positive_primary: KeyCode,
    negative_secondary: KeyCode,
    positive_secondary: KeyCode,
) -> f32 {
    let negative =
        keyboard_input.pressed(negative_primary) || keyboard_input.pressed(negative_secondary);
    let positive =
        keyboard_input.pressed(positive_primary) || keyboard_input.pressed(positive_secondary);

    match (negative, positive) {
        (true, false) => -1.0,
        (false, true) => 1.0,
        _ => 0.0,
    }
}
