// The main and shadow passes' bindings. Group 0 is the frame and the GPU owner's live buffers,
// all read-only: the renderer never writes physical state; the relight pass writes only the
// renderer's lighting cache. Group 1, bound only in the main pass,
// is the sun's sphere shadow map, which the shadow pass writes.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> view: View;
@group(0) @binding(2) var<storage, read> cells: array<vec2<u32>>;
@group(0) @binding(3) var<storage, read> window_slots: array<u32>;
@group(0) @binding(4) var<storage, read> palette: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read> bodies: array<u32>;
@group(0) @binding(6) var<storage, read> activity: array<u32>;
// The window chunks each layer draws this frame (visibility.wgsl), one list per layer.
@group(0) @binding(7) var<storage, read> layer_lists: array<u32>;
// The cached terrain lighting of every window chunk's vertices (terrain.wgsl), as the layers
// read it.
@group(0) @binding(8) var<storage, read> lighting: array<vec2<u32>>;

// Bound only by the relight pass, in a group of its own over the frame, the cells, the window
// map, the lighting cache it writes and the chunks it relights.
@group(0) @binding(9) var<storage, read_write> lighting_out: array<vec2<u32>>;
@group(0) @binding(10) var<storage, read> relight_list: array<u32>;

@group(1) @binding(0) var shadow_map: texture_depth_2d;
@group(1) @binding(1) var shadow_sampler: sampler_comparison;
