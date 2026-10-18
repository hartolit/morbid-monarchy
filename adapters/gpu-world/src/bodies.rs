//! Body slots: which body indices are in use, under which generation.
//!
//! The device holds every body's physical state; this table holds only identity. A spawn takes
//! the lowest free slot when its tick is encoded and its handle is the slot's current
//! generation; a removal frees the slot and advances its generation at once, so every later
//! command naming the old handle is answered `StaleBody` without reaching the device, and a
//! delayed sample of it cannot be mistaken for the slot's next body. The kernels check the
//! generation as well: the device's copy of a slot is the authority on whether a handle is
//! alive (a spawn whose target missed leaves its slot dead until its outcome frees it here).
//!
//! A slot whose generation reached `u32::MAX` is never reused, so a generation cannot wrap
//! into an old handle. The table's extent (one past the highest slot in use) bounds every
//! per-body dispatch; a world without bodies dispatches none.

use world_model::body::BodyHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Free,
    Live,
    /// Its generation is exhausted.
    Retired,
}

#[derive(Debug, Clone)]
pub struct BodyTable {
    slots: Vec<Slot>,
    generations: Vec<u32>,
    live: u32,
}

/// Why a body command was answered without reaching the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyRefusal {
    Stale,
    Capacity,
}

impl BodyTable {
    pub fn new(capacity: u32) -> Self {
        Self {
            slots: vec![Slot::Free; capacity as usize],
            generations: vec![0; capacity as usize],
            live: 0,
        }
    }

    pub fn capacity(&self) -> u32 {
        self.slots.len() as u32
    }

    pub fn live(&self) -> u32 {
        self.live
    }

    /// One past the highest slot in use.
    pub fn extent(&self) -> u32 {
        self.slots
            .iter()
            .rposition(|slot| *slot == Slot::Live)
            .map_or(0, |index| index as u32 + 1)
    }

    pub fn is_live(&self, body: BodyHandle) -> bool {
        let index = body.index() as usize;
        index < self.slots.len()
            && self.slots[index] == Slot::Live
            && self.generations[index] == body.generation()
    }

    /// Takes the lowest free slot.
    pub fn allocate(&mut self) -> Result<BodyHandle, BodyRefusal> {
        let index = self
            .slots
            .iter()
            .position(|slot| *slot == Slot::Free)
            .ok_or(BodyRefusal::Capacity)?;
        self.slots[index] = Slot::Live;
        self.live += 1;
        Ok(BodyHandle::new(index as u32, self.generations[index]))
    }

    /// Frees a live body's slot and advances its generation.
    pub fn release(&mut self, body: BodyHandle) -> Result<(), BodyRefusal> {
        if !self.is_live(body) {
            return Err(BodyRefusal::Stale);
        }
        let index = body.index() as usize;
        self.live -= 1;
        match self.generations[index].checked_add(1) {
            Some(next) => {
                self.generations[index] = next;
                self.slots[index] = Slot::Free;
            }
            None => self.slots[index] = Slot::Retired,
        }
        Ok(())
    }

    /// Every live handle, in slot order.
    pub fn handles(&self) -> impl Iterator<Item = BodyHandle> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| **slot == Slot::Live)
            .map(|(index, _)| BodyHandle::new(index as u32, self.generations[index]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_are_reused_lowest_first_under_a_new_generation() {
        let mut table = BodyTable::new(3);
        let a = table.allocate().expect("room");
        let b = table.allocate().expect("room");
        assert_eq!((a, b), (BodyHandle::new(0, 0), BodyHandle::new(1, 0)));
        assert_eq!(table.extent(), 2);
        table.release(a).expect("live");
        assert_eq!(table.release(a), Err(BodyRefusal::Stale), "released once");
        assert!(!table.is_live(a));
        let c = table.allocate().expect("room");
        assert_eq!(c, BodyHandle::new(0, 1));
        assert!(!table.is_live(a), "the old handle stays stale after reuse");
        table.release(b).expect("live");
        assert_eq!(table.extent(), 1);
        assert_eq!(table.live(), 1);
    }

    #[test]
    fn a_full_table_refuses_and_an_exhausted_generation_retires_its_slot() {
        let mut table = BodyTable::new(1);
        let only = table.allocate().expect("room");
        assert_eq!(table.allocate(), Err(BodyRefusal::Capacity));
        table.generations[0] = u32::MAX;
        let last = BodyHandle::new(only.index(), u32::MAX);
        table.release(last).expect("live under its last generation");
        assert_eq!(table.allocate(), Err(BodyRefusal::Capacity));
        assert_eq!(table.extent(), 0);
    }
}
