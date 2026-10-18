//! GPU execution of the live world.
//!
//! `gpu-world` owns everything that lives on the device for one session: the device itself and
//! its single submission timeline, the checked resource plan and the buffers allocated from it,
//! the packed layouts shared with the kernels, the assembled shader source, the chunk slots of
//! the resident window, and the bounded staging that moves commands, results, chunk uploads and
//! captures. It implements the runtime's [`world_runtime::port::PhysicalWorld`] port, so the
//! runtime names no GPU type and this crate makes no policy decision about residency, saving or
//! input.
//!
//! - [`context`]: adapter selection by stable identifier, the device and its granted limits,
//!   device health (loss and uncaptured errors), counted buffer creation and the timeline;
//! - [`plan`]: the complete, checked resource plan for a window and transfer configuration;
//! - [`layout`]: the packed records and the WGSL declarations generated from their owners;
//! - [`shader`]: source assembly with a line map, offline validation and the production list;
//! - [`resources`]: device buffers, compiled pipelines and the shared bind group;
//! - [`staging`]: pooled upload and readback buffers with generation-checked map delivery;
//! - [`slots`]: chunk slots and the window map, as metadata;
//! - [`bodies`]: body slots and their generations, as metadata;
//! - [`commands`], [`readback`]: command records in, identity-checked outcomes out;
//! - [`world`]: [`world::GpuWorld`], which ties them together tick by tick;
//! - [`faults`]: controlled fault injection for conformance runs.
//!
//! The physical rules are kernels on this owner: the material phases (03) under
//! `shaders/sim/`, the body substeps and body commands (04) under `shaders/body/`.

#[macro_use]
pub mod layout;
pub mod bodies;
pub mod commands;
pub mod context;
pub mod faults;
pub mod plan;
pub mod readback;
pub mod resources;
pub mod shader;
pub mod slots;
pub mod staging;
pub mod world;
