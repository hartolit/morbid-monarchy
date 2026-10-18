//! A GPU world with bodies, driven tick by tick through the port like the runtime drives it.
//!
//! Terrain comes from a shape function of the absolute cell, so a case states its ground the
//! way the baseline tests do (`world_with_terrain`). The window is three chunks on each side by
//! default, with the baseline tests' 64 × 64 grid as its centre chunk, so a body in the tests'
//! region is never near the residency edge. Every tick samples every body the scene spawned
//! and keeps the tick's report; a body's samples are its observations, never written back.

use std::collections::BTreeMap;

use gpu_world::world::{Broadphase, GpuWorldConfig, PairIterations};
use world_model::{
    body::{BodyHandle, SphereDescription},
    cell::WorldCell,
    command::{BodySpawn, Command, ElevationScale, Ray, Target},
    glam::{DVec3, IVec2, Vec3},
    material::TerrainMat,
    observation::{BodySample, WorldTotals},
    rules::SimulationRules,
    spatial::{CHUNK_CELL_COUNT, CHUNK_SIZE, ChunkKey},
};
use world_runtime::{
    ids::{Tick, WorldId},
    port::{BodyReport, CommandResult, Outcome, PhysicalWorld, ResidentWindow, SlotRef},
};

use super::super::{Check, allow_software, driver::Driver};

/// How a scene is opened.
#[derive(Debug, Clone, Copy)]
pub struct Setup {
    pub center: ChunkKey,
    pub radius: u32,
    pub rules: SimulationRules,
    pub bodies: u32,
    pub samples: u32,
    pub broadphase: Broadphase,
    pub pair_iterations: PairIterations,
    pub timestamps: bool,
    pub tick_rate_hz: f64,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            center: ChunkKey::new(0, 0, 0),
            radius: 1,
            // The baseline sphere tests run the body system alone.
            rules: SimulationRules::NONE,
            bodies: 64,
            samples: 64,
            broadphase: Broadphase::Bins,
            pair_iterations: PairIterations::DEFAULT,
            timestamps: false,
            tick_rate_hz: 60.0,
        }
    }
}

impl Setup {
    pub fn window(&self) -> ResidentWindow {
        ResidentWindow::new(self.center, self.radius, self.radius).expect("supported window")
    }

    pub fn config(&self, adapter: &str) -> GpuWorldConfig {
        let mut config = GpuWorldConfig::new(WorldId::new(0x0400), adapter, self.window());
        config.allow_software = allow_software();
        config.rules = self.rules;
        config.transfer.bodies = self.bodies;
        config.transfer.body_samples_per_tick = self.samples;
        config.broadphase = self.broadphase;
        config.pair_iterations = self.pair_iterations;
        config.timestamps = self.timestamps;
        config
    }
}

/// The baseline tests' starting column: dirt, everything else empty.
pub fn dirt() -> WorldCell {
    let mut cell = WorldCell::default();
    cell.set_terrain_mat(TerrainMat::TERRAIN_DIRT);
    cell
}

pub struct Scene {
    pub driver: Driver,
    pub setup: Setup,
    pub slots: Vec<(ChunkKey, SlotRef)>,
    /// Spawned bodies in spawn order, and each one's latest sample.
    pub bodies: Vec<BodyHandle>,
    pub latest: BTreeMap<BodyHandle, BodySample>,
    /// Where each spawn outcome placed its body.
    pub spawned: BTreeMap<BodyHandle, DVec3>,
    /// Handles sampled every tick besides the live bodies (to see a stale one reported gone).
    pub extra_observe: Vec<BodyHandle>,
    /// Every outcome of the last tick.
    pub outcomes: Vec<Outcome>,
    pub last: Option<BodyReport>,
    /// Ticks whose cell or body accounting did not balance, and the first such report.
    pub inconsistent_ticks: u64,
    pub first_inconsistent: Option<String>,
    /// Sums over the ticks run: the body substeps', and the brush strokes' sources and sinks.
    pub body_totals: BodyReport,
    /// The body sums before the last tick: what a `MeasureTotals` in the last tick saw, since
    /// the command pass runs before that tick's body substeps.
    pub prior: BodyReport,
    pub tool_fluid_in: u64,
    pub tool_solid_in: u64,
    pub tool_solid_out: u64,
    pub ticks: u64,
}

