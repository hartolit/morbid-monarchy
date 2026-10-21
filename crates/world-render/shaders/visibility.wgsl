// Which window chunks each landscape layer draws this frame, derived from the cells every frame
// (so nothing is ever stale after an edit, a load or a window move) and the camera the camera
// pass placed, and which chunks' cached lighting the relight pass must refresh:
//
// chunk_bounds   one workgroup per window chunk: the height range its layer vertices can take,
//                which layers its cells hold, and a signature of its cells compared with the
//                last frame's (a change, a load or an eviction)
// cull_chunks    one invocation per window chunk: frustum test of the bounds widened by the
//                3 x 3 neighbourhood (a chunk's edge vertices borrow its neighbours' heights and
//                materials), then the layers that can show; the chunk's lighting is out of date
//                when a chunk its lighting reads changed
// compact_lists  one workgroup: each layer's visible chunks in chunk order (a prefix sum) and
//                its instance count in the layer's indirect draw, and the chunks to relight
//
// A vertex's lighting reads cells within 6 cells of it (occlusion, layer edges) and up to 121
// cells towards the sun (the shadow march, lighting.wgsl), so a chunk's lighting depends on its
// neighbours and on the chunks within two chunk widths towards the sun. A chunk out of date
// stays so until it is drawn; the relight pass refreshes only chunks that are drawn.
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
// In a chunk record's `visible`: its cached lighting is out of date.
const LIGHTING_DIRTY: u32 = 16u;
const MIN_SUN_HORIZONTAL: f32 = 0.0001;
// The shadow march covers about 1.9 chunk widths; the cull samples the swept chunk box every
// quarter chunk out to two.
const SHADOW_REACH_SAMPLES: u32 = 8u;
const SHADOW_REACH_STEP: f32 = 0.25;

var<workgroup> group_low: atomic<u32>;
var<workgroup> group_high: atomic<u32>;
var<workgroup> group_layers: atomic<u32>;
var<workgroup> group_sum: atomic<u32>;
var<workgroup> group_mix: atomic<u32>;

// A 32-bit integer hash (lowbias32).
fn mix32(value: u32) -> u32 {
    var h = value;
    h ^= h >> 16u;
    h *= 0x7feb352du;
    h ^= h >> 15u;
    h *= 0x846ca68bu;
    h ^= h >> 16u;
    return h;
}

// Two lanes of a cell's contribution to its chunk's signature. They are combined by a wrapping
// sum and an exclusive or, which do not depend on the order the invocations add them in; the
// index makes the signature notice two cells trading places. A change detector, not a
// cryptographic one: an edit that kept both lanes would leave that chunk's lighting stale.
fn signature_sum_lane(c: vec2<u32>, index: u32) -> u32 {
    return mix32(c.x ^ mix32(c.y + index * 0x9e3779b9u));
}

fn signature_mix_lane(c: vec2<u32>, index: u32) -> u32 {
    return mix32(c.y ^ mix32(c.x ^ (index * 0x85ebca6bu + 0x632be5abu)));
}

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
        atomicStore(&group_sum, 0u);
        atomicStore(&group_mix, 0u);
    }
    workgroupBarrier();
    let slot = select(SLOT_NONE, window_slots[chunk], chunk < chunks);
    if (slot != SLOT_NONE) {
        let scale = frame.window_cells.z;
        var low = 0xffffffffu;
        var high = 0u;
        var layers = 0u;
        var sum = 0u;
        var mixed = 0u;
        for (var index = local; index < CHUNK_CELLS; index += BOUNDS_GROUP) {
            let c = cells[slot * CHUNK_CELLS + index];
            sum += signature_sum_lane(c, index);
            mixed ^= signature_mix_lane(c, index);
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
        atomicAdd(&group_sum, sum);
        atomicXor(&group_mix, mixed);
    }
    workgroupBarrier();
    if (local == 0u && chunk < chunks) {
        let last = chunk_records[chunk];
        let resident = select(0u, 1u, slot != SLOT_NONE);
        let signature_sum = atomicLoad(&group_sum);
        let signature_mix = atomicLoad(&group_mix);
        let changed = last.signature_sum != signature_sum
            || last.signature_mix != signature_mix
            || last.resident != resident;
        chunk_records[chunk] = ChunkRecord(
            bitcast<f32>(atomicLoad(&group_low)),
            bitcast<f32>(atomicLoad(&group_high)),
            atomicLoad(&group_layers),
            resident,
            last.visible & LIGHTING_DIRTY,
            signature_sum,
            signature_mix,
            select(0u, 1u, changed),
        );
    }
}

