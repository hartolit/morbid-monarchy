//! The store against real redb files in temporary directories: reads that distinguish absent,
//! corrupt and unsupported rows; publication rules; injected I/O errors and held writes; and
//! process interruption at each defined point, after which a reopen must find the previous
//! complete checkpoint or the new complete one.
//!
//! Interruption runs in a child process: this test executable re-runs itself with only
//! `interruption_child` selected and the environment naming the point, and that child aborts
//! there. The parent then opens the file the child left.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use redb::{Database, ReadableTable};
use world_model::{
    cell::WorldCell,
    command::{AmbientTemperature, ElevationScale},
    generation::ChunkTheme,
    identity,
    resume::BodySet,
    rules::SimulationRules,
    spatial::ChunkKey,
};
use world_runtime::{
    ids::{ChunkRevision, SessionEpoch, Tick, WorldId},
    port::ResidentWindow,
    session::SessionPlan,
    store::{
        CheckpointId, ChunkPayload, ChunkRow, Conflict, Generator, PhysicalConfig, Publication,
        StoreError, StoredItem, Unsupported, WorldHeader, WorldOrigin, WorldStore,
    },
};
use world_store::{
    faults::{Action, Barrier, StoreFaults},
    format,
    store::{Access, OpenOptionsStore, RedbWorldStore},
};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "world-store-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        fs::create_dir_all(&dir).expect("scratch directory");
        Self(dir)
    }

    fn path(&self, file: &str) -> PathBuf {
        self.0.join(file)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn header() -> WorldHeader {
    WorldHeader {
        world: WorldId::new(0x5EED),
        generator: Generator {
            revision: identity::GENERATOR_REVISION,
            seed: 69,
        },
        rule_seed: 7,
        origin: WorldOrigin::Created,
    }
}

fn config() -> PhysicalConfig {
    PhysicalConfig {
        rules: SimulationRules::ALL,
        ambient_temperature: AmbientTemperature::celsius(20.0).expect("supported"),
        elevation_scale: ElevationScale::DEFAULT,
    }
}

fn window() -> ResidentWindow {
    ResidentWindow::new(ChunkKey::new(0, 0, 0), 2, 2).expect("window")
}

fn publication<'a>(
    id: u64,
    tick: Option<u64>,
    heads: &'a [(ChunkKey, ChunkRevision)],
) -> Publication<'a> {
    Publication {
        id: CheckpointId::new(id),
        tick: tick.map(Tick::new),
        config: config(),
        window: window(),
        epoch: SessionEpoch::FIRST,
        heads,
        bodies: 0,
    }
}

fn create(path: &Path, faults: StoreFaults) -> RedbWorldStore {
    RedbWorldStore::create(path, &header(), &publication(1, None, &[]), faults).expect("create")
}

fn open(path: &Path, faults: StoreFaults) -> RedbWorldStore {
    RedbWorldStore::open(
        path,
        OpenOptionsStore {
            faults,
            ..OpenOptionsStore::default()
        },
    )
    .expect("open")
}

fn payload(seed: u64) -> ChunkPayload {
    let mut payload = ChunkPayload::zeroed();
    for (index, cell) in payload.cells_mut().iter_mut().enumerate() {
        *cell = WorldCell(seed.wrapping_mul(0x9E37_79B9).wrapping_add(index as u64));
    }
    payload.set_theme(ChunkTheme(seed as u8 & 7));
    payload
}

fn write(
    store: &RedbWorldStore,
    key: ChunkKey,
    revision: u64,
    seed: u64,
) -> Result<(), StoreError> {
    store.write_chunks(&[ChunkRow {
        key,
        revision: ChunkRevision::new(revision),
        payload: &payload(seed),
    }])
}

/// Writes the body set for `id` and publishes it with `heads`.
fn checkpoint(
    store: &RedbWorldStore,
    id: u64,
    tick: u64,
    heads: &[(ChunkKey, ChunkRevision)],
) -> Result<(), StoreError> {
    store.write_bodies(CheckpointId::new(id), &BodySet::default())?;
    store
        .publish(&publication(id, Some(tick), heads))
        .map(|_| ())
}

