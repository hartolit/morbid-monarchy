// Terrain contact, one workgroup of 64 invocations per body: the baseline's
// `resolve_terrain_contacts`, with the cell reads split across the workgroup.
//
// contact_prepare   reads cells only. Sweeps a fast body against the surface in steps of at most
//                   TERRAIN_SWEEP_STEP and stops it at the first step that penetrates; lifts a
//                   buried centre onto the surface; fits and filters the ground reference plane;
//                   probes with the slop and decides: a crater after a hard landing, a trail
//                   while it rolls on soft ground; then claims the footprint cells its decision
//                   and its foliage clearing may change (deform.wgsl).
// contact_apply     (deform.wgsl) changes the cells it owns.
// contact_response  reads cells only, after the apply pass committed them: up to
//                   CONTACT_ITERATIONS position pushes out of the deepest overlap, then the
//                   velocity response and the grounding against the final contacts.
//
// A probe gathers every surface triangle within `reach` of the centre in the baseline's order
// (quad rows, then columns, then the quad's two triangles) and builds the baseline's manifold:
// the deepest contact over all of them, and up to MAX_MANIFOLD_CONTACTS distinct normals in
// that order. Each invocation takes a contiguous run of triangles and keeps its contacts
// privately; a prefix sum of the counts lets them be merged in order, in batches of
// PROBE_LIST, by one invocation.

const PROBE_GRID: u32 = 20u;
const PROBE_LIST: u32 = 128u;
const PROBE_PER_LANE: u32 = 12u;
const NO_DEPTH: f32 = -3.0e38;

// What a workgroup knows about its body; one invocation fills it, then every invocation reads
// it with workgroupUniformLoad, so loop bounds and barriers stay uniform.
struct Frame {
    enabled: u32,
    was_grounded: u32,
    radius: f32,
    mass: f32,
    center: vec3<f32>,
    velocity: vec3<f32>,
    previous: vec3<f32>,
    plane_valid: u32,
    // Window-local grid coordinates (cell x, cell y) of the plane's origin.
    plane_origin: vec2<f32>,
    plane_height: f32,
    plane_slope: vec2<f32>,
    sinkage_per_softness: f32,
    max_sinkage: f32,
    decision: u32,
}

var<workgroup> frame: Frame;

struct ManifoldInfo {
    total: u32,
    count: u32,
    deepest_depth: f32,
    deepest_normal: vec3<f32>,
}

var<workgroup> manifold: ManifoldInfo;
var<workgroup> manifold_normals: array<vec3<f32>, 16>;
var<workgroup> manifold_depths: array<f32, 16>;
var<workgroup> probe_heights: array<f32, 400>;
var<workgroup> probe_lane_count: array<u32, 64>;
var<workgroup> probe_lane_depth: array<f32, 64>;
var<workgroup> probe_lane_normal: array<vec3<f32>, 64>;
var<workgroup> probe_list: array<vec4<f32>, 128>;

fn plane_at(f: Frame, cell: vec2<i32>) -> f32 {
    return f.plane_height + dot(f.plane_slope, vec2<f32>(cell) - f.plane_origin);
}

// GroundModel::column_yield: how far a column can sink, and the floor it bottoms out at.
fn column_sinkage(f: Frame, c: vec2<u32>) -> f32 {
    if (f.plane_valid == 0u) {
        return 0.0;
    }
    return min(ground_softness(c) * f.sinkage_per_softness, f.max_sinkage);
}

// GroundModel::collision_height, or the plain floor height when `yielded` is false.
fn collision_height(f: Frame, cell: vec2<i32>, yielded: bool) -> f32 {
    let col = column(cell);
    if (!col.present) {
        return MISSING_HEIGHT;
    }
    let top = top_height(col.value);
    if (!yielded) {
        return top;
    }
    let sinkage = column_sinkage(f, col.value);
    if (sinkage > 0.0) {
        let floor_at = plane_at(f, cell) - sinkage;
        if (top > floor_at) {
            return max(top - sinkage, floor_at);
        }
    }
    return top;
}

