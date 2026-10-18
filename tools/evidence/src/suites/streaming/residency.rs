//! Residency through the real owners: saves that finish late, loads that answer out of order or
//! after their key left, missing neighbours, and stored rows that cannot be read.

use redb::{Database, ReadableTable, TableDefinition};
use world_model::{
    command::{Command, EditTool, MaterialEdit},
    generation::WorldGenerator,
    glam::IVec2,
    rules::SimulationRules,
    spatial::{CHUNK_SIZE, ChunkKey},
};
use world_runtime::{
    ids::{ChunkRevision, SessionEpoch},
    port::CommandResult,
    session::SessionConfig,
    store::{Conflict, Publication, StoreError, WorldStore},
    workers::WorkerConfig,
};
use world_store::{
    faults::{Action, Barrier, StoreFaults},
    format,
};

use super::{
    super::Check,
    world::{self, Live, Scratch, open, open_store, session_config, window},
};

const CHUNKS: TableDefinition<(i32, i32, i32, u64), &[u8]> = TableDefinition::new("chunks");

pub fn stroke(live: &mut Live, tool: EditTool, cell: IVec2, radius: i32) -> Result<u32, String> {
    let edit = MaterialEdit::new(tool, cell, radius, 3).map_err(|error| error.to_string())?;
    match live.run(Command::EditMaterial(edit))? {
        CommandResult::Edited { changed, .. } => Ok(changed),
        other => Err(format!("stroke at {cell}: {other:?}")),
    }
}

fn chunk_of(cell: IVec2) -> ChunkKey {
    ChunkKey::new(
        cell.x.div_euclid(CHUNK_SIZE as i32),
        cell.y.div_euclid(CHUNK_SIZE as i32),
        0,
    )
}

/// A chunk leaves while its save is held; it comes back before the save finishes and must be
/// the newest state, not the store's; a newer save of it then finishes after the older one and
/// still wins; a deliberately older publication is refused; and the world reopens with it.
pub fn newest_retained_wins(adapter: &str, check: &mut Check) -> Result<(), String> {
    let scratch = Scratch::new("newest")?;
    let path = scratch.path("world.redb");
    world::create(&path, window(0, 0, 2), SimulationRules::NONE)?;
    let faults = StoreFaults::new();
    let store = open_store(&path, faults.clone())?;
    let mut live = open(
        adapter,
        store.clone(),
        SessionEpoch::FIRST,
        session_config(),
        |_| {},
    )?;
    live.settle()?;
    let cell = IVec2::new(-100, 10);
    let key = chunk_of(cell);
    let generated = live.column(cell)?.ok_or("not resident")?;
    stroke(&mut live, EditTool::Raise, cell, 2)?;
    let first = live.column(cell)?.ok_or("not resident")?;
    check.that(first != generated, || {
        "the stroke changed nothing".to_owned()
    });

    faults.arm(
        Barrier::BeforeChunkCommit,
        Action::Hold {
            limit: std::time::Duration::from_secs(60),
        },
        1,
    );
    live.move_to(window(3, 0, 2))?;
    live.until("the save held", |_| {
        faults.holding() == Some(Barrier::BeforeChunkCommit)
    })?;
    check.equal(
        "the outgoing chunk's newest revision",
        live.session.residency().newest(key),
        Some((ChunkRevision::new(1), false)),
    );
    live.move_to(window(0, 0, 2))?;
    check.equal(
        "returned before its save finished",
        live.column(cell)?,
        Some(first),
    );
    check.that(
        live.session.residency().summary().loads_from_retained >= 1,
        || "the returning chunk was not taken from the retained payload".to_owned(),
    );

    // A second change, saved behind the held first one.
    stroke(&mut live, EditTool::Raise, cell, 2)?;
    let second = live.column(cell)?.ok_or("not resident")?;
    live.move_to(window(3, 0, 2))?;
    live.until("the second snapshot", |session| {
        session.residency().newest(key) == Some((ChunkRevision::new(2), false))
    })?;
    faults.release();
    live.until("both saves", |session| !session.residency().busy())?;
    let mut payload = world_runtime::store::ChunkPayload::zeroed();
    for revision in [1, 2] {
        check.equal(
            &format!("revision {revision} durable"),
            store
                .read_chunk(key, Some(ChunkRevision::new(revision)), &mut payload)
                .map_err(|error| error.to_string())?,
            Some(ChunkRevision::new(revision)),
        );
    }
    let saved = live.save()?;
    check.equal(
        "the published revision",
        store
            .read_chunk(key, None, &mut payload)
            .map_err(|error| error.to_string())?,
        Some(ChunkRevision::new(2)),
    );

    // The stale-save specimen: the older revision, published late, is refused.
    let next = saved.next().ok_or("checkpoints exhausted")?;
    store
        .write_bodies(next, &Default::default())
        .map_err(|error| error.to_string())?;
    // The older row was superseded and reclaimed by the publication; put it back, as a slow
    // save would.
    let stale = world_runtime::store::ChunkPayload::zeroed();
    store
        .write_chunks(&[world_runtime::store::ChunkRow {
            key,
            revision: ChunkRevision::new(1),
            payload: &stale,
        }])
        .map_err(|error| error.to_string())?;
    let latest = store.latest().map_err(|error| error.to_string())?;
    let refused = store.publish(&Publication {
        id: next,
        tick: latest.tick,
        config: latest.config,
        window: latest.window,
        epoch: latest.epoch,
        heads: &[(key, ChunkRevision::new(1))],
        bodies: 0,
    });
    check.equal(
        "the older save is refused",
        refused.err(),
        Some(StoreError::Conflict(Conflict::HeadNotNewer {
            key,
            head: ChunkRevision::new(2),
            presented: ChunkRevision::new(1),
        })),
    );
    let report = live.session.close(std::time::Duration::from_secs(30));
    check.that(report.saved, || {
        format!("the close did not save: {report:?}")
    });
    drop(store);

    // A new store object and session from the file: the newest state is what reopens.
    let store = open_store(&path, StoreFaults::new())?;
    let mut live = open(
        adapter,
        store,
        SessionEpoch::new(2),
        session_config(),
        |_| {},
    )?;
    live.settle()?;
    live.move_to(window(0, 0, 2))?;
    check.equal("after reopening", live.column(cell)?, Some(second));
    check.metric(
        "write_age_ms_max",
        live.session.residency().summary().write_age_ms.max,
    );
    let report = live.session.close(std::time::Duration::from_secs(30));
    check.that(report.saved, || {
        format!("the second close did not save: {report:?}")
    });
    Ok(())
}

