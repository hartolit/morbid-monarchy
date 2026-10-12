//! Results the physical world reports back.
//!
//! An observation is a copy of state at a completed tick. It is never written back as
//! authority: a gameplay decision that needs the current physical state belongs inside the
//! simulation, not in a loop that reads observations and issues corrections.

use crate::body::BodyHandle;

/// One body's state after logical tick `tick`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyObservation {
    pub body: BodyHandle,
    /// The completed tick this state belongs to. Its age is the current tick minus this.
    pub tick: u64,
    /// Centre in render space (y up; cell `y` is render `-z`), world units.
    pub position: [f32; 3],
    /// World units per second, render axes.
    pub velocity: [f32; 3],
    pub grounded: bool,
    /// Share of the volume below a liquid surface, from 0 to 1.
    pub submerged_fraction: f32,
}

impl BodyObservation {
    pub fn speed(&self) -> f32 {
        let [x, y, z] = self.velocity;
        (x * x + y * y + z * z).sqrt()
    }
}