// The contact one triangle makes with a sphere at `center`: normal and depth, or depth
// NO_DEPTH when it lies beyond `reach`.
fn triangle_contact(center: vec3<f32>, radius: f32, reach: f32, t: array<vec3<f32>, 3>) -> vec4<f32> {
    let closest = closest_point_on_triangle(center, t[0], t[1], t[2]);
    let offset = center - closest;
    let distance = length(offset);
    if (distance >= reach) {
        return vec4<f32>(0.0, 1.0, 0.0, NO_DEPTH);
    }
    let raw_face = normalize_or(cross(t[1] - t[0], t[2] - t[0]), vec3<f32>(0.0, 1.0, 0.0));
    let face = select(raw_face, -raw_face, raw_face.y < 0.0);
    var normal = face;
    if (distance > DEGENERATE_DISTANCE) {
        let direction = offset / distance;
        if (dot(direction, face) < INTERNAL_EDGE_COSINE) {
            normal = direction;
        }
    }
    return vec4<f32>(normal, radius - distance);
}

// Probes the surface around `center`; every invocation must call it. The result is in
// `manifold`, `manifold_normals` and `manifold_depths`, read after the closing barrier.
fn probe(f: Frame, center: vec3<f32>, reach: f32, yielded: bool, li: u32) {
    let min_cell = vec2<i32>(i32(floor(center.x - reach)), i32(floor(-center.z - reach)));
    let max_cell = vec2<i32>(i32(floor(center.x + reach)), i32(floor(-center.z + reach)));
    let size = max_cell - min_cell + vec2<i32>(1);
    let verts = u32(size.x + 1);
    let vertex_count = verts * u32(size.y + 1);
    for (var i = li; i < vertex_count; i += 64u) {
        let cell = min_cell + vec2<i32>(i32(i % verts), i32(i / verts));
        probe_heights[i] = collision_height(f, cell, yielded);
    }
    workgroupBarrier();

    let triangles = u32(size.x * size.y) * 2u;
    let per = (triangles + 63u) / 64u;
    let first = min(li * per, triangles);
    let last = min(first + per, triangles);
    var mine: array<vec4<f32>, PROBE_PER_LANE>;
    var count = 0u;
    var best = vec4<f32>(0.0, 1.0, 0.0, NO_DEPTH);
    for (var t = first; t < last; t++) {
        let quad = t / 2u;
        let q = vec2<u32>(quad % u32(size.x), quad / u32(size.x));
        let at = q.y * verts + q.x;
        let heights = vec4<f32>(
            probe_heights[at],
            probe_heights[at + 1u],
            probe_heights[at + verts],
            probe_heights[at + verts + 1u],
        );
        if (is_missing(heights.x) || is_missing(heights.y) || is_missing(heights.z) || is_missing(heights.w)) {
            continue;
        }
        let triangle = quad_triangle(min_cell + vec2<i32>(q), heights, (t & 1u) == 1u);
        let contact = triangle_contact(center, f.radius, reach, triangle);
        if (contact.w == NO_DEPTH) {
            continue;
        }
        mine[count] = contact;
        count += 1u;
        if (contact.w > best.w) {
            best = contact;
        }
    }
    probe_lane_count[li] = count;
    probe_lane_depth[li] = best.w;
    probe_lane_normal[li] = best.xyz;
    workgroupBarrier();
    if (li == 0u) {
        var total = 0u;
        var deepest = NO_DEPTH;
        var deepest_normal = vec3<f32>(0.0, 1.0, 0.0);
        for (var lane = 0u; lane < 64u; lane++) {
            total += probe_lane_count[lane];
            if (probe_lane_count[lane] > 0u && probe_lane_depth[lane] > deepest) {
                deepest = probe_lane_depth[lane];
                deepest_normal = probe_lane_normal[lane];
            }
        }
        manifold = ManifoldInfo(total, 0u, deepest, deepest_normal);
    }
    var offset = 0u;
    for (var lane = 0u; lane < li; lane++) {
        offset += probe_lane_count[lane];
    }
    let info = workgroupUniformLoad(&manifold);
    for (var start = 0u; start < info.total; start += PROBE_LIST) {
        for (var k = 0u; k < count; k++) {
            let ordinal = offset + k;
            if (ordinal >= start && ordinal < start + PROBE_LIST) {
                probe_list[ordinal - start] = mine[k];
            }
        }
        workgroupBarrier();
        if (li == 0u) {
            let batch = min(PROBE_LIST, info.total - start);
            for (var k = 0u; k < batch; k++) {
                let contact = probe_list[k];
                var merged = false;
                for (var m = 0u; m < manifold.count; m++) {
                    if (!merged && dot(manifold_normals[m], contact.xyz) >= DUPLICATE_NORMAL_COSINE) {
                        manifold_depths[m] = max(manifold_depths[m], contact.w);
                        merged = true;
                    }
                }
                if (!merged && manifold.count < MAX_MANIFOLD_CONTACTS) {
                    manifold_normals[manifold.count] = contact.xyz;
                    manifold_depths[manifold.count] = contact.w;
                    manifold.count += 1u;
                }
            }
        }
        workgroupBarrier();
    }
}

