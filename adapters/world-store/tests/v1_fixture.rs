//! The committed version-1 world (`tests/fixtures/v1/world-v1.redb`, written by an earlier
//! build): this build reads exactly what was written, leaves the file byte for byte when it only
//! looks, continues the world in a new writing session, and refuses a damaged or unsupported copy
//! before opening it for writing, so the refused file is left byte for byte too.
//!
//! The expected content is computed by `fixtures/v1/content.rs`, not decoded from the file.

use std::{
    fs,
    path::{Path, PathBuf},
};

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use world_runtime::{
    ids::SessionEpoch,
    session::SessionPlan,
    store::{
        CheckpointId, ChunkPayload, ChunkRow, Publication, StoreError, StoredItem, Unsupported,
        WorldStore,
    },
};
use world_store::{
    format,
    store::{Access, OpenOptionsStore, RedbWorldStore},
};

#[path = "fixtures/v1/content.rs"]
mod content;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "world-store-v1-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        fs::create_dir_all(&dir).expect("scratch directory");
        Self(dir)
    }

    /// A copy of the fixture to change.
    fn copy(&self, file: &str) -> PathBuf {
        let path = self.0.join(file);
        fs::copy(fixture(), &path).expect("copy the fixture");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(content::FIXTURE)
}

fn read_only(path: &Path) -> RedbWorldStore {
    RedbWorldStore::open(
        path,
        OpenOptionsStore {
            access: Access::ReadOnly,
            ..OpenOptionsStore::default()
        },
    )
    .expect("a version-1 world opens read-only")
}

/// Checks that `store` holds the fixture's last checkpoint exactly.
fn holds_the_fixture(store: &RedbWorldStore) {
    assert_eq!(*store.header(), content::header());
    let latest = store.latest().expect("latest");
    let last = content::checkpoints().pop().expect("checkpoints");
    let id = last.id;
    assert_eq!(latest.id, CheckpointId::new(id));
    assert_eq!(latest.tick, Some(last.tick));
    assert_eq!(latest.epoch, last.epoch);
    assert_eq!(latest.config, content::config(id));
    assert_eq!(latest.window, content::window());
    assert_eq!(latest.chunks, content::published().len() as u64);
    assert_eq!(latest.bodies, 2);
    let mut payload = ChunkPayload::zeroed();
    for (key, revision) in content::published() {
        assert_eq!(
            store.read_chunk(key, None, &mut payload),
            Ok(Some(content::revision(revision))),
            "{key:?}"
        );
        assert_eq!(payload, content::payload(key, revision), "{key:?}");
    }
    assert_eq!(
        store.read_chunk(
            content::checkpoints()[0].revisions[0].0,
            Some(content::revision(1)),
            &mut payload
        ),
        Ok(None),
        "the superseded revision was reclaimed"
    );
    assert_eq!(store.read_bodies(latest.id), Ok(content::bodies(id)));
}

#[test]
fn the_v1_fixture_reads_as_written_and_looking_changes_nothing() {
    let before = fs::read(fixture()).expect("fixture bytes");
    let store = read_only(&fixture());
    holds_the_fixture(&store);
    let verified = store.verify().expect("verifies");
    assert_eq!(verified.chunks, 3);
    drop(store);
    assert!(
        fs::read(fixture()).expect("fixture bytes") == before,
        "reading the fixture changed it"
    );
}

#[test]
fn a_v1_world_continues_in_a_writing_session_of_this_build() {
    let scratch = Scratch::new("continue");
    let path = scratch.copy("world.redb");
    let store = RedbWorldStore::open(&path, OpenOptionsStore::default()).expect("opens to play");
    let plan = SessionPlan::prepare(&store).expect("a session plan");
    assert!(!plan.start.previous_unclean, "the capture closed in order");
    assert_eq!(plan.bodies, content::bodies(3));
    let key = content::published()[1].0;
    let payload = content::payload(key, 7);
    store
        .write_chunks(&[ChunkRow {
            key,
            revision: content::revision(2),
            payload: &payload,
        }])
        .expect("write");
    store
        .write_bodies(CheckpointId::new(4), &content::bodies(4))
        .expect("bodies");
    store
        .publish(&Publication {
            id: CheckpointId::new(4),
            tick: plan.manifest.tick.and_then(|tick| tick.next()),
            config: content::config(3),
            window: content::window(),
            epoch: SessionEpoch::new(4),
            heads: &[(key, content::revision(2))],
            bodies: 2,
        })
        .expect("publish");
    store.end_session().expect("end");
    drop(store);
    let store = read_only(&path);
    assert_eq!(store.latest().expect("latest").id, CheckpointId::new(4));
    let mut read = ChunkPayload::zeroed();
    assert_eq!(
        store.read_chunk(key, None, &mut read),
        Ok(Some(content::revision(2)))
    );
    assert_eq!(read, payload);
    for (other, revision) in content::published().into_iter().filter(|(k, _)| *k != key) {
        assert_eq!(
            store.read_chunk(other, None, &mut read),
            Ok(Some(content::revision(revision)))
        );
        assert_eq!(read, content::payload(other, revision));
    }
    assert!(store.verify().is_ok());
}

