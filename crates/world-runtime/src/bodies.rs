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
//! grow. The monitor remembers, per body, the counters it has already delivered as events (its
//! watermark) and emits only the difference, so a delayed, reordered or skipped sample neither
//! repeats an event nor drops one: the next sample, or the removal outcome, carries the rest.
//! Samples are keyed by handle (index and generation), so a sample of a removed body or of a
//! reused slot's earlier generation is counted and ignored.
//!
//! **Delivery is bounded and lossless.** Events wait in a ring of fixed capacity for the
//! consumer. A full ring overwrites nothing: an event that does not fit stays undelivered (the
//! watermark does not advance past it), is counted as deferred, and is delivered by the next
//! [`BodyMonitor::drain_events`], which empties the ring and then every deferred event. A
//! removal that does not fit waits in a queue of one entry per body slot; only more removals
//! than slots between two drains exceed it, and those are counted as unreported (their command
//! receipts still hold them). The pressure counters are in [`BodySummary`], outside the ring.
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
    /// Counters already delivered as events: the watermark. While the ring is full the latest
    /// sample's counters run ahead of it; the difference is delivered later, never dropped.
    pub counters: BodyCounters,
    /// The spawn event is still to be delivered.
    pub spawn_pending: bool,
}

/// Something that happened to a body, derived from its counters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BodyEvent {
    Spawned {
        body: BodyHandle,
        tick: Tick,
        position: DVec3,
    },
    /// `count` landings happened by `tick` (the sample that showed them) since the last report.
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
    /// Events delivered: into the ring, or straight to a drain.
    pub events: u64,
    /// Attempts to deliver an event into a full ring: the event waited for the next drain.
    pub events_deferred: u64,
    /// The most events the ring has held.
    pub ring_high_water: u32,
    /// Removals (and spawns that arrived after their removal) beyond the waiting queue, more
    /// than one per body slot between two drains: their events were not delivered, while their
    /// command receipts still hold the outcomes.
    pub removals_unreported: u64,
    pub last_sample_tick: Option<Tick>,
}

/// Events waiting for a drain outside any tracked body: a removal's, or the spawn of a body
/// whose removal arrived first.
#[derive(Debug, Clone, Copy)]
enum Waiting {
    Removal(Removal),
    Spawn(BodyEvent),
}

/// A removal whose events did not fit the ring.
#[derive(Debug, Clone, Copy)]
struct Removal {
    body: BodyHandle,
    tick: Tick,
    counters: BodyCounters,
    watermark: BodyCounters,
    spawn: Option<(Tick, DVec3)>,
}

#[derive(Debug)]
pub struct BodyMonitor {
    /// By handle index.
    tracked: Vec<Option<Tracked>>,
    /// The newest generation removed from each index; samples of it or older are retired.
    retired: Vec<Option<u32>>,
    /// An older generation whose entry a newer one replaced before its removal arrived (only
    /// when deliveries are reordered): kept so that removal reports from its watermark.
    superseded: Vec<Option<Tracked>>,
    events: VecDeque<BodyEvent>,
    event_capacity: usize,
    waiting: VecDeque<Waiting>,
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
            superseded: vec![None; capacity as usize],
            events: VecDeque::with_capacity(event_capacity as usize),
            event_capacity: event_capacity as usize,
            waiting: VecDeque::with_capacity(capacity as usize),
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

    /// Delivers every waiting event into `out`: the ring in arrival order, then the deferred
    /// removals, then each tracked body's deferred spawn and counter changes. A body's own
    /// events arrive in order.
    pub fn drain_events(&mut self, out: &mut Vec<BodyEvent>) {
        out.extend(self.events.drain(..));
        while let Some(waiting) = self.waiting.pop_front() {
            self.summary.events += match waiting {
                Waiting::Removal(removal) => Self::removal_events(&removal, out),
                Waiting::Spawn(event) => {
                    out.push(event);
                    1
                }
            };
        }
        for index in 0..self.tracked.len() {
            self.deliver(index, Some(out));
        }
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
        if self.is_retired(body) {
            // Its removal arrived first (and did not know the spawn): deliver, do not track.
            let event = BodyEvent::Spawned {
                body,
                tick,
                position,
            };
            if !self.waiting.is_empty() || !self.emit(event, &mut None) {
                self.wait(Waiting::Spawn(event), 1);
            }
            return;
        }
        if let Some(tracked) = self.track(body) {
            tracked.spawned = Some((tick, position));
            tracked.spawn_pending = true;
            self.deliver(body.index() as usize, None);
        }
    }

