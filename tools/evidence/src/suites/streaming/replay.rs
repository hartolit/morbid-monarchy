//! Whether a world can be rebuilt by replaying its commands from a checkpoint: the evidence a
//! command journal would rest on.
//!
//! One saved world runs four times with the same tick-scheduled commands (spheres dropped into
//! a pile on loose ground, drives, jumps, attraction and lift; water, lava, oil, fire, sand,
//! ice, raise and lower strokes; a window move away and back) and every rule on:
//!
//! ```text
//! live      tick 0 .. K without stopping; a checkpoint at C on the way
//! woken     the same, with every resident cell woken in tick C + 1
//! reopened  the checkpoint at C reopened (a later epoch), the commands of C + 1 .. K replayed
//! again     the same reopening and replay a second time
//! ```
//!
//! Residency is held to the tick: a run admits no tick while the chunks it is moving in are
//! still loading, so every run activates the same chunks before the same ticks. Each run saves
//! its world at K; the case compares those checkpoints cell for cell and body for body, and the
//! accounting the ledger added up over C + 1 .. K.
//!
//! Two replays agree exactly, and so do the reopened and the woken run: the device reproduces
//! a world from the same inputs, and a reopened checkpoint differs from the world that ran on
//! only in which cells are awake (a reopened chunk wakes all of its cells; the checkpoint does
//! not record the activity scheduling). How far the live run drifts from the replay is
//! recorded, not asserted: it is the error a command journal replayed from today's checkpoints
//! would make (ADR 0004).

use std::{collections::BTreeMap, path::Path, time::Duration};

use world_model::{
    body::{BodyHandle, BodyPreset},
    cell::WorldCell,
    command::{
        AmbientTemperature, BodySpawn, Command, Direction, Drive, EditTool, MaterialEdit, Target,
    },
    generation::WorldGenerator,
    glam::IVec2,
    resume::{BodyResume, BodySet},
    rules::SimulationRules,
    spatial::{CHUNK_SIZE, ChunkKey},
};
use world_runtime::{
    ids::{CommandId, SessionEpoch, Tick},
    port::{BodyReport, CellReport, CommandResult},
    receipts::ReceiptState,
    store::{ChunkPayload, WorldStore},
};
use world_store::faults::StoreFaults;

use super::{
    super::Check,
    world::{self, Live, Scratch, open, open_store, session_config, window},
};

/// The checkpoint the reopened runs start from, the ticks of the window's moves, and the end.
const CHECKPOINT: u64 = 240;
const MOVE_AWAY: u64 = 330;
const MOVE_BACK: u64 = 420;
const END: u64 = 600;
const RADIUS: u32 = 2;

/// One scheduled command: the tick it applies in and the command.
type Scheduled = (u64, Command);

fn stroke(tool: EditTool, x: i32, y: i32, radius: i32, strength: u8) -> Command {
    Command::EditMaterial(
        MaterialEdit::new(tool, IVec2::new(x, y), radius, strength).expect("a supported stroke"),
    )
}

fn drop_sphere(preset: BodyPreset, x: i32, y: i32, height: f32) -> Command {
    Command::SpawnBody(BodySpawn::new(preset, IVec2::new(x, y), height).expect("supported spawn"))
}

/// The handles the sixteen spawns are given in a new world: the lowest free slots, generation 0.
fn spheres() -> Vec<BodyHandle> {
    (0..16).map(|slot| BodyHandle::new(slot, 0)).collect()
}

