// The visibility passes' bindings: the frame and view, the GPU owner's cells and window map
// (read-only), and the renderer's derived chunk records, layer lists, draw arguments and the
// relight list.

struct ChunkRecord {
    // Lowest vertex any layer of the chunk can place, and the highest, in world units.
    low: f32,
    high: f32,
    // LAYER_PRESENT_* bits of the chunk's own cells.
    layers: u32,
    resident: u32,
    // Bit per layer (0 terrain .. 3 surface) the chunk draws this frame, set by the cull, and
    // LIGHTING_DIRTY while its cached lighting is out of date.
    visible: u32,
    // The signature of the chunk's cells (two hash lanes, zero when it is not resident) as the
    // last frame's bounds read them, and whether this frame's differed from it.
    signature_sum: u32,
    signature_mix: u32,
    changed: u32,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> view: View;
@group(0) @binding(2) var<storage, read> cells: array<vec2<u32>>;
@group(0) @binding(3) var<storage, read> window_slots: array<u32>;
@group(0) @binding(4) var<storage, read_write> chunk_records: array<ChunkRecord>;
@group(0) @binding(5) var<storage, read_write> layer_lists: array<u32>;
@group(0) @binding(6) var<storage, read_write> draw_args: array<atomic<u32>>;
// The chunks whose lighting the relight pass refreshes this frame, and its indirect dispatch.
@group(0) @binding(7) var<storage, read_write> relight_list: array<u32>;
@group(0) @binding(8) var<storage, read_write> relight_args: array<u32>;
