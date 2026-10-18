//! Connected scenarios: one story through the production runtime and live world, step by step.
//!
//! `resident-world` is delivered incrementally (verification evidence, "Connected scenario").
//! Each step reports on its own; a step a later stage owns is reported as not implemented, which
//! fails the scenario until that stage delivers it.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use world_model::{
    body::{BodyHandle, BodyPreset},
    cell::WorldCell,
    command::{BodySpawn, Command, Direction, Drive, EditTool, MaterialEdit, Ray, Target},
    glam::{DVec3, IVec2, Vec3},
    material::{FluidMat, GranularMat, TerrainMat},
    rules::SimulationRules,
    spatial::{CHUNK_CELL_COUNT, CHUNK_SIZE, ChunkKey},
};
use world_runtime::{
    bodies::BodyEvent,
    ids::{CommandId, Tick, WorldId},
    port::{CommandResult, ResidentWindow},
    receipts::ReceiptState,
    runtime::{Runtime, RuntimeConfig},
};

use super::{Check, allow_software, bodies::scene, transfers::runtime_upload};

/// One step of a scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub name: String,
    /// The stage that delivers the step.
    pub owner: String,
    pub summary: String,
    pub implemented: bool,
    pub passed: bool,
    pub failures: Vec<String>,
    pub notes: Vec<String>,
    pub metrics: Map<String, Value>,
}

/// What a scenario process reports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioResult {
    pub scenario: String,
    pub adapter: String,
    pub steps: Vec<StepResult>,
}

impl ScenarioResult {
    pub fn passed(&self) -> bool {
        self.steps.iter().all(|step| step.passed)
    }
}

/// The steps of `resident-world`, in order, with the stage that delivers each.
pub const RESIDENT_WORLD_STEPS: [(&str, &str, &str); 6] = [
    (
        "known-terrain",
        "04",
        "known chunks with a sand bank and a water basin become resident; columns at chunk corners, negative cells and the window edge read back as uploaded",
    ),
    (
        "sphere-into-water",
        "04",
        "a controlled sphere driven across the sand bank into the water ruts the sand and displaces water, every tick balanced",
    ),
    (
        "ray-edit-and-consumer",
        "04",
        "a brush stroke at a GPU-resolved ray is acknowledged and observed; the CPU consumer reacts to the sphere's observed entry into water and the reaction applies",
    ),
    (
        "focus-and-eviction",
        "05",
        "focus moves across chunk boundaries, dirty outgoing chunks are captured and evicted, new chunks load",
    ),
    (
        "return-and-reload",
        "05",
        "focus returns and the edited chunks reload with their changes",
    ),
    (
        "checkpoint-and-reopen",
        "05, 07",
        "a complete checkpoint is saved, the application and store are destroyed and reopened, and the promised state compares equal",
    ),
];

pub fn run(scenario: &str, adapter: &str) -> Option<ScenarioResult> {
    match scenario {
        "resident-world" => Some(resident_world(adapter)),
        _ => None,
    }
}

fn step(name: &str, check: Check, implemented: bool) -> StepResult {
    let (owner, summary) = RESIDENT_WORLD_STEPS
        .iter()
        .find(|(step, ..)| *step == name)
        .map(|(_, owner, summary)| ((*owner).to_owned(), (*summary).to_owned()))
        .unwrap_or_default();
    let (failures, notes, metrics) = check.into_parts();
    StepResult {
        name: name.to_owned(),
        owner,
        summary,
        implemented,
        passed: implemented && failures.is_empty(),
        failures,
        notes,
        metrics,
    }
}

/// The scenario's terrain, by absolute cell: a sand bank west of x = 40 (stone at 44 under six
/// units of sand), a slope down to x = 64, and east of it a stone basin at 16 holding 24 units
/// of water. Rows differ only by a one-unit ripple every 16 cells of y, so cells keep their
/// identity in the checks.
pub fn known_terrain(cell: IVec2) -> WorldCell {
    let mut column = scene::dirt();
    let ripple = u16::from(cell.y.rem_euclid(16) == 0);
    if cell.x < 40 {
        column.set_elevation(44 + ripple);
        column.set_granular_mat(GranularMat::GRANULAR_SAND);
        column.set_granular_vol(6);
    } else if cell.x < 64 {
        column.set_elevation((44 - (cell.x - 40)) as u16 + ripple);
        column.set_granular_mat(GranularMat::GRANULAR_SAND);
        column.set_granular_vol(4);
    } else {
        column.set_terrain_mat(TerrainMat::TERRAIN_STONE);
        column.set_elevation(16 + ripple);
        column.set_fluid_mat(FluidMat::FLUID_WATER);
        column.set_fluid_vol(24);
    }
    column
}

