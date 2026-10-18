//! The redb store of one saved world: `world_runtime::store::WorldStore` over one file.
//!
//! Tables, each holding rows of [`format`](mod@crate::format):
//!
//! | Table | Key | Value |
//! | --- | --- | --- |
//! | `world` | `"header"`, `"latest"`, `"session"` | header, latest pointer, session marker |
//! | `chunks` | `(x, y, z, revision)` | an immutable chunk revision |
//! | `heads` | `(x, y, z)` | the key's published revision |
//! | `bodies` | checkpoint id | the checkpoint's body set |
//! | `checkpoints` | checkpoint id | the checkpoint's manifest |
//!
//! Every write is one redb transaction committed with `Durability::Immediate` (redb's default):
//! durable when `commit` returns. Chunk revisions and body sets are written ahead of their
//! checkpoint. [`publish`](WorldStore::publish) moves the heads, writes the manifest and the
//! latest pointer in one transaction, after checking that every revision it names is durable
//! and newer than the key's head, so a crash leaves the previous complete checkpoint or the new
//! one. A second transaction then removes what the new checkpoint superseded; if that does not
//! happen, the leftovers are unreachable and the next unclean-session check removes them.
//!
//! An I/O failure makes redb refuse further transactions on that handle. The store then
//! reopens the file before the next operation, which lets redb repair it, so a caller retrying a
//! failed write retries against the real file, not a handle stuck in its error.

use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
    sync::{
        Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

use redb::{
    Builder, Database, ReadOnlyDatabase, ReadTransaction, ReadableDatabase, ReadableTable,
    ReadableTableMetadata, TableDefinition, TableError, WriteTransaction,
};
use world_model::{resume::BodySet, spatial::ChunkKey};
use world_runtime::{
    ids::ChunkRevision,
    store::{
        CheckpointId, CheckpointManifest, ChunkPayload, ChunkRow, Conflict, Publication,
        ReclaimReport, SessionStart, StoreError, StoredItem, WorldHeader, WorldStore,
    },
};

use crate::{
    faults::{Barrier, FaultBackend, StoreFaults},
    format::{self, Head, ManifestRow, SessionMarker},
};

const WORLD: TableDefinition<&str, &[u8]> = TableDefinition::new("world");
const CHUNKS: TableDefinition<(i32, i32, i32, u64), &[u8]> = TableDefinition::new("chunks");
const HEADS: TableDefinition<(i32, i32, i32), &[u8]> = TableDefinition::new("heads");
const BODIES: TableDefinition<u64, &[u8]> = TableDefinition::new("bodies");
const CHECKPOINTS: TableDefinition<u64, &[u8]> = TableDefinition::new("checkpoints");

const HEADER_KEY: &str = "header";
const LATEST_KEY: &str = "latest";
const SESSION_KEY: &str = "session";

/// How a store is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Writes allowed; an unclean previous shutdown is repaired by redb on open.
    ReadWrite,
    /// No write, no repair: a store needing repair is refused. For inspection.
    ReadOnly,
}

#[derive(Debug, Clone)]
pub struct OpenOptionsStore {
    pub access: Access,
    /// Open a store with a header but no published checkpoint (an interrupted import, for
    /// inspection or to finish one). Otherwise such a store is refused as incomplete.
    pub allow_incomplete: bool,
    pub faults: StoreFaults,
}

impl Default for OpenOptionsStore {
    fn default() -> Self {
        Self {
            access: Access::ReadWrite,
            allow_incomplete: false,
            faults: StoreFaults::new(),
        }
    }
}

enum Db {
    Write(Database),
    Read(ReadOnlyDatabase),
    /// The last handle failed and was dropped; the next operation reopens the file.
    Closed,
}

pub struct RedbWorldStore {
    path: PathBuf,
    access: Access,
    faults: StoreFaults,
    header: WorldHeader,
    db: RwLock<Db>,
    /// Set when a database operation failed; the next operation reopens before using it.
    failed: AtomicBool,
    /// One writer at a time: redb serializes write transactions too, but a reopen must not
    /// race one.
    writer: Mutex<()>,
    /// What removing superseded rows after each publication did, for reports: a failure there
    /// leaves unreachable rows, never a broken checkpoint.
    reclaimed: Mutex<ReclaimTally>,
}