/// The script every run follows, in tick order (commands apply in admission order, so a
/// command admitted behind one that is not yet due would wait for it). `wake` adds a change of
/// temperature and back in tick C + 1: that tick runs at the same temperature with every
/// resident cell awake.
fn script(wake: bool) -> Vec<Scheduled> {
    let bodies = spheres();
    let mut script = vec![
        // Loose ground for craters and ruts, and a pool beside it.
        (2, stroke(EditTool::Sand, 30, 20, 14, 6)),
        (3, stroke(EditTool::Water, -20, 30, 6, 40)),
        (4, stroke(EditTool::Oil, 70, -30, 5, 10)),
    ];
    // Spheres dropped onto one another in a pile on the sand.
    for index in 0..16i32 {
        let preset = if index % 3 == 0 {
            BodyPreset::Wood
        } else {
            BodyPreset::Metal
        };
        let tick = 10 + 2 * index as u64;
        script.push((
            tick,
            drop_sphere(
                preset,
                24 + (index % 4) * 4,
                14 + (index / 4) * 4,
                12.0 + 6.0 * index as f32,
            ),
        ));
    }
    script.extend([
        (60, stroke(EditTool::Lava, -8, 30, 3, 30)),
        (80, stroke(EditTool::Fire, 70, -30, 4, 1)),
        (120, stroke(EditTool::Water, -100, 10, 5, 60)),
        (150, stroke(EditTool::Ice, 0, -40, 4, 8)),
        (200, stroke(EditTool::Raise, 50, 50, 6, 20)),
        (250, stroke(EditTool::Lower, 20, -20, 5, 6)),
        (260, stroke(EditTool::Water, -20, 30, 6, 40)),
        (300, stroke(EditTool::Lava, -8, 30, 3, 30)),
        (310, stroke(EditTool::Oil, 70, -30, 5, 10)),
        (320, stroke(EditTool::Fire, 70, -30, 4, 1)),
        (
            360,
            Command::AttractBodies {
                target: Target::Column(IVec2::new(40, 40)),
            },
        ),
        (
            440,
            Command::LiftBodies {
                target: Target::Column(IVec2::new(36, 24)),
            },
        ),
        (450, stroke(EditTool::Sand, 36, 24, 6, 8)),
        (500, stroke(EditTool::Water, 10, -10, 4, 50)),
    ]);
    let driven = bodies[0];
    let east = Drive::controls(Direction::new(1.0, 0.3).expect("a direction"));
    for tick in (100..END).step_by(15) {
        script.push((
            tick,
            Command::DriveBody {
                body: driven,
                drive: east,
            },
        ));
    }
    for tick in [140, 280, 470] {
        script.push((tick, Command::JumpBody { body: driven }));
    }
    if wake {
        let ambient = AmbientTemperature::celsius(20.0).expect("supported");
        let other = AmbientTemperature::celsius(21.0).expect("supported");
        script.push((CHECKPOINT + 1, Command::SetAmbientTemperature(other)));
        script.push((CHECKPOINT + 1, Command::SetAmbientTemperature(ambient)));
    }
    // A stable sort keeps the order of commands within a tick.
    script.sort_by_key(|(tick, _)| *tick);
    script
}

/// Runs frames without ticks until `ready` holds: residency held so the next tick sees it.
fn hold(live: &mut Live, what: &str, ready: impl Fn(&Live) -> bool) -> Result<(), String> {
    let started = std::time::Instant::now();
    while !ready(live) {
        live.frame_ticks(0);
        if started.elapsed() > Duration::from_secs(30) {
            let target = live.session.residency().target();
            return Err(format!(
                "{what}: not ready at {:?}: {:?}; resize to {target:?}: {:?}; counters {:?}",
                last_tick(live),
                live.session.residency().summary(),
                live.session.runtime().preflight_resize(target),
                live.session.runtime().counters().pressure_refusals,
            ));
        }
        std::thread::sleep(Duration::from_micros(200));
    }
    Ok(())
}

/// Every chunk of the target window resident or staged for the next tick's activation, and
/// nothing loading or leaving.
fn staged(live: &Live) -> bool {
    let residency = live.session.residency();
    let summary = residency.summary();
    residency.window() == residency.target()
        && summary.loading == 0
        && summary.ready == 0
        && summary.evicting == 0
        && summary.resident + summary.uploading == residency.target().chunk_count()
}

fn last_tick(live: &Live) -> Option<u64> {
    live.session
        .runtime()
        .last_submitted_tick()
        .map(Tick::value)
}