fn chunk(key: ChunkKey) -> Vec<WorldCell> {
    let origin = key.origin_cell();
    (0..CHUNK_CELL_COUNT)
        .map(|index| {
            known_terrain(
                origin + IVec2::new((index % CHUNK_SIZE) as i32, (index / CHUNK_SIZE) as i32),
            )
        })
        .collect()
}

struct Story {
    runtime: Runtime,
    id: u64,
}

impl Story {
    fn submit(&mut self, command: Command, tick: u64) -> Result<CommandId, String> {
        self.id += 1;
        let id = CommandId::new(self.id);
        self.runtime
            .submit_at(id, command, Tick::new(tick))
            .map_err(|error| format!("submit {command:?}: {error:?}"))?;
        Ok(id)
    }

    fn next(&self) -> u64 {
        self.runtime
            .last_submitted_tick()
            .map_or(0, |tick| tick.value() + 1)
    }

    /// Runs frames of one tick until one more tick is submitted (a frame submits nothing while
    /// three ticks are in flight).
    fn frame(&mut self) {
        let before = self.next();
        let started = std::time::Instant::now();
        while self.next() == before && started.elapsed() < super::driver::DEVICE_PATIENCE {
            self.runtime.frame_ticks(1);
            if self.next() == before {
                std::thread::sleep(Duration::from_micros(50));
            }
        }
    }

    fn settle(&mut self) -> Result<(), String> {
        let started = std::time::Instant::now();
        loop {
            let counters = self.runtime.counters();
            if counters.ticks_observed == counters.ticks_submitted {
                return Ok(());
            }
            if started.elapsed() > super::driver::DEVICE_PATIENCE {
                return Err("submitted ticks were never observed".to_owned());
            }
            self.runtime.frame_ticks(0);
            std::thread::sleep(Duration::from_micros(100));
        }
    }

    fn result(&self, id: CommandId) -> Option<(Tick, CommandResult)> {
        match self.runtime.receipt(id)?.state {
            ReceiptState::Completed { tick, result } => Some((tick, result)),
            _ => None,
        }
    }

    /// Runs frames until `id` has completed.
    fn await_result(&mut self, id: CommandId) -> Result<(Tick, CommandResult), String> {
        for _ in 0..600 {
            if let Some(done) = self.result(id) {
                return Ok(done);
            }
            self.frame();
        }
        self.settle()?;
        self.result(id)
            .ok_or_else(|| format!("{id:?} never completed"))
    }
}

pub fn resident_world(adapter: &str) -> ScenarioResult {
    let mut steps = Vec::new();
    let mut story = None;
    let mut check = Check::default();
    match known_terrain_step(adapter, &mut check) {
        Ok(opened) => story = Some(opened),
        Err(error) => check.fail(format!("stopped early: {error}")),
    }
    steps.push(step("known-terrain", check, true));

    let mut sphere = None;
    let mut check = Check::default();
    match story.as_mut() {
        Some(story) => match sphere_step(story, &mut check) {
            Ok(body) => sphere = Some(body),
            Err(error) => check.fail(format!("stopped early: {error}")),
        },
        None => check.fail("the terrain step did not open the world"),
    }
    steps.push(step("sphere-into-water", check, true));

    let mut check = Check::default();
    match (story.as_mut(), sphere) {
        (Some(story), Some(body)) => {
            if let Err(error) = edit_and_consumer_step(story, body, &mut check) {
                check.fail(format!("stopped early: {error}"));
            }
        }
        _ => check.fail("the sphere step did not complete"),
    }
    steps.push(step("ray-edit-and-consumer", check, true));

    for (name, owner, _) in &RESIDENT_WORLD_STEPS[3..] {
        let mut check = Check::default();
        check.fail(format!(
            "not implemented: prompt {owner} delivers this step"
        ));
        steps.push(step(name, check, false));
    }
    ScenarioResult {
        scenario: "resident-world".to_owned(),
        adapter: adapter.to_owned(),
        steps,
    }
}