// Whether a chunk whose cells the lighting of chunk `at` can read changed this frame: the
// 3 x 3 neighbourhood, and with heightfield shadows the neighbourhoods of the chunk box swept
// towards the sun over the march's reach.
fn lighting_inputs_changed(at: vec2<i32>, width: u32) -> bool {
    let sun = frame.sun_direction.xyz;
    let grid = vec2<f32>(sun.x, -sun.z);
    let horizontal = length(grid);
    let shadows = frame.ground_color.w > 0.0 && sun.y > 0.0 && horizontal >= MIN_SUN_HORIZONTAL;
    let step = select(vec2<f32>(0.0), grid / max(horizontal, MIN_SUN_HORIZONTAL), shadows);
    let samples = select(0u, SHADOW_REACH_SAMPLES, shadows);
    for (var k = 0u; k <= samples; k++) {
        let centre = vec2<i32>(floor(vec2<f32>(at) + 0.5 + step * (SHADOW_REACH_STEP * f32(k))));
        for (var dy = -1; dy <= 1; dy++) {
            for (var dx = -1; dx <= 1; dx++) {
                let n = centre + vec2<i32>(dx, dy);
                if (any(n < vec2<i32>(0)) || n.x >= i32(width) || n.y >= i32(frame.window.w)) {
                    continue;
                }
                if (chunk_records[u32(n.y) * width + u32(n.x)].changed != 0u) {
                    return true;
                }
            }
        }
    }
    return false;
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
        // Its lighting is relit when it is resident again: that is a change.
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
    let dirty = (own.visible & LIGHTING_DIRTY) != 0u
        || (frame.offsets.w & LIGHTING_RELIGHT_ALL) != 0u
        || lighting_inputs_changed(at, width);
    var visible = select(0u, LIGHTING_DIRTY, dirty);
    if (box_visible(box_low, box_high)) {
        visible |= 1u;
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
        // An exclusive prefix sum of the 256 counts, by one invocation: 256 additions. The
        // unculled comparison path keeps the instance counts the CPU wrote.
        if (local == 0u) {
            var running = 0u;
            for (var i = 0u; i < COMPACT_GROUP; i++) {
                let value = partial[i];
                partial[i] = running;
                running += value;
            }
            if (frame.offsets.z == 0u) {
                atomicStore(&draw_args[layer * DRAW_ARGS_WORDS + 1u], running);
            }
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
    // The chunks to relight: out of date and drawn (every resident one on the unculled path),
    // unless lighting is computed per vertex this frame. Listing one brings it up to date.
    var count = 0u;
    for (var chunk = first; chunk < last; chunk++) {
        count += select(0u, 1u, relight_wanted(chunk_records[chunk]));
    }
    partial[local] = count;
    workgroupBarrier();
    if (local == 0u) {
        var running = 0u;
        for (var i = 0u; i < COMPACT_GROUP; i++) {
            let value = partial[i];
            partial[i] = running;
            running += value;
        }
        relight_args[0] = running;
        relight_args[1] = 1u;
        relight_args[2] = 1u;
    }
    workgroupBarrier();
    var at = partial[local];
    for (var chunk = first; chunk < last; chunk++) {
        if (relight_wanted(chunk_records[chunk])) {
            relight_list[at] = chunk;
            at += 1u;
            chunk_records[chunk].visible &= ~LIGHTING_DIRTY;
        }
    }
}

fn relight_wanted(record: ChunkRecord) -> bool {
    let drawn = frame.offsets.z != 0u || (record.visible & 1u) != 0u;
    return record.resident != 0u
        && (record.visible & LIGHTING_DIRTY) != 0u
        && drawn
        && (frame.offsets.w & LIGHTING_LIVE) == 0u;
}