fn read(
    store: &RedbWorldStore,
    key: ChunkKey,
    revision: Option<u64>,
) -> Result<Option<(u64, ChunkPayload)>, StoreError> {
    let mut into = ChunkPayload::zeroed();
    Ok(store
        .read_chunk(key, revision.map(ChunkRevision::new), &mut into)?
        .map(|revision| (revision.value(), into)))
}

const A: ChunkKey = ChunkKey {
    key: world_model::glam::IVec3::new(-1, 2, 0),
};
const B: ChunkKey = ChunkKey {
    key: world_model::glam::IVec3::new(3, -4, 0),
};

#[test]
fn a_created_world_is_complete_and_reopens_identically() {
    let scratch = Scratch::new("create");
    let path = scratch.path("world.redb");
    let store = create(&path, StoreFaults::new());
    let latest = store.latest().expect("latest");
    assert_eq!(latest.id, CheckpointId::FIRST);
    assert_eq!(latest.tick, None);
    assert_eq!(latest.next_tick(), Some(Tick::ZERO));
    assert!(
        matches!(
            RedbWorldStore::create(
                &path,
                &header(),
                &publication(1, None, &[]),
                StoreFaults::new()
            ),
            Err(StoreError::Io { .. })
        ),
        "creating over an existing file is refused"
    );
    drop(store);
    let store = open(&path, StoreFaults::new());
    assert_eq!(*store.header(), header());
    assert_eq!(store.latest().expect("latest"), latest);
    assert_eq!(store.verify().expect("verifies").chunks, 0);
}

#[test]
fn absent_found_corrupt_and_unsupported_rows_read_differently() {
    let scratch = Scratch::new("reads");
    let path = scratch.path("world.redb");
    let store = create(&path, StoreFaults::new());
    assert_eq!(
        read(&store, A, None).expect("readable"),
        None,
        "absent: may generate"
    );
    write(&store, A, 1, 11).expect("write");
    assert_eq!(
        read(&store, A, None).expect("readable"),
        None,
        "a revision no checkpoint published is not the key's saved state"
    );
    assert_eq!(
        read(&store, A, Some(1)).expect("readable").map(|(r, _)| r),
        Some(1)
    );
    assert_eq!(read(&store, A, Some(2)).expect("readable"), None);
    checkpoint(&store, 2, 10, &[(A, ChunkRevision::new(1))]).expect("publish");
    let (revision, found) = read(&store, A, None).expect("readable").expect("found");
    assert_eq!((revision, found), (1, payload(11)));
    drop(store);

    // Damage the row in place, as bit rot would, and give another row a future version.
    {
        let db = Database::open(&path).expect("raw open");
        let txn = db.begin_write().expect("write");
        {
            let mut chunks = txn
                .open_table(redb::TableDefinition::<(i32, i32, i32, u64), &[u8]>::new(
                    "chunks",
                ))
                .expect("chunks");
            let mut bytes = chunks
                .get((-1, 2, 0, 1))
                .expect("read")
                .expect("row")
                .value()
                .to_vec();
            bytes[5000] ^= 0x20;
            chunks
                .insert((-1, 2, 0, 1), bytes.as_slice())
                .expect("damage");
            let mut future = format::encode_chunk(B, ChunkRevision::new(1), &payload(3));
            future[4] = 2;
            let end = future.len() - 4;
            let checksum = format::crc32c(&future[..end]);
            future[end..].copy_from_slice(&checksum.to_le_bytes());
            chunks
                .insert((3, -4, 0, 1), future.as_slice())
                .expect("future row");
        }
        txn.commit().expect("commit");
    }
    let before = fs::read(&path).expect("bytes");
    let store = RedbWorldStore::open(
        &path,
        OpenOptionsStore {
            access: Access::ReadOnly,
            ..OpenOptionsStore::default()
        },
    )
    .expect("read-only open");
    assert_eq!(
        read(&store, A, None).err(),
        Some(StoreError::Corrupt {
            item: StoredItem::Chunk(A, ChunkRevision::new(1)),
            detail: "checksum mismatch"
        })
    );
    assert!(matches!(
        read(&store, B, Some(1)),
        Err(StoreError::Unsupported {
            detail: Unsupported::FormatVersion {
                stored: 2,
                supported: 1
            },
            ..
        })
    ));
    assert!(
        store.verify().is_err(),
        "verification finds the damaged row"
    );
    drop(store);
    assert_eq!(
        fs::read(&path).expect("bytes"),
        before,
        "refused reads changed nothing"
    );
}