/// Step 1: the known chunks become resident through the runtime and read back exactly.
fn known_terrain_step(adapter: &str, check: &mut Check) -> Result<Story, String> {
    let window = ResidentWindow::new(ChunkKey::new(0, 0, 0), 2, 2).map_err(|e| format!("{e:?}"))?;
    let mut config = gpu_world::world::GpuWorldConfig::new(WorldId::new(0x0007), adapter, window);
    config.allow_software = allow_software();
    // The terrain is read back before any rule runs; the rules start with step 2.
    config.rules = SimulationRules::NONE;
    config.transfer.bodies = 64;
    let world = gpu_world::world::GpuWorld::open(config).map_err(|e| format!("open: {e:?}"))?;
    let mut runtime =
        Runtime::new(Box::new(world), RuntimeConfig::default()).map_err(|e| format!("{e:?}"))?;
    let keys: Vec<ChunkKey> = window.view().iter().collect();
    for key in &keys {
        runtime_upload(&mut runtime, *key, &chunk(*key))?;
    }
    check.metric("chunks", keys.len());
    let mut story = Story { runtime, id: 0 };
    // Chunk corners on both sides of the origin, the window's outer corners, interior cells,
    // and one cell past the edge.
    let probes = [
        IVec2::new(-128, -128),
        IVec2::new(-1, -1),
        IVec2::new(0, 0),
        IVec2::new(63, 63),
        IVec2::new(64, 0),
        IVec2::new(-65, 17),
        IVec2::new(39, 32),
        IVec2::new(40, 32),
        IVec2::new(191, 191),
        IVec2::new(100, -100),
    ];
    let at = story.next() + 1;
    let mut ids = Vec::new();
    for cell in probes {
        ids.push((cell, story.submit(Command::QueryColumn { cell }, at)?));
    }
    let outside = story.submit(
        Command::QueryColumn {
            cell: IVec2::new(192, 0),
        },
        at,
    )?;
    for (cell, id) in ids {
        match story.await_result(id)?.1 {
            CommandResult::Column {
                cell: answered,
                column,
            } => {
                check.equal(&format!("column {cell} answered for"), answered, cell);
                check.equal(
                    &format!("column {cell} packed"),
                    column,
                    known_terrain(cell),
                );
            }
            other => check.fail(format!("column {cell}: {other:?}")),
        }
    }
    check.equal(
        "a column past the window edge",
        story.await_result(outside)?.1,
        CommandResult::ColumnNotResident {
            cell: IVec2::new(192, 0),
        },
    );
    Ok(story)
}

