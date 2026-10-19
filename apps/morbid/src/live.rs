//! A saved world open in the client: the store, the runtime's session over it and the GPU owner
//! on the window's device, composed as the headless session (`cargo evidence session`) composes
//! them, plus the window's surface on the same instance and device.
//!
//! ```text
//! open        store -> SessionPlan (latest checkpoint) -> GpuWorld::open_presenting -> WorldSession
//! frame       session.frame(elapsed): ticks, polling, residency and checkpoints; never blocks
//! close       session.close: the last completed tick is checkpointed, workers join (bounded)
//! loss        session.finish_after_loss: what the CPU held is written; the report says what is gone
//! recover     a new session under a later epoch from the latest complete checkpoint, new device
//! ```

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use gpu_world::world::{GpuWorld, GpuWorldConfig, OpenError};
use world_runtime::{
    ids::SessionEpoch,
    session::{CloseReport, LossReport, SessionConfig, SessionPlan, WorldSession},
    store::{CheckpointManifest, StoreError, WorldStore},
};
use world_store::store::{OpenOptionsStore, RedbWorldStore};

use crate::options::Autosave;

/// The longest an orderly close waits for the last ticks, writes and its checkpoint.
pub const CLOSE_LIMIT: Duration = Duration::from_secs(60);

/// What the client asks of a session it opens.
#[derive(Debug, Clone)]
pub struct SessionRequest {
    pub adapter: String,
    pub autosave: Autosave,
    /// Hand out a fault controller (device loss injection) for conformance runs; never set by
    /// the interactive client.
    pub fault_injection: bool,
}

/// What opening a world found.
#[derive(Debug, Clone, PartialEq)]
pub struct Opened {
    pub checkpoint: CheckpointManifest,
    pub epoch: SessionEpoch,
    /// The previous session did not close: what it left unpublished was reclaimed.
    pub previous_unclean: bool,
}

#[derive(Debug)]
pub enum OpenFailure {
    Store(StoreError),
    /// The checkpoint names the last representable tick or epoch: the world cannot run on.
    Exhausted(&'static str),
    Device(OpenError),
    Session(String),
}

impl std::fmt::Display for OpenFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(error) => write!(f, "the saved world could not be read: {error}"),
            Self::Exhausted(what) => write!(f, "the world's {what} are exhausted"),
            Self::Device(error) => write!(f, "the GPU world could not be opened: {error:?}"),
            Self::Session(error) => write!(f, "the session could not start: {error}"),
        }
    }
}

pub struct LiveWorld {
    pub name: String,
    pub path: PathBuf,
    pub session: WorldSession,
    pub opened: Opened,
    pub fault_controller: Option<gpu_world::faults::FaultController>,
}

pub fn session_config(autosave: &Autosave) -> SessionConfig {
    let mut config = SessionConfig::default();
    config.checkpoints.autosave_ticks = autosave.ticks();
    config
}

/// Opens a world file for writing.
pub fn open_store(path: &Path) -> Result<Arc<dyn WorldStore>, OpenFailure> {
    RedbWorldStore::open(path, OpenOptionsStore::default())
        .map(|store| Arc::new(store) as Arc<dyn WorldStore>)
        .map_err(OpenFailure::Store)
}

impl LiveWorld {
    /// Opens the latest checkpoint of `store` on the requested adapter, with a surface for
    /// `window`. `after` is the epoch of a lost session this one recovers from: the new session
    /// runs under a later one, so nothing of the lost session is mistaken for its own.
    pub fn open(
        name: String,
        path: PathBuf,
        store: Arc<dyn WorldStore>,
        request: &SessionRequest,
        window: wgpu::SurfaceTarget<'static>,
        after: Option<SessionEpoch>,
    ) -> Result<(Self, wgpu::Surface<'static>), OpenFailure> {
        let plan = SessionPlan::prepare(&*store).map_err(OpenFailure::Store)?;
        let next = plan
            .manifest
            .epoch
            .next()
            .ok_or(OpenFailure::Exhausted("session epochs"))?;
        let epoch = match after.and_then(SessionEpoch::next) {
            Some(later) if later > next => later,
            _ => next,
        };
        let mut config = GpuWorldConfig::resume(&plan, &request.adapter, epoch)
            .ok_or(OpenFailure::Exhausted("ticks"))?;
        config.fault_injection = request.fault_injection;
        let opened = Opened {
            checkpoint: plan.manifest,
            epoch,
            previous_unclean: plan.start.previous_unclean,
        };
        let (world, surface) =
            GpuWorld::open_presenting(config, window).map_err(OpenFailure::Device)?;
        let fault_controller = world.fault_controller();
        let session = WorldSession::open(
            Box::new(world),
            store,
            plan,
            session_config(&request.autosave),
        )
        .map_err(|error| OpenFailure::Session(format!("{error:?}")))?;
        Ok((
            Self {
                name,
                path,
                session,
                opened,
                fault_controller,
            },
            surface,
        ))
    }

    /// The GPU owner behind the session, for the consumers composed on its device.
    pub fn gpu(&mut self) -> &mut GpuWorld {
        // The session's port is always the `GpuWorld` `open` built, never replaced.
        self.session
            .runtime_mut()
            .port_as::<GpuWorld>()
            .expect("the live world is the GpuWorld this client opened")
    }

    pub fn gpu_ref(&self) -> &GpuWorld {
        self.session
            .runtime()
            .port_ref::<GpuWorld>()
            .expect("the live world is the GpuWorld this client opened")
    }

    /// Closes the world in order; the report says whether the close saved everything.
    pub fn close(self) -> CloseReport {
        self.session.close(CLOSE_LIMIT)
    }

    /// After the device was lost: finishes what the CPU holds and returns the store for a
    /// recovery, with what was lost and the epoch a recovery must exceed.
    pub fn finish_after_loss(self) -> LostWorld {
        let epoch = self.opened.epoch;
        let (store, report) = self.session.finish_after_loss(CLOSE_LIMIT);
        LostWorld {
            name: self.name,
            path: self.path,
            store,
            report,
            epoch,
        }
    }
}

/// A world whose device was lost, waiting for the user to recover or close it.
pub struct LostWorld {
    pub name: String,
    pub path: PathBuf,
    pub store: Arc<dyn WorldStore>,
    pub report: LossReport,
    pub epoch: SessionEpoch,
}