impl Scene {
    /// Opens the world and activates every window chunk with `shape(cell, &mut column)`
    /// applied to a dirt column.
    pub fn open(
        adapter: &str,
        setup: Setup,
        shape: impl Fn(IVec2, &mut WorldCell),
    ) -> Result<Self, String> {
        let mut driver = Driver::open(setup.config(adapter))?;
        driver.tick_rate_hz = setup.tick_rate_hz;
        let mut scene = Self {
            driver,
            setup,
            slots: Vec::new(),
            bodies: Vec::new(),
            latest: BTreeMap::new(),
            spawned: BTreeMap::new(),
            extra_observe: Vec::new(),
            outcomes: Vec::new(),
            last: None,
            inconsistent_ticks: 0,
            first_inconsistent: None,
            body_totals: BodyReport::default(),
            prior: BodyReport::default(),
            tool_fluid_in: 0,
            tool_solid_in: 0,
            tool_solid_out: 0,
            ticks: 0,
        };
        let view = setup.window().view();
        let mut keys = Vec::new();
        for y in view.min.key.y..=view.max.key.y {
            for x in view.min.key.x..=view.max.key.x {
                keys.push(ChunkKey::new(x, y, 0));
            }
        }
        let batch = scene.driver.world.capabilities().chunk_upload_slots as usize;
        for group in keys.chunks(batch) {
            for key in group {
                let cells = chunk_cells(*key, &shape);
                let slot = scene.driver.stage_upload(*key, &cells)?;
                scene.slots.push((*key, slot));
            }
            scene.step(&[])?;
        }
        Ok(scene)
    }

    /// A ray straight down onto `(x, z)`, from high above anything the tests build.
    pub fn down_at(x: f32, z: f32) -> Ray {
        Ray::new(
            DVec3::new(f64::from(x), 500.0, f64::from(z)),
            Vec3::NEG_Y,
            1000.0,
        )
        .expect("a valid ray")
    }

    /// Spawns a body whose centre starts `drop` above the column under `(x, z)` (bedrock and
    /// loose material, as the spawn tool measures it) and returns its handle once the spawn
    /// tick is observed.
    pub fn spawn(
        &mut self,
        sphere: SphereDescription,
        x: f32,
        z: f32,
        drop: f32,
    ) -> Result<BodyHandle, String> {
        let spawn = BodySpawn::with(sphere, Target::Ray(Self::down_at(x, z)), drop)
            .map_err(|error| error.to_string())?;
        let results = self.step(&[Command::SpawnBody(spawn)])?;
        match results[0] {
            CommandResult::BodySpawned { body, .. } => Ok(body),
            other => Err(format!("spawn at ({x}, {z}): {other:?}")),
        }
    }