/// Runs one tick per frame until `tick` was submitted, holding the window's moves.
fn run_to(live: &mut Live, tick: u64) -> Result<(), String> {
    let started = std::time::Instant::now();
    while last_tick(live).is_none_or(|last| last < tick) {
        let next = last_tick(live).map_or(0, |last| last + 1);
        let move_to = match next {
            MOVE_AWAY => Some(window(1, 0, RADIUS)),
            MOVE_BACK => Some(window(0, 0, RADIUS)),
            _ => None,
        };
        if let Some(target) = move_to
            && live.session.residency().target() != target
        {
            // The outgoing chunks are snapshotted and retired before this tick, which applies
            // the retirement; the window moves after it and the entering chunks are staged
            // before the next.
            live.session.set_window(target);
            hold(live, "eviction", |live| {
                live.session.residency().summary().evicting > 0
            })?;
            while last_tick(live).is_none_or(|last| last < next) {
                live.frame_ticks(1);
                std::thread::sleep(Duration::from_micros(100));
            }
            hold(live, "move", staged)?;
            continue;
        }
        live.frame_ticks(1);
        if started.elapsed() > Duration::from_secs(120) {
            return Err(format!(
                "stuck before tick {tick}: at {:?}",
                last_tick(live)
            ));
        }
    }
    Ok(())
}

/// Saves the last submitted tick and waits for the checkpoint, ticking nothing meanwhile.
fn save_here(live: &mut Live, tick: u64) -> Result<(), String> {
    live.session.save();
    hold(live, "save", |live| {
        live.session
            .checkpoints()
            .receipt()
            .is_some_and(|receipt| receipt.finished())
    })?;
    let latest = live.session.checkpoints().latest();
    if latest.tick != Some(Tick::new(tick)) {
        return Err(format!(
            "the save holds {:?}, not tick {tick}: {:?}",
            latest.tick,
            live.session.checkpoints().receipt()
        ));
    }
    Ok(())
}

/// Admits every scheduled command from `from` on, in tick order.
fn admit(live: &mut Live, script: &[Scheduled], from: u64) -> Result<(), String> {
    for (index, (tick, command)) in script.iter().enumerate() {
        if *tick < from {
            continue;
        }
        live.session
            .runtime_mut()
            .submit_at(CommandId::new(index as u64 + 1), *command, Tick::new(*tick))
            .map_err(|error| format!("admit {command:?} at {tick}: {error:?}"))?;
    }
    Ok(())
}

/// The accounting a run's ledger added up so far.
fn accounting(live: &Live) -> (CellReport, BodyReport) {
    let summary = live.session.runtime().ledger().summary();
    (summary.cells, summary.bodies)
}

/// The world a checkpoint holds: every saved chunk around the window, and the bodies.
struct Saved {
    chunks: BTreeMap<(i32, i32), Vec<WorldCell>>,
    bodies: BodySet,
}

fn read_saved(store: &dyn WorldStore) -> Result<Saved, String> {
    let latest = store.latest().map_err(|error| error.to_string())?;
    let generator = WorldGenerator::new(world::SEED);
    let mut chunks = BTreeMap::new();
    let reach = RADIUS as i32 + 1;
    for y in -reach..=reach {
        for x in -reach..=reach + 1 {
            let key = ChunkKey::new(x, y, 0);
            let mut payload = ChunkPayload::zeroed();
            let found = store
                .read_chunk(key, None, &mut payload)
                .map_err(|error| format!("read {key:?}: {error}"))?;
            let cells = match found {
                Some(_) => payload.cells().to_vec(),
                None => generator.generate_chunk(key).cells.to_vec(),
            };
            chunks.insert((x, y), cells);
        }
    }
    let bodies = store
        .read_bodies(latest.id)
        .map_err(|error| error.to_string())?;
    Ok(Saved { chunks, bodies })
}

/// What one run left.
struct Run {
    saved: Saved,
    /// The ledger's accounting over C + 1 .. K.
    cells: CellReport,
    bodies: BodyReport,
}

/// Opens a session of the store at `path`, holds until its window is staged, and returns it.
fn start(adapter: &str, path: &Path, epoch: SessionEpoch) -> Result<Live, String> {
    let store = open_store(path, StoreFaults::new())?;
    let mut live = open(adapter, store, epoch, session_config(), |gpu| {
        gpu.transfer.bodies = 64;
    })?;
    hold(&mut live, "open", staged)?;
    Ok(live)
}

