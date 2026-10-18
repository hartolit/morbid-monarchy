//! The fixed logical clock: how many ticks a presented frame owes.
//!
//! Elapsed real time accumulates; each whole tick interval makes one tick due. A frame runs at
//! most `max_ticks_per_frame` of them, and the backlog beyond that is discarded and counted, so
//! a stalled frame cannot trigger an unbounded catch-up. A tick that is due but cannot be
//! submitted (the device is behind) stays due and counts against the same cap on the next
//! frame. Ticks that never ran are never numbered: the logical tick advances only when one is
//! submitted.

use std::time::Duration;

use world_model::config;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClockError {
    TickRate(f64),
    MaxTicksPerFrame(u32),
}

#[derive(Debug, Clone)]
pub struct FixedClock {
    tick_rate_hz: f64,
    tick_seconds: f64,
    max_ticks_per_frame: u32,
    accumulated_seconds: f64,
    discarded_ticks: u64,
}

impl FixedClock {
    /// Refuses values outside [`config::TICK_RATE_HZ`] and [`config::MAX_TICKS_PER_FRAME`].
    pub fn new(tick_rate_hz: f64, max_ticks_per_frame: u32) -> Result<Self, ClockError> {
        if !(tick_rate_hz.is_finite() && config::TICK_RATE_HZ.contains(&tick_rate_hz)) {
            return Err(ClockError::TickRate(tick_rate_hz));
        }
        if !config::MAX_TICKS_PER_FRAME.contains(&max_ticks_per_frame) {
            return Err(ClockError::MaxTicksPerFrame(max_ticks_per_frame));
        }
        Ok(Self {
            tick_rate_hz,
            tick_seconds: 1.0 / tick_rate_hz,
            max_ticks_per_frame,
            accumulated_seconds: 0.0,
            discarded_ticks: 0,
        })
    }

    /// Changes the rate and the cap, keeping the accumulated time and the discarded count.
    /// Refused, with nothing changed, outside the supported ranges.
    pub fn reconfigure(
        &mut self,
        tick_rate_hz: f64,
        max_ticks_per_frame: u32,
    ) -> Result<(), ClockError> {
        let checked = Self::new(tick_rate_hz, max_ticks_per_frame)?;
        self.tick_rate_hz = checked.tick_rate_hz;
        self.tick_seconds = checked.tick_seconds;
        self.max_ticks_per_frame = checked.max_ticks_per_frame;
        Ok(())
    }

    pub fn max_ticks_per_frame(&self) -> u32 {
        self.max_ticks_per_frame
    }

    /// Ticks per second: what every submitted tick's duration is derived from.
    pub fn tick_rate_hz(&self) -> f64 {
        self.tick_rate_hz
    }

    pub fn tick_duration(&self) -> Duration {
        Duration::from_secs_f64(self.tick_seconds)
    }

    /// Adds a frame's elapsed time and returns the ticks this frame should run, discarding the
    /// backlog beyond the cap.
    pub fn advance(&mut self, elapsed: Duration) -> u32 {
        self.accumulated_seconds += elapsed.as_secs_f64();
        let due = (self.accumulated_seconds / self.tick_seconds).floor();
        let cap = f64::from(self.max_ticks_per_frame);
        if due > cap {
            // Truncation is exact here: `due - cap` is a whole number of ticks.
            self.discarded_ticks = self
                .discarded_ticks
                .saturating_add((due - cap).min(u64::MAX as f64) as u64);
            self.accumulated_seconds -= (due - cap) * self.tick_seconds;
            return self.max_ticks_per_frame;
        }
        due as u32
    }

    /// Consumes one due tick after it was submitted.
    pub fn consume(&mut self) {
        self.accumulated_seconds = (self.accumulated_seconds - self.tick_seconds).max(0.0);
    }

    pub fn discarded_ticks(&self) -> u64 {
        self.discarded_ticks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_intervals_become_ticks_and_the_backlog_is_capped() {
        let mut clock = FixedClock::new(60.0, 4).expect("supported");
        assert_eq!(clock.advance(Duration::from_millis(10)), 0);
        assert_eq!(clock.advance(Duration::from_millis(10)), 1);
        clock.consume();
        // A one-second stall owes 60 ticks; four run and the rest are discarded.
        assert_eq!(clock.advance(Duration::from_secs(1)), 4);
        assert_eq!(clock.discarded_ticks(), 56);
        for _ in 0..4 {
            clock.consume();
        }
        assert_eq!(clock.advance(Duration::ZERO), 0);
    }

    #[test]
    fn a_tick_that_could_not_be_submitted_stays_due() {
        let mut clock = FixedClock::new(10.0, 2).expect("supported");
        assert_eq!(clock.advance(Duration::from_millis(100)), 1);
        // Not consumed: the device was behind. It is still owed on the next frame.
        assert_eq!(clock.advance(Duration::ZERO), 1);
        clock.consume();
        assert_eq!(clock.advance(Duration::ZERO), 0);
    }

    #[test]
    fn unsupported_rates_are_refused() {
        assert_eq!(
            FixedClock::new(5.0, 4).err(),
            Some(ClockError::TickRate(5.0))
        );
        assert!(FixedClock::new(f64::NAN, 4).is_err());
        assert_eq!(
            FixedClock::new(60.0, 17).err(),
            Some(ClockError::MaxTicksPerFrame(17))
        );
    }
}
