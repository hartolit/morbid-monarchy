//! The saved world: its format, its redb store and the import of legacy saves.
//!
//! `world-store` implements the runtime's storage port ([`world_runtime::store::WorldStore`])
//! over one redb file per world. It decides how data is encoded and made durable; the runtime's
//! residency and checkpoint owners decide what is saved and when.
//!
//! - [`format`](mod@format): the exact bytes of every row (version 1), with lengths, checksums and the
//!   compatibility checks a reader applies before it allocates;
//! - [`store`]: [`store::RedbWorldStore`], chunk revisions, body sets and atomic checkpoint
//!   publication, reopening after an I/O failure;
//! - [`faults`]: fault injection at the storage boundary (I/O errors in redb's own writes,
//!   failures and holds at defined points, process interruption) for tests;
//! - [`legacy`]: the legacy baseline's rows and table, and their non-destructive import.

pub mod faults;
pub mod format;
pub mod legacy;
pub mod store;
