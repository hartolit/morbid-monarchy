//! Step 6 of `resident-world`: the reopened trace again under adversarial load.
//!
//! The parts run in order on one world, each from the state the previous left:
//!
//! 1. **Dense bodies at a seam** (07): 62 spheres dropped around the corner of four chunks and
//!    four tiles where the bank meets the basin, pulled into one pile there while brushes pour
//!    water and lava and add sand and lower terrain under them, the air freezes and thaws, the
//!    cosmetic events overflow and map deliveries are held. The consumer of body events stays
//!    away for the whole span and reads the spawn outcomes only afterwards. The world's totals
//!    at both ends must equal the ledger's sums of tools, reactions and compaction; the events
//!    the consumer finally drains must add up to every body's counters. A removed body's handle
//!    stays dead when its slot is reused, and a spawn beyond the table's capacity is refused.
//! 2. **A rebase across negative coordinates** (07): the window's origin moves from (−2, −2) to
//!    (−3, −3) under the resting pile, which goes on unchanged, then to a window of negative keys
//!    only, which suspends every body and evicts the seam with its write held and deliveries
//!    reversed, and back before the write lands.
//! 3. **Faults and recovery** (05): held map deliveries while a save and a growth of the window
//!    overlap, failed eviction writes, out-of-order loads, a save whose publication fails and is
//!    retried, a complete checkpoint, then a device lost before a later save's payloads reached
//!    the CPU; a new store and session recover that checkpoint with its bodies and
//!    configuration and refuse the lost session's slots.
//! 4. **Interruption and orderly close** (07): the recovered session closes in order; a child
//!    process changes the world and dies inside its save, once before the publication commits
//!    and once after; each reopen finds exactly the checkpoint the documented boundary names.
//!    A last orderly close and a fresh reopen end the trace.
//!
//! Parts 3 and 4 are in `recovery`.

use std::{collections::BTreeMap, path::Path, time::Duration};

use world_model::{
    body::{BodyHandle, BodyPreset, footprint_reach},
    command::{
        AmbientTemperature, BodySpawn, Command, Direction, Drive, EditTool, MaterialEdit, Target,
    },
    glam::{DVec2, DVec3, IVec2},
    material::FluidMat,
    observation::{BodyCounters, WorldTotals},
    rules::SimulationRules,
    spatial::{CHUNK_SIZE, ChunkKey},
};
use world_runtime::{
    bodies::BodyEvent,
    port::{CommandResult, ResidentWindow},
};
use world_store::faults::{Action, Barrier};

use super::{super::Check, Probes, Story, compare, home};

mod recovery;

pub(super) use recovery::{INTERRUPT, interruption_child};

/// The corner where chunks (0, −1), (1, −1), (0, 0) and (1, 0) meet, which is also the corner of
/// four 8 × 8 tiles: the bank's slope ends there and the basin's water begins.
const SEAM: IVec2 = IVec2::new(64, 0);
/// Spheres the pile adds to the trace's one: with it, one less than the sessions' body table.
const CLUSTER: usize = 62;
/// Ticks of the dense span: attraction for the first 150, rules off at the last.
const SPAN: u64 = 180;

pub(super) fn adversarial_recovery_step(
    adapter: &str,
    mut story: Story,
    sphere: BodyHandle,
    probes: Probes,
    check: &mut Check,
    captures: Option<&Path>,
) -> Result<(), String> {
    dense_seam(&mut story, sphere, check)?;
    let probes = negative_rebase(&mut story, probes, check)?;
    let story = recovery::faults_and_loss(adapter, story, &probes, check)?;
    recovery::interruptions(adapter, story, &probes, check, captures)
}

/// The ledger's running sums that the world's totals must agree with: what the tools added and
/// removed, what the material phases converted and what the bodies compacted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Sums {
    tool_fluid_in: u64,
    tool_solid_in: u64,
    tool_solid_out: u64,
    quench: u64,
    boil: u64,
    burn: u64,
    uptake: u64,
    compaction: u64,
}

fn sums(story: &Story) -> Sums {
    let ledger = story.session().runtime().ledger().summary();
    let cells = ledger.cells;
    Sums {
        tool_fluid_in: cells.tool_fluid_in,
        tool_solid_in: cells.tool_solid_in,
        tool_solid_out: cells.tool_solid_out,
        quench: cells.quench,
        boil: cells.boil,
        burn: cells.burn,
        uptake: cells.uptake,
        compaction: ledger.bodies.compaction,
    }
}

