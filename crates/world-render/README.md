# world-render

Draws the world the GPU owner holds, on its device and timeline: the four landscape layers
(terrain, loose material, liquids, surface) straight from the resident cells through the window
map, the spheres from the body state, and the camera placed on the device so a followed sphere
needs no readback. Lighting and atmosphere follow the baseline's: sun, hemispheric sky,
heightfield shadows and ambient occlusion, a sun shadow map of the spheres, distance fog with a
sun glow, bloom and tone mapping. It writes no physical state and keeps no copy of it.

The landscape's heightfield shadows and occlusion are derived data the renderer keeps per chunk
vertex (two buffers of 4 bytes a vertex, 33.8 kB per window chunk) and relights on the device
only for drawn chunks whose cells, or whose neighbours within the shadow's reach toward the sun,
changed since they were last lit, or for every drawn chunk when the lighting inputs change; the
visibility pass detects the changes from a signature of each chunk's cells and compacts the
relight list into an indirect dispatch. `Renderer::set_lighting_cache(false)` lights every
vertex each frame instead, the comparison the `render` suite measures against.

The client composes it: `Renderer::new` on the session's `WorldView`, `resize` with the output
size, and `encode` inside `GpuWorld::submit_presentation` each frame with a `FrameRequest`
(camera rig, lighting, cursor, debug views). Pipelines are built once per device; bind groups
over the world's buffers are rebuilt only when the window's buffers are replaced
(`WorldView::buffers_generation`, a resize to another size), never because cells changed or the
window moved; a move relights every drawn chunk, since the cache is kept per window position.

The shaders under `shaders/` are assembled after `gpu-world`'s generated declarations and cell
accessors (`world_render::shaders`), so the packed layouts have one owner; `cargo evidence check
static` validates the assemblies and `cargo evidence test gpu --suite render` executes them.
