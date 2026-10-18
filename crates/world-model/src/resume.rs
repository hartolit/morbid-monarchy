//! Body state a checkpoint keeps, so a reopened world resumes its bodies where they were.
//!
//! The live state of a body is on the GPU in its own packed record. A checkpoint does not save
//! that record: it saves these semantic values, which the GPU owner converts to and from its
//! layout, so a change of the device layout, its padding or its handles never changes what a
//! saved world means. Every field the body passes carry from one tick to the next is here,
//! because leaving one out would make a resumed body behave differently from one that never
//! stopped; the per-tick scratch (the coalesced cosmetic event) is not.
//!
//! The table of body slots is saved with them ([`BodySet`]): every slot's generation and
//! whether it is free, live or retired, so a handle issued before the checkpoint is still live,
//! or still stale, after it, and a reopened world never issues an old handle to a new body.

use glam::{IVec2, Vec2, Vec3};

use crate::{
    body::{BODY_RADIUS, BodyHandle, MAX_BODIES, MAX_BODY_SPEED, SphereDescription},
    material::FluidMat,
    observation::BodyCounters,
};

/// The filtered ground reference plane a rolling body presses its trail against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroundPlane {
    /// The point the plane is given around, relative to the body's anchor chunk, in cell axes.
    pub origin: Vec2,
    pub height: f32,
    /// Rise per cell along the cell x and y axes.
    pub slope: Vec2,
}

/// One live body as a checkpoint keeps it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyResume {
    pub handle: BodyHandle,
    pub description: SphereDescription,
    /// The chunk the position is anchored to, and the position inside it: cell x, cell y and
    /// height in world units. Anchoring keeps the precision of a body far from the origin.
    pub anchor: IVec2,
    pub local: Vec3,
    /// World units per second in render axes (x, up, z = minus cell y).
    pub velocity: Vec3,
    pub grounded: bool,
    /// Frozen at the residency edge; it resumes when the terrain under it is resident.
    pub suspended: bool,
    /// Whether `plane` holds a fitted ground plane.
    pub plane_valid: bool,
    /// Touching terrain at the end of the last substep; `terrain_normal` is meaningful.
    pub terrain_contact: bool,
    pub ground_normal: Vec3,
    pub submerged: f32,
    pub previous_submerged: f32,
    /// Wake material owed to the liquid surface, in elevation units.
    pub pending_wake: f32,
    pub plane: GroundPlane,
    pub counters: BodyCounters,
    /// Bodies touched in the last substep.
    pub touching: u32,
    /// The liquid at the centre column (`FluidMat::EMPTY` for none).
    pub liquid: FluidMat,
    /// The averaged terrain contact normal of the last substep, octahedral in two 16-bit snorms
    /// (x in the low half, y in the high half), as the contact pass encodes it.
    pub terrain_normal: u32,
}

/// What a body slot holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotState {
    Free,
    Live,
    /// The slot's generation is exhausted; it is never reused.
    Retired,
}

/// One body slot: its current generation and state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodySlot {
    pub generation: u32,
    pub state: SlotState,
}

/// Every body slot up to the highest one ever used, and the live bodies, in slot order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BodySet {
    pub slots: Vec<BodySlot>,
    pub bodies: Vec<BodyResume>,
}

/// Why a body set cannot be resumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodySetError {
    /// More slots than the supported body capacity, or than the world being opened holds.
    Capacity { slots: usize, capacity: u32 },
    /// A body's handle names another slot or generation than the slot table says.
    Handle { body: BodyHandle },
    /// A live slot without a body, or a body for a slot that is not live.
    Unmatched { slot: u32 },
    /// A position, velocity or plane value is not finite, or outside the supported envelope.
    Value {
        body: BodyHandle,
        field: &'static str,
    },
}

impl BodySet {
    pub fn live(&self) -> usize {
        self.bodies.len()
    }

    /// Checks the set against itself and against a world of `capacity` body slots: every live
    /// slot has exactly one body with its handle, and every value is finite and supported.
    pub fn validate(&self, capacity: u32) -> Result<(), BodySetError> {
        if self.slots.len() > capacity as usize || capacity > MAX_BODIES {
            return Err(BodySetError::Capacity {
                slots: self.slots.len(),
                capacity,
            });
        }
        let mut bodies = self.bodies.iter().peekable();
        for (index, slot) in self.slots.iter().enumerate() {
            let index = index as u32;
            let body = bodies.next_if(|body| body.handle.index() == index);
            match (slot.state, body) {
                (SlotState::Live, Some(body)) => {
                    if body.handle.generation() != slot.generation {
                        return Err(BodySetError::Handle { body: body.handle });
                    }
                    body.check_values()?;
                }
                (SlotState::Live, None) | (_, Some(_)) => {
                    return Err(BodySetError::Unmatched { slot: index });
                }
                _ => {}
            }
        }
        match bodies.next() {
            Some(body) => Err(BodySetError::Handle { body: body.handle }),
            None => Ok(()),
        }
    }
}

