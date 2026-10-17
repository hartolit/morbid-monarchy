//! Controlled fault injection for conformance runs.
//!
//! A session opened with fault injection enabled hands out a [`FaultController`]. Through it a
//! test can lose the device, hold or reorder map deliveries, hold submission completions, flood
//! the cosmetic event buffer, and run `react` built to drop what liquid receivers accept (an
//! unmatched transfer, which the accounting must detect). Each fault exercises the production path that handles the
//! real event: the device really is destroyed, deliveries really wait in the mailbox, and the
//! flood appends through the same `push_event` the physical kernels use. No production check is
//! relaxed to make a fault possible, and a session opened without fault injection has no
//! controller and never compiles the flood kernel.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

/// Shared fault switches. Independent flags, read with relaxed ordering: none of them
/// publishes other memory, and the owner reads them on its own thread at each poll or tick.
#[derive(Debug, Default)]
pub(crate) struct FaultSwitches {
    lose_device: AtomicBool,
    hold_deliveries: AtomicBool,
    reverse_deliveries: AtomicBool,
    hold_completions: AtomicBool,
    flood_events: AtomicU32,
    unmatched_transfers: AtomicBool,
}

impl FaultSwitches {
    pub(crate) fn take_device_loss(&self) -> bool {
        self.lose_device.swap(false, Ordering::Relaxed)
    }

    pub(crate) fn hold_deliveries(&self) -> bool {
        self.hold_deliveries.load(Ordering::Relaxed)
    }

    pub(crate) fn reverse_deliveries(&self) -> bool {
        self.reverse_deliveries.load(Ordering::Relaxed)
    }

    pub(crate) fn hold_completions(&self) -> bool {
        self.hold_completions.load(Ordering::Relaxed)
    }

    /// Events the next tick appends; consumed by that tick.
    pub(crate) fn take_flood(&self) -> u32 {
        self.flood_events.swap(0, Ordering::Relaxed)
    }

    pub(crate) fn unmatched_transfers(&self) -> bool {
        self.unmatched_transfers.load(Ordering::Relaxed)
    }
}

/// The handle a conformance run uses to inject faults into one session.
#[derive(Debug, Clone)]
pub struct FaultController(pub(crate) Arc<FaultSwitches>);

impl FaultController {
    /// Destroys the session's device at its next poll or tick, as a driver loss would. The
    /// loss is then reported through the device-lost callback.
    pub fn lose_device(&self) {
        self.0.lose_device.store(true, Ordering::Relaxed);
    }

    /// Leaves completed map deliveries undelivered until released: delayed callbacks.
    pub fn hold_deliveries(&self, hold: bool) {
        self.0.hold_deliveries.store(hold, Ordering::Relaxed);
    }

    /// Observes map deliveries in reverse buffer order: out-of-order callbacks.
    pub fn reverse_deliveries(&self, reverse: bool) {
        self.0.reverse_deliveries.store(reverse, Ordering::Relaxed);
    }

    /// Leaves submission completions unobserved until released.
    pub fn hold_completions(&self, hold: bool) {
        self.0.hold_completions.store(hold, Ordering::Relaxed);
    }

    /// Makes the next tick append `events` cosmetic events.
    pub fn flood_events(&self, events: u32) {
        self.0.flood_events.store(events, Ordering::Relaxed);
    }

    /// Runs the material phases with receivers dropping the liquid they accept while donors
    /// still drain it, until switched off: a deliberately unmatched transfer.
    pub fn unmatched_transfers(&self, on: bool) {
        self.0.unmatched_transfers.store(on, Ordering::Relaxed);
    }
}