/// The world's totals and the ledger's sums at one point: every submitted tick is observed,
/// so the sums run through the last of them, and the totals are measured as the only command
/// of the next, before its material phases and bodies.
fn exact_totals(story: &mut Story) -> Result<(WorldTotals, Sums), String> {
    story.settle()?;
    let sums = sums(story);
    let at = story.next();
    let observed = story
        .session()
        .runtime()
        .ledger()
        .summary()
        .last_observed_tick
        .map(|tick| tick.value() + 1);
    if observed != Some(at) {
        return Err(format!(
            "the sums do not run through the tick before {at}: {observed:?}"
        ));
    }
    let id = story.submit(Command::MeasureTotals, at)?;
    match story.await_result(id)? {
        (tick, CommandResult::Totals(totals)) if tick.value() == at => Ok((totals, sums)),
        other => Err(format!("totals for tick {at}: {other:?}")),
    }
}

/// Liquid and ice change only by what tools added and reactions consumed; bedrock and loose
/// material by what tools added and removed, quenched magma added, and bodies compacted.
fn check_totals(check: &mut Check, before: (WorldTotals, Sums), after: (WorldTotals, Sums)) {
    let d = |later: u64, earlier: u64| i128::from(later) - i128::from(earlier);
    let ((t0, s0), (t1, s1)) = (before, after);
    let reactions = d(s1.quench, s0.quench)
        + d(s1.boil, s0.boil)
        + d(s1.burn, s0.burn)
        + d(s1.uptake, s0.uptake);
    check.equal(
        "liquid and ice against the tools and reactions",
        d(t1.fluid_volume(), t0.fluid_volume()),
        d(s1.tool_fluid_in, s0.tool_fluid_in) - reactions,
    );
    check.equal(
        "bedrock and loose material against the tools, quenching and compaction",
        d(t1.solid(), t0.solid()),
        d(s1.tool_solid_in, s0.tool_solid_in) - d(s1.tool_solid_out, s0.tool_solid_out)
            + d(s1.quench, s0.quench)
            - d(s1.compaction, s0.compaction),
    );
}

pub(super) fn edit(
    tool: EditTool,
    cell: IVec2,
    radius: i32,
    strength: u8,
) -> Result<Command, String> {
    MaterialEdit::new(tool, cell, radius, strength)
        .map(Command::EditMaterial)
        .map_err(|error| error.to_string())
}

pub(super) fn ambient(celsius: f32) -> Result<Command, String> {
    AmbientTemperature::celsius(celsius)
        .map(Command::SetAmbientTemperature)
        .map_err(|error| format!("{error:?}"))
}

/// The dense span's commands for each tick from its first.
struct Plan {
    attract: Command,
    water: Command,
    lava: Command,
    sand: Command,
    lower: Command,
    freeze: Command,
    thaw: Command,
}

impl Plan {
    fn new() -> Result<Self, String> {
        Ok(Self {
            attract: Command::AttractBodies {
                target: Target::Column(SEAM),
            },
            water: edit(EditTool::Water, IVec2::new(50, 6), 3, 4)?,
            lava: edit(EditTool::Lava, IVec2::new(74, -6), 2, 6)?,
            sand: edit(EditTool::Sand, SEAM, 4, 3)?,
            lower: edit(EditTool::Lower, IVec2::new(60, 6), 3, 2)?,
            freeze: ambient(-20.0)?,
            thaw: ambient(25.0)?,
        })
    }

    fn commands(&self, k: u64, out: &mut Vec<Command>) {
        out.clear();
        if k < 150 {
            out.push(self.attract);
        }
        if k < 100 && k.is_multiple_of(2) {
            out.push(self.water);
        }
        if (20..80).contains(&k) && k.is_multiple_of(10) {
            out.push(self.lava);
        }
        if (30..120).contains(&k) && k.is_multiple_of(15) {
            out.push(self.sand);
        }
        match k {
            30 => out.push(self.freeze),
            90 => out.push(self.lower),
            95 => out.push(Command::MeasureTotals),
            100 => out.push(self.thaw),
            SPAN => out.push(Command::SetSimulationRules(SimulationRules::NONE)),
            _ => {}
        }
    }
}

fn event_counts(events: &[BodyEvent]) -> BTreeMap<BodyHandle, (u32, BodyCounters)> {
    let mut counts: BTreeMap<BodyHandle, (u32, BodyCounters)> = BTreeMap::new();
    for event in events {
        let (spawns, counters) = counts.entry(event.body()).or_default();
        match *event {
            BodyEvent::Spawned { .. } => *spawns += 1,
            BodyEvent::Landed { count, .. } => counters.landings += count,
            BodyEvent::EnteredLiquid { count, .. } => counters.liquid_entries += count,
            BodyEvent::TouchedBody { count, .. } => counters.body_contacts += count,
            BodyEvent::Cratered { count, .. } => counters.craters += count,
            BodyEvent::Splashed { count, .. } => counters.splashes += count,
            BodyEvent::Removed { .. } => {}
        }
    }
    counts
}

