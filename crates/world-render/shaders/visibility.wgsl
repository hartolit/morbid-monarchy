// Which window chunks each landscape layer draws this frame, derived from the cells every frame
// (so nothing is ever stale after an edit, a load or a window move) and the camera the camera
// pass placed:
//
// chunk_bounds   one workgroup per window chunk: the height range its layer vertices can take
//                and which layers its cells hold
// cull_chunks    one invocation per window chunk: frustum test of the bounds widened by the
//                3 x 3 neighbourhood (a chunk's edge vertices borrow its neighbours' heights and
//                materials), then the layers that can show
// compact_lists  one workgroup: each layer's visible chunks in chunk order (a prefix sum) and
//                its instance count in the layer's indirect draw
//
// The lists keep chunk order, the order the unculled path draws in, so blending and depth ties
// resolve the same in every frame and on both paths. A layer with no material in the chunk or
// around it draws only hidden vertices, so leaving it out changes no pixel.

const LAYER_PRESENT_GRANULAR: u32 = 1u;
const LAYER_PRESENT_FLUID: u32 = 2u;
const LAYER_PRESENT_SURFACE: u32 = 4u;
const BOUNDS_GROUP: u32 = 256u;
// Flames sway this many elevation units above their column (terrain.wgsl's FLAME_SWAY_UNITS),
// and hidden layer vertices sit this far below the floor (HIDDEN_LAYER_DEPTH).
const VERTEX_RISE_UNITS: f32 = 1.5;
const VERTEX_DROP: f32 = 0.5;
const DRAW_ARGS_WORDS: u32 = 5u;

var<workgroup> group_low: atomic<u32>;
var<workgroup> group_high: atomic<u32>;
var<workgroup> group_layers: atomic<u32>;

// Order-preserving map of a non-negative float to u32 (heights are never negative).
fn height_bits(h: f32) -> u32 {
    return bitcast<u32>(max(h, 0.0));
}

@compute @workgroup_size(256)
fn chunk_bounds(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) local: u32) {
    let chunk = group.x;
    let chunks = u32(frame.window.z * frame.window.w);
    if (local == 0u) {
        atomicStore(&group_low, 0xffffffffu);
        atomicStore(&group_high, 0u);
        atomicStore(&group_layers, 0u);
    }
    workgroupBarrier();
    let slot = select(SLOT_NONE, window_slots[chunk], chunk < chunks);
    if (slot != SLOT_NONE) {
        let scale = frame.window_cells.z;
        var low = 0xffffffffu;
        var high = 0u;
        var layers = 0u;
        for (var index = local; index < CHUNK_CELLS; index += BOUNDS_GROUP) {
            let c = cells[slot * CHUNK_CELLS + index];
            let terrain = f32(cell_elevation(c)) * scale;
            let granular = terrain + f32(cell_granular_volume(c)) * scale;
            let fluid = granular + f32(cell_fluid_volume(c)) * scale;
            let surface = granular + max(1.0, f32(cell_surface_state(c))) * scale;
            low = min(low, height_bits(terrain));
            high = max(high, height_bits(max(max(fluid, surface), granular)));
            if (cell_granular_material(c) != 0u) {
                layers |= LAYER_PRESENT_GRANULAR;
            }
            if (cell_fluid_material(c) != 0u) {
                layers |= LAYER_PRESENT_FLUID;
            }
            if (cell_surface_material(c) != 0u) {
                layers |= LAYER_PRESENT_SURFACE;
            }
        }
        atomicMin(&group_low, low);
        atomicMax(&group_high, high);
        atomicOr(&group_layers, layers);
    }
    workgroupBarrier();
    if (local == 0u && chunk < chunks) {
        chunk_records[chunk] = ChunkRecord(
            bitcast<f32>(atomicLoad(&group_low)),
            bitcast<f32>(atomicLoad(&group_high)),
            atomicLoad(&group_layers),
            select(0u, 1u, slot != SLOT_NONE),
            0u,
            0u,
            0u,
            0u,
        );
    }
}

fn matrix_row(m: mat4x4<f32>, row: u32) -> vec4<f32> {
    return vec4<f32>(m[0][row], m[1][row], m[2][row], m[3][row]);
}

