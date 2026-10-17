//! The meaning of Morbid Monarchy's world, independent of any engine, GPU API or store.
//!
//! - [`cell`]: the packed 64-bit column (`WorldCell`), its field layout and its column rules;
//! - [`material`]: material identities and their physical properties;
//! - [`spatial`]: chunk geometry, absolute chunk keys and the cell/render axis convention;
//! - [`units`]: physical units, gravity and the longest body integration step;
//! - [`config`]: supported ranges and defaults of user-adjustable settings;
//! - [`rules`]: the automaton around the column rules: rule switches, cadences, tuning
//!   constants, activity levels and keyed randomness;
//! - [`body`]: sphere presets, control strengths and the supported body envelope;
//! - [`command`] and [`observation`]: validated intent and observed results that cross between
//!   the CPU and the physical simulation;
//! - [`generation`]: deterministic terrain generation from a seed and an absolute key.
//!
//! Nothing here depends on Bevy, wgpu, a database or a thread pool. The CPU baseline, the GPU
//! simulation, storage and the evidence runner all read these definitions; none of them keeps
//! its own copy of a layout, range or default.

pub mod body;
pub mod cell;
pub mod command;
pub mod config;
pub mod generation;
pub mod material;
pub mod observation;
pub mod rules;
pub mod spatial;
pub mod units;

/// The vector types in this crate's interfaces, so consumers name the same version.
pub use glam;
