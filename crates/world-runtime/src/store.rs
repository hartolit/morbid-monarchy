//! The storage port: durable chunk revisions, body sets and published checkpoints.
//!
//! The residency and checkpoint owners decide what must be kept and when; a store makes it
//! durable. `world-store` implements this port over redb, and the composition root chooses it.
//! The port names no database type, so the owners' rules are independent of the mechanism.
//!
//! A saved world is a fixed [`WorldHeader`] and a sequence of published checkpoints. Chunk
//! payloads are immutable revisions written ahead of time in bounded transactions; a checkpoint
//! becomes visible only when [`WorldStore::publish`] atomically points every key it changes at
//! its new revision and records the [`CheckpointManifest`]. A reader therefore sees the previous
//! complete checkpoint or the new complete one, never a partial one. Revisions written but not
//! yet published are reachable only through the session that wrote them, by exact revision.
//!
//! Every operation can block on I/O, so the owners call the store from their workers, never
//! from a frame. Every failure is typed: a missing key is not an error but a corrupt, unsupported
//! or unreadable one is, and none of them may be answered by generating terrain.

use std::fmt;

use world_model::{
    cell::WorldCell,
    command::{AmbientTemperature, ElevationScale},
    generation::ChunkTheme,
    resume::BodySet,
    rules::SimulationRules,
    spatial::{CHUNK_CELL_COUNT, ChunkKey},
};

use crate::{
    ids::{ChunkRevision, SessionEpoch, Tick, WorldId},
    port::ResidentWindow,
};

/// A published checkpoint's identity: 1 for a world's first, one more for each after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CheckpointId(u64);

impl CheckpointId {
    pub const FIRST: Self = Self(1);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    pub const fn next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

impl fmt::Display for CheckpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "checkpoint {}", self.0)
    }
}

/// One chunk's saved content: exactly [`CHUNK_CELL_COUNT`] cells, row-major, and its theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkPayload {
    cells: Box<[WorldCell]>,
    theme: ChunkTheme,
}

/// A payload of the wrong length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayloadLength {
    pub presented: usize,
}

impl ChunkPayload {
    pub fn new(cells: Box<[WorldCell]>, theme: ChunkTheme) -> Result<Self, PayloadLength> {
        if cells.len() != CHUNK_CELL_COUNT {
            return Err(PayloadLength {
                presented: cells.len(),
            });
        }
        Ok(Self { cells, theme })
    }

    /// An all-zero payload, for a buffer that a read or a capture fills.
    pub fn zeroed() -> Self {
        Self {
            cells: vec![WorldCell::default(); CHUNK_CELL_COUNT].into_boxed_slice(),
            theme: ChunkTheme::default(),
        }
    }

    pub fn cells(&self) -> &[WorldCell] {
        &self.cells
    }

    /// Exactly [`CHUNK_CELL_COUNT`] cells; the length cannot change.
    pub fn cells_mut(&mut self) -> &mut [WorldCell] {
        &mut self.cells
    }

    pub fn theme(&self) -> ChunkTheme {
        self.theme
    }

    pub fn set_theme(&mut self, theme: ChunkTheme) {
        self.theme = theme;
    }

    /// Bytes this payload holds, for budgets.
    pub const BYTES: u64 = (CHUNK_CELL_COUNT * std::mem::size_of::<WorldCell>()) as u64;
}

/// How terrain is generated where nothing was saved: `world_model::generation` at a revision
/// (`world_model::identity::GENERATOR_REVISION`) with a seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Generator {
    pub revision: u32,
    pub seed: u32,
}

/// How an import read a legacy store that recorded no world metadata: what the person importing
/// it supplied, and what it found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LegacyInterpretation {
    /// The generator seed the legacy world is taken to have used for chunks it never saved.
    pub seed: u32,
    /// The elevation scale it is taken to have been played at.
    pub elevation_scale: ElevationScale,
    pub rows_imported: u64,
    /// Rows with a nonzero `z`, which the planar world never writes; preserved in the original.
    pub rows_refused: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WorldOrigin {
    /// Created empty by this format.
    Created,
    /// Imported from a store written by the legacy baseline.
    LegacyImport(LegacyInterpretation),
}

/// The fixed identity of a saved world. The store also records and checks the format, cell
/// layout and physics revisions it was written under; a world whose revisions differ from this
/// build's is refused when it is opened, so they do not appear here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldHeader {
    pub world: WorldId,
    pub generator: Generator,
    /// Keys every pseudo-random choice of the rules (`world_model::rules::RuleTick`).
    pub rule_seed: u64,
    pub origin: WorldOrigin,
}

