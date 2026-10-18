//! The legacy baseline's saved chunks, and their import into this format.
//!
//! The baseline wrote one redb table, `spatial_chunks`, keyed by `[i32; 3]`, whose values are
//! the derived bitcode encoding of `Chunk { cells: Box<[WorldCell]>, metadata: { last_simulated:
//! f64, theme: u8 } }`. This module owns those bytes: it mirrors that shape, newtypes included,
//! in private record types, so the old writer's fixtures decode and re-encode byte for byte. The
//! baseline application reads and writes through [`LegacyStore`] until 06 removes it; nothing
//! else writes this format.
//!
//! [`import`] reads a legacy file without changing it and writes a new world store beside it.
//! A legacy file records no seed or elevation scale, so the caller supplies them and the new
//! header records them as that interpretation. A row that does not decode fails the import, and
//! so does a row with a nonzero `z` unless the caller asked to leave such rows out; nothing is
//! ever generated in place of a saved row. The new store is published (made a complete world)
//! only after every row was written and read back equal; an import that stops earlier leaves an
//! incomplete store that opening refuses.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use bitcode::{Decode, Encode};
use redb::{
    Builder, Database, ReadOnlyDatabase, ReadTransaction, ReadableDatabase, ReadableTable,
    TableDefinition, TableError,
};
use world_model::{
    cell::WorldCell, generation::ChunkTheme, identity, spatial::CHUNK_CELL_COUNT, spatial::ChunkKey,
};
use world_runtime::{
    ids::{ChunkRevision, SessionEpoch, WorldId},
    port::ResidentWindow,
    store::{
        CheckpointId, ChunkPayload, ChunkRow, Generator, LegacyInterpretation, PhysicalConfig,
        Publication, StoreError, WorldHeader, WorldOrigin, WorldStore,
    },
};

use crate::{faults::StoreFaults, store::RedbWorldStore};

/// The baseline's table.
pub const TABLE: TableDefinition<[i32; 3], &[u8]> = TableDefinition::new("spatial_chunks");

#[derive(Encode, Decode)]
struct RecordCell(u64);

#[derive(Encode, Decode)]
struct RecordTheme(u8);

#[derive(Encode, Decode)]
struct RecordMetadata {
    last_simulated: f64,
    theme: RecordTheme,
}

#[derive(Encode, Decode)]
struct RecordChunk {
    cells: Box<[RecordCell]>,
    metadata: RecordMetadata,
}

/// One legacy row's content.
#[derive(Debug, Clone, PartialEq)]
pub struct LegacyChunk {
    /// The cells as stored; a valid row holds [`CHUNK_CELL_COUNT`].
    pub cells: Box<[WorldCell]>,
    /// Session-relative seconds at the time of saving; not a portable clock.
    pub last_simulated: f64,
    pub theme: ChunkTheme,
}

/// Encodes a chunk exactly as the baseline writer did.
pub fn encode(chunk: &LegacyChunk) -> Vec<u8> {
    bitcode::encode(&RecordChunk {
        cells: chunk.cells.iter().map(|cell| RecordCell(cell.0)).collect(),
        metadata: RecordMetadata {
            last_simulated: chunk.last_simulated,
            theme: RecordTheme(chunk.theme.0),
        },
    })
}

/// Decodes a row. An error means the row is corrupt or not in this format, never absent.
pub fn decode(bytes: &[u8]) -> Result<LegacyChunk, bitcode::Error> {
    let record: RecordChunk = bitcode::decode(bytes)?;
    Ok(LegacyChunk {
        cells: record.cells.iter().map(|cell| WorldCell(cell.0)).collect(),
        last_simulated: record.metadata.last_simulated,
        theme: ChunkTheme(record.metadata.theme.0),
    })
}

/// A failure of the legacy store, with the read cases kept apart.
#[derive(Debug)]
pub enum LegacyError {
    Database(String),
    /// The row exists and does not decode.
    Corrupt {
        key: ChunkKey,
        detail: String,
    },
}

impl std::fmt::Display for LegacyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(detail) => write!(f, "legacy store: {detail}"),
            Self::Corrupt { key, detail } => write!(f, "legacy row {key:?} is corrupt: {detail}"),
        }
    }
}

impl std::error::Error for LegacyError {}

fn database<E: std::fmt::Display>(error: E) -> LegacyError {
    LegacyError::Database(error.to_string())
}

/// The baseline application's store: `save.redb` with `spatial_chunks`.
pub struct LegacyStore {
    db: Arc<Database>,
}

impl LegacyStore {
    /// Opens or creates the file and its table, as the baseline did at startup.
    pub fn open_or_create(path: &Path) -> Result<Self, LegacyError> {
        let db = Database::create(path).map_err(database)?;
        let txn = db.begin_write().map_err(database)?;
        txn.open_table(TABLE).map_err(database)?;
        txn.commit().map_err(database)?;
        Ok(Self { db: Arc::new(db) })
    }