/// The counters events report (absorbed energy has no event).
fn reported(counters: BodyCounters) -> [u32; 5] {
    [
        counters.landings,
        counters.liquid_entries,
        counters.body_contacts,
        counters.craters,
        counters.splashes,
    ]
}

/// Part 1: dense overlapping body work at the seam with brush edits and phase changes, under
/// delayed consumers, held deliveries and an event flood; then stale handles and capacity.
fn dense_seam(story: &mut Story, sphere: BodyHandle, check: &mut Check) -> Result<(), String> {
    let start = exact_totals(story)?;
    // The consumer catches up, then stays away for the whole span.
    let mut events = Vec::new();
    story.runtime().bodies_mut().drain_events(&mut events);
    let sphere_start = story
        .session()
        .runtime()
        .bodies()
        .body(sphere)
        .map(|tracked| tracked.counters)
        .ok_or("the sphere is not tracked")?;
    let monitor_before = *story.session().runtime().bodies().summary();
    let ledger_before = *story.session().runtime().ledger().summary();
    let counters_before = story.session().runtime().counters();

    // The places of a 9 × 9 grid nine cells apart across the four chunks nearest the seam,
    // less those too close to the sphere; metal and wood alternate.
    let sphere_at = story
        .sample(sphere)
        .ok_or("no sample of the sphere")?
        .position;
    let sphere_cell = DVec2::new(sphere_at.x, -sphere_at.z);
    let mut cells = Vec::new();
    for j in 0..9 {
        for i in 0..9 {
            let cell = SEAM + IVec2::new(-36 + 9 * i, -36 + 9 * j);
            if (cell.as_dvec2() - sphere_cell).length() >= 10.0 {
                cells.push(cell);
            }
        }
    }
    cells.sort_by_key(|cell| (*cell - SEAM).length_squared());
    if cells.len() < CLUSTER {
        return Err(format!("only {} free places for the pile", cells.len()));
    }
    cells.truncate(CLUSTER);

    let plan = Plan::new()?;
    let first = story.next() + 1;
    story.submit(
        Command::SetSimulationRules(SimulationRules::DETERMINISTIC),
        first,
    )?;
    let mut spawns = Vec::new();
    for (k, cell) in cells.iter().enumerate() {
        let preset = if k % 2 == 0 {
            BodyPreset::Metal
        } else {
            BodyPreset::Wood
        };
        let spawn = BodySpawn::new(preset, *cell, 6.0).map_err(|error| error.to_string())?;
        spawns.push(story.submit(Command::SpawnBody(spawn), first)?);
    }
    let mut scheduled = first;
    let mut batch = Vec::new();
    let mut middle = None;
    let mut flooded = false;
    let mut held = false;
    loop {
        let next = story.next();
        if next > first + SPAN {
            break;
        }
        // Keep a few ticks of commands queued ahead, as held tools do.
        while scheduled <= first + SPAN && scheduled < next + 4 {
            plan.commands(scheduled - first, &mut batch);
            for command in &batch {
                let id = story.submit(*command, scheduled)?;
                if matches!(command, Command::MeasureTotals) {
                    middle = Some(id);
                }
            }
            scheduled += 1;
        }
        let k = next.saturating_sub(first);
        if !flooded && k >= 40 {
            // More cosmetic events than a tick can record: counted, never mixed with outcomes.
            story.live.faults.flood_events(200);
            flooded = true;
        }
        if !held && k >= 60 {
            // Delayed readbacks: completed ticks wait undelivered while frames go on.
            held = true;
            story.live.faults.hold_deliveries(true);
            for _ in 0..30 {
                story.live.frames(1);
                std::thread::sleep(Duration::from_millis(2));
            }
            story.live.faults.hold_deliveries(false);
            continue;
        }
        story.frame();
    }
    let end = exact_totals(story)?;

    // The outcomes, read only now: the receipts kept them.
    let mut pile = Vec::new();
    for id in spawns {
        match story.result(id) {
            Some((_, CommandResult::BodySpawned { body, .. })) => pile.push(body),
            other => check.fail(format!("a spawn's outcome: {other:?}")),
        }
    }
    check.equal("spheres spawned", pile.len(), CLUSTER);
    let middle = match middle.map(|id| story.result(id)) {
        Some(Some((_, CommandResult::Totals(totals)))) => totals,
        other => return Err(format!("the middle totals: {other:?}")),
    };
    let ledger = *story.session().runtime().ledger().summary();
    let counters = story.session().runtime().counters();
    check.equal(
        "inconsistent ticks",
        ledger.inconsistent_ticks,
        ledger_before.inconsistent_ticks,
    );
    check_totals(check, start, end);
    let (s0, s1) = (start.1, end.1);
    check.that(s1.quench > s0.quench, || {
        "no magma was quenched by water".to_owned()
    });
    let ice = middle.fluid[FluidMat::FLUID_ICE.0 as usize];
    check.that(ice > 0, || "no water froze in the cold".to_owned());
    check.metric("ice_units_while_cold", ice);
    check.metric(
        "ice_units_after_thaw",
        end.0.fluid[FluidMat::FLUID_ICE.0 as usize],
    );
    check.metric("quenched_units", s1.quench - s0.quench);
    check.metric("boiled_units", s1.boil - s0.boil);
    check.metric("tool_liquid_units", s1.tool_fluid_in - s0.tool_fluid_in);
    check.metric(
        "tool_solid_units",
        (s1.tool_solid_in - s0.tool_solid_in) as i64
            - (s1.tool_solid_out - s0.tool_solid_out) as i64,
    );
    check.metric("compacted_units", s1.compaction - s0.compaction);
    let bodies = ledger.bodies;
    let before = ledger_before.bodies;
    check.that(bodies.contact_pairs > before.contact_pairs, || {
        "the pile made no body contacts".to_owned()
    });
    check.equal(
        "live bodies after the span",
        bodies.live,
        CLUSTER as u32 + 1,
    );
    check.metric("contact_pairs", bodies.contact_pairs - before.contact_pairs);
    check.metric(
        "candidate_pairs",
        bodies.candidate_pairs - before.candidate_pairs,
    );
    check.metric("claims_lost", bodies.claims_lost - before.claims_lost);
    check.metric("ice_broken", bodies.ice_broken - before.ice_broken);
    check.metric("residual_overlap_max", bodies.residual_overlap);
    check.metric(
        "cosmetic_dropped_on_device",
        ledger.cosmetic_dropped_on_device - ledger_before.cosmetic_dropped_on_device,
    );
    check.that(
        ledger.cosmetic_dropped_on_device > ledger_before.cosmetic_dropped_on_device,
        || "the event flood was not counted".to_owned(),
    );
    check.that(counters.in_flight_high_water <= 3, || {
        format!("{} ticks in flight", counters.in_flight_high_water)
    });
    check.equal(
        "frame waits",
        counters.frame_waits - counters_before.frame_waits,
        0,
    );
    check.metric(
        "sample_age_ticks_max",
        story
            .session()
            .runtime()
            .bodies()
            .summary()
            .sample_age_ticks
            .max,
    );

    // The consumer comes back: every event its absence deferred arrives, and each body's events
    // add up to its counters.
    events.clear();
    story.runtime().bodies_mut().drain_events(&mut events);
    let monitor = *story.session().runtime().bodies().summary();
    check.that(
        monitor.events_deferred > monitor_before.events_deferred,
        || "the absent consumer never filled the event ring".to_owned(),
    );
    check.equal(
        "unreported removals",
        monitor.removals_unreported,
        monitor_before.removals_unreported,
    );
    check.metric("events_drained", events.len());
    check.metric(
        "events_deferred",
        monitor.events_deferred - monitor_before.events_deferred,
    );
    check.metric("event_ring_high_water", monitor.ring_high_water);
    let counts = event_counts(&events);
    for body in pile.iter().copied().chain([sphere]) {
        let Some(tracked) = story.session().runtime().bodies().body(body) else {
            check.fail(format!("{body:?} is not tracked"));
            continue;
        };
        let Some(last) = tracked.last else {
            check.fail(format!("{body:?} was never sampled"));
            continue;
        };
        let (spawned, delivered) = counts.get(&body).copied().unwrap_or_default();
        let from = if body == sphere {
            sphere_start
        } else {
            BodyCounters::default()
        };
        check.equal(
            &format!("{body:?} spawn events"),
            spawned,
            u32::from(body != sphere),
        );
        check.equal(
            &format!("{body:?} events against its counters"),
            reported(delivered),
            reported(last.counters.since(&from)),
        );
        check.equal(
            &format!("{body:?} delivered through its latest sample"),
            reported(tracked.counters),
            reported(last.counters),
        );
    }

    // A removed body's handle stays dead when its slot is reused, and a full table refuses.
    let victim = *pile
        .get(pile.len() / 2)
        .ok_or("no sphere of the pile spawned")?;
    let at = story.next();
    let removed = story.submit(Command::RemoveBody { body: victim }, at)?;
    check.that(
        matches!(story.await_result(removed)?.1, CommandResult::BodyRemoved { body, .. } if body == victim),
        || "the removal did not report the removed body".to_owned(),
    );
    let spawn_at = |story: &mut Story, cell: IVec2| -> Result<CommandResult, String> {
        let spawn =
            BodySpawn::new(BodyPreset::Metal, cell, 4.0).map_err(|error| error.to_string())?;
        let at = story.next();
        let id = story.submit(Command::SpawnBody(spawn), at)?;
        Ok(story.await_result(id)?.1)
    };
    match spawn_at(story, IVec2::new(20, -40))? {
        CommandResult::BodySpawned { body, .. } => {
            check.equal("the freed slot is reused", body.index(), victim.index());
            check.that(body.generation() != victim.generation(), || {
                format!("the reused slot kept its generation: {body:?}")
            });
        }
        other => check.fail(format!("the spawn into the freed slot: {other:?}")),
    }
    let drive = Drive::controls(Direction::new(1.0, 0.0).map_err(|error| error.to_string())?);
    let at = story.next();
    let stale = story.submit(
        Command::DriveBody {
            body: victim,
            drive,
        },
        at,
    )?;
    check.equal(
        "a command for the removed body",
        story.await_result(stale)?.1,
        CommandResult::StaleBody { body: victim },
    );
    check.that(
        story.session().runtime().bodies().body(victim).is_none(),
        || "the removed body is still tracked".to_owned(),
    );
    check.that(
        matches!(
            spawn_at(story, IVec2::new(20, -60))?,
            CommandResult::BodySpawned { .. }
        ),
        || "the last free slot refused a body".to_owned(),
    );
    check.equal(
        "a spawn beyond the table",
        spawn_at(story, IVec2::new(20, -80))?,
        CommandResult::BodyCapacity { capacity: 64 },
    );
    check.equal(
        "bodies tracked",
        story.session().runtime().bodies().bodies().count(),
        64,
    );
    Ok(())
}