/// The physical configuration a world runs with: recorded in every checkpoint, because a world
/// resumed with another configuration is a different physical world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicalConfig {
    pub rules: SimulationRules,
    pub ambient_temperature: AmbientTemperature,
    pub elevation_scale: ElevationScale,
}

/// A published checkpoint: the world as it was after one completed tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CheckpointManifest {
    pub id: CheckpointId,
    /// The completed tick the checkpoint holds. `None` for a world that has never run (created
    /// or imported): it resumes at tick 0.
    pub tick: Option<Tick>,
    pub config: PhysicalConfig,
    /// The resident window at that tick, where a reopened world starts.
    pub window: ResidentWindow,
    /// Keys with a saved revision after this checkpoint.
    pub chunks: u64,
    /// Live bodies in its body set.
    pub bodies: u32,
    /// The session that captured it; informational.
    pub epoch: SessionEpoch,
}

impl CheckpointManifest {
    /// The tick a world resumed from this checkpoint runs first.
    pub fn next_tick(&self) -> Option<Tick> {
        match self.tick {
            None => Some(Tick::ZERO),
            Some(tick) => tick.next(),
        }
    }
}

/// A chunk revision to make durable.
#[derive(Debug, Clone, Copy)]
pub struct ChunkRow<'a> {
    pub key: ChunkKey,
    pub revision: ChunkRevision,
    pub payload: &'a ChunkPayload,
}

/// A checkpoint to publish. Its chunk revisions and its body set must already be durable.
#[derive(Debug, Clone, Copy)]
pub struct Publication<'a> {
    /// One more than the latest published checkpoint's.
    pub id: CheckpointId,
    pub tick: Option<Tick>,
    pub config: PhysicalConfig,
    pub window: ResidentWindow,
    pub epoch: SessionEpoch,
    /// Every key whose newest revision at the checkpoint's tick is newer than its published
    /// one, with that revision. Each must be durable and newer than the key's current head.
    pub heads: &'a [(ChunkKey, ChunkRevision)],
    /// Live bodies in the body set written for `id`.
    pub bodies: u32,
}

/// What a reclamation removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReclaimReport {
    pub chunk_rows: u64,
    pub body_sets: u64,
    pub manifests: u64,
    pub bytes: u64,
}

/// What a session found when it began writing to a store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionStart {
    /// The previous writing session did not close the store: it may have left revisions that
    /// no checkpoint published, which were reclaimed before this session writes any.
    pub previous_unclean: bool,
    pub reclaimed: ReclaimReport,
}

/// Something the store holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredItem {
    Header,
    /// The pointer to the latest published checkpoint.
    Current,
    Manifest(CheckpointId),
    /// A key's published revision.
    Head(ChunkKey),
    Chunk(ChunkKey, ChunkRevision),
    Bodies(CheckpointId),
}

/// Why stored data is from something this build does not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsupported {
    /// Another row format version: newer data is refused without being modified.
    FormatVersion {
        stored: u16,
        supported: u16,
    },
    CellLayout {
        stored: u64,
        supported: u64,
    },
    Physics {
        stored: (u32, u64),
        supported: (u32, u64),
    },
    Generator {
        stored: u32,
        supported: u32,
    },
}

/// A write that would break the store's revision rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    /// A different payload already exists under this key and revision.
    RevisionExists {
        key: ChunkKey,
        revision: ChunkRevision,
    },
    /// A publication would move a key's head to a revision that is not newer than its current
    /// one: an older save finishing after a newer one.
    HeadNotNewer {
        key: ChunkKey,
        head: ChunkRevision,
        presented: ChunkRevision,
    },
    /// A publication names a revision that was never made durable.
    MissingRevision {
        key: ChunkKey,
        revision: ChunkRevision,
    },
    /// A publication names no durable body set.
    MissingBodies(CheckpointId),
    /// Checkpoints are published in order.
    CheckpointOrder {
        expected: CheckpointId,
        presented: CheckpointId,
    },
    /// The key is outside the planar world (`z != 0`) or the revision is 0, which means
    /// "generated, never saved".
    InvalidRow {
        key: ChunkKey,
        revision: ChunkRevision,
    },
}