#[test]
fn publication_refuses_missing_older_and_out_of_order_data_and_changes_nothing() {
    let scratch = Scratch::new("publish");
    let path = scratch.path("world.redb");
    let store = create(&path, StoreFaults::new());
    write(&store, A, 2, 1).expect("write");
    checkpoint(&store, 2, 5, &[(A, ChunkRevision::new(2))]).expect("publish");

    store
        .write_bodies(CheckpointId::new(3), &BodySet::default())
        .expect("bodies");
    let missing = [(B, ChunkRevision::new(1))];
    assert_eq!(
        store.publish(&publication(3, Some(6), &missing)).err(),
        Some(StoreError::Conflict(Conflict::MissingRevision {
            key: B,
            revision: ChunkRevision::new(1)
        }))
    );
    // The stale-save specimen: an older revision of A, saved late, must not replace the newer.
    write(&store, A, 1, 99).expect("an older revision is still a valid row");
    let stale = [(A, ChunkRevision::new(1))];
    assert_eq!(
        store.publish(&publication(3, Some(6), &stale)).err(),
        Some(StoreError::Conflict(Conflict::HeadNotNewer {
            key: A,
            head: ChunkRevision::new(2),
            presented: ChunkRevision::new(1)
        }))
    );
    assert_eq!(
        store.publish(&publication(4, Some(6), &[])).err(),
        Some(StoreError::Conflict(Conflict::CheckpointOrder {
            expected: CheckpointId::new(3),
            presented: CheckpointId::new(4)
        }))
    );
    assert_eq!(
        store.publish(&publication(3, Some(6), &[])).map(|m| m.id),
        Ok(CheckpointId::new(3))
    );
    assert_eq!(
        store.publish(&publication(4, Some(7), &[])).err(),
        Some(StoreError::Conflict(Conflict::MissingBodies(
            CheckpointId::new(4)
        )))
    );
    assert_eq!(
        write(&store, A, 2, 7).err(),
        Some(StoreError::Conflict(Conflict::RevisionExists {
            key: A,
            revision: ChunkRevision::new(2)
        })),
        "another payload under an existing revision"
    );
    write(&store, A, 2, 1).expect("the same payload again is an idempotent retry");
    assert!(matches!(
        write(&store, ChunkKey::new(0, 0, 1), 1, 1),
        Err(StoreError::Conflict(Conflict::InvalidRow { .. }))
    ));
    assert!(matches!(
        write(&store, A, 0, 1),
        Err(StoreError::Conflict(Conflict::InvalidRow { .. }))
    ));
    let latest = store.latest().expect("latest");
    assert_eq!(
        (latest.id, latest.tick),
        (CheckpointId::new(3), Some(Tick::new(6)))
    );
    assert_eq!(
        read(&store, A, None).expect("readable").map(|(r, _)| r),
        Some(2)
    );
    store
        .write_bodies(CheckpointId::new(3), &BodySet::default())
        .expect("the published body set again is an idempotent retry");
    let other = BodySet {
        slots: vec![world_model::resume::BodySlot {
            generation: 1,
            state: world_model::resume::SlotState::Free,
        }],
        bodies: Vec::new(),
    };
    assert_eq!(
        store.write_bodies(CheckpointId::new(3), &other).err(),
        Some(StoreError::Conflict(Conflict::CheckpointOrder {
            expected: CheckpointId::new(4),
            presented: CheckpointId::new(3)
        })),
        "a published body set is immutable"
    );
}

