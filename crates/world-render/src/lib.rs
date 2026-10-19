//! Native rendering of the GPU-owned world.
//!
//! The renderer draws what the GPU owner (`gpu-world`) holds, on its device and timeline: the
//! landscape layers straight from the resident cells through the window map, the spheres from
//! the body state, and the camera placed on the device so a followed body needs no readback.
//! It never writes physical state and keeps no copy of it; its own data is derived (the frame
//! uniform, the view, render targets, the shadow map) or fixed (the palette and meshes).
//!
//! - [`renderer`]: [`Renderer`], its pipelines, targets and passes;
//! - [`frame`]: what a frame shows ([`FrameRequest`], [`CameraRig`]) and the camera math the
//!   client shares with the shaders (forward vectors, cursor rays);
//! - [`lighting`]: the sun, sky, fog and palette;
//! - [`shaders`]: the WGSL assemblies, validated offline by `cargo evidence check static`;
//! - [`geometry`]: the landscape grid's indices and the sphere mesh.

pub mod frame;
pub mod geometry;
pub mod lighting;
pub mod renderer;
pub mod shaders;

pub use frame::{CameraRig, Cursor, FrameRequest, Overlay};
pub use lighting::Lighting;
pub use renderer::{RenderCounters, RenderError, Renderer};