/// Where a body's latest sample put it.
#[derive(Debug, Clone, Copy)]
struct Pose {
    body: BodyHandle,
    position: DVec3,
    tick: u64,
    suspended: bool,
}

fn poses(story: &Story) -> Vec<Pose> {
    story
        .session()
        .runtime()
        .bodies()
        .bodies()
        .filter_map(|tracked| {
            tracked.last.map(|sample| Pose {
                body: tracked.body,
                position: sample.position,
                tick: sample.tick,
                suspended: sample.suspended,
            })
        })
        .collect()
}

fn pose_of(poses: &[Pose], body: BodyHandle) -> Option<&Pose> {
    poses.iter().find(|pose| pose.body == body)
}

/// The most a body may move over 30 frames and count as still, in world units: settled spheres
/// creep a few ten-thousandths of a unit a tick.
const STILL: f64 = 0.02;
/// The most a still body may move in the tick that moves its window or resumes it, in world
/// units: a wrong origin or resume displaces bodies by a unit or more in that tick (contact
/// pushes a body out of the terrain it was placed into), while one tick of the pile's own motion
/// moves a still body by hundredths of a unit.
const JUMP: f64 = 0.5;
/// The most the median still body may move over the frames after a change, in world units: a
/// pile placed above its support moves little in the first tick and falls for the frames after
/// (about 0.4 units in ten ticks), while restless neighbours nudge a few still bodies, by up to
/// a few units, but not most of them.
const DRIFT: f64 = 0.1;