/// Loads answered out of request order or after their key left are never applied to a later
/// request or a reused slot: after a burst of window moves every resident chunk holds its own
/// key's content.
pub fn reordered_and_stale_loads(adapter: &str, check: &mut Check) -> Result<(), String> {
    let scratch = Scratch::new("reorder")?;
    let path = scratch.path("world.redb");
    world::create(&path, window(0, 0, 2), SimulationRules::NONE)?;
    let store = open_store(&path, StoreFaults::new())?;
    let mut live = open(
        adapter,
        store,
        SessionEpoch::FIRST,
        session_config(),
        |_| {},
    )?;
    live.settle()?;
    // Distinct saved content in four chunks, so a chunk landing under another key shows.
    let cells = [
        IVec2::new(-120, -120),
        IVec2::new(70, -10),
        IVec2::new(10, 130),
        IVec2::new(-40, 40),
    ];
    let mut expected = Vec::new();
    for (index, cell) in cells.iter().enumerate() {
        for _ in 0..=index {
            stroke(&mut live, EditTool::Raise, *cell, 1)?;
        }
        expected.push((*cell, live.column(*cell)?.ok_or("not resident")?));
    }
    live.save()?;
    let report = live.session.close(std::time::Duration::from_secs(30));
    check.that(report.saved, || format!("close: {report:?}"));

    let store = open_store(&path, StoreFaults::new())?;
    let config = SessionConfig {
        workers: WorkerConfig {
            loaders: 4,
            load_queue: 64,
            write_queue: 16,
        },
        residency: world_runtime::residency::ResidencyConfig {
            loads_in_flight: 64,
            ..session_config().residency
        },
        ..session_config()
    };
    let mut live = open(adapter, store, SessionEpoch::new(2), config, |_| {})?;
    let centers = [(0, 0), (6, 0), (0, 6), (-6, -6), (3, -3), (0, 0)];
    for step in 0..48 {
        let (x, y) = centers[step % centers.len()];
        live.session.set_window(window(x, y, 2));
        live.frames(1);
    }
    live.move_to(window(0, 0, 2))?;
    let summary = *live.session.residency().summary();
    check.metric("stale_loads", summary.stale_loads);
    check.metric("loads_out_of_order", summary.loads_out_of_order);
    check.metric("window_moves", summary.window_moves);
    check.that(summary.stale_loads > 0, || {
        format!("no load was answered after its key left: {summary:?}")
    });
    check.that(summary.loads_out_of_order > 0, || {
        format!("no load was answered out of order: {summary:?}")
    });
    for (cell, column) in &expected {
        check.equal(
            &format!("saved column {cell}"),
            live.column(*cell)?,
            Some(*column),
        );
    }
    // Every other chunk holds its generated terrain: spot the corner cells of each.
    let generator = WorldGenerator::new(world::SEED);
    for key in window(0, 0, 2).view().iter() {
        if expected.iter().any(|(cell, _)| chunk_of(*cell) == key) {
            continue;
        }
        let generated = generator.generate_chunk(key);
        for local in [(0, 0), (63, 0), (0, 63), (63, 63), (31, 17)] {
            let cell = key.origin_cell() + IVec2::new(local.0, local.1);
            let index = local.1 as usize * CHUNK_SIZE + local.0 as usize;
            check.equal(
                &format!("generated column {cell}"),
                live.column(cell)?,
                Some(generated.cells[index]),
            );
        }
    }
    let report = live.session.close(std::time::Duration::from_secs(30));
    check.that(report.saved, || format!("close: {report:?}"));
    Ok(())
}

