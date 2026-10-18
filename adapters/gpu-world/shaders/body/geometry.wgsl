// The physical surface and the shapes bodies have, shared by the body substeps and the ray
// queries. Needs the globals `tick`, `body_params`, `cells` and `window_slots`.
//
// Window-local render coordinates: x is the cell x, z is minus the cell y, y is up. The surface
// is the triangulated heightfield the landscape mesh draws: a vertex at every cell, at the
// height of the column's solid top (bedrock, loose material and ice) times the elevation
// scale, and each quad between `(x, y)` and `(x + 1, y + 1)` split along the diagonal from
// `(x + 1, y)` to `(x, y + 1)` (monarch-engine's `GridPhysicsApi::quad_triangles_with`). A
// column that is not resident has no height: no triangle uses it.

const MISSING_HEIGHT: f32 = -3.0e38;

fn is_missing(h: f32) -> bool {
    return h < -1.0e38;
}

struct Column {
    present: bool,
    value: vec2<u32>,
    index: u32,
}

fn column(cell: vec2<i32>) -> Column {
    let slot = resident_slot(cell);
    if (slot == SLOT_NONE) {
        return Column(false, vec2<u32>(0u), 0u);
    }
    let index = cell_index(slot, cell);
    return Column(true, cells[index], index);
}

// WorldCell::solid_top: bedrock, loose material and, when the liquid is frozen, the ice.
fn solid_top(c: vec2<u32>) -> u32 {
    var frozen = 0u;
    if (fluid_is_solid(cell_fluid_material(c))) {
        frozen = cell_fluid_volume(c);
    }
    return cell_elevation(c) + cell_granular_volume(c) + frozen;
}

fn top_height(c: vec2<u32>) -> f32 {
    return f32(solid_top(c)) * body_params.elevation_scale;
}

fn surface_height(cell: vec2<i32>) -> f32 {
    let col = column(cell);
    if (!col.present) {
        return MISSING_HEIGHT;
    }
    return top_height(col.value);
}

// world_model::material::ground_softness.
fn ground_softness(c: vec2<u32>) -> f32 {
    if (fluid_is_solid(cell_fluid_material(c))) {
        return 0.0;
    }
    if (cell_granular_volume(c) > 0u) {
        return loose_softness(cell_granular_material(c));
    }
    return bedrock_softness(cell_terrain_material(c));
}

struct Liquid {
    present: bool,
    material: u32,
    volume: u32,
    surface: f32,
}

// The liquid occupying a column and the height of its free surface (GridPhysicsApi's
// get_fluid_surface): none for a dry, frozen or empty column.
fn liquid_in(c: vec2<u32>) -> Liquid {
    let material = cell_fluid_material(c);
    let volume = cell_fluid_volume(c);
    if (material == FLUID_EMPTY || fluid_is_solid(material) || volume == 0u) {
        return Liquid(false, 0u, 0u, 0.0);
    }
    let units = cell_elevation(c) + cell_granular_volume(c) + volume;
    return Liquid(true, material, volume, f32(units) * body_params.elevation_scale);
}

fn liquid_at(cell: vec2<i32>) -> Liquid {
    let col = column(cell);
    if (!col.present) {
        return Liquid(false, 0u, 0u, 0.0);
    }
    return liquid_in(col.value);
}

// f32::round: halves round away from zero (WGSL's round rounds them to even).
fn round_away(x: f32) -> f32 {
    let magnitude = abs(x);
    let whole = floor(magnitude);
    return sign(x) * (whole + select(0.0, 1.0, magnitude - whole >= 0.5));
}

// world_model::spatial::nearest_cell and quad_cell, in window-local coordinates.
fn nearest_cell(p: vec3<f32>) -> vec2<i32> {
    return vec2<i32>(i32(round_away(p.x)), i32(round_away(-p.z)));
}

fn quad_cell(p: vec3<f32>) -> vec2<i32> {
    return vec2<i32>(i32(floor(p.x)), i32(floor(-p.z)));
}

