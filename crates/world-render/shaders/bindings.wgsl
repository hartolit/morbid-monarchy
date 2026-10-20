// The main and shadow passes' bindings. Group 0 is the frame and the GPU owner's live buffers,
// all read-only: the renderer never writes physical state. Group 1, bound only in the main pass,
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

@group(1) @binding(0) var shadow_map: texture_depth_2d;
@group(1) @binding(1) var shadow_sampler: sampler_comparison;
