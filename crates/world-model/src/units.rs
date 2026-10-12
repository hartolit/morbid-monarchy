//! Physical units and the fixed body integration step.
//!
//! Lengths are world units. One world unit is the horizontal spacing of cell vertices, so a cell
//! is one unit wide. Heights are elevation units (integers in a cell) times
//! [`crate::config::DEFAULT_ELEVATION_SCALE`] or the configured scale. Time is in seconds, and
//! the simulation advances in whole ticks of `1 / tick_rate` seconds
//! ([`crate::config::TICK_RATE_HZ`]).

/// Vertical acceleration of every body, in world units per second squared (render `-y`).
pub const GRAVITY_Y: f32 = -32.0;

/// Longest body integration step, in seconds. A longer interval is split into equal substeps,
/// so contact response and damping do not depend on how time is divided.
pub const MAX_BODY_SUBSTEP_SECONDS: f32 = 1.0 / 120.0;