fn vertex(cell: vec2<i32>, height: f32) -> vec3<f32> {
    return vec3<f32>(f32(cell.x), height, -f32(cell.y));
}

// GridPhysicsApi::get_interpolated_floor_height; MISSING_HEIGHT when a corner is not resident.
fn interpolated_floor(x: f32, z: f32) -> f32 {
    let grid = vec2<f32>(floor(x), floor(-z));
    let cell = vec2<i32>(grid);
    let h00 = surface_height(cell);
    let h10 = surface_height(cell + vec2<i32>(1, 0));
    let h01 = surface_height(cell + vec2<i32>(0, 1));
    let h11 = surface_height(cell + vec2<i32>(1, 1));
    if (is_missing(h00) || is_missing(h10) || is_missing(h01) || is_missing(h11)) {
        return MISSING_HEIGHT;
    }
    let tx = x - grid.x;
    let ty = -z - grid.y;
    if (tx + ty <= 1.0) {
        return h00 + (h10 - h00) * tx + (h01 - h00) * ty;
    }
    return h11 + (h01 - h11) * (1.0 - tx) + (h10 - h11) * (1.0 - ty);
}

// The closest point to `p` on triangle `abc`, by Voronoi region classification (Ericson,
// Real-Time Collision Detection, 5.1.5), as the baseline computes it.
fn closest_point_on_triangle(p: vec3<f32>, a: vec3<f32>, b: vec3<f32>, c: vec3<f32>) -> vec3<f32> {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if (d1 <= 0.0 && d2 <= 0.0) {
        return a;
    }
    let bp = p - b;
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if (d3 >= 0.0 && d4 <= d3) {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if (vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0) {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if (d6 >= 0.0 && d5 <= d6) {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if (vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0) {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if (va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0) {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let inverse = 1.0 / (va + vb + vc);
    return a + ab * (vb * inverse) + ac * (vc * inverse);
}

// world_model::body::submerged_volume_fraction.
fn submerged_volume_fraction(depth: f32, radius: f32) -> f32 {
    if (radius <= 0.0) {
        return 0.0;
    }
    let h = clamp(depth, 0.0, 2.0 * radius);
    return h * h * (3.0 * radius - h) / (4.0 * radius * radius * radius);
}

// A unit direction in one word: octahedral, two 16-bit snorms.
fn sign_not_zero(v: vec2<f32>) -> vec2<f32> {
    return select(vec2<f32>(-1.0), vec2<f32>(1.0), v >= vec2<f32>(0.0));
}

fn encode_direction(n: vec3<f32>) -> u32 {
    var p = n.xy / (abs(n.x) + abs(n.y) + abs(n.z));
    if (n.z < 0.0) {
        p = (1.0 - abs(p.yx)) * sign_not_zero(p);
    }
    return pack2x16snorm(p);
}

fn decode_direction(word: u32) -> vec3<f32> {
    let p = unpack2x16snorm(word);
    var n = vec3<f32>(p.x, p.y, 1.0 - abs(p.x) - abs(p.y));
    let fold = max(-n.z, 0.0);
    n.x += select(fold, -fold, n.x >= 0.0);
    n.y += select(fold, -fold, n.y >= 0.0);
    return normalize(n);
}

fn normalize_or(v: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let length_squared = dot(v, v);
    if (!(length_squared > 0.0) || length_squared > 3.0e38) {
        return fallback;
    }
    return v * inverseSqrt(length_squared);
}

// The two triangles of quad `cell` from its corner heights (h00, h10, h01, h11).
fn quad_triangle(cell: vec2<i32>, heights: vec4<f32>, second: bool) -> array<vec3<f32>, 3> {
    let v10 = vertex(cell + vec2<i32>(1, 0), heights.y);
    let v01 = vertex(cell + vec2<i32>(0, 1), heights.z);
    if (second) {
        return array<vec3<f32>, 3>(v10, vertex(cell + vec2<i32>(1, 1), heights.w), v01);
    }
    return array<vec3<f32>, 3>(vertex(cell, heights.x), v10, v01);
}

// Where a ray meets a triangle (Möller–Trumbore), or a negative distance. The barycentric
// bounds are widened by a hair so a ray through a shared edge hits one of its triangles.
fn ray_triangle(origin: vec3<f32>, direction: vec3<f32>, t: array<vec3<f32>, 3>) -> f32 {
    let e1 = t[1] - t[0];
    let e2 = t[2] - t[0];
    let p = cross(direction, e2);
    let det = dot(e1, p);
    if (abs(det) < 1e-12) {
        return -1.0;
    }
    let inverse = 1.0 / det;
    let s = origin - t[0];
    let u = dot(s, p) * inverse;
    let q = cross(s, e1);
    let v = dot(direction, q) * inverse;
    if (u < -1e-6 || v < -1e-6 || u + v > 1.0 + 1e-6) {
        return -1.0;
    }
    return dot(e2, q) * inverse;
}

// Where a ray enters a sphere: 0 when it starts inside, negative when it misses.
fn ray_sphere(origin: vec3<f32>, direction: vec3<f32>, center: vec3<f32>, radius: f32) -> f32 {
    let offset = origin - center;
    let b = dot(offset, direction);
    let c = dot(offset, offset) - radius * radius;
    if (c <= 0.0) {
        return 0.0;
    }
    let discriminant = b * b - c;
    if (discriminant < 0.0 || b > 0.0) {
        return -1.0;
    }
    return -b - sqrt(discriminant);
}

// world_model::body::mul_div_floor: floor(a * b / c) for a, b <= c < 2^21.
fn mul_div_floor(a: u32, b: u32, c: u32) -> u32 {
    if (c == 0u) {
        return 0u;
    }
    let high = a >> 10u;
    let low = a & 0x3FFu;
    let x = high * b;
    return (x / c) * 1024u + ((x % c) * 1024u + low * b) / c;
}

// A body's centre in window-local render coordinates: its anchor chunk relative to the
// window's lowest chunk, plus its position inside the anchor. `far` is set when the anchor is
// too far from the window for the difference to be exact (it is then outside it).
struct Placement {
    position: vec3<f32>,
    far: bool,
}

fn chunk_offset(anchor: i32, origin: i32) -> vec2<i32> {
    // Wrapping difference and its overflow flag: the true difference does not fit an i32 when
    // the operands' signs differ and the result's sign differs from the anchor's.
    let d = anchor - origin;
    let overflow = ((anchor ^ origin) & (anchor ^ d)) < 0;
    return vec2<i32>(d, select(0, 1, overflow || d < -1048576 || d > 1048576));
}

fn place(s: BodyState) -> Placement {
    let dx = chunk_offset(s.anchor_x, tick.origin_key_x);
    let dy = chunk_offset(s.anchor_y, tick.origin_key_y);
    let size = f32(CHUNK_SIZE);
    let cell_x = f32(dx.x) * size + s.local_x;
    let cell_y = f32(dy.x) * size + s.local_y;
    return Placement(vec3<f32>(cell_x, s.height, -cell_y), dx.y != 0 || dy.y != 0);
}

// Stores a window-local render position into a body's anchor and in-anchor position, moving
// the anchor so the in-anchor cell coordinates stay in [0, 64). Returns the anchor shift in
// cells, which the caller applies to anything else kept relative to the anchor.
fn anchor_position(s: ptr<function, BodyState>, position: vec3<f32>) -> vec2<f32> {
    let size = f32(CHUNK_SIZE);
    let dx = chunk_offset((*s).anchor_x, tick.origin_key_x).x;
    let dy = chunk_offset((*s).anchor_y, tick.origin_key_y).x;
    let local = vec2<f32>(position.x - f32(dx) * size, -position.z - f32(dy) * size);
    let shift = floor(local / size);
    (*s).anchor_x += i32(shift.x);
    (*s).anchor_y += i32(shift.y);
    (*s).local_x = local.x - shift.x * size;
    (*s).local_y = local.y - shift.y * size;
    (*s).height = position.y;
    return shift * size;
}