/// A neighbour whose row cannot be read is a closed boundary: water poured at the corner of four
/// chunks with negative keys spreads into the three that are resident, none of it is lost, and
/// none reaches the missing one, which is never generated over.
pub fn missing_neighbours_are_closed(adapter: &str, check: &mut Check) -> Result<(), String> {
    let scratch = Scratch::new("missing")?;
    let path = scratch.path("world.redb");
    world::create(&path, window(0, 0, 1), SimulationRules::NONE)?;
    {
        // Save the corner chunks, then damage (-1, -1)'s row.
        let store = open_store(&path, StoreFaults::new())?;
        let mut live = open(
            adapter,
            store,
            SessionEpoch::FIRST,
            session_config(),
            |_| {},
        )?;
        live.settle()?;
        for cell in [IVec2::new(-30, -30), IVec2::new(30, 30)] {
            stroke(&mut live, EditTool::Lower, cell, 1)?;
        }
        live.save()?;
        let report = live.session.close(std::time::Duration::from_secs(30));
        check.that(report.saved, || format!("close: {report:?}"));
    }
    let missing = ChunkKey::new(-1, -1, 0);
    let damaged = damage(&path, missing, |bytes| bytes[4000] ^= 0x40)?;
    let store = open_store(&path, StoreFaults::new())?;
    let mut live = open(
        adapter,
        store,
        SessionEpoch::new(2),
        session_config(),
        |_| {},
    )?;
    live.settle()?;
    let failed: Vec<_> = live
        .session
        .residency()
        .failed()
        .map(|(key, error)| (key, error.clone()))
        .collect();
    check.that(
        failed.len() == 1
            && failed[0].0 == missing
            && matches!(failed[0].1, StoreError::Corrupt { .. }),
        || format!("failed keys: {failed:?}"),
    );
    check.equal(
        "the missing chunk is not resident",
        live.column(IVec2::new(-10, -10))?,
        None,
    );
    let edit = MaterialEdit::new(EditTool::Water, IVec2::new(0, 0), 4, 3)
        .map_err(|error| error.to_string())?;
    let (changed, refused) = match live.run(Command::EditMaterial(edit))? {
        CommandResult::Edited {
            changed,
            not_resident,
            ..
        } => (changed, not_resident),
        other => return Err(format!("stroke: {other:?}")),
    };
    check.that(changed > 0, || "no water was painted".to_owned());
    check.that(refused > 0, || {
        "the stroke's part in the missing chunk was not refused".to_owned()
    });
    // The stroke crosses the corner into each resident chunk.
    for cell in [IVec2::new(1, 1), IVec2::new(-2, 1), IVec2::new(1, -2)] {
        let column = live.column(cell)?.ok_or("not resident")?;
        check.that(column.fluid_vol() > 0, || {
            format!("no water painted at {cell}")
        });
    }
    let painted = totals(&mut live)?;
    live.run(Command::SetSimulationRules(SimulationRules {
        liquid: true,
        ..SimulationRules::NONE
    }))?;
    live.frames(120);
    let after = totals(&mut live)?;
    check.equal(
        "water after flowing equals what was painted",
        after,
        painted,
    );
    for cell in [IVec2::new(-1, -1), IVec2::new(-2, -1), IVec2::new(-1, -2)] {
        check.equal(
            &format!("no water entered the missing chunk at {cell}"),
            live.column(cell)?,
            None,
        );
    }
    check.metric("painted_cells", changed);
    check.metric("refused_cells", refused);
    let report = live.session.close(std::time::Duration::from_secs(30));
    check.that(report.saved, || format!("close: {report:?}"));
    check.equal(
        "the damaged row is unchanged",
        read_row(&path, missing)?,
        Some(damaged),
    );
    Ok(())
}

