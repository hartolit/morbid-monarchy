//! Chunk slots and the resident window map: metadata only, never cell contents.
//!
//! The cell buffer holds one chunk per slot. A slot moves through
//!
//! ```text
//! free -> reserved -> staged (upload written) -> activating (in a submitted tick) -> resident
//! resident -> retire requested -> retiring (in a submitted tick) -> free
//! ```
//!
//! Reserving a slot advances its generation, so a [`SlotRef`] names one occupancy and an older
//! reference is stale. A retiring slot becomes free only after the submission that cleared it
//! has completed, so no earlier GPU use can see a recycled slot. The window map entry of a
//! chunk names its slot from the tick that activates it until the tick that retires it, and is
//! `SLOT_NONE` otherwise: a chunk that is not resident is a closed boundary for every kernel.

use world_model::spatial::ChunkKey;
use world_runtime::{
    ids::{ChunkRevision, SessionId, Tick},
    port::{PayloadError, Pressure, Refusal, ResidentWindow, SlotRef},
};

use crate::{layout::codes, staging::StagingRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Free,
    Reserved {
        key: ChunkKey,
    },
    Staged {
        key: ChunkKey,
        revision: ChunkRevision,
        upload: StagingRef,
    },
    Activating {
        key: ChunkKey,
        revision: ChunkRevision,
        serial: u64,
        tick: Tick,
    },
    Resident {
        key: ChunkKey,
        revision: ChunkRevision,
    },
    RetireRequested {
        key: ChunkKey,
    },
    Retiring {
        serial: u64,
    },
}

#[derive(Debug, Clone, Copy)]
struct Slot {
    generation: u32,
    state: State,
}

/// A change to a slot record and its window entry, applied by a tick's copies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Patch {
    pub slot: u32,
    pub window_index: u32,
    /// The slot's new record.
    pub record: crate::layout::ChunkSlotRecord,
    /// The window entry's new value: the slot, or `SLOT_NONE`.
    pub window_entry: u32,
}

/// A staged upload that the next tick copies into its slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StagedUpload {
    pub slot: SlotRef,
    pub key: ChunkKey,
    pub revision: ChunkRevision,
    pub upload: StagingRef,
}

/// A resident chunk a resize keeps: its key and its slot before and after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Move {
    pub key: ChunkKey,
    pub from: u32,
    pub to: u32,
}

/// An upload that became resident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Activated {
    pub slot: SlotRef,
    pub key: ChunkKey,
    pub revision: ChunkRevision,
    pub tick: Tick,
}

pub struct SlotTable {
    session: SessionId,
    window: ResidentWindow,
    slots: Vec<Slot>,
    /// Per window chunk: the slot that holds or will hold it, or `SLOT_NONE`.
    claimed: Vec<u32>,
    patch_capacity: usize,
    /// Patches requested and not yet encoded: retirements and staged activations.
    pending_patches: usize,
}

impl SlotTable {
    pub fn new(session: SessionId, window: ResidentWindow, patch_capacity: u32) -> Self {
        let count = window.chunk_count() as usize;
        Self {
            session,
            window,
            slots: vec![
                Slot {
                    generation: 0,
                    state: State::Free,
                };
                count
            ],
            claimed: vec![codes::SLOT_NONE; count],
            patch_capacity: patch_capacity as usize,
            pending_patches: 0,
        }
    }

    pub fn window(&self) -> ResidentWindow {
        self.window
    }

    pub fn capacity(&self) -> u32 {
        self.slots.len() as u32
    }

    /// The window index of `key`, row-major from the window's lowest key.
    pub fn window_index(&self, key: ChunkKey) -> Option<u32> {
        if !self.window.contains(key) {
            return None;
        }
        let min = self.window.min_key();
        let x = (i64::from(key.key.x) - i64::from(min.key.x)) as u32;
        let y = (i64::from(key.key.y) - i64::from(min.key.y)) as u32;
        Some(y * self.window.width() + x)
    }

    fn slot_ref(&self, index: usize) -> SlotRef {
        SlotRef::new(self.session, index as u32, self.slots[index].generation)
    }