    /// A body a checkpoint restored. Its counters are what it had already reported before
    /// the checkpoint, so they become its watermark: resuming it repeats no event, and its
    /// spawn is not reported again. `_at` is the checkpoint's tick (`None` for one that never ran).
    pub fn restored(&mut self, body: BodyHandle, _at: Option<Tick>, counters: BodyCounters) {
        if let Some(tracked) = self.track(body) {
            tracked.counters = counters;
            tracked.spawned = None;
            tracked.spawn_pending = false;
            tracked.last = None;
        }
    }

    /// A removal outcome with the counters the body ended with.
    pub fn removed(&mut self, body: BodyHandle, tick: Tick, counters: BodyCounters) {
        let index = body.index() as usize;
        if index >= self.tracked.len() {
            return;
        }
        // A body that was never sampled reports everything it did, and its spawn if that
        // outcome arrived but was not delivered.
        let entry = match (self.tracked[index], self.superseded[index]) {
            (Some(tracked), _) if tracked.body == body => {
                self.tracked[index] = None;
                Some(tracked)
            }
            (_, Some(old)) if old.body == body => {
                self.superseded[index] = None;
                Some(old)
            }
            _ => None,
        };
        let (watermark, spawn) = entry.map_or((BodyCounters::default(), None), |tracked| {
            (
                tracked.counters,
                tracked.spawned.filter(|_| tracked.spawn_pending),
            )
        });
        self.retired[index] = Some(body.generation());
        self.summary.tracked = self.tracked.iter().flatten().count() as u32;
        if self.controlled == Some(body) {
            self.controlled = None;
        }
        let removal = Removal {
            body,
            tick,
            counters,
            watermark,
            spawn,
        };
        // A removal's events enter the ring together or wait together, behind any removal
        // already waiting.
        let count = Self::removal_count(&removal);
        if self.waiting.is_empty() && self.events.len() + count <= self.event_capacity {
            self.summary.events += Self::removal_events(&removal, &mut self.events);
            self.note_ring();
        } else {
            self.wait(Waiting::Removal(removal), count);
        }
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
        if self.is_retired(body) {
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
        tracked.last = Some(sample);
        self.deliver(body.index() as usize, None);
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
                // An older generation still tracked: its removal has not arrived yet.
                if let Some(older) = current {
                    self.superseded[index] = Some(older);
                }
                self.tracked[index] = Some(Tracked {
                    body,
                    spawned: None,
                    last: None,
                    counters: BodyCounters::default(),
                    spawn_pending: false,
                });
                self.summary.tracked = self.tracked.iter().flatten().count() as u32;
            }
        }
        self.tracked[index].as_mut()
    }

    /// Delivers what the body in slot `index` has not reported yet: its spawn, then each
    /// counter its latest sample shows grown. Into the ring while it has room (`drain` is
    /// `None`), advancing the watermark only for what was delivered, or all of it into `drain`.
    fn deliver(&mut self, index: usize, mut drain: Option<&mut Vec<BodyEvent>>) {
        let Some(mut tracked) = self.tracked[index] else {
            return;
        };
        let body = tracked.body;
        if tracked.spawn_pending
            && let Some((tick, position)) = tracked.spawned
            && self.emit(
                BodyEvent::Spawned {
                    body,
                    tick,
                    position,
                },
                &mut drain,
            )
        {
            tracked.spawn_pending = false;
        }
        if let Some(last) = tracked.last {
            let tick = Tick::new(last.tick);
            let delta = last.counters.since(&tracked.counters);
            for kind in Kind::ALL {
                let count = kind.get(&delta);
                if count != 0 && self.emit(kind.event(body, tick, count), &mut drain) {
                    kind.set(&mut tracked.counters, kind.get(&last.counters));
                }
            }
            // Absorbed energy is a quantity, not an event: the watermark follows the sample.
            tracked.counters.absorbed_energy = last.counters.absorbed_energy;
        }
        self.tracked[index] = Some(tracked);
    }

    /// Whether `body` is a generation its slot has already removed.
    fn is_retired(&self, body: BodyHandle) -> bool {
        self.retired
            .get(body.index() as usize)
            .copied()
            .flatten()
            .is_some_and(|retired| body.generation() <= retired)
    }

    /// Queues events that did not fit the ring behind earlier ones: one entry per body slot
    /// at most, beyond which they are counted unreported.
    fn wait(&mut self, waiting: Waiting, events: usize) {
        if self.waiting.len() < self.tracked.len() {
            self.summary.events_deferred += events as u64;
            self.waiting.push_back(waiting);
        } else {
            self.summary.removals_unreported += 1;
        }
    }

    fn emit(&mut self, event: BodyEvent, drain: &mut Option<&mut Vec<BodyEvent>>) -> bool {
        if let Some(out) = drain {
            out.push(event);
        } else if self.events.len() < self.event_capacity {
            self.events.push_back(event);
            self.note_ring();
        } else {
            self.summary.events_deferred += 1;
            return false;
        }
        self.summary.events += 1;
        true
    }

    fn note_ring(&mut self) {
        self.summary.ring_high_water = self.summary.ring_high_water.max(self.events.len() as u32);
    }

    fn removal_count(removal: &Removal) -> usize {
        let delta = removal.counters.since(&removal.watermark);
        let grown = Kind::ALL
            .iter()
            .filter(|kind| kind.get(&delta) != 0)
            .count();
        usize::from(removal.spawn.is_some()) + grown + 1
    }

    /// A removal's events: its undelivered spawn, what grew since the watermark, then the
    /// removal itself. Returns how many.
    fn removal_events(removal: &Removal, out: &mut impl Extend<BodyEvent>) -> u64 {
        let body = removal.body;
        if let Some((tick, position)) = removal.spawn {
            out.extend([BodyEvent::Spawned {
                body,
                tick,
                position,
            }]);
        }
        let delta = removal.counters.since(&removal.watermark);
        for kind in Kind::ALL {
            let count = kind.get(&delta);
            if count != 0 {
                out.extend([kind.event(body, removal.tick, count)]);
            }
        }
        out.extend([BodyEvent::Removed {
            body,
            tick: removal.tick,
            counters: removal.counters,
        }]);
        Self::removal_count(removal) as u64
    }
}

