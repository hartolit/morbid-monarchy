//! A saved world for streaming cases: a real redb store in a temporary directory and a live
//! session over the GPU owner, composed the way the application composes them.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use gpu_world::{
    faults::FaultController,
    world::{GpuWorld, GpuWorldConfig},
};
use world_model::{
    cell::WorldCell,
    command::{AmbientTemperature, Command, ElevationScale},
    glam::IVec2,
    identity,
    rules::SimulationRules,
    spatial::ChunkKey,
};
use world_runtime::{
    ids::{CommandId, SessionEpoch, Tick, WorldId},
    port::{CommandResult, ResidentWindow},
    receipts::ReceiptState,
    session::{SessionConfig, SessionPlan, WorldSession},
    store::{
        CheckpointId, Generator, PhysicalConfig, Publication, WorldHeader, WorldOrigin, WorldStore,
    },
};
use world_store::{
    faults::StoreFaults,
    store::{OpenOptionsStore, RedbWorldStore},
};

use super::super::{allow_software, driver::DEVICE_PATIENCE};

/// A directory removed when dropped.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Result<Self, String> {
        let dir = std::env::temp_dir().join(format!(
            "morbid-streaming-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        Ok(Self(dir))
    }

    pub fn path(&self, file: &str) -> PathBuf {
        self.0.join(file)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub const SEED: u32 = 69;

pub fn window(x: i32, y: i32, radius: u32) -> ResidentWindow {
    ResidentWindow::new(ChunkKey::new(x, y, 0), radius, radius).expect("supported window")
}

/// Creates a new world at `path`: generator seed 69, the given rules, opening at `window`.
pub fn create(path: &Path, window: ResidentWindow, rules: SimulationRules) -> Result<(), String> {
    let header = WorldHeader {
        world: WorldId::new(0x05_7EA4),
        generator: Generator {
            revision: identity::GENERATOR_REVISION,
            seed: SEED,
        },
        rule_seed: 0x5EED,
        origin: WorldOrigin::Created,
    };
    let config = PhysicalConfig {
        rules,
        ambient_temperature: AmbientTemperature::celsius(20.0).expect("supported"),
        elevation_scale: ElevationScale::DEFAULT,
    };
    RedbWorldStore::create(
        path,
        &header,
        &Publication {
            id: CheckpointId::FIRST,
            tick: None,
            config,
            window,
            epoch: SessionEpoch::FIRST,
            heads: &[],
            bodies: 0,
        },
        StoreFaults::new(),
    )
    .map(|_| ())
    .map_err(|error| format!("create store: {error}"))
}

pub fn open_store(path: &Path, faults: StoreFaults) -> Result<Arc<RedbWorldStore>, String> {
    RedbWorldStore::open(
        path,
        OpenOptionsStore {
            faults,
            ..OpenOptionsStore::default()
        },
    )
    .map(Arc::new)
    .map_err(|error| format!("open store: {error}"))
}

/// The native client's window showing the scenario's sessions, when it runs visibly.
pub type SharedViewer = std::rc::Rc<std::cell::RefCell<morbid_app::viewer::Viewer>>;

/// A live session, and the controller of its device's faults.
pub struct Live {
    pub session: WorldSession,
    pub faults: FaultController,
    next_command: u64,
    /// Every frame this session runs is also presented in this window.
    pub viewer: Option<SharedViewer>,
}

pub fn session_config() -> SessionConfig {
    let mut config = SessionConfig::default();
    config.checkpoints.autosave_ticks = None;
    config.checkpoints.retry_frames = 4;
    config.residency.retry_frames = 4;
    config
}

/// Opens a session of the store's latest checkpoint on `adapter` under `epoch`.
pub fn open(
    adapter: &str,
    store: Arc<RedbWorldStore>,
    epoch: SessionEpoch,
    config: SessionConfig,
    tweak: impl FnOnce(&mut GpuWorldConfig),
) -> Result<Live, String> {
    open_viewed(adapter, store, epoch, config, tweak, None)
}

/// [`open`], presenting in `viewer`'s window when there is one: the world is opened with a
/// surface for it and the viewer draws each frame on the session's device.
pub fn open_viewed(
    adapter: &str,
    store: Arc<RedbWorldStore>,
    epoch: SessionEpoch,
    config: SessionConfig,
    tweak: impl FnOnce(&mut GpuWorldConfig),
    viewer: Option<SharedViewer>,
) -> Result<Live, String> {
    let plan = SessionPlan::prepare(&*store).map_err(|error| format!("prepare: {error}"))?;
    let mut gpu = GpuWorldConfig::resume(&plan, adapter, epoch).ok_or("no next tick")?;
    gpu.allow_software = allow_software();
    gpu.fault_injection = true;
    gpu.transfer.bodies = 64;
    tweak(&mut gpu);
    let world = match &viewer {
        None => GpuWorld::open(gpu).map_err(|error| format!("open world: {error:?}"))?,
        Some(viewer) => {
            let mut viewer = viewer.borrow_mut();
            viewer.detach();
            let (mut world, surface) = GpuWorld::open_presenting(gpu, viewer.target()?)
                .map_err(|error| format!("open world: {error:?}"))?;
            viewer.attach(&mut world, surface)?;
            world
        }
    };
    let faults = world
        .fault_controller()
        .ok_or("the world was opened without fault injection")?;
    let session = WorldSession::open(Box::new(world), store as Arc<dyn WorldStore>, plan, config)
        .map_err(|error| format!("open session: {error:?}"))?;
    Ok(Live {
        session,
        faults,
        next_command: 1,
        viewer,
    })
}

impl Live {
    /// One frame owing `ticks` ticks, presented in the viewer's window when there is one.
    pub fn frame_ticks(&mut self, ticks: u32) {
        self.session.frame_ticks(ticks);
        if let Some(viewer) = &self.viewer
            && let Err(error) = viewer.borrow_mut().present(&mut self.session)
        {
            eprintln!("viewer: {error}");
        }
    }

    /// Releases the viewer's hold on this session's device before the session ends.
    pub fn detach(&self) {
        if let Some(viewer) = &self.viewer {
            viewer.borrow_mut().detach();
        }
    }

    pub fn set_caption(&self, caption: &str) {
        if let Some(viewer) = &self.viewer {
            viewer.borrow_mut().set_caption(caption);
        }
    }

    /// The viewer's next frame, written as `path` (a maintenance readback).
    pub fn capture(&mut self, path: &Path) -> Result<(), String> {
        let Some(viewer) = self.viewer.clone() else {
            return Ok(());
        };
        let capture = viewer.borrow_mut().capture(&mut self.session)?;
        crate::native::write_png(path, &capture)
    }

    /// Runs frames of one tick until `done` holds.
    pub fn until(
        &mut self,
        what: &str,
        mut done: impl FnMut(&WorldSession) -> bool,
    ) -> Result<u32, String> {
        let started = Instant::now();
        let mut frames = 0;
        loop {
            self.frame_ticks(1);
            frames += 1;
            if done(&self.session) {
                return Ok(frames);
            }
            if started.elapsed() > DEVICE_PATIENCE * 3 {
                return Err(format!(
                    "{what}: not done after {frames} frames: {:?}",
                    self.session.residency().summary()
                ));
            }
            std::thread::sleep(Duration::from_micros(100));
        }
    }

    /// Runs `frames` frames of one tick.
    pub fn frames(&mut self, frames: u32) {
        for _ in 0..frames {
            self.frame_ticks(1);
            std::thread::sleep(Duration::from_micros(100));
        }
    }

    pub fn settle(&mut self) -> Result<u32, String> {
        self.until("settle", |session| session.residency().settled())
    }

    pub fn submit(&mut self, command: Command) -> Result<CommandId, String> {
        let id = CommandId::new(self.next_command);
        self.next_command += 1;
        self.session
            .runtime_mut()
            .submit(id, command)
            .map_err(|error| format!("submit {command:?}: {error:?}"))?;
        Ok(id)
    }

    pub fn result(&mut self, id: CommandId) -> Result<(Tick, CommandResult), String> {
        let started = Instant::now();
        loop {
            if let Some(receipt) = self.session.runtime().receipt(id)
                && let ReceiptState::Completed { tick, result } = receipt.state
            {
                return Ok((tick, result));
            }
            if started.elapsed() > DEVICE_PATIENCE {
                return Err(format!("{id:?} never completed"));
            }
            self.frame_ticks(1);
            std::thread::sleep(Duration::from_micros(100));
        }
    }

    /// Runs a command and waits for its result.
    pub fn run(&mut self, command: Command) -> Result<CommandResult, String> {
        let id = self.submit(command)?;
        Ok(self.result(id)?.1)
    }

    pub fn column(&mut self, cell: IVec2) -> Result<Option<WorldCell>, String> {
        match self.run(Command::QueryColumn { cell })? {
            CommandResult::Column { column, .. } => Ok(Some(column)),
            CommandResult::ColumnNotResident { .. } => Ok(None),
            other => Err(format!("query {cell}: {other:?}")),
        }
    }

    /// Saves and waits for the save to commit.
    pub fn save(&mut self) -> Result<CheckpointId, String> {
        self.session.save();
        self.until("save", |session| {
            session
                .checkpoints()
                .receipt()
                .is_some_and(|receipt| receipt.finished())
        })?;
        let receipt = self.session.checkpoints().receipt().cloned();
        receipt
            .as_ref()
            .and_then(|receipt| receipt.committed())
            .ok_or_else(|| format!("save did not commit: {receipt:?}"))
    }

    pub fn move_to(&mut self, window: ResidentWindow) -> Result<u32, String> {
        self.session.set_window(window);
        self.until("move", |session| {
            session.residency().window() == window && session.residency().settled()
        })
    }
}
