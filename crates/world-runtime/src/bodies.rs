//! The CPU consumer of body observations: which bodies exist, what happened to them, and how
//! old that knowledge is.
//!
//! The live world owns every body's physical state. The CPU learns about a body three ways:
//! its spawn outcome, the samples the runtime asks for each tick, and its removal outcome.
//! None of them is a writable pose; the monitor keeps the latest sample per body as an
//! observation of a completed tick.
//!
//! **Important outcomes cannot be lost.** A body's interaction counters (landings, liquid
//! entries, body contacts, craters, splashes, absorbed energy) live on the device and only
//! grow. The monitor remembers, per body, the counters it has already turned into events (its
//! watermark) and emits only the difference, so a delayed, reordered or skipped sample neither
//! repeats an event nor drops one: the next sample, or the removal outcome, carries the rest.
//! Samples are keyed by handle (index and generation), so a sample of a removed body or of a
//! reused slot's earlier generation is counted and ignored.
//!
//! **Notifications are bounded.** Events wait in a ring for the consumer; when it is full the
//! oldest is overwritten and counted, which is the pressure signal. The counters a lost
//! notification described stay in [`Tracked::counters`].
//!
//! **Observation is selected.** One tick samples a bounded number of bodies: the controlled
//! body every tick, the others in turn ([`BodyMonitor::select`]).

use std::collections::VecDeque;

use world_model::{
    body::BodyHandle,
    glam::DVec3,
    observation::{BodyCounters, BodySample},
};

use crate::{ids::Tick, ledger::Series};

/// What the monitor knows about one body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tracked {
    pub body: BodyHandle,
    /// Where and when the spawn outcome placed it, once that outcome arrived.
    pub spawned: Option<(Tick, DVec3)>,
    /// The newest sample applied.
    pub last: Option<BodySample>,
    /// Counters already turned into events: the watermark.
    pub counters: BodyCounters,
}

/// Something that happened to a body, derived from its counters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BodyEvent {
    Spawned {
        body: BodyHandle,
        tick: Tick,
        position: DVec3,
    },
    /// `count` landings happened by `tick` since the last report.
    Landed {
        body: BodyHandle,
        tick: Tick,
        count: u32,
    },
    EnteredLiquid {
        body: BodyHandle,
        tick: Tick,
        count: u32,
    },
    TouchedBody {
        body: BodyHandle,
        tick: Tick,
        count: u32,
    },
    Cratered {
        body: BodyHandle,
        tick: Tick,
        count: u32,
    },
    Splashed {
        body: BodyHandle,
        tick: Tick,
        count: u32,
    },
    Removed {
        body: BodyHandle,
        tick: Tick,
        counters: BodyCounters,
    },
}

impl BodyEvent {
    pub fn body(&self) -> BodyHandle {
        match *self {
            Self::Spawned { body, .. }
            | Self::Landed { body, .. }
            | Self::EnteredLiquid { body, .. }
            | Self::TouchedBody { body, .. }
            | Self::Cratered { body, .. }
            | Self::Splashed { body, .. }
            | Self::Removed { body, .. } => body,
        }
    }

    pub fn tick(&self) -> Tick {
        match *self {
            Self::Spawned { tick, .. }
            | Self::Landed { tick, .. }
            | Self::EnteredLiquid { tick, .. }
            | Self::TouchedBody { tick, .. }
            | Self::Cratered { tick, .. }
            | Self::Splashed { tick, .. }
            | Self::Removed { tick, .. } => tick,
        }
    }
}

/// The monitor's own accounting, for displays and reports.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BodySummary {
    pub tracked: u32,
    pub samples: u64,
    /// Samples not newer than one already applied for the same body (delayed or reordered).
    pub stale_samples: u64,
    /// Samples of a body the monitor no longer tracks: removed, or an earlier generation of a
    /// reused slot.
    pub retired_samples: u64,
    /// Selected bodies the world reported gone.
    pub gone: u64,
    /// Ticks between a sample's tick and the runtime's last submitted tick when it was applied.
    pub sample_age_ticks: Series,
    pub events: u64,
    /// Events the ring overwrote before the consumer drained them.
    pub events_overwritten: u64,
    pub last_sample_tick: Option<Tick>,
}

