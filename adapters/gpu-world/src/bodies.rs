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

use world_model::{
    body::{BodyHandle, SphereDescription},
    glam::{IVec2, Vec2, Vec3},
    material::FluidMat,
    observation::BodyCounters,
    resume::{BodyResume, BodySlot, GroundPlane, SlotState},
};

use crate::layout::{BodyConfig, BodyState, codes};

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

    /// Every slot up to the highest one ever used (live, freed or retired), as a checkpoint
    /// keeps them; slots beyond were never used and need no record.
    pub fn export(&self) -> Vec<BodySlot> {
        let used = self
            .slots
            .iter()
            .zip(&self.generations)
            .rposition(|(slot, generation)| *slot != Slot::Free || *generation != 0)
            .map_or(0, |index| index + 1);
        (0..used)
            .map(|index| BodySlot {
                generation: self.generations[index],
                state: match self.slots[index] {
                    Slot::Free => SlotState::Free,
                    Slot::Live => SlotState::Live,
                    Slot::Retired => SlotState::Retired,
                },
            })
            .collect()
    }

    /// A table holding `slots` (a checkpoint's), with every slot beyond them free at
    /// generation 0. `None` when `slots` does not fit the capacity.
    pub fn restored(capacity: u32, slots: &[BodySlot]) -> Option<Self> {
        if slots.len() > capacity as usize {
            return None;
        }
        let mut table = Self::new(capacity);
        for (index, slot) in slots.iter().enumerate() {
            table.generations[index] = slot.generation;
            table.slots[index] = match slot.state {
                SlotState::Free => Slot::Free,
                SlotState::Live => {
                    table.live += 1;
                    Slot::Live
                }
                SlotState::Retired => Slot::Retired,
            };
        }
        Some(table)
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

/// Why a device body record cannot be read as a resumable body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyRecordError {
    /// The record is not alive, or holds another generation than the table's.
    NotLive,
    /// Its configuration is outside the supported envelope.
    Description,
}

/// The semantic body a device record holds, for a checkpoint.
pub fn resume_from_device(
    handle: BodyHandle,
    state: &BodyState,
    config: &BodyConfig,
) -> Result<BodyResume, BodyRecordError> {
    if state.flags & codes::BODY_ALIVE == 0 || state.generation != handle.generation() {
        return Err(BodyRecordError::NotLive);
    }
    let description = SphereDescription::new(config.mass, config.radius, config.relative_density)
        .map_err(|_| BodyRecordError::Description)?;
    Ok(BodyResume {
        handle,
        description,
        anchor: IVec2::new(state.anchor_x, state.anchor_y),
        local: Vec3::new(state.local_x, state.local_y, state.height),
        velocity: Vec3::new(state.velocity_x, state.velocity_y, state.velocity_z),
        grounded: state.flags & codes::BODY_GROUNDED != 0,
        suspended: state.flags & codes::BODY_SUSPENDED != 0,
        plane_valid: state.flags & codes::BODY_PLANE_VALID != 0,
        terrain_contact: state.flags & codes::BODY_TERRAIN != 0,
        ground_normal: Vec3::new(state.normal_x, state.normal_y, state.normal_z),
        submerged: state.submerged,
        previous_submerged: state.previous_submerged,
        pending_wake: state.pending_wake,
        plane: GroundPlane {
            origin: Vec2::new(state.plane_origin_x, state.plane_origin_y),
            height: state.plane_height,
            slope: Vec2::new(state.plane_slope_x, state.plane_slope_y),
        },
        counters: BodyCounters {
            landings: state.landings,
            liquid_entries: state.liquid_entries,
            body_contacts: state.body_contacts,
            craters: state.craters,
            splashes: state.splashes,
            absorbed_energy: state.absorbed_energy,
        },
        touching: state.touching,
        liquid: FluidMat(state.liquid.min(u32::from(u8::MAX)) as u8),
        terrain_normal: state.terrain_normal,
    })
}