// ContactManifold::ground_normal.
fn ground_normal() -> vec3<f32> {
    var sum = vec3<f32>(0.0);
    for (var m = 0u; m < manifold.count; m++) {
        if (manifold_depths[m] >= manifold.deepest_depth - GROUND_NORMAL_BAND) {
            sum += manifold_normals[m];
        }
    }
    return normalize_or(sum, manifold.deepest_normal);
}

fn gravity_length() -> f32 {
    return abs(GRAVITY_Y);
}

// resolve_contact_velocity: the bounce, the one-sided sweeps over the contacts, then rolling
// friction along the ground. One invocation.
fn contact_velocity(velocity: vec3<f32>, dt: f32) -> vec3<f32> {
    var v = velocity;
    let normal = ground_normal();
    let approach = dot(v, normal);
    if (manifold.deepest_depth >= -CONSTRAINT_TOLERANCE && approach < 0.0) {
        let settle = max(gravity_length() * dt + GROUNDING_DRIFT_BUFFER, MINIMUM_BOUNCE_SPEED);
        let restitution = select(0.0, IMPACT_RESTITUTION, -approach > settle);
        v -= (1.0 + restitution) * approach * normal;
    }
    for (var sweep = 0u; sweep < VELOCITY_ITERATIONS; sweep++) {
        for (var m = 0u; m < manifold.count; m++) {
            let n = manifold_normals[m];
            let speed = dot(v, n);
            if (manifold_depths[m] >= -CONSTRAINT_TOLERANCE && speed < 0.0) {
                v -= speed * n;
            }
        }
    }
    if (normal.y < GROUND_NORMAL_MIN_Y) {
        return v;
    }
    let normal_velocity = dot(v, normal) * normal;
    let tangential = v - normal_velocity;
    let slope = length(vec2<f32>(normal.x, normal.z)) / normal.y;
    let speed = length(tangential);
    let is_static = speed < STATIC_FRICTION_SPEED && slope <= STATIC_FRICTION_COEFFICIENT;
    var retained = vec3<f32>(0.0);
    if (!is_static && speed > 0.0) {
        let loss = ROLLING_RESISTANCE * gravity_length() * normal.y * dt;
        let damped = max(speed - loss, 0.0) * exp(-GROUND_DAMPING_RATE * dt);
        retained = tangential * (damped / speed);
    }
    return normal_velocity + retained;
}