#[derive(Debug)]
pub struct BodyMonitor {
    /// By handle index.
    tracked: Vec<Option<Tracked>>,
    /// The newest generation removed from each index; samples of it or older are retired.
    retired: Vec<Option<u32>>,
    events: VecDeque<BodyEvent>,
    event_capacity: usize,
    summary: BodySummary,
    controlled: Option<BodyHandle>,
    cursor: usize,
}

impl BodyMonitor {
    /// `capacity` is the world's body capacity; `event_capacity` bounds waiting events.
    pub fn new(capacity: u32, event_capacity: u32) -> Self {
        Self {
            tracked: vec![None; capacity as usize],
            retired: vec![None; capacity as usize],
            events: VecDeque::with_capacity(event_capacity as usize),
            event_capacity: event_capacity as usize,
            summary: BodySummary::default(),
            controlled: None,
            cursor: 0,
        }
    }

    pub fn summary(&self) -> &BodySummary {
        &self.summary
    }

    pub fn body(&self, body: BodyHandle) -> Option<&Tracked> {
        self.tracked
            .get(body.index() as usize)
            .and_then(Option::as_ref)
            .filter(|tracked| tracked.body == body)
    }

    pub fn bodies(&self) -> impl Iterator<Item = &Tracked> {
        self.tracked.iter().flatten()
    }

    /// The body sampled every tick, if any.
    pub fn controlled(&self) -> Option<BodyHandle> {
        self.controlled
    }

    pub fn set_controlled(&mut self, body: Option<BodyHandle>) {
        self.controlled = body;
    }