    fn check(&self, slot: SlotRef) -> Result<usize, Refusal> {
        if slot.session() != self.session {
            return Err(Refusal::StaleSession {
                current: self.session,
                presented: slot.session(),
            });
        }
        let index = slot.index() as usize;
        match self.slots.get(index) {
            Some(entry) if entry.generation == slot.generation() => Ok(index),
            _ => Err(Refusal::StaleSlot(slot)),
        }
    }

    pub fn reserve(&mut self, key: ChunkKey) -> Result<SlotRef, Refusal> {
        let position = self
            .window_index(key)
            .ok_or(Refusal::InvalidPayload(PayloadError::KeyOutsideWindow(key)))?;
        if self.claimed[position as usize] != codes::SLOT_NONE {
            return Err(Refusal::InvalidPayload(PayloadError::KeyAlreadyResident(
                key,
            )));
        }
        let index = self
            .slots
            .iter()
            .position(|slot| slot.state == State::Free)
            .ok_or(Refusal::Pressure(Pressure::Slots))?;
        let slot = &mut self.slots[index];
        slot.generation = slot.generation.wrapping_add(1);
        slot.state = State::Reserved { key };
        self.claimed[position as usize] = index as u32;
        Ok(self.slot_ref(index))
    }

    /// Validates an upload into `slot` before its payload is staged.
    pub fn check_upload(&self, slot: SlotRef) -> Result<ChunkKey, Refusal> {
        let index = self.check(slot)?;
        match self.slots[index].state {
            State::Reserved { key } if self.pending_patches < self.patch_capacity => Ok(key),
            State::Reserved { .. } => Err(Refusal::Pressure(Pressure::SlotChanges)),
            _ => Err(Refusal::InvalidPayload(PayloadError::SlotState(slot))),
        }
    }

    pub fn stage(&mut self, slot: SlotRef, revision: ChunkRevision, upload: StagingRef) {
        if let Ok(index) = self.check(slot)
            && let State::Reserved { key } = self.slots[index].state
        {
            self.slots[index].state = State::Staged {
                key,
                revision,
                upload,
            };
            self.pending_patches += 1;
        }
    }

    /// Retires a resident or reserved slot. A reserved slot is freed at once; a resident one
    /// is cleared by the next tick and freed when that tick completes. Either way the slot's
    /// generation advances now, so the retired reference is stale from this call on.
    pub fn retire(&mut self, slot: SlotRef) -> Result<(), Refusal> {
        let index = self.check(slot)?;
        match self.slots[index].state {
            State::Reserved { key } => {
                self.unclaim(key);
                self.slots[index].state = State::Free;
            }
            State::Resident { key, .. } => {
                if self.pending_patches >= self.patch_capacity {
                    return Err(Refusal::Pressure(Pressure::SlotChanges));
                }
                self.unclaim(key);
                self.slots[index].state = State::RetireRequested { key };
                self.pending_patches += 1;
            }
            _ => return Err(Refusal::InvalidPayload(PayloadError::SlotState(slot))),
        }
        let entry = &mut self.slots[index];
        entry.generation = entry.generation.wrapping_add(1);
        Ok(())
    }

    fn unclaim(&mut self, key: ChunkKey) {
        if let Some(position) = self.window_index(key) {
            self.claimed[position as usize] = codes::SLOT_NONE;
        }
    }

    /// The slot and identity a capture of `slot` copies from: resident, or activating with the
    /// next tick at the latest.
    pub fn capture_source(&self, slot: SlotRef) -> Result<(ChunkKey, ChunkRevision), Refusal> {
        let index = self.check(slot)?;
        match self.slots[index].state {
            State::Staged { key, revision, .. }
            | State::Activating { key, revision, .. }
            | State::Resident { key, revision } => Ok((key, revision)),
            _ => Err(Refusal::InvalidPayload(PayloadError::SlotState(slot))),
        }
    }