    /// Reads one row: `Ok(None)` only when the key is absent.
    pub fn read_chunk(&self, key: ChunkKey) -> Result<Option<LegacyChunk>, LegacyError> {
        let txn = self.db.begin_read().map_err(database)?;
        let table = txn.open_table(TABLE).map_err(database)?;
        let Some(row) = table
            .get([key.key.x, key.key.y, key.key.z])
            .map_err(database)?
        else {
            return Ok(None);
        };
        decode(row.value())
            .map(Some)
            .map_err(|error| LegacyError::Corrupt {
                key,
                detail: error.to_string(),
            })
    }

    /// Writes rows in one transaction, as the baseline's save batches did.
    pub fn write_batch(&self, rows: &[(ChunkKey, &[u8])]) -> Result<(), LegacyError> {
        if rows.is_empty() {
            return Ok(());
        }
        let txn = self.db.begin_write().map_err(database)?;
        {
            let mut table = txn.open_table(TABLE).map_err(database)?;
            for (key, bytes) in rows {
                table
                    .insert([key.key.x, key.key.y, key.key.z], *bytes)
                    .map_err(database)?;
            }
        }
        txn.commit().map_err(database)
    }
}

/// What to do with legacy rows whose key has `z != 0`, which the planar world cannot hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonPlanarRows {
    /// Fail the import; nothing is published.
    Refuse,
    /// Leave them out (they stay in the original) and count them in the report.
    Skip,
}

/// What the person importing a legacy world supplies.
#[derive(Debug, Clone, Copy)]
pub struct ImportOptions {
    pub world: WorldId,
    /// The seed the legacy world is taken to have been generated with (the baseline used 69).
    pub seed: u32,
    pub rule_seed: u64,
    pub config: PhysicalConfig,
    /// Where the imported world opens.
    pub window: ResidentWindow,
    pub non_planar: NonPlanarRows,
    /// Rows per destination transaction.
    pub rows_per_transaction: usize,
}

/// What an import did.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportReport {
    pub destination: PathBuf,
    pub interpretation: LegacyInterpretation,
    /// The original needed redb's repair, so a copy of it was repaired and read instead.
    pub read_repaired_copy: Option<PathBuf>,
    /// Keys left out because their `z` is not 0.
    pub refused_keys: Vec<ChunkKey>,
    /// The range of the rows' session-relative save times, which the new format does not keep.
    pub last_simulated: Option<(f64, f64)>,
    pub checkpoint: CheckpointId,
}

#[derive(Debug)]
pub enum ImportError {
    /// The destination already exists; an import never writes into an existing file.
    DestinationExists(PathBuf),
    /// The original could not be opened or read.
    Source(String),
    /// The original has no `spatial_chunks` table.
    NotLegacy,
    /// Rows that do not decode, or do not hold a whole chunk. The original is unchanged and
    /// nothing was published.
    CorruptRows(Vec<(ChunkKey, String)>),
    /// Rows with `z != 0` under [`NonPlanarRows::Refuse`].
    NonPlanarRows(Vec<ChunkKey>),
    /// Writing or checking the destination failed; it is left incomplete, and opening it is
    /// refused.
    Destination(StoreError),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DestinationExists(path) => {
                write!(f, "{} exists; import writes a new store", path.display())
            }
            Self::Source(detail) => write!(f, "reading the legacy store: {detail}"),
            Self::NotLegacy => write!(f, "no `spatial_chunks` table: not a legacy store"),
            Self::CorruptRows(rows) => write!(f, "{} legacy rows do not decode", rows.len()),
            Self::NonPlanarRows(keys) => write!(f, "{} legacy rows have z != 0", keys.len()),
            Self::Destination(error) => write!(f, "writing the new store: {error}"),
        }
    }
}

impl std::error::Error for ImportError {}

enum Source {
    Original(ReadOnlyDatabase),
    Copy(Database),
}

impl Source {
    fn begin_read(&self) -> Result<ReadTransaction, ImportError> {
        match self {
            Self::Original(db) => db.begin_read(),
            Self::Copy(db) => db.begin_read(),
        }
        .map_err(|error| ImportError::Source(error.to_string()))
    }
}