#[test]
fn injected_io_errors_fail_writes_cleanly_and_a_retry_reaches_the_file() {
    let scratch = Scratch::new("io");
    let path = scratch.path("world.redb");
    let faults = StoreFaults::new();
    let store = create(&path, faults.clone());

    // A write error inside redb's commit: the row is not there, and the next attempt reopens
    // the file and succeeds.
    faults.fail_writes(1);
    assert!(matches!(write(&store, A, 1, 5), Err(StoreError::Io { .. })));
    assert_eq!(
        read(&store, A, Some(1)).expect("reopened and readable"),
        None
    );
    write(&store, A, 1, 5).expect("retry");
    assert_eq!(
        read(&store, A, Some(1)).expect("readable").map(|(r, _)| r),
        Some(1)
    );

    // A write error inside the publication's commit: the commit never happened, the previous
    // checkpoint stays, and a retry publishes.
    store
        .write_bodies(CheckpointId::new(2), &BodySet::default())
        .expect("bodies");
    faults.fail_writes(1);
    let heads = [(A, ChunkRevision::new(1))];
    assert!(matches!(
        store.publish(&publication(2, Some(9), &heads)),
        Err(StoreError::Io { .. })
    ));
    assert_eq!(store.latest().expect("latest").id, CheckpointId::FIRST);
    assert_eq!(read(&store, A, None).expect("readable"), None);
    store
        .publish(&publication(2, Some(9), &heads))
        .expect("retry publishes");
    assert_eq!(
        read(&store, A, None).expect("readable").map(|(r, _)| r),
        Some(1)
    );

    // A sync error is ambiguous: the commit's pages were written and may survive. The same
    // writes again succeed whichever way it went, and a different publication under the same
    // identity is refused.
    write(&store, A, 2, 8).expect("write");
    let heads = [(A, ChunkRevision::new(2))];
    store
        .write_bodies(CheckpointId::new(3), &BodySet::default())
        .expect("bodies");
    faults.fail_syncs(1);
    assert!(matches!(
        store.publish(&publication(3, Some(12), &heads)),
        Err(StoreError::Io { .. })
    ));
    let after_failure = store.latest().expect("latest").id;
    assert!(after_failure == CheckpointId::new(2) || after_failure == CheckpointId::new(3));
    store
        .write_bodies(CheckpointId::new(3), &BodySet::default())
        .expect("the same body set again");
    store
        .publish(&publication(3, Some(12), &heads))
        .expect("retry succeeds");
    assert_eq!(store.latest().expect("latest").id, CheckpointId::new(3));
    assert!(matches!(
        store.publish(&publication(3, Some(13), &heads)),
        Err(StoreError::Conflict(Conflict::CheckpointOrder { .. }))
    ));
    assert_eq!(
        read(&store, A, None).expect("readable").map(|(r, _)| r),
        Some(2)
    );

    // A returned failure at a barrier is a simulation, not an I/O error of the file.
    faults.arm(Barrier::BeforeChunkCommit, Action::Fail, 1);
    let (writes_before, _, _) = faults.io_counts();
    assert!(matches!(
        write(&store, B, 1, 6),
        Err(StoreError::Io {
            operation: "injected fault",
            ..
        })
    ));
    assert_eq!(
        faults.io_counts().0,
        writes_before,
        "nothing reached the file"
    );
    assert_eq!(read(&store, B, Some(1)).expect("readable"), None);
    write(&store, B, 1, 6).expect("unarmed again");
    drop(store);
    let store = open(&path, StoreFaults::new());
    assert_eq!(store.verify().expect("verifies").chunks, 1);
}