    /// Takes the staged uploads and requested retirements for the tick being encoded, and
    /// writes their patches into `patches` (retirements first, so a key can move to another
    /// slot within one tick).
    pub fn encode(
        &mut self,
        serial: u64,
        tick: Tick,
        uploads: &mut Vec<StagedUpload>,
        patches: &mut Vec<Patch>,
    ) {
        for pass in [false, true] {
            for index in 0..self.slots.len() {
                let generation = self.slots[index].generation;
                let state = self.slots[index].state;
                match (pass, state) {
                    (false, State::RetireRequested { key }) => {
                        if let Some(window_index) = self.window_index(key) {
                            patches.push(Patch {
                                slot: index as u32,
                                window_index,
                                record: slot_record(
                                    key,
                                    generation,
                                    ChunkRevision::default(),
                                    codes::SLOT_FREE,
                                ),
                                window_entry: codes::SLOT_NONE,
                            });
                        }
                        self.slots[index].state = State::Retiring { serial };
                    }
                    (
                        true,
                        State::Staged {
                            key,
                            revision,
                            upload,
                        },
                    ) => {
                        if let Some(window_index) = self.window_index(key) {
                            patches.push(Patch {
                                slot: index as u32,
                                window_index,
                                record: slot_record(
                                    key,
                                    generation,
                                    revision,
                                    codes::SLOT_RESIDENT,
                                ),
                                window_entry: index as u32,
                            });
                        }
                        uploads.push(StagedUpload {
                            slot: self.slot_ref(index),
                            key,
                            revision,
                            upload,
                        });
                        self.slots[index].state = State::Activating {
                            key,
                            revision,
                            serial,
                            tick,
                        };
                    }
                    _ => {}
                }
            }
        }
        self.pending_patches = 0;
    }

    /// Finishes slots whose tick completed: activations become resident (reported in
    /// `activated`), retirements free their slot.
    pub fn completed(&mut self, completed_serial: u64, activated: &mut Vec<Activated>) {
        for index in 0..self.slots.len() {
            match self.slots[index].state {
                State::Activating {
                    key,
                    revision,
                    serial,
                    tick,
                } if serial <= completed_serial => {
                    self.slots[index].state = State::Resident { key, revision };
                    activated.push(Activated {
                        slot: self.slot_ref(index),
                        key,
                        revision,
                        tick,
                    });
                }
                State::Retiring { serial } if serial <= completed_serial => {
                    self.slots[index].state = State::Free;
                }
                _ => {}
            }
        }
    }