const WORLD: TableDefinition<&str, &[u8]> = TableDefinition::new("world");
const BODIES: TableDefinition<u64, &[u8]> = TableDefinition::new("bodies");
const CHECKPOINTS: TableDefinition<u64, &[u8]> = TableDefinition::new("checkpoints");
const CHUNKS: TableDefinition<(i32, i32, i32, u64), &[u8]> = TableDefinition::new("chunks");

/// Flips one bit inside a row's body: its checksum no longer matches.
fn flip(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes[12] ^= 0x10;
    bytes
}

/// A row of a future format version, with a valid checksum.
fn future(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes[4] = 2;
    let end = bytes.len() - 4;
    let checksum = format::crc32c(&bytes[..end]);
    bytes[end..].copy_from_slice(&checksum.to_le_bytes());
    bytes
}

/// Rewrites one row of the copy at `path` through redb, as damage inside a valid database file.
fn damage<K: redb::Key + 'static>(
    path: &Path,
    table: TableDefinition<K, &'static [u8]>,
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

#[test]
fn damaged_and_unsupported_v1_worlds_are_refused_before_anything_is_written() {
    let scratch = Scratch::new("damaged");
    let latest = CheckpointId::new(3);
    // What is damaged, the damaged copy, and whether a refusal names that damage.
    type Expected = fn(&StoreError) -> bool;
    let cases: Vec<(&str, PathBuf, Expected)> = vec![
        ("header", scratch.copy("header.redb"), |error| {
            matches!(
                error,
                StoreError::Corrupt {
                    item: StoredItem::Header,
                    ..
                }
            )
        }),
        (
            "future header",
            scratch.copy("future-header.redb"),
            |error| {
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
            },
        ),
        ("latest pointer", scratch.copy("latest.redb"), |error| {
            matches!(
                error,
                StoreError::Corrupt {
                    item: StoredItem::Current,
                    ..
                }
            )
        }),
        (
            "manifest",
            scratch.copy("manifest.redb"),
            |error| matches!(error, StoreError::Corrupt { item: StoredItem::Manifest(id), .. } if *id == CheckpointId::new(3)),
        ),
        (
            "body set",
            scratch.copy("bodies.redb"),
            |error| matches!(error, StoreError::Corrupt { item: StoredItem::Bodies(id), .. } if *id == CheckpointId::new(3)),
        ),
    ];
    damage(&cases[0].1, WORLD, "header", flip);
    damage(&cases[1].1, WORLD, "header", future);
    damage(&cases[2].1, WORLD, "latest", flip);
    damage(&cases[3].1, CHECKPOINTS, latest.value(), flip);
    damage(&cases[4].1, BODIES, latest.value(), flip);
    for (what, path, expected) in &cases {
        let before = fs::read(path).expect("bytes");
        // What the client does to open a world for play.
        let refused = RedbWorldStore::open(path, OpenOptionsStore::default())
            .and_then(|store| SessionPlan::prepare(&store).map(|_| ()));
        match refused {
            Err(error) => assert!(expected(&error), "{what}: refused as {error:?}"),
            Ok(()) => panic!("{what}: the damaged world was opened"),
        }
        assert!(
            fs::read(path).expect("bytes") == before,
            "{what}: the refused world was changed"
        );
    }
}

#[test]
fn a_damaged_chunk_row_stays_out_and_survives_a_session() {
    let scratch = Scratch::new("chunk");
    let path = scratch.copy("world.redb");
    let (key, revision) = content::published()[2];
    let row_key = (key.key.x, key.key.y, key.key.z, revision);
    damage(&path, CHUNKS, row_key, flip);
    let damaged = {
        let db = Database::open(&path).expect("raw open");
        let txn = db.begin_read().expect("read");
        let rows = txn.open_table(CHUNKS).expect("chunks");
        rows.get(row_key)
            .expect("read")
            .expect("row")
            .value()
            .to_vec()
    };
    let store = RedbWorldStore::open(&path, OpenOptionsStore::default()).expect("opens to play");
    let plan = SessionPlan::prepare(&store).expect("only the chunk is damaged");
    assert!(!plan.start.previous_unclean);
    let mut payload = ChunkPayload::zeroed();
    assert!(matches!(
        store.read_chunk(key, None, &mut payload),
        Err(StoreError::Corrupt {
            item: StoredItem::Chunk(found, _),
            ..
        }) if found == key
    ));
    for (other, revision) in content::published().into_iter().filter(|(k, _)| *k != key) {
        assert_eq!(
            store.read_chunk(other, None, &mut payload),
            Ok(Some(content::revision(revision)))
        );
    }
    store.end_session().expect("end");
    drop(store);
    let db = Database::open(&path).expect("raw open");
    let txn = db.begin_read().expect("read");
    let rows = txn.open_table(CHUNKS).expect("chunks");
    assert_eq!(
        rows.get(row_key).expect("read").expect("row").value(),
        damaged.as_slice(),
        "the damaged row is kept byte for byte"
    );
    assert!(rows.iter().expect("rows").count() == 3);
}