/// The live run from tick 0 to K. `copy_at_checkpoint` receives a copy of the store as it was
/// once the checkpoint at C was published; `wake` wakes every cell in tick C + 1.
fn live_run(
    adapter: &str,
    path: &Path,
    copy_at_checkpoint: Option<&Path>,
    wake: bool,
) -> Result<Run, String> {
    let mut live = start(adapter, path, SessionEpoch::FIRST)?;
    let script = script(wake);
    admit(&mut live, &script, 0)?;
    run_to(&mut live, 99)?;
    let spawned: Vec<BodyHandle> = script
        .iter()
        .enumerate()
        .filter(|(_, (_, command))| matches!(command, Command::SpawnBody(_)))
        .filter_map(|(index, _)| {
            match live
                .session
                .runtime()
                .receipt(CommandId::new(index as u64 + 1))
                .map(|receipt| receipt.state)
            {
                Some(ReceiptState::Completed {
                    result: CommandResult::BodySpawned { body, .. },
                    ..
                }) => Some(body),
                _ => None,
            }
        })
        .collect();
    if spawned != spheres() {
        return Err(format!("the spheres were given {spawned:?}"));
    }
    run_to(&mut live, CHECKPOINT)?;
    save_here(&mut live, CHECKPOINT)?;
    if let Some(copy) = copy_at_checkpoint {
        std::fs::copy(path, copy).map_err(|error| format!("copy the store: {error}"))?;
    }
    let (cells_at, bodies_at) = accounting(&live);
    run_to(&mut live, END)?;
    save_here(&mut live, END)?;
    let (cells_end, bodies_end) = accounting(&live);
    let report = live.session.close(Duration::from_secs(30));
    if !report.saved {
        return Err(format!("the live run's close did not save: {report:?}"));
    }
    let store = open_store(path, StoreFaults::new())?;
    let saved = read_saved(&*store)?;
    Ok(Run {
        saved,
        cells: cell_difference(&cells_end, &cells_at),
        bodies: body_difference(&bodies_end, &bodies_at),
    })
}

/// The checkpoint at C reopened under `epoch` and the commands after it replayed.
fn reopened_run(adapter: &str, path: &Path, epoch: SessionEpoch) -> Result<Run, String> {
    let mut live = start(adapter, path, epoch)?;
    admit(&mut live, &script(false), CHECKPOINT + 1)?;
    run_to(&mut live, END)?;
    save_here(&mut live, END)?;
    let (cells, body_report) = accounting(&live);
    let body_report = body_difference(&body_report, &BodyReport::default());
    let report = live.session.close(Duration::from_secs(30));
    if !report.saved {
        return Err(format!("a reopened run's close did not save: {report:?}"));
    }
    let store = open_store(path, StoreFaults::new())?;
    Ok(Run {
        saved: read_saved(&*store)?,
        cells,
        bodies: body_report,
    })
}

fn cell_difference(end: &CellReport, start: &CellReport) -> CellReport {
    CellReport {
        fluid_in: end.fluid_in - start.fluid_in,
        fluid_out: end.fluid_out - start.fluid_out,
        solid_in: end.solid_in - start.solid_in,
        solid_out: end.solid_out - start.solid_out,
        quench: end.quench - start.quench,
        boil: end.boil - start.boil,
        burn: end.burn - start.burn,
        uptake: end.uptake - start.uptake,
        erosion: end.erosion - start.erosion,
        tool_fluid_in: end.tool_fluid_in - start.tool_fluid_in,
        tool_solid_in: end.tool_solid_in - start.tool_solid_in,
        tool_solid_out: end.tool_solid_out - start.tool_solid_out,
        candidate_tiles: end.candidate_tiles.wrapping_sub(start.candidate_tiles),
        process_tiles: end.process_tiles.wrapping_sub(start.process_tiles),
        active_tiles: end.active_tiles.wrapping_sub(start.active_tiles),
        awake_cells: end.awake_cells.wrapping_sub(start.awake_cells),
        active_cells: end.active_cells.wrapping_sub(start.active_cells),
        changed_cells: end.changed_cells.wrapping_sub(start.changed_cells),
        rejected_transfers: end
            .rejected_transfers
            .wrapping_sub(start.rejected_transfers),
        stale_flow: end.stale_flow.wrapping_sub(start.stale_flow),
        receive_mismatches: end
            .receive_mismatches
            .wrapping_sub(start.receive_mismatches),
    }
}