    /// Staged uploads that no tick took; used when the session ends.
    pub fn staged(&self) -> impl Iterator<Item = StagedUpload> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| match slot.state {
                State::Staged {
                    key,
                    revision,
                    upload,
                } => Some(StagedUpload {
                    slot: SlotRef::new(self.session, index as u32, slot.generation),
                    key,
                    revision,
                    upload,
                }),
                _ => None,
            })
    }

    /// Activations still waiting for their tick to complete; used when the session ends.
    pub fn activating(&self) -> impl Iterator<Item = (SlotRef, ChunkKey, ChunkRevision)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| match slot.state {
                State::Activating { key, revision, .. } => Some((
                    SlotRef::new(self.session, index as u32, slot.generation),
                    key,
                    revision,
                )),
                _ => None,
            })
    }

    /// Returns staged uploads that no tick will carry to the reserved state, appending them to
    /// `into`; used when admission closes.
    pub fn cancel_staged(&mut self, into: &mut Vec<StagedUpload>) {
        for index in 0..self.slots.len() {
            if let State::Staged {
                key,
                revision,
                upload,
            } = self.slots[index].state
            {
                into.push(StagedUpload {
                    slot: self.slot_ref(index),
                    key,
                    revision,
                    upload,
                });
                self.slots[index].state = State::Reserved { key };
                self.pending_patches = self.pending_patches.saturating_sub(1);
            }
        }
    }

    /// Whether a staged, activating or retiring change is outstanding; each finishes with the
    /// next completed tick.
    pub fn has_pending_changes(&self) -> bool {
        self.slots.iter().any(|slot| {
            matches!(
                slot.state,
                State::Staged { .. } | State::Activating { .. } | State::RetireRequested { .. }
            )
        })
    }

    fn reserved(&self) -> u32 {
        self.slots
            .iter()
            .filter(|slot| matches!(slot.state, State::Reserved { .. }))
            .count() as u32
    }

    /// Resident chunks, by key, slot and revision.
    pub fn resident(&self) -> impl Iterator<Item = (ChunkKey, u32, ChunkRevision)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| match slot.state {
                State::Resident { key, revision } => Some((key, index as u32, revision)),
                _ => None,
            })
    }

    /// A table for `window` holding the resident chunks of `self` that lie inside it, packed
    /// into the lowest slots, with generations beyond any `self` issued so no old reference
    /// names a new occupancy. Returns the table and the moves of the kept chunks.
    pub fn resized(&self, window: ResidentWindow) -> Result<(Self, Vec<Move>), Refusal> {
        if self.has_pending_changes() {
            return Err(Refusal::Pressure(Pressure::SlotChanges));
        }
        let reserved = self.reserved();
        if reserved > 0 {
            return Err(Refusal::InvalidPayload(
                PayloadError::UnfilledReservations { count: reserved },
            ));
        }
        let outside = self
            .resident()
            .filter(|(key, ..)| !window.contains(*key))
            .count() as u32;
        if outside > 0 {
            return Err(Refusal::InvalidPayload(
                PayloadError::ResidentOutsideNewWindow { count: outside },
            ));
        }
        let floor = self
            .slots
            .iter()
            .map(|slot| slot.generation)
            .max()
            .unwrap_or(0);
        let mut table = Self::new(self.session, window, self.patch_capacity as u32);
        for slot in &mut table.slots {
            slot.generation = floor;
        }
        let mut moves = Vec::new();
        for (new_index, (key, old_index, revision)) in self.resident().enumerate() {
            let slot = &mut table.slots[new_index];
            slot.generation = floor.wrapping_add(1);
            slot.state = State::Resident { key, revision };
            if let Some(position) = table.window_index(key) {
                table.claimed[position as usize] = new_index as u32;
            }
            moves.push(Move {
                key,
                from: old_index,
                to: new_index as u32,
            });
        }
        Ok((table, moves))
    }

    /// The window map and slot records `self` describes, for a freshly created buffer.
    pub fn write_window_map(&self, out: &mut [u32]) {
        out.fill(codes::SLOT_NONE);
        for (key, slot, _) in self.resident() {
            if let Some(position) = self.window_index(key) {
                out[position as usize] = slot;
            }
        }
    }

    pub fn slot_records(&self) -> impl Iterator<Item = crate::layout::ChunkSlotRecord> + '_ {
        self.slots.iter().map(|slot| match slot.state {
            State::Resident { key, revision } => {
                slot_record(key, slot.generation, revision, codes::SLOT_RESIDENT)
            }
            _ => slot_record(
                ChunkKey::default(),
                slot.generation,
                ChunkRevision::default(),
                codes::SLOT_FREE,
            ),
        })
    }

    pub fn slot_ref_for(&self, index: u32) -> SlotRef {
        self.slot_ref(index as usize)
    }
}

fn slot_record(
    key: ChunkKey,
    generation: u32,
    revision: ChunkRevision,
    state: u32,
) -> crate::layout::ChunkSlotRecord {
    let [revision_lo, revision_hi] = revision.to_words();
    crate::layout::ChunkSlotRecord {
        key_x: key.key.x,
        key_y: key.key.y,
        key_z: key.key.z,
        generation,
        revision_lo,
        revision_hi,
        state,
        reserved0: 0,
    }
}

#[cfg(test)]
mod tests {
    use world_runtime::ids::{SessionEpoch, WorldId};

    use super::*;

    fn session() -> SessionId {
        SessionId {
            world: WorldId::new(1),
            epoch: SessionEpoch::FIRST,
        }
    }

    fn table() -> SlotTable {
        let window = ResidentWindow::new(ChunkKey::new(0, 0, 0), 1, 1).expect("window");
        SlotTable::new(session(), window, 4)
    }

    const UPLOAD: StagingRef = StagingRef {
        index: 0,
        generation: 1,
    };

    #[test]
    fn a_slot_becomes_resident_only_when_its_tick_completes() {
        let mut slots = table();
        let key = ChunkKey::new(-1, 1, 0);
        let slot = slots.reserve(key).expect("free slot");
        assert_eq!(
            slots.reserve(key),
            Err(Refusal::InvalidPayload(PayloadError::KeyAlreadyResident(
                key
            )))
        );
        slots.check_upload(slot).expect("reserved");
        slots.stage(slot, ChunkRevision::new(3), UPLOAD);
        let (mut uploads, mut patches) = (Vec::new(), Vec::new());
        slots.encode(7, Tick::new(2), &mut uploads, &mut patches);
        assert_eq!(uploads.len(), 1);
        assert_eq!(patches[0].window_entry, slot.index());
        assert_eq!(
            patches[0].window_index, 6,
            "row 2, column 0 of a 3 x 3 window"
        );
        let mut activated = Vec::new();
        slots.completed(6, &mut activated);
        assert!(activated.is_empty(), "serial 7 has not completed");
        slots.completed(7, &mut activated);
        assert_eq!(activated[0].slot, slot);
        assert_eq!(activated[0].tick, Tick::new(2));
        assert_eq!(slots.resident().count(), 1);
    }