    /// Collects waiting events in arrival order.
    pub fn drain_events(&mut self) -> impl Iterator<Item = BodyEvent> + '_ {
        self.events.drain(..)
    }

    /// Chooses at most `limit` bodies to sample next: the controlled body, then the others in
    /// turn, so every tracked body is sampled at least once every `tracked / (limit - 1)`
    /// ticks.
    pub fn select(&mut self, limit: usize, out: &mut Vec<BodyHandle>) {
        out.clear();
        if limit == 0 {
            return;
        }
        if let Some(controlled) = self.controlled.filter(|body| self.body(*body).is_some()) {
            out.push(controlled);
        }
        let slots = self.tracked.len();
        let mut visited = 0;
        while out.len() < limit && visited < slots {
            let index = self.cursor;
            self.cursor = (self.cursor + 1) % slots.max(1);
            visited += 1;
            if let Some(tracked) = self.tracked[index]
                && Some(tracked.body) != self.controlled
            {
                out.push(tracked.body);
            }
        }
    }

    /// A spawn outcome.
    pub fn spawned(&mut self, body: BodyHandle, tick: Tick, position: DVec3) {
        if let Some(tracked) = self.track(body) {
            tracked.spawned = Some((tick, position));
            self.push(BodyEvent::Spawned {
                body,
                tick,
                position,
            });
        }
    }

    /// A removal outcome with the counters the body ended with.
    pub fn removed(&mut self, body: BodyHandle, tick: Tick, counters: BodyCounters) {
        let index = body.index() as usize;
        if index >= self.tracked.len() {
            return;
        }
        // A body that was never sampled reports everything it did.
        let watermark = match self.tracked[index] {
            Some(tracked) if tracked.body == body => {
                self.tracked[index] = None;
                tracked.counters
            }
            _ => BodyCounters::default(),
        };
        self.retired[index] = Some(body.generation());
        self.summary.tracked = self.tracked.iter().flatten().count() as u32;
        if self.controlled == Some(body) {
            self.controlled = None;
        }
        self.report(body, tick, counters.since(&watermark));
        self.push(BodyEvent::Removed {
            body,
            tick,
            counters,
        });
    }

    /// A selected body the world no longer holds.
    pub fn gone(&mut self, body: BodyHandle) {
        self.summary.gone += 1;
        if self.controlled == Some(body) {
            self.controlled = None;
        }
    }

    /// Applies a sample. `observed_at` is the runtime's last submitted tick.
    pub fn sample(&mut self, sample: BodySample, observed_at: Option<Tick>) {
        self.summary.samples += 1;
        let body = sample.body;
        let tick = Tick::new(sample.tick);
        if self
            .retired
            .get(body.index() as usize)
            .copied()
            .flatten()
            .is_some_and(|retired| body.generation() <= retired)
        {
            self.summary.retired_samples += 1;
            return;
        }
        if let Some(age) = observed_at.map(|observed| observed.since(tick)) {
            self.summary.sample_age_ticks.add(age as f64);
        }
        self.summary.last_sample_tick = Some(
            self.summary
                .last_sample_tick
                .map_or(tick, |seen| seen.max(tick)),
        );
        let Some(tracked) = self.track(body) else {
            self.summary.retired_samples += 1;
            return;
        };
        if tracked.last.is_some_and(|last| last.tick >= sample.tick) {
            self.summary.stale_samples += 1;
            return;
        }
        let delta = sample.counters.since(&tracked.counters);
        tracked.counters = sample.counters;
        tracked.last = Some(sample);
        self.report(body, tick, delta);
    }

    /// The entry for `body`, created when its index is free. `None` when the index holds a
    /// newer generation.
    fn track(&mut self, body: BodyHandle) -> Option<&mut Tracked> {
        let index = body.index() as usize;
        let current = *self.tracked.get(index)?;
        match current {
            Some(tracked) if tracked.body.generation() > body.generation() => return None,
            Some(tracked) if tracked.body == body => {}
            _ => {
                self.tracked[index] = Some(Tracked {
                    body,
                    spawned: None,
                    last: None,
                    counters: BodyCounters::default(),
                });
                self.summary.tracked = self.tracked.iter().flatten().count() as u32;
            }
        }
        self.tracked[index].as_mut()
    }

    fn report(&mut self, body: BodyHandle, tick: Tick, delta: BodyCounters) {
        let pairs = [
            (delta.landings, 0),
            (delta.liquid_entries, 1),
            (delta.body_contacts, 2),
            (delta.craters, 3),
            (delta.splashes, 4),
        ];
        for (count, kind) in pairs {
            if count == 0 {
                continue;
            }
            self.push(match kind {
                0 => BodyEvent::Landed { body, tick, count },
                1 => BodyEvent::EnteredLiquid { body, tick, count },
                2 => BodyEvent::TouchedBody { body, tick, count },
                3 => BodyEvent::Cratered { body, tick, count },
                _ => BodyEvent::Splashed { body, tick, count },
            });
        }
    }

    fn push(&mut self, event: BodyEvent) {
        self.summary.events += 1;
        if self.event_capacity == 0 {
            self.summary.events_overwritten += 1;
            return;
        }
        if self.events.len() == self.event_capacity {
            self.events.pop_front();
            self.summary.events_overwritten += 1;
        }
        self.events.push_back(event);
    }
}

#[cfg(test)]
mod tests {
    use world_model::glam::Vec3;

    use super::*;

    fn sample(body: BodyHandle, tick: u64, counters: BodyCounters) -> BodySample {
        BodySample {
            body,
            tick,
            position: DVec3::ZERO,
            velocity: Vec3::ZERO,
            ground_normal: Vec3::Y,
            grounded: false,
            suspended: false,
            submerged_fraction: 0.0,
            liquid: None,
            touching: 0,
            counters,
        }
    }