/// The counters that become events.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Landings,
    LiquidEntries,
    BodyContacts,
    Craters,
    Splashes,
}

impl Kind {
    const ALL: [Self; 5] = [
        Self::Landings,
        Self::LiquidEntries,
        Self::BodyContacts,
        Self::Craters,
        Self::Splashes,
    ];

    fn get(self, counters: &BodyCounters) -> u32 {
        match self {
            Self::Landings => counters.landings,
            Self::LiquidEntries => counters.liquid_entries,
            Self::BodyContacts => counters.body_contacts,
            Self::Craters => counters.craters,
            Self::Splashes => counters.splashes,
        }
    }

    fn set(self, counters: &mut BodyCounters, value: u32) {
        match self {
            Self::Landings => counters.landings = value,
            Self::LiquidEntries => counters.liquid_entries = value,
            Self::BodyContacts => counters.body_contacts = value,
            Self::Craters => counters.craters = value,
            Self::Splashes => counters.splashes = value,
        }
    }

    fn event(self, body: BodyHandle, tick: Tick, count: u32) -> BodyEvent {
        match self {
            Self::Landings => BodyEvent::Landed { body, tick, count },
            Self::LiquidEntries => BodyEvent::EnteredLiquid { body, tick, count },
            Self::BodyContacts => BodyEvent::TouchedBody { body, tick, count },
            Self::Craters => BodyEvent::Cratered { body, tick, count },
            Self::Splashes => BodyEvent::Splashed { body, tick, count },
        }
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
        let mut events = Vec::new();
        monitor.drain_events(&mut events);
        events
            .iter()
            .filter_map(|event| match event {
                BodyEvent::Landed { count, .. } => Some(*count),
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
    fn a_removal_arriving_after_the_slots_next_generation_reports_from_its_watermark() {
        let mut monitor = BodyMonitor::new(1, 16);
        let old = BodyHandle::new(0, 0);
        let new = BodyHandle::new(0, 1);
        monitor.sample(sample(old, 2, landings(1)), None);
        assert_eq!(landed(&mut monitor), 1);
        // Reordered: the new generation's sample before the old one's removal.
        monitor.sample(sample(new, 4, landings(1)), None);
        monitor.removed(old, Tick::new(3), landings(2));
        let mut events = Vec::new();
        monitor.drain_events(&mut events);
        let landed_by = |body| -> u32 {
            events
                .iter()
                .filter_map(|event| match event {
                    BodyEvent::Landed {
                        body: of, count, ..
                    } if *of == body => Some(*count),
                    _ => None,
                })
                .sum()
        };
        assert_eq!(
            landed_by(old),
            1,
            "only the landing the old body had not reported"
        );
        assert_eq!(landed_by(new), 1);
        assert!(
            monitor.body(new).is_some(),
            "the new generation stays tracked"
        );
    }

    #[test]
    fn a_full_ring_defers_events_and_a_drain_delivers_every_one_once() {
        let mut monitor = BodyMonitor::new(8, 2);
        let bodies: Vec<BodyHandle> = (0..4).map(|index| BodyHandle::new(index, 0)).collect();
        for (k, body) in bodies.iter().enumerate() {
            monitor.sample(sample(*body, 1, landings(k as u32 + 1)), None);
        }
        // Two fit; two wait, their watermarks unmoved.
        let summary = *monitor.summary();
        assert_eq!((summary.events, summary.events_deferred), (2, 2));
        assert_eq!(summary.ring_high_water, 2);
        assert_eq!(
            monitor.body(bodies[3]).map(|t| t.counters),
            Some(landings(0))
        );
        // More landings while the ring is full, and a removal that cannot fit.
        monitor.sample(sample(bodies[3], 2, landings(6)), None);
        monitor.removed(bodies[2], Tick::new(3), landings(5));
        assert_eq!(monitor.summary().removals_unreported, 0);
        let mut events = Vec::new();
        monitor.drain_events(&mut events);
        let mut landed = [0u32; 4];
        let mut removed = 0;
        for event in &events {
            match event {
                BodyEvent::Landed { body, count, .. } => landed[body.index() as usize] += count,
                BodyEvent::Removed { .. } => removed += 1,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(
            landed,
            [1, 2, 5, 6],
            "every landing once, the removal's included"
        );
        assert_eq!(removed, 1);
        assert_eq!(
            monitor.body(bodies[3]).map(|t| t.counters),
            Some(landings(6))
        );
        // Nothing is left over, and nothing repeats.
        assert_eq!(events.len(), 5, "{events:?}");
        monitor.drain_events(&mut events);
        assert_eq!(events.len(), 5, "{events:?}");
        monitor.sample(sample(bodies[3], 2, landings(6)), None);
        assert_eq!(monitor.summary().events, 5);
    }

    #[test]
    fn removals_beyond_one_per_slot_between_drains_are_counted_unreported() {
        let mut monitor = BodyMonitor::new(1, 1);
        for generation in 0..3 {
            let body = BodyHandle::new(0, generation);
            monitor.spawned(body, Tick::new(u64::from(generation)), DVec3::ZERO);
            monitor.removed(body, Tick::new(u64::from(generation)), landings(1));
        }
        // The first spawn fits the ring and its removal waits in the queue of one per slot; the
        // two later removals find it full.
        let summary = *monitor.summary();
        assert_eq!(summary.removals_unreported, 2);
        let mut events = Vec::new();
        monitor.drain_events(&mut events);
        let removed = events
            .iter()
            .filter(|event| matches!(event, BodyEvent::Removed { .. }))
            .count();
        assert_eq!(removed, 1, "{events:?}");
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