// The workgroup's frame for body `b`, from its state, configuration and work.
fn load_contact_frame(b: u32) -> Frame {
    var f: Frame;
    let s = load_body_state(state_base(b));
    let flags = bodies[work_word(b, WORK_FLAGS)];
    f.enabled = select(0u, 1u, (s.flags & BODY_ALIVE) != 0u && (flags & WORK_ACTIVE) != 0u);
    f.was_grounded = select(0u, 1u, (flags & WORK_WAS_GROUNDED) != 0u);
    let config = load_body_config(config_base(b));
    f.radius = config.radius;
    f.mass = config.mass;
    f.center = work_vec3(b, WORK_POS_A);
    f.velocity = work_vec3(b, WORK_VEL_A);
    f.previous = work_vec3(b, WORK_PREV);
    f.plane_valid = select(0u, 1u, (s.flags & BODY_PLANE_VALID) != 0u);
    let anchor = place(s).position - vec3<f32>(s.local_x, s.height, -s.local_y);
    f.plane_origin = vec2<f32>(anchor.x, -anchor.z) + vec2<f32>(s.plane_origin_x, s.plane_origin_y);
    f.plane_height = s.plane_height;
    f.plane_slope = vec2<f32>(s.plane_slope_x, s.plane_slope_y);
    f.sinkage_per_softness = work_f32(b, WORK_SINKAGE_PER_SOFTNESS);
    f.max_sinkage = work_f32(b, WORK_MAX_SINKAGE);
    f.decision = bodies[work_word(b, WORK_DECISION)];
    return f;
}

struct Plane {
    valid: bool,
    origin: vec2<f32>,
    height: f32,
    slope: vec2<f32>,
}

fn plane_value(p: Plane, cell: vec2<f32>) -> f32 {
    return p.height + dot(p.slope, cell - p.origin);
}

fn det3(a: mat3x3<f32>) -> f32 {
    // Row-major as the baseline writes it: a[column][row] holds row `row`, column `column`.
    return a[0][0] * (a[1][1] * a[2][2] - a[2][1] * a[1][2])
        - a[1][0] * (a[0][1] * a[2][2] - a[2][1] * a[0][2])
        + a[2][0] * (a[0][1] * a[1][2] - a[1][1] * a[0][2]);
}

// ReferencePlane::solve over the samples selected by `mask`.
fn solve_plane(origin: vec2<f32>, cells: array<vec2<f32>, 12>, heights: array<f32, 12>, mask: u32) -> Plane {
    var n = 0.0;
    var sx = 0.0;
    var sy = 0.0;
    var sxx = 0.0;
    var syy = 0.0;
    var sxy = 0.0;
    var sh = 0.0;
    var shx = 0.0;
    var shy = 0.0;
    for (var k = 0u; k < 12u; k++) {
        if ((mask & (1u << k)) == 0u) {
            continue;
        }
        let d = cells[k] - origin;
        n += 1.0;
        sx += d.x;
        sy += d.y;
        sxx += d.x * d.x;
        syy += d.y * d.y;
        sxy += d.x * d.y;
        sh += heights[k];
        shx += heights[k] * d.x;
        shy += heights[k] * d.y;
    }
    if (n < f32(RUT_MIN_REFERENCE_SAMPLES)) {
        return Plane(false, origin, 0.0, vec2<f32>(0.0));
    }
    // Columns of the matrix [[n, sx, sy], [sx, sxx, sxy], [sy, sxy, syy]] (symmetric).
    let m = mat3x3<f32>(vec3<f32>(n, sx, sy), vec3<f32>(sx, sxx, sxy), vec3<f32>(sy, sxy, syy));
    let determinant = det3(m);
    if (abs(determinant) < PLANE_SINGULARITY) {
        return Plane(false, origin, 0.0, vec2<f32>(0.0));
    }
    let rhs = vec3<f32>(sh, shx, shy);
    var m0 = m;
    m0[0] = rhs;
    var m1 = m;
    m1[1] = rhs;
    var m2 = m;
    m2[2] = rhs;
    return Plane(true, origin, det3(m0) / determinant, vec2<f32>(det3(m1) / determinant, det3(m2) / determinant));
}