/// The pile at rest: the poses at the end of the 30 frames that found it so, and how far each
/// body strayed from where it was during them.
struct Rest {
    poses: Vec<Pose>,
    strays: BTreeMap<BodyHandle, f64>,
}

impl Rest {
    /// The bodies that strayed no more than [`STILL`] at any frame. Being back within it after 30
    /// frames is not enough: some spheres in a crushed pile rock in a cycle of a few ticks (three
    /// in the cases traced), back where they were every 30 ticks and up to a third of a unit away
    /// one tick later.
    fn still(&self) -> Vec<BodyHandle> {
        self.strays
            .iter()
            .filter(|(_, stray)| **stray <= STILL)
            .map(|(body, _)| *body)
            .collect()
    }
}

/// Runs frames until at least three quarters of the bodies moved no more than [`STILL`] over 30
/// frames. Some spheres in a crushed pile never settle (they rock against their neighbours);
/// they are reported, not waited for.
fn pile_at_rest(story: &mut Story, check: &mut Check) -> Result<Rest, String> {
    let mut still_counts = Vec::new();
    for round in 1..=60 {
        let start = poses(story);
        let mut strays: BTreeMap<BodyHandle, f64> =
            start.iter().map(|pose| (pose.body, 0.0)).collect();
        let mut stray = |now: &[Pose]| {
            for first in &start {
                let moved = pose_of(now, first.body).map_or(f64::INFINITY, |pose| {
                    (pose.position - first.position).length()
                });
                strays
                    .entry(first.body)
                    .and_modify(|stray| *stray = stray.max(moved));
            }
        };
        for _ in 0..30 {
            story.frame();
            stray(&poses(story));
        }
        story.settle()?;
        let now = poses(story);
        stray(&now);
        let still = start
            .iter()
            .filter(|first| {
                pose_of(&now, first.body)
                    .is_some_and(|pose| (pose.position - first.position).length() <= STILL)
            })
            .count();
        if still * 4 >= now.len() * 3 {
            check.metric("frames_to_rest", round * 30);
            return Ok(Rest { poses: now, strays });
        }
        still_counts.push(still);
    }
    Err(format!(
        "the pile never came to rest; still bodies after each 30 frames: {still_counts:?}"
    ))
}