#[test]
fn a_held_write_delays_only_itself() {
    let scratch = Scratch::new("hold");
    let path = scratch.path("world.redb");
    let faults = StoreFaults::new();
    let store = std::sync::Arc::new(create(&path, faults.clone()));
    write(&store, A, 1, 1).expect("write");
    faults.arm(
        Barrier::BeforeChunkCommit,
        Action::Hold {
            limit: Duration::from_secs(20),
        },
        1,
    );
    let writer = {
        let store = std::sync::Arc::clone(&store);
        std::thread::spawn(move || write(&store, B, 1, 2))
    };
    let started = std::time::Instant::now();
    while faults.holding() != Some(Barrier::BeforeChunkCommit) {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the write never reached the hold"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    // Reads go on while the write is held, and do not see it.
    assert_eq!(
        read(&store, A, Some(1)).expect("readable").map(|(r, _)| r),
        Some(1)
    );
    assert_eq!(read(&store, B, Some(1)).expect("readable"), None);
    faults.release();
    writer
        .join()
        .expect("writer")
        .expect("released write commits");
    assert_eq!(
        read(&store, B, Some(1)).expect("readable").map(|(r, _)| r),
        Some(1)
    );
}

#[test]
fn reclamation_keeps_published_and_named_live_revisions_only() {
    let scratch = Scratch::new("reclaim");
    let path = scratch.path("world.redb");
    let store = create(&path, StoreFaults::new());
    write(&store, A, 1, 1).expect("write");
    checkpoint(&store, 2, 1, &[(A, ChunkRevision::new(1))]).expect("publish");
    write(&store, A, 2, 2).expect("write");
    checkpoint(&store, 3, 2, &[(A, ChunkRevision::new(2))]).expect("publish");
    let tally = store.reclaim_tally();
    assert_eq!(
        (tally.removed.chunk_rows, tally.failures),
        (1, 0),
        "A@1 was superseded"
    );
    assert_eq!(tally.removed.manifests, 2);
    write(&store, A, 3, 3).expect("unpublished");
    write(&store, B, 1, 4).expect("unpublished");
    let report = store
        .reclaim(&[(A, ChunkRevision::new(3))])
        .expect("reclaim");
    assert_eq!(report.chunk_rows, 1, "only B@1, which nothing names, goes");
    assert_eq!(
        read(&store, A, Some(3)).expect("readable").map(|(r, _)| r),
        Some(3)
    );
    assert_eq!(read(&store, B, Some(1)).expect("readable"), None);
    assert_eq!(
        read(&store, A, None).expect("readable").map(|(r, _)| r),
        Some(2)
    );
    assert_eq!(store.row_counts().expect("counts"), [2, 1, 1, 1]);
}

#[test]
fn read_only_stores_never_write() {
    let scratch = Scratch::new("readonly");
    let path = scratch.path("world.redb");
    drop(create(&path, StoreFaults::new()));
    let before = fs::read(&path).expect("bytes");
    let store = RedbWorldStore::open(
        &path,
        OpenOptionsStore {
            access: Access::ReadOnly,
            ..OpenOptionsStore::default()
        },
    )
    .expect("read-only open");
    assert_eq!(store.latest().expect("latest").id, CheckpointId::FIRST);
    assert!(matches!(write(&store, A, 1, 1), Err(StoreError::Io { .. })));
    assert!(store.begin_session().is_err());
    drop(store);
    assert_eq!(fs::read(&path).expect("bytes"), before);
}

/// Opening for writing changes a redb file's bytes, so files that are not saved worlds (the old
/// writer's save, an empty redb file, bytes that are not redb) are refused before that, unchanged.
#[test]
fn files_that_are_not_saved_worlds_are_refused_unchanged() {
    let scratch = Scratch::new("foreign");
    let legacy = scratch.path("save.redb");
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy-v0/legacy-chunks.redb"),
        &legacy,
    )
    .expect("copy fixture");
    let empty = scratch.path("empty.redb");
    drop(Database::create(&empty).expect("empty redb"));
    let junk = scratch.path("junk.redb");
    fs::write(&junk, vec![0x5a; 8192]).expect("junk");
    for path in [&legacy, &empty, &junk] {
        let before = fs::read(path).expect("bytes");
        let refused = RedbWorldStore::open(path, OpenOptionsStore::default());
        assert!(
            matches!(
                refused,
                Err(StoreError::Corrupt {
                    item: StoredItem::Header,
                    ..
                } | StoreError::Io { .. })
            ),
            "{}: {:?}",
            path.display(),
            refused.err()
        );
        assert_eq!(fs::read(path).expect("bytes"), before, "{}", path.display());
    }
}