// ReferencePlane::fit: least squares through twelve samples on a ring, refitted once without
// the samples far from the first fit.
fn fit_plane(center: vec3<f32>, ring: f32) -> Plane {
    let origin = vec2<f32>(center.x, -center.z);
    var cells: array<vec2<f32>, 12>;
    var heights: array<f32, 12>;
    var mask = 0u;
    for (var k = 0u; k < RUT_REFERENCE_SAMPLES; k++) {
        let angle = f32(k) / f32(RUT_REFERENCE_SAMPLES) * 6.28318530717958647692;
        let cell = vec2<i32>(
            i32(round_away(origin.x + cos(angle) * ring)),
            i32(round_away(origin.y + sin(angle) * ring)),
        );
        let h = surface_height(cell);
        cells[k] = vec2<f32>(cell);
        heights[k] = h;
        if (!is_missing(h)) {
            mask |= 1u << k;
        }
    }
    let first = solve_plane(origin, cells, heights, mask);
    if (!first.valid) {
        return first;
    }
    var undisturbed = 0u;
    for (var k = 0u; k < 12u; k++) {
        if ((mask & (1u << k)) != 0u && abs(heights[k] - plane_value(first, cells[k])) <= RUT_TRIM_DEPTH) {
            undisturbed |= 1u << k;
        }
    }
    let second = solve_plane(origin, cells, heights, undisturbed);
    if (second.valid) {
        return second;
    }
    return first;
}

@compute @workgroup_size(64, 1, 1)
fn contact_prepare(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let b = wg.x;
    if (li == 0u) {
        frame = load_contact_frame(b);
    }
    var f = workgroupUniformLoad(&frame);
    if (f.enabled == 0u || b >= body_params.body_count) {
        return;
    }
    let dt = body_params.substep_seconds;

    // A fast body sweeps the surface in short steps and stops where it first penetrates.
    var center = f.center;
    let steps = u32(ceil(distance(center, f.previous) / TERRAIN_SWEEP_STEP));
    var swept = false;
    if (steps > 1u) {
        for (var k = 1u; k <= steps; k++) {
            let at = mix(f.previous, f.center, f32(k) / f32(steps));
            probe(f, at, f.radius, false, li);
            let info = workgroupUniformLoad(&manifold);
            if (info.total > 0u && info.deepest_depth > 0.0) {
                center = at;
                swept = true;
                break;
            }
        }
    }

    if (li == 0u) {
        // Terrain raised under a body (brush strokes, neighbouring craters) can bury its centre,
        // where closest-point normals would point back into the ground. Lift it out first.
        var lifted = center;
        let surface = interpolated_floor(center.x, center.z);
        if (!is_missing(surface)) {
            lifted.y = max(lifted.y, surface);
        }
        // GroundModel::new: this substep's reference, filtered against the previous one while
        // the body stays grounded.
        var plane = fit_plane(lifted, f.radius * RUT_REFERENCE_RING_RATIO);
        let weight = 1.0 - exp(-dt / RUT_REFERENCE_SMOOTHING_SECONDS);
        if (plane.valid && f.plane_valid != 0u && f.was_grounded != 0u) {
            // previous.blend(fitted, weight): the previous plane re-expressed at the new origin.
            let from_height = f.plane_height + dot(f.plane_slope, plane.origin - f.plane_origin);
            plane.height = from_height + (plane.height - from_height) * weight;
            plane.slope = f.plane_slope + (plane.slope - f.plane_slope) * weight;
        }
        f.plane_valid = select(0u, 1u, plane.valid);
        f.plane_origin = plane.origin;
        f.plane_height = plane.height;
        f.plane_slope = plane.slope;
        let load = sqrt(max(f.mass / RUT_REFERENCE_MASS, 0.0));
        f.sinkage_per_softness = RUT_DEPTH_SCALE * load;
        f.max_sinkage = f.radius * MAX_SINKAGE_RATIO;
        f.center = lifted;
        frame = f;
    }
    f = workgroupUniformLoad(&frame);

    // Ground deforms in two ways: a landing from the air craters it in proportion to the
    // impact energy; while the sphere stays in contact its weight presses a bounded trail.
    probe(f, f.center, f.radius + CONTACT_SLOP, true, li);
    if (li == 0u) {
        var decision = DECIDE_NONE;
        var approach = 0.0;
        var energy = 0.0;
        var normal = vec3<f32>(0.0, 1.0, 0.0);
        if (manifold.total > 0u) {
            normal = ground_normal();
            if (normal.y >= GROUND_NORMAL_MIN_Y) {
                approach = -dot(f.velocity, normal);
                energy = 0.5 * f.mass * approach * approach;
                if (f.was_grounded == 0u && approach > 0.0 && energy > MIN_DEFORMATION_ENERGY) {
                    decision = DECIDE_CRATER;
                } else if (f.plane_valid != 0u) {
                    decision = DECIDE_RUT;
                }
            }
        }
        f.decision = decision;
        frame = f;
        record_decision(b, f, approach, energy, normal, swept);
    }
    f = workgroupUniformLoad(&frame);
    claim_footprint(b, f, li);
}