/// Reclamation after publications, added up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReclaimTally {
    pub removed: ReclaimReport,
    pub failures: u64,
}

fn io<E: std::fmt::Display>(operation: &'static str) -> impl Fn(E) -> StoreError {
    move |error| StoreError::Io {
        operation,
        detail: error.to_string(),
    }
}

fn injected(detail: String) -> StoreError {
    StoreError::Io {
        operation: "injected fault",
        detail,
    }
}

fn chunk_key(key: ChunkKey, revision: ChunkRevision) -> (i32, i32, i32, u64) {
    (key.key.x, key.key.y, key.key.z, revision.value())
}

fn head_key(key: ChunkKey) -> (i32, i32, i32) {
    (key.key.x, key.key.y, key.key.z)
}

/// Opens a table in a read transaction; a table no write has created yet reads as empty.
fn read_table<K: redb::Key + 'static, V: redb::Value + 'static>(
    txn: &ReadTransaction,
    table: TableDefinition<K, V>,
) -> Result<Option<redb::ReadOnlyTable<K, V>>, StoreError> {
    match txn.open_table(table) {
        Ok(table) => Ok(Some(table)),
        Err(TableError::TableDoesNotExist(_)) => Ok(None),
        Err(error) => Err(io("open table")(error)),
    }
}

impl RedbWorldStore {
    /// Creates a new world at `path` (which must not exist) with its first checkpoint: no
    /// chunk saved yet, no body, the given configuration and window. The world is complete.
    pub fn create(
        path: &Path,
        header: &WorldHeader,
        first: &Publication<'_>,
        faults: StoreFaults,
    ) -> Result<Self, StoreError> {
        let store = Self::create_unpublished(path, header, faults)?;
        store.write_bodies(first.id, &BodySet::default())?;
        store.publish(first)?;
        Ok(store)
    }