/// Rewrites one row of the database at `path` through redb, as damage inside a valid file.
fn damage_row<K: redb::Key + 'static>(
    path: &Path,
    table: redb::TableDefinition<K, &'static [u8]>,
    key: K::SelfType<'_>,
    change: fn(Vec<u8>) -> Vec<u8>,
) where
    for<'a> K::SelfType<'a>: Copy,
{
    let db = Database::open(path).expect("raw open");
    let txn = db.begin_write().expect("write");
    {
        let mut rows = txn.open_table(table).expect("table");
        let bytes = rows.get(key).expect("read").expect("row").value().to_vec();
        rows.insert(key, change(bytes).as_slice()).expect("damage");
    }
    txn.commit().expect("commit");
}

/// Re-seals a row whose bytes were changed on purpose, so only the change is wrong.
fn reseal(mut bytes: Vec<u8>) -> Vec<u8> {
    let end = bytes.len() - 4;
    let checksum = format::crc32c(&bytes[..end]);
    bytes[end..].copy_from_slice(&checksum.to_le_bytes());
    bytes
}

/// Opening a world for play changes its file even when nothing is written, so a world whose
/// header, latest checkpoint or latest body set is damaged, or from another format version or
/// physics revision, must be refused by the read-only look before that, and left byte for byte.
#[test]
fn damaged_and_foreign_worlds_are_refused_before_anything_is_written() {
    let scratch = Scratch::new("refused");
    let original = scratch.path("world.redb");
    {
        let store = create(&original, StoreFaults::new());
        write(&store, A, 1, 3).expect("write");
        checkpoint(&store, 2, 40, &[(A, ChunkRevision::new(1))]).expect("publish");
        store.end_session().expect("closed in order");
    }
    const WORLD: redb::TableDefinition<&str, &[u8]> = redb::TableDefinition::new("world");
    const BODIES: redb::TableDefinition<u64, &[u8]> = redb::TableDefinition::new("bodies");
    const CHECKPOINTS: redb::TableDefinition<u64, &[u8]> =
        redb::TableDefinition::new("checkpoints");
    // Byte 12 lies inside every row's body; byte 4 is the format version; byte 20 of the
    // header is its physics revision.
    let flip = |mut bytes: Vec<u8>| {
        bytes[12] ^= 0x10;
        bytes
    };
    let flip: fn(Vec<u8>) -> Vec<u8> = flip;
    type Expected = fn(&StoreError) -> bool;
    let cases: Vec<(&str, Expected)> = vec![
        ("damaged header", |error| {
            matches!(
                error,
                StoreError::Corrupt {
                    item: StoredItem::Header,
                    ..
                }
            )
        }),
        ("future format version", |error| {
            matches!(
                error,
                StoreError::Unsupported {
                    item: StoredItem::Header,
                    detail: Unsupported::FormatVersion {
                        stored: 2,
                        supported: 1
                    },
                }
            )
        }),
        ("another physics revision", |error| {
            matches!(
                error,
                StoreError::Unsupported {
                    item: StoredItem::Header,
                    detail: Unsupported::Physics { .. },
                }
            )
        }),
        ("damaged latest pointer", |error| {
            matches!(
                error,
                StoreError::Corrupt {
                    item: StoredItem::Current,
                    ..
                }
            )
        }),
        (
            "damaged manifest",
            |error| matches!(error, StoreError::Corrupt { item: StoredItem::Manifest(id), .. } if *id == CheckpointId::new(2)),
        ),
        (
            "damaged body set",
            |error| matches!(error, StoreError::Corrupt { item: StoredItem::Bodies(id), .. } if *id == CheckpointId::new(2)),
        ),
    ];
    for (index, (what, expected)) in cases.iter().enumerate() {
        let path = scratch.path(&format!("case-{index}.redb"));
        fs::copy(&original, &path).expect("copy");
        match index {
            0 => damage_row(&path, WORLD, "header", flip),
            1 => damage_row(&path, WORLD, "header", |mut bytes| {
                bytes[4] = 2;
                reseal(bytes)
            }),
            2 => damage_row(&path, WORLD, "header", |mut bytes| {
                bytes[20] = bytes[20].wrapping_add(1);
                reseal(bytes)
            }),
            3 => damage_row(&path, WORLD, "latest", flip),
            4 => damage_row(&path, CHECKPOINTS, 2, flip),
            _ => damage_row(&path, BODIES, 2, flip),
        }
        let before = fs::read(&path).expect("bytes");
        // What the client does to open a world for play.
        let refused = RedbWorldStore::open(&path, OpenOptionsStore::default())
            .and_then(|store| SessionPlan::prepare(&store).map(|_| ()));
        match refused {
            Err(error) => assert!(expected(&error), "{what}: refused as {error:?}"),
            Ok(()) => panic!("{what}: the world was opened"),
        }
        assert!(
            fs::read(&path).expect("bytes") == before,
            "{what}: the refused world was changed"
        );
    }
}