// Stores what the apply and response passes need: the lifted centre, the plane, the decision
// and its parameters.
fn record_decision(b: u32, f: Frame, approach: f32, energy: f32, normal: vec3<f32>, swept: bool) {
    var s = load_body_state(state_base(b));
    let anchor = place(s).position - vec3<f32>(s.local_x, s.height, -s.local_y);
    s.flags = select(s.flags & ~BODY_PLANE_VALID, s.flags | BODY_PLANE_VALID, f.plane_valid != 0u);
    s.plane_origin_x = f.plane_origin.x - anchor.x;
    s.plane_origin_y = f.plane_origin.y + anchor.z;
    s.plane_height = f.plane_height;
    s.plane_slope_x = f.plane_slope.x;
    s.plane_slope_y = f.plane_slope.y;
    store_body_state(state_base(b), s);
    set_work_vec3(b, WORK_CENTER, f.center);
    bodies[work_word(b, WORK_DECISION)] = f.decision;
    set_work_f32(b, WORK_APPROACH, approach);
    set_work_vec3(b, WORK_NORMAL, normal);
    set_work_f32(b, WORK_ENERGY, energy);
    set_work_f32(b, WORK_SINKAGE_PER_SOFTNESS, f.sinkage_per_softness);
    set_work_f32(b, WORK_MAX_SINKAGE, f.max_sinkage);
    if (swept) {
        bodies[work_word(b, WORK_FLAGS)] |= WORK_SWEPT;
        count_body(BDIAG_SWEPT_BODIES, 1u);
    }
    if (f.decision == DECIDE_CRATER) {
        let extension = min(sqrt(energy * ENERGY_TO_DEFORMATION), f.radius * MAX_DEFORMATION_SIZE_RATIO);
        set_work_f32(b, WORK_EXTENSION, extension);
        set_work_f32(b, WORK_CRUSH_COST, COST_CRUSH_TERRAIN * outward_resistance(f.center, f.radius));
        set_work_f32(b, WORK_RIM_CEILING, f.center.y + f.radius * BLADE_CEILING_RATIO);
    }
    if (f.decision == DECIDE_RUT) {
        let horizontal = vec2<f32>(f.velocity.x, f.velocity.z);
        var heading = vec2<f32>(0.0);
        if (length(horizontal) > RUT_HEADING_MIN_SPEED) {
            heading = normalize(horizontal);
        }
        set_work_f32(b, WORK_HEADING, heading.x);
        set_work_f32(b, WORK_HEADING + 1u, heading.y);
        let cell = nearest_cell(f.center);
        set_work_f32(b, WORK_BERM_CEILING, plane_at(f, cell) + f.radius * RUT_BERM_HEIGHT_RATIO);
    }
}

