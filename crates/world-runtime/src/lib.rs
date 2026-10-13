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
//! - [`clock`]: the fixed logical clock and its bounded catch-up;
//! - [`runtime`]: the `bevy_ecs` composition that runs one frame at a time.

pub mod clock;
pub mod ids;
pub mod ledger;
pub mod port;
pub mod receipts;
pub mod runtime;