impl BodyResume {
    fn check_values(&self) -> Result<(), BodySetError> {
        let finite = |v: Vec3| v.is_finite();
        let fail = |field| BodySetError::Value {
            body: self.handle,
            field,
        };
        // A body anchored within one chunk of its anchor's bounds: the passes re-anchor every
        // substep, so a resumed body never lies farther out than one substep's travel.
        let reach = 64.0 + 2.0 * *BODY_RADIUS.end();
        if !finite(self.local) || self.local.x.abs() > reach || self.local.y.abs() > reach {
            return Err(fail("local"));
        }
        if !finite(self.velocity) || self.velocity.length() > MAX_BODY_SPEED * 1.001 {
            return Err(fail("velocity"));
        }
        if !finite(self.ground_normal) {
            return Err(fail("ground_normal"));
        }
        for (field, value) in [
            ("submerged", self.submerged),
            ("previous_submerged", self.previous_submerged),
        ] {
            if !(0.0..=1.0).contains(&value) {
                return Err(fail(field));
            }
        }
        if !(self.pending_wake.is_finite() && self.pending_wake >= 0.0) {
            return Err(fail("pending_wake"));
        }
        let plane = self.plane;
        if !(plane.origin.is_finite() && plane.height.is_finite() && plane.slope.is_finite()) {
            return Err(fail("plane"));
        }
        if self.liquid.0 > 7 {
            return Err(fail("liquid"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::BodyPreset;

    fn body(index: u32, generation: u32) -> BodyResume {
        BodyResume {
            handle: BodyHandle::new(index, generation),
            description: BodyPreset::Metal.description(),
            anchor: IVec2::new(-3, 2),
            local: Vec3::new(10.5, 40.25, 21.0),
            velocity: Vec3::new(1.0, -2.0, 0.5),
            grounded: true,
            suspended: false,
            plane_valid: true,
            terrain_contact: true,
            ground_normal: Vec3::Y,
            submerged: 0.25,
            previous_submerged: 0.0,
            pending_wake: 0.5,
            plane: GroundPlane {
                origin: Vec2::new(10.0, 40.0),
                height: 20.0,
                slope: Vec2::new(0.1, -0.2),
            },
            counters: BodyCounters::default(),
            touching: 1,
            liquid: FluidMat::FLUID_WATER,
            terrain_normal: 0x7FFF_0000,
        }
    }

    #[test]
    fn a_body_set_resumes_only_when_slots_and_bodies_agree() {
        let live = BodySlot {
            generation: 4,
            state: SlotState::Live,
        };
        let free = BodySlot {
            generation: 2,
            state: SlotState::Free,
        };
        let set = BodySet {
            slots: vec![free, live, live],
            bodies: vec![body(1, 4), body(2, 4)],
        };
        assert_eq!(set.validate(8), Ok(()));
        assert_eq!(set.live(), 2);
        assert_eq!(
            set.validate(2),
            Err(BodySetError::Capacity {
                slots: 3,
                capacity: 2
            })
        );
        let stale = BodySet {
            bodies: vec![body(1, 3), body(2, 4)],
            ..set.clone()
        };
        assert_eq!(
            stale.validate(8),
            Err(BodySetError::Handle {
                body: BodyHandle::new(1, 3)
            })
        );
        let missing = BodySet {
            bodies: vec![body(1, 4)],
            ..set.clone()
        };
        assert_eq!(
            missing.validate(8),
            Err(BodySetError::Unmatched { slot: 2 })
        );
        let in_free_slot = BodySet {
            bodies: vec![body(0, 2), body(1, 4), body(2, 4)],
            ..set.clone()
        };
        assert_eq!(
            in_free_slot.validate(8),
            Err(BodySetError::Unmatched { slot: 0 })
        );
        let mut fast = set.clone();
        fast.bodies[0].velocity = Vec3::new(900.0, 0.0, 0.0);
        assert_eq!(
            fast.validate(8),
            Err(BodySetError::Value {
                body: BodyHandle::new(1, 4),
                field: "velocity"
            })
        );
        let mut nan = set;
        nan.bodies[1].local.y = f32::NAN;
        assert!(matches!(
            nan.validate(8),
            Err(BodySetError::Value { field: "local", .. })
        ));
    }
}