#[test]
fn a_store_without_a_published_checkpoint_is_incomplete() {
    let scratch = Scratch::new("incomplete");
    let path = scratch.path("world.redb");
    drop(RedbWorldStore::create_unpublished(&path, &header(), StoreFaults::new()).expect("create"));
    assert!(matches!(
        RedbWorldStore::open(&path, OpenOptionsStore::default()),
        Err(StoreError::Incomplete)
    ));
    let store = RedbWorldStore::open(
        &path,
        OpenOptionsStore {
            allow_incomplete: true,
            ..OpenOptionsStore::default()
        },
    )
    .expect("inspection may open it");
    assert_eq!(store.latest(), Err(StoreError::Incomplete));
}

// --- Process interruption ---

const CHILD_STORE: &str = "WORLD_STORE_INTERRUPTION_STORE";
const CHILD_POINT: &str = "WORLD_STORE_INTERRUPTION_POINT";

/// The child half of the interruption tests: does nothing unless the parent set the variables.
/// With them, it opens the store the parent prepared, arms an abort at the named point and runs
/// the next checkpoint's writes, which it never finishes.
#[test]
fn interruption_child() {
    let (Ok(path), Ok(point)) = (std::env::var(CHILD_STORE), std::env::var(CHILD_POINT)) else {
        return;
    };
    let faults = StoreFaults::new();
    let store = open(Path::new(&path), faults.clone());
    match point.split_once(':') {
        Some(("writes", count)) => faults.abort_after_writes(count.parse().expect("count")),
        _ => {
            let barrier = match point.as_str() {
                "before-chunk-commit" => Barrier::BeforeChunkCommit,
                "after-chunk-commit" => Barrier::AfterChunkCommit,
                "before-bodies-commit" => Barrier::BeforeBodiesCommit,
                "before-publish-commit" => Barrier::BeforePublishCommit,
                "after-publish-commit" => Barrier::AfterPublishCommit,
                other => panic!("unknown point {other}"),
            };
            faults.arm(barrier, Action::Abort, 1);
        }
    }
    store.begin_session().expect("session");
    write(&store, A, 2, 22).expect("chunk");
    write(&store, B, 1, 33).expect("chunk");
    checkpoint(
        &store,
        3,
        20,
        &[(A, ChunkRevision::new(2)), (B, ChunkRevision::new(1))],
    )
    .expect("publish");
    // Reached only when the point was never hit: the parent treats a clean exit as a failure.
}