/// The device records that resume `body`. The tick's coalesced event starts empty, as every
/// tick's first substep leaves it.
pub fn device_from_resume(body: &BodyResume) -> (BodyState, BodyConfig) {
    let flags = codes::BODY_ALIVE
        | if body.grounded {
            codes::BODY_GROUNDED
        } else {
            0
        }
        | if body.suspended {
            codes::BODY_SUSPENDED
        } else {
            0
        }
        | if body.plane_valid {
            codes::BODY_PLANE_VALID
        } else {
            0
        }
        | if body.terrain_contact {
            codes::BODY_TERRAIN
        } else {
            0
        };
    let state = BodyState {
        anchor_x: body.anchor.x,
        anchor_y: body.anchor.y,
        local_x: body.local.x,
        local_y: body.local.y,
        height: body.local.z,
        velocity_x: body.velocity.x,
        velocity_y: body.velocity.y,
        velocity_z: body.velocity.z,
        flags,
        generation: body.handle.generation(),
        normal_x: body.ground_normal.x,
        normal_y: body.ground_normal.y,
        normal_z: body.ground_normal.z,
        submerged: body.submerged,
        previous_submerged: body.previous_submerged,
        pending_wake: body.pending_wake,
        plane_origin_x: body.plane.origin.x,
        plane_origin_y: body.plane.origin.y,
        plane_height: body.plane.height,
        plane_slope_x: body.plane.slope.x,
        plane_slope_y: body.plane.slope.y,
        landings: body.counters.landings,
        liquid_entries: body.counters.liquid_entries,
        body_contacts: body.counters.body_contacts,
        craters: body.counters.craters,
        splashes: body.counters.splashes,
        absorbed_energy: body.counters.absorbed_energy,
        touching: body.touching,
        liquid: u32::from(body.liquid.0),
        tick_events: 0,
        tick_impact: 0.0,
        terrain_normal: body.terrain_normal,
    };
    let description = body.description;
    let config = BodyConfig {
        mass: description.mass(),
        radius: description.radius(),
        relative_density: description.relative_density(),
        inverse_mass: 1.0 / description.mass(),
    };
    (state, config)
}

#[cfg(test)]
mod tests {
    use world_model::body::BodyPreset;

    use super::*;

    #[test]
    fn a_table_exports_the_slots_it_used_and_restores_them_exactly() {
        let mut table = BodyTable::new(6);
        let a = table.allocate().expect("room");
        let b = table.allocate().expect("room");
        let _c = table.allocate().expect("room");
        table.release(a).expect("live");
        table.release(b).expect("live");
        let b2 = table.allocate().expect("room");
        assert_eq!(b2, BodyHandle::new(0, 1));
        let slots = table.export();
        assert_eq!(
            slots
                .iter()
                .map(|slot| (slot.generation, slot.state))
                .collect::<Vec<_>>(),
            vec![
                (1, SlotState::Live),
                (1, SlotState::Free),
                (0, SlotState::Live)
            ]
        );
        let restored = BodyTable::restored(6, &slots).expect("fits");
        assert_eq!(restored.export(), slots);
        assert_eq!((restored.live(), restored.extent()), (2, 3));
        assert!(restored.is_live(b2) && !restored.is_live(b) && !restored.is_live(a));
        assert!(BodyTable::restored(2, &slots).is_none());
        let mut next = restored;
        assert_eq!(
            next.allocate(),
            Ok(BodyHandle::new(1, 1)),
            "a freed slot is reused under its advanced generation, never an old handle"
        );
    }

    #[test]
    fn device_records_round_trip_through_the_semantic_body() {
        let body = BodyResume {
            handle: BodyHandle::new(3, 7),
            description: BodyPreset::Metal.description(),
            anchor: IVec2::new(-5, 9),
            local: Vec3::new(12.25, 40.5, 19.0),
            velocity: Vec3::new(3.0, -1.5, 0.25),
            grounded: true,
            suspended: true,
            plane_valid: false,
            terrain_contact: true,
            ground_normal: Vec3::new(0.0, 0.8, 0.6),
            submerged: 0.4,
            previous_submerged: 0.3,
            pending_wake: 0.75,
            plane: GroundPlane {
                origin: Vec2::new(12.0, 40.0),
                height: 18.5,
                slope: Vec2::new(-0.25, 0.5),
            },
            counters: BodyCounters {
                landings: 9,
                liquid_entries: 8,
                body_contacts: 7,
                craters: 6,
                splashes: 5,
                absorbed_energy: 4,
            },
            touching: 3,
            liquid: FluidMat::FLUID_WATER,
            terrain_normal: 0x8001_7FFF,
        };
        let (state, config) = device_from_resume(&body);
        assert_eq!(
            state.flags,
            codes::BODY_ALIVE | codes::BODY_GROUNDED | codes::BODY_SUSPENDED | codes::BODY_TERRAIN
        );
        assert_eq!(resume_from_device(body.handle, &state, &config), Ok(body));
        assert_eq!(
            resume_from_device(BodyHandle::new(3, 8), &state, &config),
            Err(BodyRecordError::NotLive),
            "another generation is not this body"
        );
        let dead = BodyState { flags: 0, ..state };
        assert_eq!(
            resume_from_device(body.handle, &dead, &config),
            Err(BodyRecordError::NotLive)
        );
    }

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
