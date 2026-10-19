# world-render

Draws the world the GPU owner holds, on its device and timeline: the four landscape layers
(terrain, loose material, liquids, surface) straight from the resident cells through the window
map, the spheres from the body state, and the camera placed on the device so a followed sphere
needs no readback. Lighting and atmosphere follow the baseline's: sun, hemispheric sky,
heightfield shadows and ambient occlusion, a sun shadow map of the spheres, distance fog with a
sun glow, bloom and tone mapping. It writes no physical state and keeps no copy of it.

The client composes it: `Renderer::new` on the session's `WorldView`, `resize` with the output
size, and `encode` inside `GpuWorld::submit_presentation` each frame with a `FrameRequest`
(camera rig, lighting, cursor, debug views). Pipelines are built once per device; bind groups
over the world's buffers are rebuilt only when the window's buffers are replaced
(`WorldView::buffers_generation`), never because cells changed.

The shaders under `shaders/` are assembled after `gpu-world`'s generated declarations and cell
accessors (`world_render::shaders`), so the packed layouts have one owner; `cargo evidence check
static` validates the assemblies and `cargo evidence test gpu --suite render` executes them.