/// Whether `window` holds every cell a resting sphere at `position` reads, with a cell to spare:
/// the residency edge has no reason to suspend it.
fn well_inside(window: ResidentWindow, position: DVec3) -> bool {
    let reach = f64::from(footprint_reach(BodyPreset::Metal.sphere().radius, 0.0)) + 1.0;
    let low = window.min_key().origin_cell().as_dvec2();
    let high =
        low + DVec2::new(f64::from(window.width()), f64::from(window.height())) * CHUNK_SIZE as f64;
    let cell = DVec2::new(position.x, -position.z);
    (cell - reach).cmpge(low).all() && (cell + reach).cmplt(high).all()
}

/// Moves the window to `window` a tick a frame, as `Live::move_to` does, and runs on until every
/// body has a sample after the change, or 60 frames after the move settled. Returns the frames
/// and, for each body that has one, its last sample before the change and its first after it:
/// the pile keeps ticking while the window moves, and only the tick between the two samples can
/// carry the change. `changed` says whether a sample is after it, given the first tick encoded
/// against the new window.
fn move_across(
    story: &mut Story,
    window: ResidentWindow,
    changed: impl Fn(&Pose, u64) -> bool,
) -> Result<(u32, Vec<(Pose, Pose)>), String> {
    let mut before: BTreeMap<BodyHandle, Pose> = poses(story)
        .into_iter()
        .map(|pose| (pose.body, pose))
        .collect();
    let mut after = BTreeMap::new();
    story.live.session.set_window(window);
    let started = std::time::Instant::now();
    let mut first_tick = None;
    let mut frames = 0;
    let mut settled_frames = 0;
    loop {
        story.live.frame_ticks(1);
        frames += 1;
        // The window moves after the frame's ticks were submitted: the next is the first
        // encoded against it.
        if first_tick.is_none() && story.session().residency().window() == window {
            first_tick = Some(story.next());
        }
        for pose in poses(story) {
            if first_tick.is_some_and(|first| changed(&pose, first)) {
                after.entry(pose.body).or_insert(pose);
            } else if !after.contains_key(&pose.body) {
                before.insert(pose.body, pose);
            }
        }
        if first_tick.is_some() && story.session().residency().settled() {
            settled_frames += 1;
            if after.len() == before.len() || settled_frames > 60 {
                break;
            }
        }
        if started.elapsed() > super::super::driver::DEVICE_PATIENCE * 3 {
            return Err(format!(
                "move: not done after {frames} frames: {:?}",
                story.session().residency().summary()
            ));
        }
        std::thread::sleep(Duration::from_micros(100));
    }
    Ok((
        frames,
        before
            .into_values()
            .filter_map(|last| after.get(&last.body).map(|first| (last, *first)))
            .collect(),
    ))
}

