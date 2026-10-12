//! Supported ranges and defaults of the settings a user, a tool or a scenario can change.
//!
//! Every consumer (the simulation defaults, the developer UI's sliders, scenario validation and
//! later the runtime's configuration commands) reads these definitions instead of keeping its
//! own copy. A range is the supported product limit, not a hardware limit: a request outside it
//! is refused with a reason, never silently clamped.

use std::ops::RangeInclusive;

/// Fixed simulation rate, in ticks per second.
pub const TICK_RATE_HZ: RangeInclusive<f64> = 10.0..=240.0;
pub const DEFAULT_TICK_RATE_HZ: f64 = 60.0;

/// Ticks a single presented frame may run to catch up before the backlog is discarded.
pub const MAX_TICKS_PER_FRAME: RangeInclusive<u32> = 1..=16;
pub const DEFAULT_MAX_TICKS_PER_FRAME: u32 = 4;

/// Ambient air temperature, in degrees Celsius. Water freezes below 0 °C.
pub const AMBIENT_TEMPERATURE_C: RangeInclusive<f32> = -30.0..=40.0;
pub const DEFAULT_AMBIENT_TEMPERATURE_C: f32 = 12.0;

/// World units per elevation unit: the vertical size of one unit of bedrock, loose material or
/// liquid.
///
/// This is physical configuration, not a display setting: bodies collide with, float in and
/// deform the scaled surface, and the renderer draws the same surface. Changing it takes effect
/// at a tick boundary and is part of the configuration a checkpoint records; the cells store
/// integer units and do not change.
pub const ELEVATION_SCALE: RangeInclusive<f32> = 0.01..=5.0;
pub const DEFAULT_ELEVATION_SCALE: f32 = 0.5;

/// Chunks kept active on each side of the focus chunk, per axis.
pub const ACTIVE_RADIUS_CHUNKS: RangeInclusive<u32> = 1..=32;
pub const DEFAULT_ACTIVE_RADIUS_CHUNKS: u32 = 6;

/// Radius of a material brush, in cells. Radius 64 touches at most 12,853 cells per stroke.
pub const BRUSH_RADIUS_CELLS: RangeInclusive<i32> = 0..=64;
pub const DEFAULT_BRUSH_RADIUS_CELLS: i32 = 5;

/// Units a brush adds or removes per cell per stroke.
pub const BRUSH_STRENGTH: RangeInclusive<u8> = 1..=255;
pub const DEFAULT_BRUSH_STRENGTH: u8 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_inside_their_ranges() {
        assert!(TICK_RATE_HZ.contains(&DEFAULT_TICK_RATE_HZ));
        assert!(MAX_TICKS_PER_FRAME.contains(&DEFAULT_MAX_TICKS_PER_FRAME));
        assert!(AMBIENT_TEMPERATURE_C.contains(&DEFAULT_AMBIENT_TEMPERATURE_C));
        assert!(ELEVATION_SCALE.contains(&DEFAULT_ELEVATION_SCALE));
        assert!(ACTIVE_RADIUS_CHUNKS.contains(&DEFAULT_ACTIVE_RADIUS_CHUNKS));
        assert!(BRUSH_RADIUS_CELLS.contains(&DEFAULT_BRUSH_RADIUS_CELLS));
        assert!(BRUSH_STRENGTH.contains(&DEFAULT_BRUSH_STRENGTH));
    }

    #[test]
    fn largest_brush_touches_the_documented_cell_count() {
        let radius = *BRUSH_RADIUS_CELLS.end();
        let cells = (-radius..=radius)
            .flat_map(|dy| (-radius..=radius).map(move |dx| (dx, dy)))
            .filter(|(dx, dy)| dx * dx + dy * dy <= radius * radius)
            .count();
        assert_eq!(cells, 12_853);
    }
}