/// A bit-flipped, a truncated and a future-version row each keep their chunk out (never
/// generated), are reported as what they are, survive a session byte for byte, and are not
/// named by a later checkpoint.
pub fn refused_rows_stay_out(adapter: &str, check: &mut Check) -> Result<(), String> {
    let scratch = Scratch::new("refused")?;
    let path = scratch.path("world.redb");
    world::create(&path, window(0, 0, 1), SimulationRules::NONE)?;
    let cells = [
        IVec2::new(-40, 20),
        IVec2::new(20, -40),
        IVec2::new(100, 100),
    ];
    {
        let store = open_store(&path, StoreFaults::new())?;
        let mut live = open(
            adapter,
            store,
            SessionEpoch::FIRST,
            session_config(),
            |_| {},
        )?;
        live.settle()?;
        for cell in cells {
            stroke(&mut live, EditTool::Raise, cell, 1)?;
        }
        live.save()?;
        let report = live.session.close(std::time::Duration::from_secs(30));
        check.that(report.saved, || format!("close: {report:?}"));
    }
    let keys: Vec<ChunkKey> = cells.iter().map(|cell| chunk_of(*cell)).collect();
    let flipped = damage(&path, keys[0], |bytes| bytes[100] ^= 1)?;
    let truncated = damage(&path, keys[1], |bytes| bytes.truncate(1000))?;
    let future = damage(&path, keys[2], |bytes| {
        bytes[4] = 9;
        let end = bytes.len() - 4;
        let checksum = format::crc32c(&bytes[..end]);
        bytes[end..].copy_from_slice(&checksum.to_le_bytes());
    })?;
    let store = open_store(&path, StoreFaults::new())?;
    let mut live = open(
        adapter,
        store,
        SessionEpoch::new(2),
        session_config(),
        |_| {},
    )?;
    live.settle()?;
    let mut failed: Vec<_> = live
        .session
        .residency()
        .failed()
        .map(|(key, error)| (key, error.clone()))
        .collect();
    failed.sort_by_key(|(key, _)| (key.key.x, key.key.y));
    check.equal("refused keys", failed.len(), 3);
    for (key, error) in &failed {
        let unsupported = *key == keys[2];
        check.that(
            if unsupported {
                matches!(error, StoreError::Unsupported { .. })
            } else {
                matches!(error, StoreError::Corrupt { .. })
            },
            || format!("{key:?}: {error:?}"),
        );
    }
    for cell in cells {
        check.equal(&format!("{cell} is out"), live.column(cell)?, None);
    }
    check.equal(
        "nothing was generated in their place",
        live.session.residency().summary().loads_generated,
        6,
    );
    // A later checkpoint does not name them: their heads stay as they were.
    stroke(&mut live, EditTool::Raise, IVec2::new(10, 10), 1)?;
    live.save()?;
    let report = live.session.close(std::time::Duration::from_secs(30));
    check.that(report.saved, || format!("close: {report:?}"));
    for (key, bytes, what) in [
        (keys[0], flipped, "flipped"),
        (keys[1], truncated, "truncated"),
        (keys[2], future, "future"),
    ] {
        check.equal(
            &format!("the {what} row is unchanged"),
            read_row(&path, key)?,
            Some(bytes),
        );
    }
    Ok(())
}

fn totals(live: &mut Live) -> Result<u64, String> {
    match live.run(Command::MeasureTotals)? {
        CommandResult::Totals(totals) => Ok(totals.fluid.iter().sum()),
        other => Err(format!("totals: {other:?}")),
    }
}

/// Applies `damage` to the published row of `key` (revision 1), with the store closed, and
/// returns the bytes now stored.
fn damage(
    path: &std::path::Path,
    key: ChunkKey,
    damage: impl FnOnce(&mut Vec<u8>),
) -> Result<Vec<u8>, String> {
    let db = Database::open(path).map_err(|error| error.to_string())?;
    let txn = db.begin_write().map_err(|error| error.to_string())?;
    let bytes = {
        let mut table = txn.open_table(CHUNKS).map_err(|error| error.to_string())?;
        let row_key = (key.key.x, key.key.y, key.key.z, 1);
        let mut bytes = table
            .get(row_key)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("no row for {key:?}"))?
            .value()
            .to_vec();
        damage(&mut bytes);
        table
            .insert(row_key, bytes.as_slice())
            .map_err(|error| error.to_string())?;
        bytes
    };
    txn.commit().map_err(|error| error.to_string())?;
    Ok(bytes)
}

fn read_row(path: &std::path::Path, key: ChunkKey) -> Result<Option<Vec<u8>>, String> {
    let db = redb::Builder::new()
        .open_read_only(path)
        .map_err(|error| error.to_string())?;
    use redb::ReadableDatabase;
    let txn = db.begin_read().map_err(|error| error.to_string())?;
    let table = txn.open_table(CHUNKS).map_err(|error| error.to_string())?;
    Ok(table
        .get((key.key.x, key.key.y, key.key.z, 1))
        .map_err(|error| error.to_string())?
        .map(|row| row.value().to_vec()))
}