    /// Runs one tick of `commands`, sampling every spawned body, and returns the results.
    pub fn step(&mut self, commands: &[Command]) -> Result<Vec<CommandResult>, String> {
        let tick = self.driver.next_tick;
        self.driver.outcomes.clear();
        self.driver.observe.clear();
        self.driver.observe.extend(self.bodies.iter().copied());
        self.driver
            .observe
            .extend(self.extra_observe.iter().copied());
        let ids = self
            .driver
            .submit(commands)
            .map_err(|refusal| format!("submit: {refusal:?}"))?;
        self.driver.settle()?;
        let mut results = Vec::with_capacity(ids.len());
        for id in &ids {
            results.push(
                self.driver
                    .result(*id)
                    .ok_or_else(|| format!("no outcome for {id:?}"))?,
            );
        }
        for result in &results {
            if let CommandResult::BodySpawned { body, position } = result {
                self.bodies.push(*body);
                self.spawned.insert(*body, *position);
            }
            if let CommandResult::BodyRemoved { body, .. } = result {
                self.bodies.retain(|kept| kept != body);
                self.latest.remove(body);
            }
        }
        let (mut report, mut bodies) = (None, None);
        for outcome in &self.driver.outcomes {
            match outcome {
                Outcome::Body(sample) if sample.tick == tick.value() => {
                    self.latest.insert(sample.body, *sample);
                }
                Outcome::TickCompleted(completed) if completed.tick == tick => {
                    report = Some(*completed);
                }
                Outcome::BodiesCompleted { tick: of, report } if *of == tick => {
                    bodies = Some(*report);
                }
                _ => {}
            }
        }
        let report = report.ok_or_else(|| format!("no report for {tick:?}"))?;
        let bodies = bodies.ok_or_else(|| format!("no body report for {tick:?}"))?;
        self.outcomes.clear();
        self.outcomes.extend(self.driver.outcomes.iter().copied());
        if !report.cells.consistent() || !bodies.consistent() {
            self.inconsistent_ticks += 1;
            self.first_inconsistent.get_or_insert_with(|| {
                format!(
                    "tick {}: cells {:?}, bodies {:?}",
                    tick.value(),
                    report.cells.imbalance(),
                    bodies.imbalance()
                )
            });
        }
        self.prior = self.body_totals;
        accumulate(&mut self.body_totals, &bodies);
        self.tool_fluid_in += report.cells.tool_fluid_in;
        self.tool_solid_in += report.cells.tool_solid_in;
        self.tool_solid_out += report.cells.tool_solid_out;
        self.ticks += 1;
        self.last = Some(bodies);
        Ok(results)
    }

    pub fn run(&mut self, ticks: u32) -> Result<(), String> {
        for _ in 0..ticks {
            self.step(&[])?;
        }
        Ok(())
    }

    pub fn sample(&self, body: BodyHandle) -> Result<BodySample, String> {
        self.latest
            .get(&body)
            .copied()
            .ok_or_else(|| format!("no sample of {body:?}"))
    }

    /// The body's centre in render space, as `f32` world units (the tests' region is small):
    /// its latest sample, or where it spawned before its first sample.
    pub fn position(&self, body: BodyHandle) -> Result<Vec3, String> {
        match self.latest.get(&body) {
            Some(sample) => Ok(sample.position.as_vec3()),
            None => self
                .spawned
                .get(&body)
                .map(|position| position.as_vec3())
                .ok_or_else(|| format!("{body:?} never spawned")),
        }
    }

    pub fn totals(&mut self) -> Result<WorldTotals, String> {
        match self.step(&[Command::MeasureTotals])?[0] {
            CommandResult::Totals(totals) => Ok(totals),
            other => Err(format!("totals: {other:?}")),
        }
    }

    pub fn column(&mut self, cell: IVec2) -> Result<WorldCell, String> {
        match self.step(&[Command::QueryColumn { cell }])?[0] {
            CommandResult::Column { column, .. } => Ok(column),
            other => Err(format!("column {cell}: {other:?}")),
        }
    }

    /// Every resident chunk's cells, keyed by chunk (a labelled diagnostic transfer).
    pub fn capture_all(&mut self) -> Result<BTreeMap<(i32, i32), Vec<WorldCell>>, String> {
        let mut tickets = Vec::new();
        for (key, slot) in &self.slots {
            let ticket = self
                .driver
                .world
                .capture_chunk(*slot)
                .map_err(|refusal| format!("capture {key:?}: {refusal:?}"))?;
            tickets.push((*key, ticket));
        }
        self.step(&[])?;
        let mut captured = BTreeMap::new();
        for (key, ticket) in tickets {
            self.driver.poll_until("capture", |driver| {
                driver.outcomes.iter().any(|outcome| {
                    matches!(outcome, Outcome::CaptureReady { ticket: ready, .. } if *ready == ticket)
                })
            })?;
            let mut cells = vec![WorldCell::default(); CHUNK_CELL_COUNT];
            self.driver
                .world
                .take_capture(ticket, &mut cells)
                .map_err(|refusal| format!("take capture: {refusal:?}"))?;
            captured.insert((key.key.x, key.key.y), cells);
        }
        Ok(captured)
    }