fn body_difference(end: &BodyReport, start: &BodyReport) -> BodyReport {
    BodyReport {
        substeps: end.substeps.wrapping_sub(start.substeps),
        live: end.live,
        suspended: end.suspended,
        candidate_pairs: end.candidate_pairs - start.candidate_pairs,
        pair_visits: end.pair_visits - start.pair_visits,
        contact_pairs: end.contact_pairs.wrapping_sub(start.contact_pairs),
        swept_contacts: end.swept_contacts.wrapping_sub(start.swept_contacts),
        terrain_contacts: end.terrain_contacts.wrapping_sub(start.terrain_contacts),
        // Largest values over every tick since the session began: a span's own cannot be
        // taken apart from them, so a difference leaves them out.
        residual_overlap: 0.0,
        residual_approach: 0.0,
        speed_limited: end.speed_limited.wrapping_sub(start.speed_limited),
        swept_bodies: end.swept_bodies.wrapping_sub(start.swept_bodies),
        fluid_in: end.fluid_in - start.fluid_in,
        fluid_out: end.fluid_out - start.fluid_out,
        solid_in: end.solid_in - start.solid_in,
        solid_out: end.solid_out - start.solid_out,
        compaction: end.compaction - start.compaction,
        ice_broken: end.ice_broken.wrapping_sub(start.ice_broken),
        organics_cleared: end.organics_cleared.wrapping_sub(start.organics_cleared),
        craters: end.craters.wrapping_sub(start.craters),
        ruts: end.ruts.wrapping_sub(start.ruts),
        splashes: end.splashes.wrapping_sub(start.splashes),
        wakes: end.wakes.wrapping_sub(start.wakes),
        claims_lost: end.claims_lost.wrapping_sub(start.claims_lost),
    }
}

/// The integer quantities of the accounting that do not depend on which cells were scheduled.
fn conserved(cells: &CellReport, bodies: &BodyReport) -> [u64; 17] {
    [
        cells.fluid_in,
        cells.fluid_out,
        cells.solid_in,
        cells.solid_out,
        cells.quench,
        cells.boil,
        cells.burn,
        cells.uptake,
        cells.erosion,
        cells.tool_fluid_in,
        cells.tool_solid_in,
        cells.tool_solid_out,
        bodies.fluid_in,
        bodies.fluid_out,
        bodies.solid_in,
        bodies.solid_out,
        bodies.compaction,
    ]
}

/// How two saved worlds differ.
#[derive(Debug, Default)]
struct Difference {
    chunks: u32,
    cells: u32,
    bodies: u32,
    /// The largest distance between a body's two positions, in world units.
    body_distance: f32,
    /// Whether the body sets differ in anything but their bodies' states (slots, handles).
    slots_differ: bool,
}

fn position(body: &BodyResume) -> world_model::glam::Vec3 {
    let size = CHUNK_SIZE as f32;
    world_model::glam::Vec3::new(
        body.anchor.x as f32 * size + body.local.x,
        body.anchor.y as f32 * size + body.local.y,
        body.local.z,
    )
}

fn compare(left: &Saved, right: &Saved) -> Difference {
    let mut difference = Difference::default();
    for (key, cells) in &left.chunks {
        let other = &right.chunks[key];
        let differing = cells.iter().zip(other).filter(|(a, b)| a != b).count() as u32;
        if differing > 0 {
            difference.chunks += 1;
            difference.cells += differing;
        }
    }
    difference.slots_differ = left.bodies.slots != right.bodies.slots
        || left.bodies.bodies.len() != right.bodies.bodies.len();
    for (a, b) in left.bodies.bodies.iter().zip(&right.bodies.bodies) {
        if a != b {
            difference.bodies += 1;
            difference.body_distance = difference
                .body_distance
                .max(position(a).distance(position(b)));
        }
    }
    difference
}

