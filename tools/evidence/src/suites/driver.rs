//! Drives a [`GpuWorld`] through its port the way the runtime does, for suite cases.
//!
//! Submission never waits. Results are collected by repeated nonblocking polls with a short
//! sleep between them and an overall time limit, so a case that never completes fails instead
//! of hanging. The harness waits; the world does not.

use std::time::{Duration, Instant};

use gpu_world::world::{GpuWorld, GpuWorldConfig};
use world_model::{
    cell::WorldCell,
    command::Command,
    glam::IVec2,
    spatial::{CHUNK_CELL_COUNT, ChunkKey},
};
use world_runtime::{
    ids::{ChunkRevision, CommandId, SessionId, Tick, WorldId},
    port::{
        AdmittedCommand, CommandResult, Outcome, PhysicalWorld, PollReport, Refusal,
        ResidentWindow, SlotRef, SubmissionTicket, TickBatch,
    },
};

/// How long a case waits for the device before it fails.
pub const DEVICE_PATIENCE: Duration = Duration::from_secs(10);

/// A world of `radius` chunks around the origin chunk with every material rule off, so the
/// transfer cases see exactly the cells they wrote (the material phases still run and wake, but
/// change nothing).
pub fn config(adapter: &str, radius: u32) -> GpuWorldConfig {
    let window = ResidentWindow::new(ChunkKey::new(0, 0, 0), radius, radius)
        .expect("radius inside the supported window range");
    let mut config = GpuWorldConfig::new(WorldId::new(0x5EED), adapter, window);
    config.allow_software = super::allow_software();
    config.rules = world_model::rules::SimulationRules::NONE;
    config
}

pub struct Driver {
    pub world: GpuWorld,
    pub next_tick: Tick,
    pub outcomes: Vec<Outcome>,
    next_id: u64,
    in_flight: u32,
}

impl Driver {
    pub fn open(config: GpuWorldConfig) -> Result<Self, String> {
        let world = GpuWorld::open(config).map_err(|error| format!("open: {error:?}"))?;
        Ok(Self::new(world))
    }

    pub fn new(world: GpuWorld) -> Self {
        let capacity = world.capabilities().outcomes_per_poll as usize;
        Self {
            world,
            next_tick: Tick::ZERO,
            outcomes: Vec::with_capacity(capacity),
            next_id: 1,
            in_flight: 0,
        }
    }

    pub fn session(&self) -> SessionId {
        self.world.session()
    }