/// Checks that a change of the window (`change`, `name` in the metrics) left the resting pile
/// to its own motion: no still body moved more than [`JUMP`] in the tick of the change
/// (`across`, from [`move_across`]), and the median still body no more than [`DRIFT`] from
/// `earlier` to `later`, over the frames after it.
fn pile_unmoved(
    check: &mut Check,
    change: &str,
    name: &str,
    still: &[BodyHandle],
    across: &[(Pose, Pose)],
    earlier: &[Pose],
    later: &[Pose],
) {
    let mut in_the_tick = Vec::new();
    for body in still {
        match across.iter().find(|(last, _)| last.body == *body) {
            Some((last, first)) => in_the_tick.push((
                (first.position - last.position).length(),
                *body,
                first.tick - last.tick,
            )),
            None => check.fail(format!("{body:?} has no sample after {change}")),
        }
    }
    let ticks = in_the_tick.iter().map(|(.., ticks)| *ticks).max();
    check.metric(&format!("ticks_across_{name}"), ticks.unwrap_or(0));
    let jump = in_the_tick.iter().max_by(|a, b| a.0.total_cmp(&b.0));
    check.metric(
        &format!("largest_move_of_still_bodies_in_the_tick_of_{name}"),
        jump.map_or(0.0, |(moved, ..)| *moved),
    );
    if let Some((moved, body, _)) = jump {
        check.that(*moved <= JUMP, || {
            format!("{body:?} moved {moved} units in the tick {change}")
        });
    }
    let mut after: Vec<f64> = still
        .iter()
        .map(
            |body| match (pose_of(earlier, *body), pose_of(later, *body)) {
                (Some(first), Some(last)) => (last.position - first.position).length(),
                _ => f64::INFINITY,
            },
        )
        .collect();
    after.sort_by(f64::total_cmp);
    let median = after.get(after.len() / 2).copied().unwrap_or(f64::INFINITY);
    check.metric(&format!("median_move_of_still_bodies_after_{name}"), median);
    check.metric(
        &format!("largest_move_of_still_bodies_after_{name}"),
        after.last().copied().unwrap_or(f64::INFINITY),
    );
    check.that(median <= DRIFT, || {
        format!("half the still bodies moved more than {DRIFT} units after {change}: {median}")
    });
}

