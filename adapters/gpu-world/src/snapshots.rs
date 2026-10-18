//! Snapshots: chunks and whole worlds copied on the device at a submission boundary, then
//! streamed to the CPU through the capture staging pool.
//!
//! A snapshot is taken in two steps so that only changed state crosses to the CPU:
//!
//! ```text
//! requested -> copied (at the end of a submission: the cells into an image on the device,
//!              the slot's change word onto a flag sheet, and the word cleared)
//!           -> the sheet maps: unchanged (reported, freed, no payload)
//!                           or changed -> streaming (image -> capture staging) -> ready
//!           -> taken by the CPU, freed
//! ```
//!
//! Chunk snapshots (of outgoing chunks) use a fixed image of `snapshot_entries` chunks. A world
//! snapshot copies the whole cell array, every change word and every body slot's records into a
//! world image it allocates for itself, within the plan's `world_image_bytes`; it streams its
//! changed chunks and its bodies, and frees the image when the last of them is taken. At most
//! one world snapshot is in flight. Each submission that copies snapshots reserves one capture
//! staging buffer as its flag sheet: the world's change words first (one per slot), then one
//! word per chunk snapshot in copy order.
//!
//! The change word of a slot is set by every kernel that writes a cell value and cleared by
//! the copy that reads it, in timeline order, so "unchanged" means unchanged since the slot was
//! activated or last snapshotted. A snapshot is an obligation (`world_runtime::port`): nothing
//! here retains a payload after the CPU takes it.

use world_model::{resume::BodySlot, spatial::ChunkKey};
use world_runtime::{
    ids::{ChunkRevision, SessionId, Tick},
    port::{SnapshotTicket, WorldSnapshotTicket},
};

use crate::staging::StagingRef;

/// A chunk snapshot image entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkEntry {
    pub generation: u32,
    pub state: EntryState,
}

/// What a chunk snapshot holds about its chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Source {
    pub slot: u32,
    pub key: ChunkKey,
    pub revision: ChunkRevision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryState {
    Free,
    Requested(Source),
    /// Copied at `tick`; its change word is word `position` of `sheet`.
    Copied {
        source: Source,
        tick: Option<Tick>,
        sheet: StagingRef,
        position: u32,
    },
    /// Changed: its cells wait for a capture staging buffer.
    Changed {
        source: Source,
        tick: Option<Tick>,
    },
    Streaming {
        source: Source,
        tick: Option<Tick>,
        staging: StagingRef,
    },
    Ready {
        source: Source,
        tick: Option<Tick>,
        staging: StagingRef,
    },
}

/// A world snapshot's chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorldChunk {
    pub source: Source,
    pub stream: ChunkStream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkStream {
    /// Copied; the sheet has not been read yet.
    Copied,
    /// Reported unchanged: nothing more to do.
    Unchanged,
    Changed,
    Streaming(StagingRef),
    Ready(StagingRef),
    Taken,
}

impl ChunkStream {
    pub fn done(self) -> bool {
        matches!(self, Self::Unchanged | Self::Taken)
    }
}