/// Step 2: with the rules on, a controlled sphere is driven east across the bank into the
/// basin.
fn sphere_step(story: &mut Story, check: &mut Check) -> Result<BodyHandle, String> {
    let at = story.next() + 1;
    story.submit(
        Command::SetSimulationRules(SimulationRules::DETERMINISTIC),
        at,
    )?;
    let spawn =
        BodySpawn::new(BodyPreset::Metal, IVec2::new(8, 32), 2.0).map_err(|e| e.to_string())?;
    let spawned = story.submit(Command::SpawnBody(spawn), at)?;
    let body = match story.await_result(spawned)?.1 {
        CommandResult::BodySpawned { body, .. } => body,
        other => return Err(format!("spawn: {other:?}")),
    };
    story.runtime.set_controlled(Some(body));
    let east = Drive::controls(Direction::new(1.0, 0.0).map_err(|e| e.to_string())?);
    let started = story.next();
    let mut entered = None;
    let mut events = Vec::new();
    let mut log: BTreeMap<&'static str, u32> = BTreeMap::new();
    let mut scheduled = started;
    for _ in 0..900 {
        let next = story.next();
        // Keep a few ticks of drive commands queued ahead, as held controls do.
        while scheduled < next + 4 {
            story.submit(Command::DriveBody { body, drive: east }, scheduled)?;
            scheduled += 1;
        }
        story.frame();
        events.clear();
        story.runtime.bodies_mut().drain_events(&mut events);
        for event in &events {
            let kind = match event {
                BodyEvent::Spawned { .. } => "spawned",
                BodyEvent::Landed { .. } => "landed",
                BodyEvent::EnteredLiquid { .. } => "entered_liquid",
                BodyEvent::TouchedBody { .. } => "touched_body",
                BodyEvent::Cratered { .. } => "cratered",
                BodyEvent::Splashed { .. } => "splashed",
                BodyEvent::Removed { .. } => "removed",
            };
            *log.entry(kind).or_default() += 1;
            if let BodyEvent::EnteredLiquid { tick, .. } = event {
                entered.get_or_insert(*tick);
            }
        }
        if let Some(sample) = story.runtime.bodies().body(body).and_then(|t| t.last)
            && entered.is_some()
            && sample.position.x > 72.0
        {
            break;
        }
    }
    story.settle()?;
    let sample = story
        .runtime
        .bodies()
        .body(body)
        .and_then(|tracked| tracked.last)
        .ok_or("no sample of the sphere")?;
    check.that(entered.is_some(), || {
        format!("the sphere never entered water: {sample:?}")
    });
    check.that(sample.position.x > 64.0, || {
        format!("the sphere did not reach the basin: {sample:?}")
    });
    check.that(
        sample.liquid == Some(FluidMat::FLUID_WATER) && sample.submerged_fraction > 0.0,
        || format!("the last sample is not in water: {sample:?}"),
    );
    for (kind, count) in &log {
        check.metric(&format!("events_{kind}"), *count);
    }
    check.that(log.get("landed").copied().unwrap_or(0) > 0, || {
        "no landing event".to_owned()
    });
    if let Some(tick) = entered {
        check.metric("entered_water_at_tick", tick.value() - started);
    }
    let ledger = *story.runtime.ledger().summary();
    check.equal("inconsistent ticks", ledger.inconsistent_ticks, 0);
    check.metric("ticks_observed", ledger.ticks_observed);
    check.metric(
        "sample_age_ticks_max",
        story.runtime.bodies().summary().sample_age_ticks.max,
    );
    check.that(
        story.runtime.bodies().summary().sample_age_ticks.max <= 2.0,
        || "a controlled-body sample was applied more than two ticks late".to_owned(),
    );
    // The body accounting the device reported: the sphere moved sand (its rut) and water.
    let moved = ledger.bodies;
    check.that(moved.solid_in > 0 && moved.ruts > 0, || {
        format!("the sphere left no rut: {moved:?}")
    });
    check.that(moved.fluid_in > 0, || {
        format!("the sphere displaced no water: {moved:?}")
    });
    check.metric("sand_moved_units", moved.solid_in);
    check.metric("water_displaced_units", moved.fluid_in);
    check.metric("ruts", moved.ruts);
    check.metric("wakes", moved.wakes);
    check.metric("splashes", moved.splashes);
    check.metric("craters", moved.craters);
    Ok(body)
}