    /// Creates a store at `path` with only its header: incomplete until a first checkpoint is
    /// published. Import uses this, so an interrupted import is never mistaken for a world.
    pub fn create_unpublished(
        path: &Path,
        header: &WorldHeader,
        faults: StoreFaults,
    ) -> Result<Self, StoreError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(io("create store file"))?;
        let backend = FaultBackend::new(file, faults.clone()).map_err(io("lock store file"))?;
        let database = Builder::new()
            .create_with_backend(backend)
            .map_err(io("create database"))?;
        let txn = database.begin_write().map_err(io("begin write"))?;
        {
            let mut world = txn.open_table(WORLD).map_err(io("open table"))?;
            world
                .insert(HEADER_KEY, format::encode_header(header).as_slice())
                .map_err(io("write header"))?;
            for table in [BODIES, CHECKPOINTS] {
                txn.open_table(table).map_err(io("create table"))?;
            }
            txn.open_table(CHUNKS).map_err(io("create table"))?;
            txn.open_table(HEADS).map_err(io("create table"))?;
        }
        txn.commit().map_err(io("commit header"))?;
        Ok(Self {
            path: path.to_owned(),
            access: Access::ReadWrite,
            faults,
            header: *header,
            db: RwLock::new(Db::Write(database)),
            failed: AtomicBool::new(false),
            writer: Mutex::new(()),
            reclaimed: Mutex::new(ReclaimTally::default()),
        })
    }

    /// Opens an existing world. Refuses a header from a format, layout, physics or generator
    /// revision this build does not read, and (unless allowed) a store without a published
    /// checkpoint; neither changes the file.
    pub fn open(path: &Path, options: OpenOptionsStore) -> Result<Self, StoreError> {
        if !path.is_file() {
            return Err(StoreError::Io {
                operation: "open store",
                detail: format!("{} is not a file", path.display()),
            });
        }
        let db = match options.access {
            Access::ReadWrite => Db::Write(Self::open_write(path, &options.faults)?),
            Access::ReadOnly => Db::Read(
                Builder::new()
                    .open_read_only(path)
                    .map_err(io("open store read-only"))?,
            ),
        };
        let txn = match &db {
            Db::Write(database) => database.begin_read(),
            Db::Read(database) => database.begin_read(),
            Db::Closed => unreachable!("just opened"),
        }
        .map_err(io("begin read"))?;
        let world = read_table(&txn, WORLD)?.ok_or(StoreError::Corrupt {
            item: StoredItem::Header,
            detail: "no world table: not a saved world",
        })?;
        let header =
            world
                .get(HEADER_KEY)
                .map_err(io("read header"))?
                .ok_or(StoreError::Corrupt {
                    item: StoredItem::Header,
                    detail: "no header",
                })?;
        let header = format::decode_header(header.value())?;
        drop(world);
        drop(txn);
        let store = Self {
            path: path.to_owned(),
            access: options.access,
            faults: options.faults,
            header,
            db: RwLock::new(db),
            failed: AtomicBool::new(false),
            writer: Mutex::new(()),
            reclaimed: Mutex::new(ReclaimTally::default()),
        };
        match store.latest() {
            Ok(_) => {}
            Err(StoreError::Incomplete) if options.allow_incomplete => {}
            Err(error) => return Err(error),
        }
        Ok(store)
    }

    fn open_write(path: &Path, faults: &StoreFaults) -> Result<Database, StoreError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(io("open store file"))?;
        let backend = FaultBackend::new(file, faults.clone()).map_err(io("lock store file"))?;
        Builder::new()
            .create_with_backend(backend)
            .map_err(io("open database"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn faults(&self) -> &StoreFaults {
        &self.faults
    }

    /// What reclaiming after publications removed, and how often it failed.
    pub fn reclaim_tally(&self) -> ReclaimTally {
        *self
            .reclaimed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Reopens the file after a failure: redb repairs it if the failure left it unclean.
    fn recover(&self) -> Result<(), StoreError> {
        if !self.failed.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut db = self
            .db
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.failed.load(Ordering::Acquire) {
            return Ok(());
        }
        *db = Db::Closed;
        let reopened = match self.access {
            Access::ReadWrite => Db::Write(Self::open_write(&self.path, &self.faults)?),
            Access::ReadOnly => Db::Read(
                Builder::new()
                    .open_read_only(&self.path)
                    .map_err(io("reopen store read-only"))?,
            ),
        };
        *db = reopened;
        self.failed.store(false, Ordering::Release);
        Ok(())
    }

    /// Runs `read` in a read transaction. A database failure schedules a reopen.
    fn read<T>(
        &self,
        read: impl FnOnce(&ReadTransaction) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.recover()?;
        let db = self
            .db
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let txn = match &*db {
            Db::Write(database) => database.begin_read(),
            Db::Read(database) => database.begin_read(),
            Db::Closed => {
                self.failed.store(true, Ordering::Release);
                return Err(StoreError::Io {
                    operation: "read",
                    detail: "the store is reopening after a failure".to_owned(),
                });
            }
        };
        let result = txn.map_err(io("begin read")).and_then(|txn| read(&txn));
        if matches!(result, Err(StoreError::Io { .. })) {
            self.failed.store(true, Ordering::Release);
        }
        result
    }

    /// Runs `write` in a write transaction and commits it, with `before_commit` reached between
    /// staging and the commit. Nothing is committed when `write` or the barrier fails.
    fn write<T>(
        &self,
        before_commit: Option<Barrier>,
        write: impl FnOnce(&WriteTransaction) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        if self.access == Access::ReadOnly {
            return Err(StoreError::Io {
                operation: "write",
                detail: "the store was opened read-only".to_owned(),
            });
        }
        let _writer = self
            .writer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.recover()?;
        let db = self
            .db
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Db::Write(database) = &*db else {
            self.failed.store(true, Ordering::Release);
            return Err(StoreError::Io {
                operation: "write",
                detail: "the store is reopening after a failure".to_owned(),
            });
        };
        let result = (|| {
            let txn = database.begin_write().map_err(io("begin write"))?;
            let value = write(&txn)?;
            if let Some(barrier) = before_commit {
                self.faults.reach(barrier).map_err(injected)?;
            }
            txn.commit().map_err(io("commit"))?;
            Ok(value)
        })();
        if matches!(result, Err(StoreError::Io { .. })) {
            self.failed.store(true, Ordering::Release);
        }
        result
    }

    fn latest_in(txn: &ReadTransaction) -> Result<Option<ManifestRow>, StoreError> {
        let Some(world) = read_table(txn, WORLD)? else {
            return Ok(None);
        };
        let Some(latest) = world.get(LATEST_KEY).map_err(io("read latest"))? else {
            return Ok(None);
        };
        let id = format::decode_latest(latest.value())?;
        let checkpoints = read_table(txn, CHECKPOINTS)?.ok_or(StoreError::Corrupt {
            item: StoredItem::Manifest(id),
            detail: "the latest checkpoint has no manifest table",
        })?;
        let manifest = checkpoints
            .get(id.value())
            .map_err(io("read manifest"))?
            .ok_or(StoreError::Corrupt {
                item: StoredItem::Manifest(id),
                detail: "the latest checkpoint's manifest is missing",
            })?;
        format::decode_manifest(manifest.value(), id).map(Some)
    }

    /// The latest manifest with what its row adds (for inspection).
    pub fn latest_row(&self) -> Result<ManifestRow, StoreError> {
        self.read(Self::latest_in)?.ok_or(StoreError::Incomplete)
    }

    /// Every published head, in key order (for inspection and verification).
    pub fn heads(&self) -> Result<Vec<(ChunkKey, Head)>, StoreError> {
        self.read(|txn| {
            let Some(heads) = read_table(txn, HEADS)? else {
                return Ok(Vec::new());
            };
            let mut out = Vec::with_capacity(heads.len().map_err(io("count heads"))? as usize);
            for entry in heads.iter().map_err(io("iterate heads"))? {
                let (key, value) = entry.map_err(io("read head"))?;
                let (x, y, z) = key.value();
                let key = ChunkKey::new(x, y, z);
                out.push((key, format::decode_head(value.value(), key)?));
            }
            Ok(out)
        })
    }

    /// Rows in each table: chunks, heads, body sets, manifests.
    pub fn row_counts(&self) -> Result<[u64; 4], StoreError> {
        self.read(|txn| {
            let mut counts = [0; 4];
            if let Some(table) = read_table(txn, CHUNKS)? {
                counts[0] = table.len().map_err(io("count"))?;
            }
            if let Some(table) = read_table(txn, HEADS)? {
                counts[1] = table.len().map_err(io("count"))?;
            }
            if let Some(table) = read_table(txn, BODIES)? {
                counts[2] = table.len().map_err(io("count"))?;
            }
            if let Some(table) = read_table(txn, CHECKPOINTS)? {
                counts[3] = table.len().map_err(io("count"))?;
            }
            Ok(counts)
        })
    }

    /// Reads and checks every published row: each head's chunk and the latest body set.
    pub fn verify(&self) -> Result<VerifyReport, StoreError> {
        let manifest = self.latest_row()?;
        let heads = self.heads()?;
        let mut payload = ChunkPayload::zeroed();
        for (key, head) in &heads {
            if self.read_chunk(*key, Some(head.revision), &mut payload)? != Some(head.revision) {
                return Err(StoreError::Corrupt {
                    item: StoredItem::Chunk(*key, head.revision),
                    detail: "a published revision is missing",
                });
            }
        }
        let bodies = self.read_bodies(manifest.manifest.id)?;
        if heads.len() as u64 != manifest.manifest.chunks
            || bodies.live() as u32 != manifest.manifest.bodies
        {
            return Err(StoreError::Corrupt {
                item: StoredItem::Manifest(manifest.manifest.id),
                detail: "the manifest's counts disagree with its rows",
            });
        }
        Ok(VerifyReport {
            checkpoint: manifest.manifest.id,
            chunks: heads.len() as u64,
            bodies: bodies.live() as u64,
        })
    }

    /// Removes chunk rows of `key` older than `revision`, and manifests and body sets older
    /// than `checkpoint`: what a publication superseded.
    fn reclaim_superseded(
        &self,
        moved: &[(ChunkKey, ChunkRevision)],
        checkpoint: CheckpointId,
    ) -> Result<ReclaimReport, StoreError> {
        self.write(Some(Barrier::AfterPublishCommit), |txn| {
            let mut report = ReclaimReport::default();
            {
                let mut chunks = txn.open_table(CHUNKS).map_err(io("open table"))?;
                for (key, revision) in moved {
                    let (x, y, z) = head_key(*key);
                    chunks
                        .retain_in((x, y, z, 0)..(x, y, z, revision.value()), |_, value| {
                            report.chunk_rows += 1;
                            report.bytes += value.len() as u64;
                            false
                        })
                        .map_err(io("reclaim chunks"))?;
                }
            }
            for (table, count) in [
                (BODIES, &mut report.body_sets),
                (CHECKPOINTS, &mut report.manifests),
            ] {
                let mut table = txn.open_table(table).map_err(io("open table"))?;
                table
                    .retain_in(..checkpoint.value(), |_, _| {
                        *count += 1;
                        false
                    })
                    .map_err(io("reclaim checkpoints"))?;
            }
            Ok(report)
        })
    }
}

impl RedbWorldStore {
    /// Checks that `row`, the latest checkpoint, is exactly what `publication` would publish.
    fn confirm_published(
        txn: &WriteTransaction,
        row: ManifestRow,
        publication: &Publication<'_>,
    ) -> Result<CheckpointManifest, StoreError> {
        let manifest = row.manifest;
        let order = StoreError::Conflict(Conflict::CheckpointOrder {
            expected: manifest.id.next().unwrap_or(manifest.id),
            presented: publication.id,
        });
        if manifest.tick != publication.tick
            || manifest.config != publication.config
            || manifest.window != publication.window
            || manifest.epoch != publication.epoch
            || manifest.bodies != publication.bodies
            || row.moved as usize != publication.heads.len()
        {
            return Err(order);
        }
        let heads = txn.open_table(HEADS).map_err(io("open table"))?;
        for (key, revision) in publication.heads {
            let head = heads
                .get(head_key(*key))
                .map_err(io("read head"))?
                .map(|row| format::decode_head(row.value(), *key))
                .transpose()?;
            if head
                != Some(Head {
                    revision: *revision,
                    checkpoint: publication.id,
                })
            {
                return Err(order);
            }
        }
        Ok(manifest)
    }
}

/// What [`RedbWorldStore::verify`] read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifyReport {
    pub checkpoint: CheckpointId,
    pub chunks: u64,
    pub bodies: u64,
}

impl WorldStore for RedbWorldStore {
    fn header(&self) -> &WorldHeader {
        &self.header
    }

    fn latest(&self) -> Result<CheckpointManifest, StoreError> {
        Ok(self.latest_row()?.manifest)
    }

    fn read_chunk(
        &self,
        key: ChunkKey,
        revision: Option<ChunkRevision>,
        into: &mut ChunkPayload,
    ) -> Result<Option<ChunkRevision>, StoreError> {
        self.read(|txn| {
            let revision = match revision {
                Some(revision) => revision,
                None => {
                    let Some(heads) = read_table(txn, HEADS)? else {
                        return Ok(None);
                    };
                    let Some(head) = heads.get(head_key(key)).map_err(io("read head"))? else {
                        return Ok(None);
                    };
                    format::decode_head(head.value(), key)?.revision
                }
            };
            let Some(chunks) = read_table(txn, CHUNKS)? else {
                return Ok(None);
            };
            let Some(row) = chunks
                .get(chunk_key(key, revision))
                .map_err(io("read chunk"))?
            else {
                return Ok(None);
            };
            format::decode_chunk(row.value(), key, revision, into)?;
            Ok(Some(revision))
        })
    }

    fn read_bodies(&self, checkpoint: CheckpointId) -> Result<BodySet, StoreError> {
        self.read(|txn| {
            let missing = StoreError::Corrupt {
                item: StoredItem::Bodies(checkpoint),
                detail: "the checkpoint's body set is missing",
            };
            let bodies = read_table(txn, BODIES)?.ok_or(missing.clone())?;
            let row = bodies
                .get(checkpoint.value())
                .map_err(io("read bodies"))?
                .ok_or(missing)?;
            let (set, checksum) = format::decode_bodies(row.value(), checkpoint)?;
            if let Some(latest) = Self::latest_in(txn)?
                && latest.manifest.id == checkpoint
                && latest.bodies_checksum != checksum
            {
                return Err(StoreError::Corrupt {
                    item: StoredItem::Bodies(checkpoint),
                    detail: "the body set is not the one its manifest published",
                });
            }
            Ok(set)
        })
    }

    fn write_chunks(&self, rows: &[ChunkRow<'_>]) -> Result<(), StoreError> {
        for row in rows {
            if row.key.key.z != 0 || row.revision.value() == 0 {
                return Err(StoreError::Conflict(Conflict::InvalidRow {
                    key: row.key,
                    revision: row.revision,
                }));
            }
        }
        if rows.is_empty() {
            return Ok(());
        }
        self.write(Some(Barrier::BeforeChunkCommit), |txn| {
            let mut chunks = txn.open_table(CHUNKS).map_err(io("open table"))?;
            for row in rows {
                let bytes = format::encode_chunk(row.key, row.revision, row.payload);
                let key = chunk_key(row.key, row.revision);
                if let Some(existing) = chunks.get(key).map_err(io("read chunk"))? {
                    // A retry of a write whose acknowledgement was lost writes the same bytes;
                    // anything else under this revision is a different payload.
                    if existing.value() == bytes.as_slice() {
                        continue;
                    }
                    return Err(StoreError::Conflict(Conflict::RevisionExists {
                        key: row.key,
                        revision: row.revision,
                    }));
                }
                chunks
                    .insert(key, bytes.as_slice())
                    .map_err(io("write chunk"))?;
            }
            Ok(())
        })?;
        self.faults
            .reach(Barrier::AfterChunkCommit)
            .map_err(injected)
    }

    fn write_bodies(&self, checkpoint: CheckpointId, bodies: &BodySet) -> Result<(), StoreError> {
        bodies
            .validate(world_model::body::MAX_BODIES)
            .map_err(|_| StoreError::Corrupt {
                item: StoredItem::Bodies(checkpoint),
                detail: "refusing to write an inconsistent body set",
            })?;
        let bytes = format::encode_bodies(checkpoint, bodies);
        self.write(Some(Barrier::BeforeBodiesCommit), |txn| {
            let latest = {
                let world = txn.open_table(WORLD).map_err(io("open table"))?;
                let latest = world.get(LATEST_KEY).map_err(io("read latest"))?;
                latest
                    .map(|row| format::decode_latest(row.value()))
                    .transpose()?
            };
            let mut table = txn.open_table(BODIES).map_err(io("open table"))?;
            if latest.is_some_and(|latest| checkpoint <= latest) {
                // A published checkpoint's bodies are immutable. The same bytes again are a
                // retry after an ambiguous failure (a failed sync whose commit survived).
                let same = table
                    .get(checkpoint.value())
                    .map_err(io("read bodies"))?
                    .is_some_and(|row| row.value() == bytes.as_slice());
                if same {
                    return Ok(());
                }
                return Err(StoreError::Conflict(Conflict::CheckpointOrder {
                    expected: latest.and_then(CheckpointId::next).unwrap_or(checkpoint),
                    presented: checkpoint,
                }));
            }
            table
                .insert(checkpoint.value(), bytes.as_slice())
                .map_err(io("write bodies"))?;
            Ok(())
        })
    }

    fn publish(&self, publication: &Publication<'_>) -> Result<CheckpointManifest, StoreError> {
        let manifest = self.write(Some(Barrier::BeforePublishCommit), |txn| {
            let previous = {
                let world = txn.open_table(WORLD).map_err(io("open table"))?;
                let latest = world.get(LATEST_KEY).map_err(io("read latest"))?;
                match latest {
                    None => None,
                    Some(row) => {
                        let id = format::decode_latest(row.value())?;
                        let checkpoints = txn.open_table(CHECKPOINTS).map_err(io("open table"))?;
                        let manifest = checkpoints
                            .get(id.value())
                            .map_err(io("read manifest"))?
                            .ok_or(StoreError::Corrupt {
                                item: StoredItem::Manifest(id),
                                detail: "the latest checkpoint's manifest is missing",
                            })?;
                        Some(format::decode_manifest(manifest.value(), id)?)
                    }
                }
            };
            if let Some(row) = previous
                && row.manifest.id == publication.id
            {
                // Already published: a retry after an ambiguous failure (a commit whose sync
                // failed may still have reached the file) succeeds when it is the same
                // publication, and is refused as out of order otherwise.
                return Self::confirm_published(txn, row, publication);
            }
            let expected = previous
                .map_or(Some(CheckpointId::FIRST), |row| row.manifest.id.next())
                .ok_or(StoreError::Conflict(Conflict::CheckpointOrder {
                    expected: publication.id,
                    presented: publication.id,
                }))?;
            if publication.id != expected {
                return Err(StoreError::Conflict(Conflict::CheckpointOrder {
                    expected,
                    presented: publication.id,
                }));
            }
            let bodies_checksum = {
                let bodies = txn.open_table(BODIES).map_err(io("open table"))?;
                let row = bodies
                    .get(publication.id.value())
                    .map_err(io("read bodies"))?
                    .ok_or(StoreError::Conflict(Conflict::MissingBodies(
                        publication.id,
                    )))?;
                let (set, checksum) = format::decode_bodies(row.value(), publication.id)?;
                if set.live() as u32 != publication.bodies {
                    return Err(StoreError::Conflict(Conflict::MissingBodies(
                        publication.id,
                    )));
                }
                checksum
            };
            let mut added = 0u64;
            {
                let chunks = txn.open_table(CHUNKS).map_err(io("open table"))?;
                let mut heads = txn.open_table(HEADS).map_err(io("open table"))?;
                for (key, revision) in publication.heads {
                    if key.key.z != 0 || revision.value() == 0 {
                        return Err(StoreError::Conflict(Conflict::InvalidRow {
                            key: *key,
                            revision: *revision,
                        }));
                    }
                    let durable = chunks
                        .get(chunk_key(*key, *revision))
                        .map_err(io("read chunk"))?
                        .is_some_and(|row| row.value().len() == format::CHUNK_BYTES);
                    if !durable {
                        return Err(StoreError::Conflict(Conflict::MissingRevision {
                            key: *key,
                            revision: *revision,
                        }));
                    }
                    let current = heads
                        .get(head_key(*key))
                        .map_err(io("read head"))?
                        .map(|row| format::decode_head(row.value(), *key))
                        .transpose()?;
                    match current {
                        Some(head) if head.revision >= *revision => {
                            return Err(StoreError::Conflict(Conflict::HeadNotNewer {
                                key: *key,
                                head: head.revision,
                                presented: *revision,
                            }));
                        }
                        Some(_) => {}
                        None => added += 1,
                    }
                    heads
                        .insert(
                            head_key(*key),
                            format::encode_head(Head {
                                revision: *revision,
                                checkpoint: publication.id,
                            })
                            .as_slice(),
                        )
                        .map_err(io("write head"))?;
                }
            }
            let manifest = CheckpointManifest {
                id: publication.id,
                tick: publication.tick,
                config: publication.config,
                window: publication.window,
                chunks: previous.map_or(0, |row| row.manifest.chunks) + added,
                bodies: publication.bodies,
                epoch: publication.epoch,
            };
            let row = ManifestRow {
                manifest,
                moved: publication.heads.len() as u32,
                bodies_checksum,
            };
            txn.open_table(CHECKPOINTS)
                .map_err(io("open table"))?
                .insert(
                    publication.id.value(),
                    format::encode_manifest(&row).as_slice(),
                )
                .map_err(io("write manifest"))?;
            txn.open_table(WORLD)
                .map_err(io("open table"))?
                .insert(LATEST_KEY, format::encode_latest(publication.id).as_slice())
                .map_err(io("write latest"))?;
            Ok(manifest)
        })?;
        // The checkpoint is published. Reclaiming what it superseded can fail or be
        // interrupted without harm: the leftovers are unreachable, and are counted here.
        let reclaimed = self.reclaim_superseded(publication.heads, publication.id);
        let mut tally = self
            .reclaimed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match reclaimed {
            Ok(report) => {
                tally.removed.chunk_rows += report.chunk_rows;
                tally.removed.body_sets += report.body_sets;
                tally.removed.manifests += report.manifests;
                tally.removed.bytes += report.bytes;
            }
            Err(_) => tally.failures += 1,
        }
        Ok(manifest)
    }

    fn reclaim(&self, keep: &[(ChunkKey, ChunkRevision)]) -> Result<ReclaimReport, StoreError> {
        let latest = self.read(Self::latest_in)?.map(|row| row.manifest.id);
        let heads: std::collections::HashMap<(i32, i32, i32), u64> = self
            .heads()?
            .into_iter()
            .map(|(key, head)| (head_key(key), head.revision.value()))
            .collect();
        let keep: std::collections::HashSet<(i32, i32, i32, u64)> = keep
            .iter()
            .map(|(key, revision)| chunk_key(*key, *revision))
            .collect();
        self.write(None, |txn| {
            let mut report = ReclaimReport::default();
            {
                let mut chunks = txn.open_table(CHUNKS).map_err(io("open table"))?;
                chunks
                    .retain(|key, value| {
                        let (x, y, z, revision) = key;
                        let reachable =
                            heads.get(&(x, y, z)) == Some(&revision) || keep.contains(&key);
                        if !reachable {
                            report.chunk_rows += 1;
                            report.bytes += value.len() as u64;
                        }
                        reachable
                    })
                    .map_err(io("reclaim chunks"))?;
            }
            for (table, count) in [
                (BODIES, &mut report.body_sets),
                (CHECKPOINTS, &mut report.manifests),
            ] {
                let mut table = txn.open_table(table).map_err(io("open table"))?;
                table
                    .retain(|id, _| {
                        let reachable = Some(id) == latest.map(CheckpointId::value);
                        if !reachable {
                            *count += 1;
                        }
                        reachable
                    })
                    .map_err(io("reclaim checkpoints"))?;
            }
            Ok(report)
        })
    }

    fn begin_session(&self) -> Result<SessionStart, StoreError> {
        let marker = self.read(|txn| {
            let Some(world) = read_table(txn, WORLD)? else {
                return Ok(None);
            };
            world
                .get(SESSION_KEY)
                .map_err(io("read session marker"))?
                .map(|row| format::decode_session(row.value()))
                .transpose()
        })?;
        let previous_unclean = marker == Some(SessionMarker::Open);
        let reclaimed = if previous_unclean {
            self.reclaim(&[])?
        } else {
            ReclaimReport::default()
        };
        self.write(None, |txn| {
            txn.open_table(WORLD)
                .map_err(io("open table"))?
                .insert(
                    SESSION_KEY,
                    format::encode_session(SessionMarker::Open).as_slice(),
                )
                .map_err(io("write session marker"))?;
            Ok(())
        })?;
        Ok(SessionStart {
            previous_unclean,
            reclaimed,
        })
    }

    fn end_session(&self) -> Result<(), StoreError> {
        self.write(None, |txn| {
            txn.open_table(WORLD)
                .map_err(io("open table"))?
                .insert(
                    SESSION_KEY,
                    format::encode_session(SessionMarker::Closed).as_slice(),
                )
                .map_err(io("write session marker"))?;
            Ok(())
        })
    }
}