/// A world snapshot's bodies: the table's slots at the copy, and the device records streaming
/// back in pieces.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyStream {
    pub slots: Vec<BodySlot>,
    /// Body slots whose records were copied: one past the highest live slot.
    pub extent: u32,
    /// The copied records, as they arrive: `extent` states, then `extent` configurations.
    pub bytes: Vec<u8>,
    /// Bytes not yet scheduled for streaming, from this offset.
    pub next: u64,
    /// Pieces in flight: the staging buffer, the offset and the length.
    pub pieces: Vec<(StagingRef, u64, u64)>,
    pub received: u64,
    pub state: BodyStreamState,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BodyStreamState {
    /// Waiting for the copy and its pieces.
    Pending,
    /// Decoded and reported; waiting for the CPU.
    Ready(world_model::resume::BodySet),
    Taken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldState {
    Requested,
    /// Copied at `tick`; its change words open `sheet`.
    Copied {
        tick: Option<Tick>,
        sheet: StagingRef,
    },
    /// The sheet was read: chunks and bodies stream.
    Decoded {
        tick: Option<Tick>,
    },
}

pub struct WorldImage {
    pub id: u32,
    /// The physical configuration of the tick the snapshot copied, set when it is copied.
    pub config: Option<world_runtime::store::PhysicalConfig>,
    pub buffer: wgpu::Buffer,
    pub bytes: u64,
    /// Bytes of the cell array copied first; the body records follow it.
    pub cell_bytes: u64,
    pub state: WorldState,
    pub chunks: Vec<WorldChunk>,
    pub bodies: BodyStream,
}

impl WorldImage {
    pub fn ticket(&self, session: SessionId) -> WorldSnapshotTicket {
        WorldSnapshotTicket::new(session, self.id)
    }

    pub fn chunk_ticket(&self, session: SessionId, slot: u32) -> SnapshotTicket {
        SnapshotTicket::new(session, self.id, slot, self.id)
    }

    /// Whether every chunk and the bodies have been reported and taken.
    pub fn drained(&self) -> bool {
        matches!(self.state, WorldState::Decoded { .. })
            && self.chunks.iter().all(|chunk| chunk.stream.done())
            && self.bodies.state == BodyStreamState::Taken
    }
}

/// A flag sheet in flight: a capture staging buffer that receives the change words of one
/// submission's snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sheet {
    pub staging: StagingRef,
    /// The world snapshot whose change words open the sheet, if any.
    pub world: Option<u32>,
}

/// Every snapshot the session holds.
pub struct Snapshots {
    pub image: wgpu::Buffer,
    pub entries: Vec<ChunkEntry>,
    pub world: Option<WorldImage>,
    pub next_world: u32,
    pub sheets: Vec<Sheet>,
}

impl Snapshots {
    pub fn new(image: wgpu::Buffer, entries: u32) -> Self {
        Self {
            image,
            entries: vec![
                ChunkEntry {
                    generation: 0,
                    state: EntryState::Free,
                };
                entries as usize
            ],
            world: None,
            next_world: 1,
            sheets: Vec::new(),
        }
    }

    /// Takes a free chunk image entry for `source`.
    pub fn reserve(&mut self, session: SessionId, source: Source) -> Option<SnapshotTicket> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.state == EntryState::Free)?;
        let entry = &mut self.entries[index];
        entry.generation = entry.generation.wrapping_add(1);
        entry.state = EntryState::Requested(source);
        Some(SnapshotTicket::new(
            session,
            0,
            index as u32,
            entry.generation,
        ))
    }

    pub fn entry_ticket(&self, session: SessionId, index: usize) -> SnapshotTicket {
        SnapshotTicket::new(session, 0, index as u32, self.entries[index].generation)
    }

    /// The chunk entry a ticket names, if it is still that entry's.
    pub fn entry_index(&self, ticket: SnapshotTicket) -> Option<usize> {
        let index = ticket.entry() as usize;
        (ticket.image() == 0
            && self
                .entries
                .get(index)
                .is_some_and(|entry| entry.generation == ticket.generation()))
        .then_some(index)
    }

    /// Whether a requested snapshot names a slot (so the slots must not move yet).
    pub fn has_requested(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| matches!(entry.state, EntryState::Requested(_)))
            || self
                .world
                .as_ref()
                .is_some_and(|world| world.state == WorldState::Requested)
    }

    /// Whether anything waits for a submission: a requested copy, or a changed chunk or body
    /// piece to stream.
    pub fn has_pending_copies(&self) -> bool {
        self.has_requested()
            || self
                .entries
                .iter()
                .any(|entry| matches!(entry.state, EntryState::Changed { .. }))
            || self.world.as_ref().is_some_and(|world| {
                matches!(world.state, WorldState::Decoded { .. })
                    && (world
                        .chunks
                        .iter()
                        .any(|chunk| chunk.stream == ChunkStream::Changed)
                        || world.bodies.next < world.bodies.bytes.len() as u64)
            })
    }

    /// Whether a copy or a stream is in flight on the device.
    pub fn in_flight(&self) -> bool {
        !self.sheets.is_empty()
            || self
                .entries
                .iter()
                .any(|entry| matches!(entry.state, EntryState::Streaming { .. }))
            || self.world.as_ref().is_some_and(|world| {
                world
                    .chunks
                    .iter()
                    .any(|chunk| matches!(chunk.stream, ChunkStream::Streaming(_)))
                    || !world.bodies.pieces.is_empty()
            })
    }
}