    #[test]
    fn retired_slots_are_reused_only_after_their_tick_and_old_references_are_stale() {
        let mut slots = table();
        let key = ChunkKey::new(0, 0, 0);
        let slot = slots.reserve(key).expect("slot");
        slots.stage(slot, ChunkRevision::new(1), UPLOAD);
        let (mut uploads, mut patches, mut activated) = (Vec::new(), Vec::new(), Vec::new());
        slots.encode(1, Tick::new(0), &mut uploads, &mut patches);
        slots.completed(1, &mut activated);
        slots.retire(slot).expect("resident");
        patches.clear();
        slots.encode(2, Tick::new(1), &mut uploads, &mut patches);
        assert_eq!(patches[0].window_entry, codes::SLOT_NONE);
        // Eight more keys fill every other slot; the retiring one is not free yet.
        for (x, y) in [
            (-1, -1),
            (0, -1),
            (1, -1),
            (-1, 0),
            (1, 0),
            (-1, 1),
            (0, 1),
            (1, 1),
        ] {
            slots.reserve(ChunkKey::new(x, y, 0)).expect("free slot");
        }
        assert_eq!(slots.reserve(key), Err(Refusal::Pressure(Pressure::Slots)));
        slots.completed(2, &mut activated);
        let reused = slots.reserve(key).expect("freed after its tick");
        assert_eq!(reused.index(), slot.index());
        assert_ne!(reused.generation(), slot.generation());
        assert_eq!(slots.retire(slot), Err(Refusal::StaleSlot(slot)));
        assert_eq!(slots.capture_source(slot), Err(Refusal::StaleSlot(slot)));
        let other = SessionId {
            world: WorldId::new(1),
            epoch: SessionEpoch::new(2),
        };
        let foreign = SlotRef::new(other, reused.index(), reused.generation());
        assert!(matches!(
            slots.retire(foreign),
            Err(Refusal::StaleSession { .. })
        ));
    }

    #[test]
    fn a_resize_keeps_resident_chunks_inside_and_refuses_to_drop_others() {
        let mut slots = table();
        let inside = ChunkKey::new(1, 1, 0);
        let slot = slots.reserve(inside).expect("slot");
        slots.stage(slot, ChunkRevision::new(4), UPLOAD);
        let (mut uploads, mut patches, mut activated) = (Vec::new(), Vec::new(), Vec::new());
        slots.encode(1, Tick::new(0), &mut uploads, &mut patches);
        assert_eq!(
            slots
                .resized(ResidentWindow::new(ChunkKey::new(0, 0, 0), 2, 2).expect("window"))
                .err(),
            Some(Refusal::Pressure(Pressure::SlotChanges)),
            "an activation in flight blocks a resize"
        );
        slots.completed(1, &mut activated);
        let smaller = ResidentWindow::new(ChunkKey::new(-1, -1, 0), 1, 1).expect("window");
        assert_eq!(
            slots.resized(smaller).err(),
            Some(Refusal::InvalidPayload(
                PayloadError::ResidentOutsideNewWindow { count: 1 }
            ))
        );
        let larger = ResidentWindow::new(ChunkKey::new(0, 0, 0), 2, 2).expect("window");
        let (resized, moves) = slots.resized(larger).expect("fits");
        assert_eq!(
            moves,
            vec![Move {
                key: inside,
                from: slot.index(),
                to: 0
            }]
        );
        let new_ref = resized.slot_ref_for(0);
        assert!(new_ref.generation() > slot.generation());
        let mut map = vec![0; resized.capacity() as usize];
        resized.write_window_map(&mut map);
        assert_eq!(
            map.iter()
                .filter(|entry| **entry != codes::SLOT_NONE)
                .count(),
            1
        );
        assert_eq!(map[(3 * 5 + 3) as usize], 0);
    }
}