// GridPhysicsApi::compute_outward_resistance around the crater's centre column.
fn outward_resistance(center: vec3<f32>, radius: f32) -> f32 {
    let cell = nearest_cell(center);
    var baseline = surface_height(cell);
    if (is_missing(baseline)) {
        baseline = center.y - radius;
    }
    var accumulated = 0.0;
    var samples = 0u;
    for (var ring = 1u; ring <= RESISTANCE_SAMPLE_RINGS; ring++) {
        let offset = i32(ring * RESISTANCE_SAMPLE_STRIDE);
        let points = array<vec2<i32>, 4>(
            cell + vec2<i32>(offset, offset),
            cell + vec2<i32>(-offset, offset),
            cell + vec2<i32>(offset, -offset),
            cell + vec2<i32>(-offset, -offset),
        );
        for (var p = 0u; p < 4u; p++) {
            let h = surface_height(points[p]);
            if (!is_missing(h) && abs(h - baseline) <= RESISTANCE_CLIFF_THRESHOLD) {
                accumulated += h;
                samples += 1u;
            }
        }
    }
    if (samples == 0u) {
        return 3.0e38;
    }
    let depth = max(accumulated / f32(samples) - baseline, 0.0);
    return 1.0 + depth * RESISTANCE_MULTIPLIER;
}

@compute @workgroup_size(64, 1, 1)
fn contact_response(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let b = wg.x;
    if (li == 0u) {
        var loaded = load_contact_frame(b);
        loaded.center = work_vec3(b, WORK_CENTER);
        frame = loaded;
    }
    let f = workgroupUniformLoad(&frame);
    if (f.enabled == 0u || b >= body_params.body_count) {
        return;
    }
    let dt = body_params.substep_seconds;
    var center = f.center;
    for (var iteration = 0u; iteration < CONTACT_ITERATIONS; iteration++) {
        probe(f, center, f.radius, true, li);
        let info = workgroupUniformLoad(&manifold);
        if (info.total == 0u || info.deepest_depth <= 0.0) {
            break;
        }
        center += info.deepest_normal * info.deepest_depth;
    }
    probe(f, center, f.radius + CONTACT_SLOP, true, li);
    if (li == 0u) {
        var velocity = f.velocity;
        var grounded = false;
        var normal = vec3<f32>(0.0, 1.0, 0.0);
        var terrain = vec3<f32>(0.0, 1.0, 0.0);
        if (manifold.total > 0u) {
            velocity = contact_velocity(velocity, dt);
            terrain = ground_normal();
            grounded = terrain.y >= GROUND_NORMAL_MIN_Y;
            if (grounded) {
                normal = terrain;
            }
            count_body(BDIAG_TERRAIN_CONTACTS, 1u);
        }
        set_work_vec3(b, WORK_POS_A, center);
        set_work_vec3(b, WORK_VEL_A, velocity);
        var s = load_body_state(state_base(b));
        if (grounded && (s.flags & BODY_GROUNDED) == 0u) {
            s.landings += 1u;
            s.tick_events |= TICK_LANDED;
        }
        s.flags = select(s.flags & ~BODY_GROUNDED, s.flags | BODY_GROUNDED, grounded);
        // The terrain the pair solve slides this body along next substep.
        s.flags = select(s.flags & ~BODY_TERRAIN, s.flags | BODY_TERRAIN, manifold.total > 0u);
        s.terrain_normal = encode_direction(terrain);
        s.normal_x = normal.x;
        s.normal_y = normal.y;
        s.normal_z = normal.z;
        store_body_state(state_base(b), s);
    }
}