/// Reads the legacy store at `source` and writes a new world store at `destination`.
pub fn import(
    source: &Path,
    destination: &Path,
    options: &ImportOptions,
    faults: StoreFaults,
) -> Result<ImportReport, ImportError> {
    if destination.exists() {
        return Err(ImportError::DestinationExists(destination.to_owned()));
    }
    // Open the original read-only. If it was not closed cleanly, redb must repair it, which
    // writes; the original is never repaired, a copy of it is.
    let (database, repaired) = match Builder::new().open_read_only(source) {
        Ok(db) => (Source::Original(db), None),
        Err(redb::DatabaseError::RepairAborted) => {
            let copy = destination.with_extension("legacy-repair-copy");
            if copy.exists() {
                return Err(ImportError::DestinationExists(copy));
            }
            fs::copy(source, &copy).map_err(|error| ImportError::Source(error.to_string()))?;
            let db =
                Database::open(&copy).map_err(|error| ImportError::Source(error.to_string()))?;
            (Source::Copy(db), Some(copy))
        }
        Err(error) => return Err(ImportError::Source(error.to_string())),
    };

    // Decode and validate every row before writing anything.
    let txn = database.begin_read()?;
    let table = match txn.open_table(TABLE) {
        Ok(table) => table,
        Err(TableError::TableDoesNotExist(_)) => return Err(ImportError::NotLegacy),
        Err(error) => return Err(ImportError::Source(error.to_string())),
    };
    let mut keys = Vec::new();
    let mut corrupt = Vec::new();
    let mut refused = Vec::new();
    let mut times: Option<(f64, f64)> = None;
    for entry in table
        .iter()
        .map_err(|error| ImportError::Source(error.to_string()))?
    {
        let (key, value) = entry.map_err(|error| ImportError::Source(error.to_string()))?;
        let [x, y, z] = key.value();
        let key = ChunkKey::new(x, y, z);
        match decode(value.value()) {
            Err(error) => corrupt.push((key, error.to_string())),
            Ok(chunk) if chunk.cells.len() != CHUNK_CELL_COUNT => corrupt.push((
                key,
                format!("{} cells instead of {CHUNK_CELL_COUNT}", chunk.cells.len()),
            )),
            Ok(_) if z != 0 => refused.push(key),
            Ok(chunk) => {
                let t = chunk.last_simulated;
                times = Some(times.map_or((t, t), |(low, high)| (low.min(t), high.max(t))));
                keys.push(key);
            }
        }
    }
    if !corrupt.is_empty() {
        return Err(ImportError::CorruptRows(corrupt));
    }
    if !refused.is_empty() && options.non_planar == NonPlanarRows::Refuse {
        return Err(ImportError::NonPlanarRows(refused));
    }

    let interpretation = LegacyInterpretation {
        seed: options.seed,
        elevation_scale: options.config.elevation_scale,
        rows_imported: keys.len() as u64,
        rows_refused: refused.len() as u64,
    };
    let header = WorldHeader {
        world: options.world,
        generator: Generator {
            revision: identity::GENERATOR_REVISION,
            seed: options.seed,
        },
        rule_seed: options.rule_seed,
        origin: WorldOrigin::LegacyImport(interpretation),
    };
    let store = RedbWorldStore::create_unpublished(destination, &header, faults)
        .map_err(ImportError::Destination)?;
    let revision = ChunkRevision::new(1);
    let batch = options.rows_per_transaction.max(1);
    let mut payloads = Vec::with_capacity(batch);
    for group in keys.chunks(batch) {
        payloads.clear();
        for key in group {
            let row = read_row(&table, *key)?;
            let payload = ChunkPayload::new(row.cells, row.theme)
                .map_err(|_| ImportError::CorruptRows(vec![(*key, "wrong length".to_owned())]))?;
            payloads.push((*key, payload));
        }
        let rows: Vec<ChunkRow<'_>> = payloads
            .iter()
            .map(|(key, payload)| ChunkRow {
                key: *key,
                revision,
                payload,
            })
            .collect();
        store
            .write_chunks(&rows)
            .map_err(ImportError::Destination)?;
    }
    // Every written row must read back equal before the store becomes a world.
    let mut read_back = ChunkPayload::zeroed();
    for key in &keys {
        let original = read_row(&table, *key)?;
        let found = store
            .read_chunk(*key, Some(revision), &mut read_back)
            .map_err(ImportError::Destination)?;
        if found != Some(revision)
            || read_back.cells() != &original.cells[..]
            || read_back.theme() != original.theme
        {
            return Err(ImportError::Destination(StoreError::Corrupt {
                item: world_runtime::store::StoredItem::Chunk(*key, revision),
                detail: "an imported row did not read back equal",
            }));
        }
    }
    drop(table);
    drop(txn);
    let heads: Vec<(ChunkKey, ChunkRevision)> = keys.iter().map(|key| (*key, revision)).collect();
    store
        .write_bodies(CheckpointId::FIRST, &Default::default())
        .map_err(ImportError::Destination)?;
    store
        .publish(&Publication {
            id: CheckpointId::FIRST,
            tick: None,
            config: options.config,
            window: options.window,
            epoch: SessionEpoch::FIRST,
            heads: &heads,
            bodies: 0,
        })
        .map_err(ImportError::Destination)?;
    Ok(ImportReport {
        destination: destination.to_owned(),
        interpretation,
        read_repaired_copy: repaired,
        refused_keys: refused,
        last_simulated: times,
        checkpoint: CheckpointId::FIRST,
    })
}

fn read_row(
    table: &redb::ReadOnlyTable<[i32; 3], &[u8]>,
    key: ChunkKey,
) -> Result<LegacyChunk, ImportError> {
    let row = table
        .get([key.key.x, key.key.y, key.key.z])
        .map_err(|error| ImportError::Source(error.to_string()))?
        .ok_or_else(|| ImportError::Source(format!("row {key:?} vanished while importing")))?;
    decode(row.value()).map_err(|error| ImportError::CorruptRows(vec![(key, error.to_string())]))
}