    /// Reports the scene's accounting to `check`: every tick must balance.
    pub fn finish(&self, check: &mut Check, label: &str) {
        check.that(self.inconsistent_ticks == 0, || {
            format!(
                "{label}: {} ticks did not balance; first: {}",
                self.inconsistent_ticks,
                self.first_inconsistent.clone().unwrap_or_default()
            )
        });
        let totals = &self.body_totals;
        check.metric(&format!("{label}_ticks"), self.ticks);
        check.metric(&format!("{label}_candidate_pairs"), totals.candidate_pairs);
        check.metric(
            &format!("{label}_terrain_contacts"),
            totals.terrain_contacts,
        );
        check.metric(&format!("{label}_solid_moved"), totals.solid_out);
        check.metric(&format!("{label}_fluid_moved"), totals.fluid_out);
        check.metric(&format!("{label}_compaction"), totals.compaction);
        check.metric(&format!("{label}_craters"), totals.craters);
        check.metric(&format!("{label}_ruts"), totals.ruts);
        check.metric(&format!("{label}_splashes"), totals.splashes);
        check.metric(&format!("{label}_wakes"), totals.wakes);
        check.metric(&format!("{label}_claims_lost"), totals.claims_lost);
        check.metric(&format!("{label}_speed_limited"), totals.speed_limited);
    }

    pub fn tick(&self) -> Tick {
        self.driver.next_tick
    }

    /// Retires a resident chunk in the next tick (which this does not run).
    pub fn retire(&mut self, key: ChunkKey) -> Result<(), String> {
        let position = self
            .slots
            .iter()
            .position(|(resident, _)| *resident == key)
            .ok_or_else(|| format!("{key:?} is not resident"))?;
        let (_, slot) = self.slots.remove(position);
        self.driver
            .world
            .retire_slot(slot)
            .map_err(|refusal| format!("retire {key:?}: {refusal:?}"))
    }

    /// Stages `cells` for `key`; they become resident with the next tick.
    pub fn upload(&mut self, key: ChunkKey, cells: &[WorldCell]) -> Result<(), String> {
        let slot = self.driver.stage_upload(key, cells)?;
        self.slots.push((key, slot));
        Ok(())
    }
}

/// Adds one tick's body report to running sums (maxima for the residuals).
pub fn accumulate(sum: &mut BodyReport, tick: &BodyReport) {
    sum.substeps += tick.substeps;
    sum.live = tick.live;
    sum.suspended = tick.suspended;
    sum.candidate_pairs += tick.candidate_pairs;
    sum.contact_pairs += tick.contact_pairs;
    sum.swept_contacts += tick.swept_contacts;
    sum.terrain_contacts += tick.terrain_contacts;
    sum.residual_overlap = sum.residual_overlap.max(tick.residual_overlap);
    sum.residual_approach = sum.residual_approach.max(tick.residual_approach);
    sum.speed_limited += tick.speed_limited;
    sum.swept_bodies += tick.swept_bodies;
    sum.fluid_in += tick.fluid_in;
    sum.fluid_out += tick.fluid_out;
    sum.solid_in += tick.solid_in;
    sum.solid_out += tick.solid_out;
    sum.compaction += tick.compaction;
    sum.ice_broken += tick.ice_broken;
    sum.organics_cleared += tick.organics_cleared;
    sum.craters += tick.craters;
    sum.ruts += tick.ruts;
    sum.splashes += tick.splashes;
    sum.wakes += tick.wakes;
    sum.claims_lost += tick.claims_lost;
}

/// One chunk of `shape` applied to dirt columns.
pub fn chunk_cells(key: ChunkKey, shape: &impl Fn(IVec2, &mut WorldCell)) -> Vec<WorldCell> {
    let origin = key.origin_cell();
    (0..CHUNK_CELL_COUNT)
        .map(|index| {
            let cell =
                origin + IVec2::new((index % CHUNK_SIZE) as i32, (index / CHUNK_SIZE) as i32);
            let mut column = dirt();
            shape(cell, &mut column);
            column
        })
        .collect()
}

/// The elevation scale every scene starts with.
pub fn scale() -> f32 {
    ElevationScale::DEFAULT.value()
}