    pub fn next_id(&mut self) -> CommandId {
        let id = CommandId::new(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn submit_admitted(
        &mut self,
        commands: &[AdmittedCommand],
    ) -> Result<SubmissionTicket, Refusal> {
        let batch = TickBatch {
            session: self.session(),
            tick: self.next_tick,
            commands,
        };
        let ticket = self.world.submit_tick(&batch)?;
        self.next_tick = Tick::new(self.next_tick.value() + 1);
        Ok(ticket)
    }

    /// Submits `commands` as the next tick under fresh identities.
    pub fn submit(&mut self, commands: &[Command]) -> Result<Vec<CommandId>, Refusal> {
        let admitted: Vec<AdmittedCommand> = commands
            .iter()
            .map(|command| AdmittedCommand {
                id: self.next_id(),
                command: *command,
            })
            .collect();
        self.submit_admitted(&admitted)?;
        Ok(admitted.iter().map(|command| command.id).collect())
    }

    pub fn poll(&mut self) -> PollReport {
        let report = self.world.poll(&mut self.outcomes);
        self.in_flight = report.in_flight;
        report
    }

    /// Submissions in flight as of the last poll.
    pub fn poll_report_in_flight(&self) -> u32 {
        self.in_flight
    }

    /// Polls without blocking until `done` holds, sleeping briefly between polls.
    pub fn poll_until(
        &mut self,
        what: &str,
        mut done: impl FnMut(&Self) -> bool,
    ) -> Result<(), String> {
        let started = Instant::now();
        loop {
            self.poll();
            if done(self) {
                return Ok(());
            }
            if started.elapsed() > DEVICE_PATIENCE {
                return Err(format!("{what}: not done after {DEVICE_PATIENCE:?}"));
            }
            std::thread::sleep(Duration::from_micros(200));
        }
    }

    /// Polls until every submitted tick has been observed.
    pub fn settle(&mut self) -> Result<(), String> {
        self.poll_until("settle", |driver| {
            let counters = driver.world.counters();
            counters.ticks_observed == counters.ticks_submitted
        })
    }

    pub fn result(&self, id: CommandId) -> Option<CommandResult> {
        self.outcomes.iter().find_map(|outcome| match outcome {
            Outcome::Command(command) if command.id == id => Some(command.result),
            _ => None,
        })
    }

    /// Submits `commands` as one tick, waits for it and returns their results in order.
    pub fn run(&mut self, commands: &[Command]) -> Result<Vec<CommandResult>, String> {
        let ids = self
            .submit(commands)
            .map_err(|refusal| format!("submit: {refusal:?}"))?;
        self.settle()?;
        ids.iter()
            .map(|id| {
                self.result(*id)
                    .ok_or_else(|| format!("no outcome for {id:?}"))
            })
            .collect()
    }

    pub fn query(&mut self, cell: IVec2) -> Result<CommandResult, String> {
        Ok(self.run(&[Command::QueryColumn { cell }])?[0])
    }

    /// Reserves a slot for `key` and stages `cells`, polling while the upload pool is under
    /// pressure (its buffers return after their tick completes and they are remapped).
    pub fn stage_upload(&mut self, key: ChunkKey, cells: &[WorldCell]) -> Result<SlotRef, String> {
        let session = self.session();
        let slot = self
            .world
            .reserve_slot(session, key)
            .map_err(|refusal| format!("reserve {key:?}: {refusal:?}"))?;
        let started = Instant::now();
        loop {
            match self.world.upload_chunk(slot, ChunkRevision::new(1), cells) {
                Ok(_) => return Ok(slot),
                Err(Refusal::Pressure(_)) if started.elapsed() < DEVICE_PATIENCE => {
                    self.poll();
                    std::thread::sleep(Duration::from_micros(200));
                }
                Err(refusal) => return Err(format!("upload {key:?}: {refusal:?}")),
            }
        }
    }

    /// Reserves a slot for `key`, uploads `cells` and waits until they are resident.
    pub fn upload(&mut self, key: ChunkKey, cells: &[WorldCell]) -> Result<SlotRef, String> {
        let session = self.session();
        let slot = self
            .world
            .reserve_slot(session, key)
            .map_err(|refusal| format!("reserve {key:?}: {refusal:?}"))?;
        self.world
            .upload_chunk(slot, ChunkRevision::new(1), cells)
            .map_err(|refusal| format!("upload {key:?}: {refusal:?}"))?;
        self.submit(&[])
            .map_err(|refusal| format!("activation tick: {refusal:?}"))?;
        self.poll_until("activation", |driver| {
            driver.outcomes.iter().any(|outcome| {
                matches!(outcome, Outcome::UploadActivated { ticket, .. } if ticket.slot == slot)
            })
        })?;
        self.settle()?;
        Ok(slot)
    }

    /// Captures a resident chunk after the next tick and returns its cells.
    pub fn capture(&mut self, slot: SlotRef) -> Result<Vec<WorldCell>, String> {
        let ticket = self
            .world
            .capture_chunk(slot)
            .map_err(|refusal| format!("capture: {refusal:?}"))?;
        self.submit(&[])
            .map_err(|refusal| format!("capture tick: {refusal:?}"))?;
        self.poll_until("capture", |driver| {
            driver.outcomes.iter().any(|outcome| {
                matches!(outcome, Outcome::CaptureReady { ticket: ready, .. } if *ready == ticket)
            })
        })?;
        let mut cells = vec![WorldCell::default(); CHUNK_CELL_COUNT];
        self.world
            .take_capture(ticket, &mut cells)
            .map_err(|refusal| format!("take capture: {refusal:?}"))?;
        self.settle()?;
        Ok(cells)
    }
}

/// A deterministic chunk of arbitrary words (SplitMix64), covering every bit pattern of both
/// words; layout tests need values a generator would never produce.
pub fn pattern(seed: u64) -> Vec<WorldCell> {
    let mut state = seed;
    (0..CHUNK_CELL_COUNT)
        .map(|_| {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            WorldCell(z ^ (z >> 31))
        })
        .collect()
}

/// A flat chunk: every cell has `elevation` stone and nothing else.
pub fn flat(elevation: u16) -> Vec<WorldCell> {
    let mut cell = WorldCell::default();
    cell.set_terrain_mat(world_model::material::TerrainMat::TERRAIN_STONE);
    cell.set_elevation(elevation);
    vec![cell; CHUNK_CELL_COUNT]
}