// Whether the box is at least partly inside the view frustum (left, right, bottom, top and
// near planes of the reverse-Z infinite projection; there is no far plane).
fn box_visible(low: vec3<f32>, high: vec3<f32>) -> bool {
    let m = view.view_proj;
    let w = matrix_row(m, 3u);
    let planes = array<vec4<f32>, 5>(
        w + matrix_row(m, 0u),
        w - matrix_row(m, 0u),
        w + matrix_row(m, 1u),
        w - matrix_row(m, 1u),
        w - matrix_row(m, 2u),
    );
    for (var i = 0u; i < 5u; i++) {
        let plane = planes[i];
        // The box corner farthest along the plane's normal.
        let corner = select(low, high, plane.xyz >= vec3<f32>(0.0));
        if (dot(plane.xyz, corner) + plane.w < 0.0) {
            return false;
        }
    }
    return true;
}

@compute @workgroup_size(64)
fn cull_chunks(@builtin(global_invocation_id) id: vec3<u32>) {
    let chunk = id.x;
    let width = u32(frame.window.z);
    let chunks = width * u32(frame.window.w);
    if (chunk >= chunks) {
        return;
    }
    let own = chunk_records[chunk];
    if (own.resident == 0u) {
        chunk_records[chunk].visible = 0u;
        return;
    }
    let at = vec2<i32>(i32(chunk % width), i32(chunk / width));
    var low = own.low;
    var high = own.high;
    var layers = own.layers;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let n = at + vec2<i32>(dx, dy);
            if (any(n < vec2<i32>(0)) || n.x >= i32(width) || n.y >= i32(frame.window.w)) {
                continue;
            }
            let record = chunk_records[u32(n.y) * width + u32(n.x)];
            if (record.resident == 0u) {
                continue;
            }
            low = min(low, record.low);
            high = max(high, record.high);
            layers |= record.layers;
        }
    }
    let scale = frame.window_cells.z;
    let size = f32(CHUNK_SIZE);
    // The chunk's vertex grid spans one cell past its far edges (the neighbours' first cells).
    let box_low = vec3<f32>(f32(at.x) * size, low - VERTEX_DROP, -(f32(at.y) * size + size + 1.0));
    let box_high = vec3<f32>(f32(at.x) * size + size + 1.0, high + VERTEX_RISE_UNITS * scale + 0.01, -f32(at.y) * size);
    var visible = 0u;
    if (box_visible(box_low, box_high)) {
        visible = 1u;
        if ((layers & LAYER_PRESENT_GRANULAR) != 0u) {
            visible |= 2u;
        }
        if ((layers & LAYER_PRESENT_FLUID) != 0u) {
            visible |= 4u;
        }
        if ((layers & LAYER_PRESENT_SURFACE) != 0u) {
            visible |= 8u;
        }
    }
    chunk_records[chunk].visible = visible;
}

const COMPACT_GROUP: u32 = 256u;
var<workgroup> partial: array<u32, 256>;

@compute @workgroup_size(256)
fn compact_lists(@builtin(local_invocation_index) local: u32) {
    let chunks = u32(frame.window.z * frame.window.w);
    let per = (chunks + COMPACT_GROUP - 1u) / COMPACT_GROUP;
    let first = local * per;
    let last = min(first + per, chunks);
    for (var layer = 0u; layer < 4u; layer++) {
        let bit = 1u << layer;
        var count = 0u;
        for (var chunk = first; chunk < last; chunk++) {
            let shown = chunk_records[chunk].resident != 0u && (chunk_records[chunk].visible & bit) != 0u;
            count += select(0u, 1u, shown);
        }
        partial[local] = count;
        workgroupBarrier();
        // An exclusive prefix sum of the 256 counts, by one invocation: 256 additions.
        if (local == 0u) {
            var running = 0u;
            for (var i = 0u; i < COMPACT_GROUP; i++) {
                let value = partial[i];
                partial[i] = running;
                running += value;
            }
            atomicStore(&draw_args[layer * DRAW_ARGS_WORDS + 1u], running);
        }
        workgroupBarrier();
        var at = partial[local];
        for (var chunk = first; chunk < last; chunk++) {
            let shown = chunk_records[chunk].resident != 0u && (chunk_records[chunk].visible & bit) != 0u;
            if (shown) {
                layer_lists[layer * chunks + at] = chunk;
                at += 1u;
            }
        }
        workgroupBarrier();
    }
}
