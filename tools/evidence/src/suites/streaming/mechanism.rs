//! The snapshot mechanism on the device: which writers mark a chunk changed, what a chunk and a
//! world snapshot hold, and a world snapshot's bodies restored into a new session.

use std::collections::BTreeMap;

use world_model::{
    body::{BodyHandle, BodyPreset},
    cell::WorldCell,
    command::{Command, Direction, Drive, EditTool, MaterialEdit},
    glam::IVec2,
    material::{FluidMat, GranularMat},
    resume::{BodySet, SlotState},
    rules::SimulationRules,
    spatial::{CHUNK_CELL_COUNT, ChunkKey},
};
use world_runtime::{
    ids::{SessionEpoch, Tick},
    port::{CommandResult, Outcome, PhysicalWorld, SlotRef},
};

use super::super::{
    Check,
    bodies::scene::{Scene, Setup, chunk_cells, dirt},
    driver::{Driver, config, flat},
};

fn edit(tool: EditTool, cell: IVec2, radius: i32) -> Command {
    Command::EditMaterial(MaterialEdit::new(tool, cell, radius, 4).expect("valid stroke"))
}

/// What a world snapshot held: its tick, each chunk's cells when it changed, and the bodies.
pub type WorldTaken = (
    Option<Tick>,
    BTreeMap<(i32, i32), Option<Vec<WorldCell>>>,
    BodySet,
);

/// Takes a world snapshot through a transfer submission and collects every chunk's report
/// (with a changed chunk's cells) and the bodies.
pub fn world_snapshot(driver: &mut Driver) -> Result<WorldTaken, String> {
    let session = driver.session();
    let ticket = driver
        .world
        .snapshot_world(session)
        .map_err(|refusal| format!("world snapshot: {refusal:?}"))?;
    driver.outcomes.clear();
    let mut chunks = BTreeMap::new();
    let mut bodies = None;
    let mut expected = None;
    let mut tick = None;
    let started = std::time::Instant::now();
    loop {
        let _ = driver.world.submit_transfers(session);
        driver.poll();
        for outcome in std::mem::take(&mut driver.outcomes) {
            match outcome {
                Outcome::Snapshot {
                    ticket: chunk,
                    key,
                    changed,
                    tick: at,
                    ..
                } if chunk.world() == Some(ticket) => {
                    tick = at;
                    let cells = if changed {
                        let mut cells = vec![WorldCell::default(); CHUNK_CELL_COUNT];
                        driver
                            .world
                            .take_snapshot(chunk, &mut cells)
                            .map_err(|refusal| format!("take {key:?}: {refusal:?}"))?;
                        Some(cells)
                    } else {
                        None
                    };
                    chunks.insert((key.key.x, key.key.y), cells);
                }
                Outcome::BodySnapshot {
                    ticket: reported,
                    chunks: count,
                    tick: at,
                    ..
                } if reported == ticket => {
                    let mut set = BodySet::default();
                    driver
                        .world
                        .take_body_snapshot(ticket, &mut set)
                        .map_err(|refusal| format!("take bodies: {refusal:?}"))?;
                    bodies = Some(set);
                    expected = Some(count as usize);
                    tick = at;
                }
                Outcome::Abandoned { obligation, reason } => {
                    return Err(format!("{obligation:?} abandoned: {reason:?}"));
                }
                _ => {}
            }
        }
        if let (Some(count), Some(_)) = (expected, &bodies)
            && chunks.len() == count
        {
            return Ok((tick, chunks, bodies.unwrap_or_default()));
        }
        if started.elapsed() > super::super::driver::DEVICE_PATIENCE {
            return Err(format!(
                "the world snapshot never completed: {} chunks, bodies {}",
                chunks.len(),
                bodies.is_some()
            ));
        }
        std::thread::sleep(std::time::Duration::from_micros(200));
    }
}