fn record(check: &mut Check, name: &str, difference: &Difference) {
    check.metric(&format!("{name}_chunks_differing"), difference.chunks);
    check.metric(&format!("{name}_cells_differing"), difference.cells);
    check.metric(&format!("{name}_bodies_differing"), difference.bodies);
    check.metric(
        &format!("{name}_body_distance"),
        f64::from(difference.body_distance),
    );
    check.note(format!("{name}: {difference:?}"));
}

/// The four runs and their comparisons.
pub fn replay_reproduction(adapter: &str, check: &mut Check) -> Result<(), String> {
    let scratch = Scratch::new("replay")?;
    let origin = scratch.path("origin.redb");
    world::create(&origin, window(0, 0, RADIUS), SimulationRules::ALL)?;
    let live_path = scratch.path("live.redb");
    let woken_path = scratch.path("woken.redb");
    let at_checkpoint = scratch.path("checkpoint.redb");
    for path in [&live_path, &woken_path] {
        std::fs::copy(&origin, path).map_err(|error| error.to_string())?;
    }
    let live = live_run(adapter, &live_path, Some(&at_checkpoint), false)?;
    let woken = live_run(adapter, &woken_path, None, true)?;
    let mut reopened = Vec::new();
    for (index, epoch) in [SessionEpoch::new(2), SessionEpoch::new(3)]
        .into_iter()
        .enumerate()
    {
        let path = scratch.path(&format!("reopened-{index}.redb"));
        std::fs::copy(&at_checkpoint, &path).map_err(|error| error.to_string())?;
        reopened.push(reopened_run(adapter, &path, epoch)?);
    }

    // Coverage: the span after the checkpoint holds contacts, deformation, stochastic rules
    // and residency changes.
    let span = &reopened[0];
    check.that(span.bodies.contact_pairs > 0, || {
        format!("no body contacts after the checkpoint: {:?}", span.bodies)
    });
    check.that(span.bodies.craters + span.bodies.ruts > 0, || {
        format!("no deformation after the checkpoint: {:?}", span.bodies)
    });
    check.that(span.cells.uptake > 0 && span.cells.burn > 0, || {
        format!(
            "no growth or burning after the checkpoint: {:?}",
            span.cells
        )
    });
    check.that(span.cells.quench + span.cells.boil > 0, || {
        format!(
            "no lava meeting water after the checkpoint: {:?}",
            span.cells
        )
    });

    let twice = compare(&reopened[0].saved, &reopened[1].saved);
    record(check, "reopened_twice", &twice);
    check.that(
        twice.cells == 0 && twice.bodies == 0 && !twice.slots_differ,
        || format!("two replays of the same checkpoint differ: {twice:?}"),
    );
    check.equal(
        "accounting of two replays",
        (reopened[0].cells, reopened[0].bodies),
        (reopened[1].cells, reopened[1].bodies),
    );

    let woken_reopened = compare(&woken.saved, &reopened[0].saved);
    record(check, "woken_vs_reopened", &woken_reopened);
    check.that(
        woken_reopened.cells == 0 && woken_reopened.bodies == 0 && !woken_reopened.slots_differ,
        || format!("the reopened world differs from the woken one: {woken_reopened:?}"),
    );
    check.equal(
        "accounting of the woken and the reopened run",
        (woken.cells, woken.bodies),
        (reopened[0].cells, reopened[0].bodies),
    );

    let drift = compare(&live.saved, &reopened[0].saved);
    record(check, "live_vs_reopened", &drift);
    let (live_conserved, replayed) = (
        conserved(&live.cells, &live.bodies),
        conserved(&reopened[0].cells, &reopened[0].bodies),
    );
    check.metric(
        "live_vs_reopened_accounting_differing",
        live_conserved
            .iter()
            .zip(&replayed)
            .filter(|(a, b)| a != b)
            .count(),
    );
    check.note(format!(
        "accounting over C + 1 .. K, live {live_conserved:?}, reopened {replayed:?}"
    ));
    Ok(())
}