/// Step 3: a stroke at a GPU-resolved ray, its acknowledgement and its observation, and a CPU
/// consumer reacting to the sphere's observed state.
fn edit_and_consumer_step(
    story: &mut Story,
    body: BodyHandle,
    check: &mut Check,
) -> Result<(), String> {
    // Aim at the sand bank from above, as a pointer would.
    let ray = Ray::new(
        DVec3::new(20.4, 120.0, -24.6),
        Vec3::new(0.0, -1.0, -0.05),
        400.0,
    )
    .map_err(|e| e.to_string())?;
    let edit =
        MaterialEdit::at(EditTool::Sand, Target::Ray(ray), 2, 8).map_err(|e| e.to_string())?;
    let at = story.next() + 1;
    let query = story.submit(Command::QueryRay { ray }, at)?;
    let stroke = story.submit(Command::EditMaterial(edit), at)?;
    let (query_tick, hit) = story.await_result(query)?;
    let (stroke_tick, edited) = story.await_result(stroke)?;
    let hit_cell = match hit {
        CommandResult::Ray {
            surface: world_model::observation::SurfaceHit::Hit { cell, .. },
            ..
        } => cell,
        other => return Err(format!("ray query: {other:?}")),
    };
    check.equal("query and stroke in one tick", query_tick, stroke_tick);
    let center = match edited {
        CommandResult::Edited {
            center, changed, ..
        } => {
            check.that(changed > 0, || "the stroke changed nothing".to_owned());
            check.metric("stroke_changed_cells", changed);
            center
        }
        other => return Err(format!("stroke: {other:?}")),
    };
    check.equal("stroke centre is the ray's hit", center, hit_cell);
    // Eventual observation: a later column query sees the added sand.
    let observe = story.submit(Command::QueryColumn { cell: center }, 0)?;
    match story.await_result(observe)? {
        (tick, CommandResult::Column { column, .. }) => {
            check.that(tick > stroke_tick, || {
                "observed before the acknowledgement".to_owned()
            });
            check.that(
                column.granular_vol() > known_terrain(center).granular_vol(),
                || format!("the stroke's sand is not observed: {column:?}"),
            );
            check.metric(
                "observation_ticks_after_ack",
                tick.value() - stroke_tick.value(),
            );
        }
        other => check.fail(format!("observation: {other:?}")),
    }

    // The consumer: while the controlled sphere's observed summary says it is in water, it
    // holds the lift at the sphere's column, one command a tick, as a held tool does; it records
    // how old each observation was and when its first reaction applied.
    let mut first = None;
    let mut rose = false;
    let mut left_water = None;
    let mut lifts = 0u32;
    let mut last_reacted = None;
    for _ in 0..240 {
        story.frame();
        let Some(sample) = story.runtime.bodies().body(body).and_then(|t| t.last) else {
            continue;
        };
        if first.is_some() {
            rose |= sample.velocity.y > 0.0;
            if sample.submerged_fraction == 0.0 && rose {
                left_water = Some(sample.tick);
                break;
            }
        }
        if sample.submerged_fraction > 0.0 && last_reacted != Some(sample.tick) {
            last_reacted = Some(sample.tick);
            let cell = IVec2::new(
                sample.position.x.round() as i32,
                (-sample.position.z).round() as i32,
            );
            let observed_at = story.runtime.last_submitted_tick().map_or(0, |t| t.value());
            let id = story.submit(
                Command::LiftBodies {
                    target: Target::Column(cell),
                },
                0,
            )?;
            lifts += 1;
            first.get_or_insert((sample.tick, observed_at, id));
        }
    }
    let (sampled, observed_at, id) = first.ok_or("the consumer never saw the sphere in water")?;
    check.metric("observation_age_at_reaction_ticks", observed_at - sampled);
    check.metric("lift_commands", lifts);
    let (applied, result) = story.await_result(id)?;
    match result {
        CommandResult::Lifted { bodies, .. } => {
            check.that(bodies >= 1, || "the reaction lifted nothing".to_owned());
        }
        other => check.fail(format!("reaction: {other:?}")),
    }
    check.metric("reaction_ticks_from_sample", applied.value() - sampled);
    check.that(applied.value() - sampled <= 4, || {
        format!(
            "the reaction applied {} ticks after its observation",
            applied.value() - sampled
        )
    });
    check.that(rose, || {
        "the lifted sphere was never observed rising".to_owned()
    });
    match left_water {
        Some(tick) => check.metric("left_water_ticks_after_reaction", tick - applied.value()),
        None => check.fail("the lifted sphere was never observed out of the water"),
    }
    story.settle()?;
    let ledger = *story.runtime.ledger().summary();
    check.equal("inconsistent ticks", ledger.inconsistent_ticks, 0);
    check.equal("unknown outcomes", story.runtime.unknown_outcomes(), 0);
    check.metric(
        "input_latency_ticks_mean",
        ledger.input_latency_ticks.mean().unwrap_or(0.0),
    );
    check.metric(
        "observation_age_ticks_mean",
        ledger.observation_age_ticks.mean().unwrap_or(0.0),
    );
    Ok(())
}