/// Part 2: the window's origin moves across negative coordinates under the resting pile, then
/// the seam leaves (its write held, deliveries reversed) and comes back before the write lands.
fn negative_rebase(story: &mut Story, probes: Probes, check: &mut Check) -> Result<Probes, String> {
    let rest = pile_at_rest(story, check)?;
    let still_bodies = rest.still();
    check.metric("still_bodies", still_bodies.len());
    check.that(still_bodies.len() * 4 >= rest.poses.len(), || {
        format!(
            "{} of {} bodies stayed still at rest, too few to judge the window's moves by",
            still_bodies.len(),
            rest.poses.len()
        )
    });
    for (body, stray) in &rest.strays {
        if still_bodies.contains(body) {
            continue;
        }
        if let Some(sample) = story.sample(*body) {
            check.note(format!(
                "{body:?} still moving at rest: strayed {stray:.2} units over 30 frames, speed \
                 {:.2}, grounded {}, submerged {:.2}, touching {}",
                sample.speed(),
                sample.grounded,
                sample.submerged_fraction,
                sample.touching,
            ));
        }
    }
    // The pile reshaped the bank; the probes are taken again where no body rests, from the
    // earlier steps' columns and from all four seam chunks and the negative ones.
    let previous = story.columns(&probes.cells)?;
    let reshaped = previous
        .iter()
        .zip(&probes.columns)
        .filter(|(now, before)| now != before)
        .count();
    check.metric("probes_the_pile_changed", reshaped);
    let clear = |cell: &IVec2| {
        rest.poses.iter().all(|pose| {
            (DVec2::new(pose.position.x, -pose.position.z) - cell.as_dvec2()).length() >= 10.0
        })
    };
    let seam_probes: Vec<IVec2> = [
        IVec2::new(4, -60),
        IVec2::new(124, -60),
        IVec2::new(4, 60),
        IVec2::new(124, 60),
        IVec2::new(74, -6),
        IVec2::new(50, 6),
        SEAM,
        IVec2::new(-60, -60),
        IVec2::new(-1, -100),
    ]
    .into_iter()
    .filter(clear)
    .collect();
    check.metric("seam_probes", seam_probes.len());
    check.that(seam_probes.len() >= 4, || {
        format!("the pile covers the seam probes: {seam_probes:?}")
    });
    let cells: Vec<IVec2> = probes
        .cells
        .iter()
        .copied()
        .filter(clear)
        .chain(seam_probes)
        .collect();
    let columns = story.columns(&cells)?;
    let probes = Probes { cells, columns };
    let summary_before = *story.session().residency().summary();
    let seam_chunks = [
        ChunkKey::new(0, -1, 0),
        ChunkKey::new(1, -1, 0),
        ChunkKey::new(0, 0, 0),
        ChunkKey::new(1, 0, 0),
    ];

    story.faults.arm(
        Barrier::BeforeChunkCommit,
        Action::Hold {
            limit: Duration::from_secs(120),
        },
        1,
    );
    story.live.faults.reverse_deliveries(true);
    let mut frames = 0;
    // The origin moves from (−2, −2) to (−3, −3); the seam stays resident and every body keeps
    // its pace.
    let before = poses(story);
    let shifted =
        ResidentWindow::new(ChunkKey::new(-1, -1, 0), 2, 2).map_err(|e| format!("{e:?}"))?;
    let (moved, across) = move_across(story, shifted, |pose, first| pose.tick >= first)?;
    frames += moved;
    story.live.frames(10);
    story.settle()?;
    let shifted_poses = poses(story);
    // A sphere that left the pile may rest where the move takes its terrain away, and is
    // rightly suspended there; every body the shifted window holds keeps its pace.
    check.metric(
        "bodies_the_rebase_left_at_the_edge",
        shifted_poses
            .iter()
            .filter(|pose| !well_inside(shifted, pose.position))
            .count(),
    );
    let suspended: Vec<_> = shifted_poses
        .iter()
        .filter(|pose| pose.suspended && well_inside(shifted, pose.position))
        .map(|pose| (pose.body, pose.position))
        .collect();
    check.that(suspended.is_empty(), || {
        format!("a move that kept their terrain suspended {suspended:?}")
    });
    pile_unmoved(
        check,
        "the origin moved",
        "the_rebase",
        &still_bodies,
        &across,
        &before,
        &shifted_poses,
    );

    // Negative keys only: the seam leaves and is captured; every body is suspended and frozen.
    let away = ResidentWindow::new(ChunkKey::new(-4, -4, 0), 2, 2).map_err(|e| format!("{e:?}"))?;
    frames += story.live.move_to(away)?;
    let faults = story.faults.clone();
    story.until("an outgoing write held", |_| {
        faults.holding() == Some(Barrier::BeforeChunkCommit)
    })?;
    check.that(
        seam_chunks
            .iter()
            .all(|key| !story.session().residency().is_resident(*key)),
        || "a seam chunk is still resident".to_owned(),
    );
    check.that(
        seam_chunks.iter().any(|key| {
            story
                .session()
                .residency()
                .newest(*key)
                .is_some_and(|(_, durable)| !durable)
        }),
        || "no seam chunk waits on its held write".to_owned(),
    );
    story.live.frames(10);
    story.settle()?;
    let frozen = poses(story);
    story.live.frames(10);
    story.settle()?;
    let still_poses = poses(story);
    let report = story.session().runtime().ledger().summary().bodies;
    check.equal("bodies suspended while away", report.suspended, report.live);
    check.equal("bodies while away", report.live, 64);
    check.that(
        frozen.iter().all(|pose| {
            pose.suspended
                && pose_of(&still_poses, pose.body).is_some_and(|later| {
                    later.suspended && later.tick > pose.tick && later.position == pose.position
                })
        }),
        || "a suspended body moved or was not sampled while away".to_owned(),
    );

    // Home while the write is still held: the captured state returns, the pile resumes.
    let (moved, across) = move_across(story, home(), |pose, _| !pose.suspended)?;
    frames += moved;
    check.that(
        story.faults.holding() == Some(Barrier::BeforeChunkCommit),
        || "the write finished before the window came back".to_owned(),
    );
    let returned = story.columns(&probes.cells)?;
    compare(check, "after the rebase", &probes, &returned);
    let summary = *story.session().residency().summary();
    check.that(
        summary.loads_from_retained > summary_before.loads_from_retained,
        || "no seam chunk was taken from its retained payload".to_owned(),
    );
    story.live.frames(10);
    story.settle()?;
    let resumed = poses(story);
    check.that(resumed.iter().all(|pose| !pose.suspended), || {
        "a body stayed suspended at home".to_owned()
    });
    pile_unmoved(
        check,
        "the pile resumed",
        "the_return",
        &still_bodies,
        &across,
        &still_poses,
        &resumed,
    );
    story.faults.release();
    story.live.faults.reverse_deliveries(false);
    story.until("the held writes", |session| !session.residency().busy())?;
    check.that(
        seam_chunks.iter().all(|key| {
            story
                .session()
                .residency()
                .newest(*key)
                .is_some_and(|(_, durable)| durable)
        }),
        || "a seam chunk's write did not become durable".to_owned(),
    );
    check.metric("rebase_frames", frames);
    check.metric(
        "rebase_loads_from_retained",
        summary.loads_from_retained - summary_before.loads_from_retained,
    );
    check.metric(
        "rebase_stale_loads",
        summary.stale_loads - summary_before.stale_loads,
    );
    Ok(probes)
}