/// Why a store operation failed. Nothing was changed by a failed write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The bytes under an existing key do not decode: a failed checksum, an impossible length,
    /// an invalid value. The bytes are left as they are.
    Corrupt {
        item: StoredItem,
        detail: &'static str,
    },
    /// The data was written by a format or revision this build does not read.
    Unsupported {
        item: StoredItem,
        detail: Unsupported,
    },
    /// The database, or the device under it, failed.
    Io {
        operation: &'static str,
        detail: String,
    },
    Conflict(Conflict),
    /// The store holds no complete checkpoint: never created, or an interrupted import.
    Incomplete,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Corrupt { item, detail } => write!(f, "corrupt {item:?}: {detail}"),
            Self::Unsupported { item, detail } => write!(f, "unsupported {item:?}: {detail:?}"),
            Self::Io { operation, detail } => write!(f, "{operation} failed: {detail}"),
            Self::Conflict(conflict) => write!(f, "refused write: {conflict:?}"),
            Self::Incomplete => write!(f, "the store holds no complete checkpoint"),
        }
    }
}

impl std::error::Error for StoreError {}

/// The owner of a world's durable data, as the residency and checkpoint owners see it.
///
/// Every method may block on I/O; the owners call them from worker threads.
pub trait WorldStore: Send + Sync {
    fn header(&self) -> &WorldHeader;

    /// The latest published checkpoint.
    fn latest(&self) -> Result<CheckpointManifest, StoreError>;

    /// Reads a chunk into `into`: the published revision when `revision` is `None`, or that
    /// exact durable revision. `Ok(None)` means the key has no published revision (generation
    /// is permitted) or the exact revision does not exist (it is not).
    fn read_chunk(
        &self,
        key: ChunkKey,
        revision: Option<ChunkRevision>,
        into: &mut ChunkPayload,
    ) -> Result<Option<ChunkRevision>, StoreError>;

    /// The body set of a published checkpoint.
    fn read_bodies(&self, checkpoint: CheckpointId) -> Result<BodySet, StoreError>;

    /// Makes `rows` durable in one transaction: all of them or none.
    fn write_chunks(&self, rows: &[ChunkRow<'_>]) -> Result<(), StoreError>;

    /// Makes a body set durable for the checkpoint `checkpoint` will publish.
    fn write_bodies(&self, checkpoint: CheckpointId, bodies: &BodySet) -> Result<(), StoreError>;

    /// Publishes a checkpoint atomically: its heads, its manifest and the latest pointer
    /// change together or not at all.
    fn publish(&self, publication: &Publication<'_>) -> Result<CheckpointManifest, StoreError>;

    /// Removes what no published checkpoint reaches and `keep` does not name: superseded
    /// revisions, unpublished revisions of an earlier session, older body sets and manifests.
    /// `keep` lists the live session's unpublished revisions.
    fn reclaim(&self, keep: &[(ChunkKey, ChunkRevision)]) -> Result<ReclaimReport, StoreError>;

    /// Starts a writing session. When the previous one did not end, this reclaims what it left
    /// unpublished first, so revisions this session writes cannot meet them.
    fn begin_session(&self) -> Result<SessionStart, StoreError>;

    /// Records that the session ended in order: everything it needed is published.
    fn end_session(&self) -> Result<(), StoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payloads_hold_whole_chunks_and_checkpoints_resume_after_their_tick() {
        assert!(ChunkPayload::new(vec![WorldCell::default(); 7].into(), ChunkTheme(0)).is_err());
        let payload = ChunkPayload::zeroed();
        assert_eq!(payload.cells().len(), CHUNK_CELL_COUNT);
        assert_eq!(ChunkPayload::BYTES, 32 * 1024);
        let window = ResidentWindow::new(ChunkKey::new(0, 0, 0), 1, 1).expect("window");
        let config = PhysicalConfig {
            rules: SimulationRules::ALL,
            ambient_temperature: AmbientTemperature::celsius(20.0).expect("supported"),
            elevation_scale: ElevationScale::DEFAULT,
        };
        let mut manifest = CheckpointManifest {
            id: CheckpointId::FIRST,
            tick: None,
            config,
            window,
            chunks: 0,
            bodies: 0,
            epoch: SessionEpoch::FIRST,
        };
        assert_eq!(manifest.next_tick(), Some(Tick::ZERO));
        manifest.tick = Some(Tick::new(41));
        assert_eq!(manifest.next_tick(), Some(Tick::new(42)));
        manifest.tick = Some(Tick::new(u64::MAX));
        assert_eq!(manifest.next_tick(), None);
        assert_eq!(CheckpointId::new(u64::MAX).next(), None);
    }
}
