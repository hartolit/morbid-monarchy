use std::path::PathBuf;

use bevy::{
    prelude::*,
    window::{PresentMode, WindowLevel, WindowResolution},
};
use monarch_engine::{core::entities::observer::resolve_observer_kinematics, prelude::*};

use crate::runtime::{
    dev_tools::DevToolsPlugin,
    input::{
        drive_possessed_sphere, handle_possession_toggle, manage_os_cursor_boundary,
        observer_hardware_ingest, setup_observer, sync_lens_orientation, sync_world_focus,
        update_camera_tether_and_zoom,
    },
    persistence,
    render::WorldRenderPlugin,
};

pub mod baseline;
mod runtime;

/// Seed of every world the baseline application generates.
pub const STARTUP_SEED: u32 = 69;

/// How the application is composed.
pub struct AppOptions {
    /// Directory holding `save.redb`; it is created when missing.
    pub world_dir: PathBuf,
    /// Asset directory. `None` keeps Bevy's default lookup relative to the package.
    pub asset_dir: Option<PathBuf>,
    /// Requested window size and presentation. `None` keeps the platform defaults.
    pub window: Option<WindowRequest>,
    pub input: InputMode,
}

pub struct WindowRequest {
    pub physical_size: [u32; 2],
    pub vsync: bool,
    /// Keeps the window above others. Measurement runs use it because a compositor throttles
    /// the presentation of a covered window, which would be timed as the application's work.
    pub always_on_top: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    /// Keyboard and mouse drive the observer, possession and tools.
    Interactive,
    /// No keyboard or mouse system changes the world; the caller supplies observer pose and tool
    /// actions through [`baseline`].
    Scripted,
}

impl AppOptions {
    /// The ordinary desktop application: `world_data/` in the working directory.
    pub fn interactive() -> Self {
        Self {
            world_dir: PathBuf::from(persistence::WORLD_DATA_DIR),
            asset_dir: None,
            window: None,
            input: InputMode::Interactive,
        }
    }
}

pub fn run() {
    build_app(AppOptions::interactive()).run();
}

pub fn build_app(options: AppOptions) -> App {
    let world_db = persistence::initialize_database(&options.world_dir);

    let mut window = Window {
        title: "Morbid Monarchy".to_string(),
        ..default()
    };
    if let Some(request) = &options.window {
        let [width, height] = request.physical_size;
        window.resolution = WindowResolution::new(width, height).with_scale_factor_override(1.0);
        window.present_mode = if request.vsync {
            PresentMode::AutoVsync
        } else {
            PresentMode::AutoNoVsync
        };
        if request.always_on_top {
            window.window_level = WindowLevel::AlwaysOnTop;
        }
    }
    let mut asset_plugin = AssetPlugin::default();
    if let Some(dir) = &options.asset_dir {
        asset_plugin.file_path = dir.to_string_lossy().into_owned();
    }

    let interactive = options.input == InputMode::Interactive;
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(window),
                ..default()
            })
            .set(ImagePlugin::default_nearest())
            .set(asset_plugin),
    )
    .add_plugins(MonarchEnginePlugin)
    .add_plugins(WorldRenderPlugin)
    .add_plugins(DevToolsPlugin {
        pointer_tools: interactive,
    })
    .insert_resource(world_db)
    .insert_resource(persistence::WorldSeed(STARTUP_SEED))
    .init_resource::<persistence::ChunkSaveQueue>()
    .init_resource::<persistence::PersistenceCounters>()
    .insert_resource(persistence::SaveTimer(Timer::from_seconds(
        2.0,
        TimerMode::Repeating,
    )))
    .add_systems(Startup, setup_observer);

    if interactive {
        app.add_systems(
            Update,
            (
                manage_os_cursor_boundary,
                handle_possession_toggle,
                drive_possessed_sphere,
                observer_hardware_ingest,
                sync_lens_orientation,
            )
                .chain(),
        )
        .add_systems(
            Update,
            update_camera_tether_and_zoom.after(resolve_observer_kinematics),
        );
    } else {
        app.add_systems(Update, sync_lens_orientation);
    }

    app.add_systems(Update, sync_world_focus.after(sync_lens_orientation))
        .add_systems(
            Update,
            (
                persistence::handle_load_requests,
                persistence::handle_unload_events,
                persistence::process_save_queue,
                persistence::emergency_flush_on_exit,
                persistence::poll_load_tasks,
                persistence::poll_save_tasks,
            ),
        );
    app
}