    fn landings(count: u32) -> BodyCounters {
        BodyCounters {
            landings: count,
            ..BodyCounters::default()
        }
    }

    fn landed(monitor: &mut BodyMonitor) -> u32 {
        monitor
            .drain_events()
            .filter_map(|event| match event {
                BodyEvent::Landed { count, .. } => Some(count),
                _ => None,
            })
            .sum()
    }

    #[test]
    fn delayed_and_reordered_samples_apply_each_change_once() {
        let mut monitor = BodyMonitor::new(4, 16);
        let body = BodyHandle::new(2, 0);
        monitor.spawned(body, Tick::new(1), DVec3::ZERO);
        monitor.sample(sample(body, 5, landings(1)), Some(Tick::new(6)));
        // Tick 4 arrives late, then tick 9 after a skipped 6..8.
        monitor.sample(sample(body, 4, landings(1)), Some(Tick::new(9)));
        monitor.sample(sample(body, 9, landings(3)), Some(Tick::new(9)));
        assert_eq!(
            landed(&mut monitor),
            3,
            "one landing, then the two the gap held"
        );
        let summary = *monitor.summary();
        assert_eq!(summary.stale_samples, 1);
        assert_eq!(summary.sample_age_ticks.max, 5.0);
        assert_eq!(summary.last_sample_tick, Some(Tick::new(9)));
    }

    #[test]
    fn a_reused_slot_does_not_inherit_or_repeat_the_old_body() {
        let mut monitor = BodyMonitor::new(2, 16);
        let old = BodyHandle::new(0, 0);
        let new = BodyHandle::new(0, 1);
        monitor.spawned(old, Tick::new(1), DVec3::ZERO);
        monitor.sample(sample(old, 2, landings(1)), None);
        monitor.removed(old, Tick::new(3), landings(2));
        assert_eq!(
            landed(&mut monitor),
            2,
            "the removal reports the landing it held"
        );
        monitor.spawned(new, Tick::new(4), DVec3::ONE);
        // A late sample of the old body after the slot was reused.
        monitor.sample(sample(old, 3, landings(2)), None);
        monitor.sample(sample(new, 5, landings(1)), None);
        assert_eq!(landed(&mut monitor), 1);
        assert_eq!(monitor.summary().retired_samples, 1);
        assert!(monitor.body(old).is_none());
        assert_eq!(
            monitor.body(new).map(|tracked| tracked.counters),
            Some(landings(1))
        );
    }

    #[test]
    fn a_full_ring_counts_what_it_overwrote_and_the_counters_stay_exact() {
        let mut monitor = BodyMonitor::new(8, 2);
        for index in 0..4 {
            let body = BodyHandle::new(index, 0);
            monitor.sample(sample(body, 1, landings(index + 1)), None);
        }
        assert_eq!(monitor.summary().events_overwritten, 2);
        assert_eq!(monitor.drain_events().count(), 2);
        let total: u32 = monitor
            .bodies()
            .map(|tracked| tracked.counters.landings)
            .sum();
        assert_eq!(total, 10);
    }

    #[test]
    fn selection_keeps_the_controlled_body_and_rotates_the_rest() {
        let mut monitor = BodyMonitor::new(5, 0);
        for index in 0..5 {
            monitor.spawned(BodyHandle::new(index, 0), Tick::new(0), DVec3::ZERO);
        }
        let controlled = BodyHandle::new(3, 0);
        monitor.set_controlled(Some(controlled));
        let mut chosen = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..2 {
            monitor.select(3, &mut chosen);
            assert_eq!(chosen.len(), 3);
            assert_eq!(chosen[0], controlled);
            seen.extend(chosen.iter().map(|body| body.index()));
        }
        assert_eq!(
            seen.len(),
            5,
            "two ticks of three samples cover all five bodies"
        );
        monitor.removed(controlled, Tick::new(2), BodyCounters::default());
        assert_eq!(monitor.controlled(), None);
    }
}
