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
    body::{BodyHandle, BodyPreset},
    command::{
        AmbientTemperature, BodySpawn, Command, Direction, Drive, EditTool, MaterialEdit, Target,
    },
    glam::{DVec2, DVec3, IVec2},
    material::FluidMat,
    observation::{BodyCounters, WorldTotals},
    rules::SimulationRules,
    spatial::ChunkKey,
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
/// The most a still body may move across a window move or a suspension, in world units: the
/// pile's restless spheres still nudge their neighbours by about a tenth of a unit, and a wrong
/// origin or resume displaces a body by whole cells (a unit or more).
const JUMP: f64 = 0.5;

/// Runs frames until at least three quarters of the bodies moved no more than [`STILL`] over
/// 30 frames; returns the poses at both ends of that span. Some spheres in a crushed pile never
/// settle (they rock against their neighbours); they are reported, not waited for.
fn pile_at_rest(story: &mut Story, check: &mut Check) -> Result<(Vec<Pose>, Vec<Pose>), String> {
    let mut earlier = poses(story);
    let mut still_counts = Vec::new();
    for round in 1..=60 {
        for _ in 0..30 {
            story.frame();
        }
        story.settle()?;
        let now = poses(story);
        let still = still(&earlier, &now).len();
        if still * 4 >= now.len() * 3 {
            check.metric("frames_to_rest", round * 30);
            return Ok((earlier, now));
        }
        still_counts.push(still);
        earlier = now;
    }
    Err(format!(
        "the pile never came to rest; still bodies after each 30 frames: {still_counts:?}"
    ))
}

/// The bodies that moved no more than [`STILL`] from `earlier` to `later`.
fn still(earlier: &[Pose], later: &[Pose]) -> Vec<BodyHandle> {
    earlier
        .iter()
        .filter(|pose| {
            pose_of(later, pose.body)
                .is_some_and(|next| (next.position - pose.position).length() <= STILL)
        })
        .map(|pose| pose.body)
        .collect()
}

/// The farthest any of `bodies` moved from `earlier` to `later`.
fn largest_move_of(bodies: &[BodyHandle], earlier: &[Pose], later: &[Pose]) -> f64 {
    bodies
        .iter()
        .map(
            |body| match (pose_of(earlier, *body), pose_of(later, *body)) {
                (Some(first), Some(last)) => (last.position - first.position).length(),
                _ => f64::INFINITY,
            },
        )
        .fold(0.0, f64::max)
}

/// Part 2: the window's origin moves across negative coordinates under the resting pile, then
/// the seam leaves (its write held, deliveries reversed) and comes back before the write lands.
fn negative_rebase(story: &mut Story, probes: Probes, check: &mut Check) -> Result<Probes, String> {
    let (earlier, resting) = pile_at_rest(story, check)?;
    let still_bodies = still(&earlier, &resting);
    check.metric("still_bodies", still_bodies.len());
    for pose in &resting {
        if still_bodies.contains(&pose.body) {
            continue;
        }
        if let Some(sample) = story.sample(pose.body) {
            check.note(format!(
                "{:?} still moving at rest: {:.2} units over 30 frames, speed {:.2}, grounded {}, \
                 submerged {:.2}, touching {}",
                pose.body,
                pose_of(&earlier, pose.body)
                    .map_or(f64::INFINITY, |first| (pose.position - first.position)
                        .length()),
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
        resting.iter().all(|pose| {
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
    frames += story.live.move_to(shifted)?;
    story.live.frames(10);
    story.settle()?;
    let shifted_poses = poses(story);
    check.that(shifted_poses.iter().all(|pose| !pose.suspended), || {
        "a body was suspended by a move that kept its terrain".to_owned()
    });
    let jump = largest_move_of(&still_bodies, &before, &shifted_poses);
    check.metric("largest_move_of_still_bodies_across_the_rebase", jump);
    check.metric(
        "largest_move_across_the_rebase",
        largest_move_of(
            &before.iter().map(|pose| pose.body).collect::<Vec<_>>(),
            &before,
            &shifted_poses,
        ),
    );
    check.that(jump <= JUMP, || {
        format!("a still body moved {jump} units when the origin moved")
    });

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
    frames += story.live.move_to(home())?;
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
    let moved = largest_move_of(&still_bodies, &still_poses, &resumed);
    check.metric("largest_move_of_still_bodies_after_return", moved);
    check.that(moved <= JUMP, || {
        format!("a still body resumed {moved} units from where it was frozen")
    });
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