/// Runs the child to `point` and returns the store it left, and whether the child was in fact
/// interrupted (a write count past the child's last write lets it finish).
fn interrupt(point: &str) -> (PathBuf, bool) {
    let scratch = Scratch::new(&format!("interrupt-{}", point.replace(':', "-")));
    let path = scratch.path("world.redb");
    {
        let store = create(&path, StoreFaults::new());
        write(&store, A, 1, 11).expect("write");
        checkpoint(&store, 2, 10, &[(A, ChunkRevision::new(1))]).expect("publish");
        store.end_session().expect("clean");
    }
    let status = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "interruption_child",
            "--exact",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env(CHILD_STORE, &path)
        .env(CHILD_POINT, point)
        .output()
        .expect("child runs");
    let interrupted = !status.status.success();
    // Keep the directory: the caller inspects it, then removes it.
    let kept = path.clone();
    std::mem::forget(scratch);
    (kept, interrupted)
}

/// After an interruption the store holds checkpoint 2 (A@1) or checkpoint 3 (A@2, B@1), whole,
/// and the next session reclaims whatever the interrupted one left unpublished. `marked` says
/// the interrupted session had recorded itself open (every barrier lies after that).
fn check_interrupted(path: &Path, expect_new: Option<bool>, marked: bool) -> bool {
    let store = open(path, StoreFaults::new());
    let latest = store.latest().expect("a complete checkpoint survives");
    let new = latest.id == CheckpointId::new(3);
    if let Some(expected) = expect_new {
        assert_eq!(
            new, expected,
            "checkpoint after the interruption: {latest:?}"
        );
    }
    let verified = store.verify().expect("the surviving checkpoint is whole");
    let a = read(&store, A, None).expect("readable").expect("A");
    let b = read(&store, B, None).expect("readable");
    if new {
        assert_eq!((verified.chunks, a.0, a.1.clone()), (2, 2, payload(22)));
        assert_eq!(b, Some((1, payload(33))));
    } else {
        assert_eq!((verified.chunks, a.0, a.1.clone()), (1, 1, payload(11)));
        assert_eq!(b, None);
    }
    let start = store.begin_session().expect("session");
    if marked {
        assert!(
            start.previous_unclean,
            "the interrupted session never ended"
        );
    }
    let rows = store.row_counts().expect("counts")[0];
    assert_eq!(rows, verified.chunks, "only published revisions remain");
    store.end_session().expect("end");
    drop(store);
    let _ = fs::remove_dir_all(path.parent().expect("directory"));
    new
}

#[test]
fn interruption_before_the_publication_commits_keeps_the_previous_checkpoint() {
    for point in [
        "before-chunk-commit",
        "after-chunk-commit",
        "before-bodies-commit",
        "before-publish-commit",
    ] {
        let (path, interrupted) = interrupt(point);
        assert!(interrupted, "{point}: the child was not interrupted");
        check_interrupted(&path, Some(false), true);
    }
}

#[test]
fn interruption_after_the_publication_commits_keeps_the_new_checkpoint() {
    let (path, interrupted) = interrupt("after-publish-commit");
    assert!(interrupted, "the child was not interrupted");
    check_interrupted(&path, Some(true), true);
}

#[test]
fn interruption_inside_commits_never_exposes_a_partial_checkpoint() {
    // Abort at every file write from the first of the child's session through its publication,
    // so some interruptions land inside redb's commits. Each must leave one whole checkpoint.
    let (mut old, mut new) = (0, 0);
    for count in 1..=200 {
        let (path, interrupted) = interrupt(&format!("writes:{count}"));
        if check_interrupted(&path, None, false) {
            new += 1;
        } else {
            old += 1;
        }
        if !interrupted {
            // Past the child's last write: every earlier count was an interruption.
            break;
        }
    }
    assert!(
        old > 0 && new > 0,
        "interruptions on both sides of the publication: {old} {new}"
    );
}
