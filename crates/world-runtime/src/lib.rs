//! CPU orchestration of the live world.
//!
//! The runtime decides what the physical world is asked to do and when, and consumes what it
//! reports back. It does not simulate cells or bodies and never names a GPU type: the live
//! world sits behind the [`port::PhysicalWorld`] port, which `gpu-world` implements and the
//! composition root selects.
//!
//! - [`ids`]: world, session, command, tick and revision identities;
//! - [`port`]: the batch-oriented port, its tickets, refusals, outcomes and counters;
//! - [`receipts`]: command admission, retries, conflicts and bounded retention;
//! - [`ledger`]: the consumer of outcomes, separating important results from cosmetic events;
//! - [`bodies`]: the consumer of body observations, deriving events from cumulative counters;
//! - [`clock`]: the fixed logical clock and its bounded catch-up;
//! - [`runtime`]: the `bevy_ecs` composition that runs one frame at a time;
//! - [`store`]: the storage port: chunk revisions, body sets and published checkpoints;
//! - [`workers`]: bounded threads that read, generate and write for the owners below;
//! - [`residency`]: which chunks are resident, where each chunk's newest content is, and
//!   moving the window without losing a change;
//! - [`checkpoint`]: consistent checkpoints at a completed tick, published atomically;
//! - [`session`]: one saved world's live session: open, frames, close and the loss report.

pub mod bodies;
pub mod checkpoint;
pub mod clock;
pub mod ids;
pub mod ledger;
pub mod port;
pub mod receipts;
pub mod residency;
pub mod runtime;
pub mod session;
pub mod store;
#[cfg(test)]
mod testing;
pub mod workers;