/// Edits, material flow and body deformation each mark exactly the chunks they change; a chunk
/// snapshot clears the mark, an activation starts without one, and a world snapshot reports and
/// streams exactly the changed chunks.
pub fn change_flags(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 1))?;
    let window = driver.world.capabilities().window;
    let mut slots: BTreeMap<(i32, i32), SlotRef> = BTreeMap::new();
    let mut uploaded: BTreeMap<(i32, i32), Vec<WorldCell>> = BTreeMap::new();
    for key in window.view().iter() {
        let cells = if key == ChunkKey::new(0, 1, 0) {
            // Loose ground for the body to crater.
            chunk_cells(key, &|_, column: &mut WorldCell| {
                *column = dirt();
                column.set_elevation(20);
                column.set_granular_mat(GranularMat::GRANULAR_SAND);
                column.set_granular_vol(8);
            })
        } else {
            flat(20)
        };
        slots.insert((key.key.x, key.key.y), driver.upload(key, &cells)?);
        uploaded.insert((key.key.x, key.key.y), cells);
    }
    let slot = |slots: &BTreeMap<(i32, i32), SlotRef>, x, y| slots[&(x, y)];

    // Freshly activated chunks are unchanged.
    let fresh = driver.snapshot(slot(&slots, 0, 0))?;
    check.that(!fresh.changed && fresh.cells.is_none(), || {
        format!("a fresh chunk reported changed: {fresh:?}")
    });

    // An edit inside chunk (0, 0) marks it, and only it.
    driver.run(&[edit(EditTool::Raise, IVec2::new(32, 32), 3)])?;
    let edited = driver.snapshot(slot(&slots, 0, 0))?;
    let neighbour = driver.snapshot(slot(&slots, 1, 0))?;
    check.that(edited.changed, || {
        "the edited chunk reported unchanged".to_owned()
    });
    check.that(!neighbour.changed, || {
        "the untouched neighbour reported changed".to_owned()
    });
    let inspected = driver.capture(slot(&slots, 0, 0))?;
    check.that(edited.cells.as_deref() == Some(&inspected[..]), || {
        "the snapshot's cells differ from an inspection capture".to_owned()
    });
    check.that(inspected != uploaded[&(0, 0)], || {
        "the edit changed nothing".to_owned()
    });
    let again = driver.snapshot(slot(&slots, 0, 0))?;
    check.that(!again.changed, || {
        "a second snapshot without a change reported changed: the first did not clear it".to_owned()
    });
    check.equal(
        "snapshot tick",
        again.tick,
        driver.next_tick.value().checked_sub(1).map(Tick::new),
    );

    // A stroke across the seam marks both chunks; a world snapshot reports exactly those as
    // changed and streams their cells.
    driver.run(&[edit(EditTool::Lower, IVec2::new(63, 40), 4)])?;
    let (_, chunks, bodies) = world_snapshot(&mut driver)?;
    let changed: Vec<(i32, i32)> = chunks
        .iter()
        .filter(|(_, cells)| cells.is_some())
        .map(|(key, _)| *key)
        .collect();
    check.equal("changed by the seam stroke", changed, vec![(0, 0), (1, 0)]);
    check.equal("chunks in the world snapshot", chunks.len(), 9);
    check.equal("bodies in the world snapshot", bodies.live(), 0);
    for key in [(0, 0), (1, 0)] {
        let inspected = driver.capture(slot(&slots, key.0, key.1))?;
        check.that(chunks[&key].as_deref() == Some(&inspected[..]), || {
            format!("world snapshot cells of {key:?} differ from an inspection capture")
        });
    }

    // Material flow alone (no edit after the snapshot) marks the chunk it changes.
    driver.run(&[edit(EditTool::Water, IVec2::new(-32, 20), 2)])?;
    world_snapshot(&mut driver)?;
    driver.run(&[Command::SetSimulationRules(SimulationRules {
        liquid: true,
        ..SimulationRules::NONE
    })])?;
    for _ in 0..8 {
        driver.run(&[])?;
    }
    driver.run(&[Command::SetSimulationRules(SimulationRules::NONE)])?;
    let (_, chunks, _) = world_snapshot(&mut driver)?;
    let flowed: Vec<(i32, i32)> = chunks
        .iter()
        .filter(|(_, cells)| cells.is_some())
        .map(|(key, _)| *key)
        .collect();
    check.equal("changed by flowing water", flowed, vec![(-1, 0)]);
    let water: u32 = chunks[&(-1, 0)].as_ref().map_or(0, |cells| {
        cells
            .iter()
            .filter(|cell| cell.fluid_mat() == FluidMat::FLUID_WATER)
            .count() as u32
    });
    check.that(water > 13, || {
        format!("the water did not spread beyond its stroke: {water} cells")
    });

    // A falling body craters loose ground: the body passes alone mark its chunk.
    let spawn = world_model::command::BodySpawn::new(BodyPreset::Metal, IVec2::new(32, 96), 30.0)
        .map_err(|error| error.to_string())?;
    let results = driver.run(&[Command::SpawnBody(spawn)])?;
    check.that(
        matches!(results[0], CommandResult::BodySpawned { .. }),
        || format!("spawn: {:?}", results[0]),
    );
    world_snapshot(&mut driver)?;
    for _ in 0..90 {
        driver.run(&[])?;
    }
    let (_, chunks, bodies) = world_snapshot(&mut driver)?;
    let deformed: Vec<(i32, i32)> = chunks
        .iter()
        .filter(|(_, cells)| cells.is_some())
        .map(|(key, _)| *key)
        .collect();
    check.equal("changed by the body", deformed, vec![(0, 1)]);
    check.equal("bodies after the spawn", bodies.live(), 1);

    // A snapshot through a transfer submission holds the last submitted tick.
    let last = Tick::new(driver.next_tick.value() - 1);
    let ticket = driver
        .world
        .snapshot_chunk(slot(&slots, -1, -1))
        .map_err(|refusal| format!("snapshot: {refusal:?}"))?;
    let submitted = driver
        .world
        .submit_transfers(driver.session())
        .map_err(|refusal| format!("transfers: {refusal:?}"))?;
    check.that(submitted, || {
        "the transfer submission carried nothing".to_owned()
    });
    let between = driver.await_snapshot(ticket)?;
    check.equal("transfer snapshot tick", between.tick, Some(last));
    check.equal(
        "the transfer submission ran no tick",
        driver.world.next_tick(),
        Tick::new(last.value() + 1),
    );

    // A changed chunk retired without a snapshot, then uploaded again: activation clears the
    // mark, so the new occupancy starts unchanged.
    driver.run(&[edit(EditTool::Raise, IVec2::new(80, 80), 2)])?;
    let key = ChunkKey::new(1, 1, 0);
    driver
        .world
        .retire_slot(slot(&slots, 1, 1))
        .map_err(|refusal| format!("retire: {refusal:?}"))?;
    driver.run(&[])?;
    let reused = driver.upload(key, &uploaded[&(1, 1)])?;
    let after = driver.snapshot(reused)?;
    check.that(!after.changed, || {
        "a reactivated slot inherited its previous occupancy's change".to_owned()
    });
    let counters = driver.world.counters();
    check.metric("snapshot_copy_bytes", counters.snapshot_copy_bytes);
    check.metric("snapshot_readback_bytes", counters.readback.snapshots);
    check.metric("transfer_submissions", counters.transfer_submissions);
    Ok(())
}

/// A world snapshot's bodies, restored with its chunks into a new session that starts at the
/// next tick, continue exactly as the original world does: same states bit for bit, the same
/// live handles, the removed one still stale and no body twice.
pub fn restore_bodies(adapter: &str, check: &mut Check) -> Result<(), String> {
    let setup = Setup::default();
    let ground = |cell: IVec2, column: &mut WorldCell| {
        column.set_elevation(20 + (cell.x.rem_euclid(17) == 0) as u16);
        column.set_granular_mat(GranularMat::GRANULAR_SAND);
        column.set_granular_vol(4);
    };
    let mut original = Scene::open(adapter, setup, ground)?;
    let a = original.spawn(BodyPreset::Metal.description(), 20.0, -20.0, 12.0)?;
    let b = original.spawn(BodyPreset::Wood.description(), 40.0, -30.0, 6.0)?;
    let c = original.spawn(BodyPreset::Metal.description(), 30.0, -44.0, 20.0)?;
    original.run(40)?;
    let removed = match original.step(&[Command::RemoveBody { body: b }])?[0] {
        CommandResult::BodyRemoved { body, .. } => body,
        other => return Err(format!("remove: {other:?}")),
    };
    original.bodies.retain(|body| *body != removed);
    let d = original.spawn(BodyPreset::Wood.description(), 44.0, -12.0, 4.0)?;
    check.equal("the removed body's slot is reused", d.index(), b.index());
    check.that(d.generation() > b.generation(), || {
        "the reused slot kept its generation".to_owned()
    });
    original.run(20)?;
    original.driver.settle()?;

    let (tick, chunks, bodies) = world_snapshot(&mut original.driver)?;
    check.equal(
        "snapshot tick",
        tick,
        Some(Tick::new(original.driver.next_tick.value() - 1)),
    );
    check.equal("live bodies", bodies.live(), 3);
    check.equal(
        "slot states",
        bodies
            .slots
            .iter()
            .map(|slot| slot.state)
            .collect::<Vec<_>>(),
        vec![SlotState::Live; 3],
    );
    check.equal(
        "the reused slot's generation",
        bodies.slots[1].generation,
        d.generation(),
    );

    // The restored session: the next tick, a later epoch, every chunk staged before any tick.
    let mut config = setup.config(adapter);
    config.epoch = SessionEpoch::new(2);
    config.first_tick = original.driver.next_tick;
    let mut restored = Driver::open(config)?;
    restored.next_tick = original.driver.next_tick;
    let session = restored.session();
    restored
        .world
        .restore_bodies(session, &bodies)
        .map_err(|refusal| format!("restore: {refusal:?}"))?;
    for (key, slot) in &original.slots {
        let cells = match &chunks[&(key.key.x, key.key.y)] {
            Some(cells) => cells.clone(),
            None => chunk_cells(*key, &ground),
        };
        restored.stage_upload(*key, &cells)?;
        let _ = slot;
    }
    check.that(
        restored.world.restore_bodies(session, &bodies).is_err(),
        || "a second restore was accepted".to_owned(),
    );

    // Both worlds run the same ticks with the same commands; the bodies must agree exactly.
    let live = [a, c, d];
    let east = Drive::controls(Direction::new(1.0, 0.0).map_err(|e| e.to_string())?);
    let mut mismatches = 0;
    for step in 0..60 {
        let commands = if step % 10 == 0 {
            vec![Command::DriveBody {
                body: a,
                drive: east,
            }]
        } else {
            Vec::new()
        };
        original.driver.observe = live.to_vec();
        restored.observe = live.to_vec();
        original.driver.outcomes.clear();
        restored.outcomes.clear();
        original.driver.run(&commands)?;
        restored.run(&commands)?;
        let samples = |driver: &Driver| -> Vec<_> {
            driver
                .outcomes
                .iter()
                .filter_map(|outcome| match outcome {
                    Outcome::Body(sample) => Some(*sample),
                    _ => None,
                })
                .collect()
        };
        let (left, right) = (samples(&original.driver), samples(&restored));
        if left.len() != 3 || left != right {
            mismatches += 1;
            if mismatches == 1 {
                check.fail(format!(
                    "tick {step}: original {left:?}, restored {right:?}"
                ));
            }
        }
    }
    check.equal("ticks whose samples differ", mismatches, 0);
    match restored.run(&[Command::DriveBody {
        body: b,
        drive: east,
    }])?[0]
    {
        CommandResult::StaleBody { body } => check.equal("stale handle", body, b),
        other => check.fail(format!("the removed handle was accepted: {other:?}")),
    }
    check.equal("live bodies after restore", restored.world.live_bodies(), 3);
    let spawn = world_model::command::BodySpawn::new(BodyPreset::Wood, IVec2::new(10, 10), 4.0)
        .map_err(|e| e.to_string())?;
    match restored.run(&[Command::SpawnBody(spawn)])?[0] {
        CommandResult::BodySpawned { body, .. } => {
            check.equal(
                "a new spawn takes the next free slot",
                body,
                BodyHandle::new(3, 0),
            );
        }
        other => check.fail(format!("spawn after restore: {other:?}")),
    }
    Ok(())
}
