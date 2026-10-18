// Body commands and rays, dispatched in command order inside the tick's command pass, after
// `begin_tick` gave each command its result record and default status.
//
// resolve_target  one workgroup per ray: the ray's first meeting with the physical surface (the
//                 same triangulated heightfield bodies collide with) and the nearest body it
//                 crosses. The ray is cut into 64 segments; each invocation marches its own
//                 segment's quads in order, so the first segment with an event holds the first
//                 event of the ray, and traversal is bounded by RAY_MAX_DISTANCE. A quad with a
//                 corner that is not resident ends the ray as `SURFACE_NOT_RESIDENT` when the ray
//                 crosses it below the highest possible surface: what lies there is unknown. A
//                 ray-targeted command reads the hit from its result; a command whose ray missed
//                 is answered `RESULT_TARGET_MISSED` and changes nothing.
// spawn_body, remove_body, drive_body, jump_body   one invocation, on the body the command
//                 names; the generation must match the slot's.
// attract_bodies, lift_bodies   one invocation per body slot.

var<workgroup> ray_lane_event: array<vec2<f32>, 64>;
var<workgroup> ray_lane_cell: array<vec2<i32>, 64>;
// Each lane's nearest body: its distance and its slot (BODY_NONE for none). Slots stay u32:
// BODY_NONE has no exact f32.
var<workgroup> ray_lane_body_distance: array<f32, 64>;
var<workgroup> ray_lane_body_slot: array<u32, 64>;

struct Ray {
    origin: vec3<f32>,
    direction: vec3<f32>,
    length: f32,
}

fn command_ray(command: CommandRecord) -> Ray {
    return Ray(
        vec3<f32>(command.origin_x, command.origin_y, command.origin_z),
        vec3<f32>(command.direction_x, command.direction_y, command.direction_z),
        command.ray_length,
    );
}

fn quad_corner_heights(cell: vec2<i32>) -> vec4<f32> {
    return vec4<f32>(
        surface_height(cell),
        surface_height(cell + vec2<i32>(1, 0)),
        surface_height(cell + vec2<i32>(0, 1)),
        surface_height(cell + vec2<i32>(1, 1)),
    );
}

// Marches the quads the ray crosses between `t0` and `t1`. Returns (t, kind) of the first event
// (kind SURFACE_HIT or SURFACE_NOT_RESIDENT) with its quad in `cell`, or kind SURFACE_MISS.
fn march(ray: Ray, t0: f32, t1: f32, cell: ptr<function, vec2<i32>>) -> vec2<f32> {
    // Grid space: x is the cell x, y is the cell y (render -z).
    let o = vec2<f32>(ray.origin.x, -ray.origin.z);
    let d = vec2<f32>(ray.direction.x, -ray.direction.z);
    let start = o + d * t0;
    var q = vec2<i32>(floor(start));
    let step = vec2<i32>(select(-1, 1, d.x > 0.0), select(-1, 1, d.y > 0.0));
    let big = 3.0e38;
    let delta = vec2<f32>(select(big, abs(1.0 / d.x), d.x != 0.0), select(big, abs(1.0 / d.y), d.y != 0.0));
    var next = vec2<f32>(
        select(big, t0 + (select(f32(q.x), f32(q.x) + 1.0, d.x > 0.0) - start.x) / d.x, d.x != 0.0),
        select(big, t0 + (select(f32(q.y), f32(q.y) + 1.0, d.y > 0.0) - start.y) / d.y, d.y != 0.0),
    );
    var enter = t0;
    for (var guard = 0u; guard < 128u && enter <= t1; guard++) {
        let leave = min(min(next.x, next.y), t1);
        let heights = quad_corner_heights(q);
        if (is_missing(heights.x) || is_missing(heights.y) || is_missing(heights.z) || is_missing(heights.w)) {
            let low = min(ray.origin.y + ray.direction.y * enter, ray.origin.y + ray.direction.y * leave);
            if (low <= body_params.max_surface) {
                *cell = q;
                return vec2<f32>(max(enter, 0.0), f32(SURFACE_NOT_RESIDENT));
            }
        } else {
            var best = big;
            for (var second = 0u; second < 2u; second++) {
                let t = ray_triangle(ray.origin, ray.direction, quad_triangle(q, heights, second == 1u));
                if (t >= 0.0 && t >= enter - 1e-4 && t <= leave + 1e-4 && t <= ray.length) {
                    best = min(best, t);
                }
            }
            if (best < big) {
                *cell = q;
                return vec2<f32>(best, f32(SURFACE_HIT));
            }
        }
        if (next.x < next.y) {
            enter = next.x;
            next.x += delta.x;
            q.x += step.x;
        } else {
            enter = next.y;
            next.y += delta.y;
            q.y += step.y;
        }
    }
    return vec2<f32>(big, f32(SURFACE_MISS));
}

@compute @workgroup_size(64, 1, 1)
fn resolve_target(@builtin(local_invocation_index) li: u32) {
    let index = dispatch_params.command_index;
    let command = commands[index];
    let ray = command_ray(command);
    let segment = ray.length / 64.0;
    var cell = vec2<i32>(0);
    ray_lane_event[li] = march(ray, segment * f32(li), segment * f32(li + 1u), &cell);
    ray_lane_cell[li] = cell;
    // The nearest body this invocation's share of the slots crosses.
    var nearest_distance = 3.0e38;
    var nearest_slot = BODY_NONE;
    for (var b = li; b < body_params.body_count; b += 64u) {
        let s = load_body_state(state_base(b));
        if ((s.flags & BODY_ALIVE) == 0u) {
            continue;
        }
        let placed = place(s);
        if (placed.far) {
            continue;
        }
        let t = ray_sphere(ray.origin, ray.direction, placed.position, load_body_config(config_base(b)).radius);
        if (t >= 0.0 && t <= ray.length && t < nearest_distance) {
            nearest_distance = t;
            nearest_slot = b;
        }
    }
    ray_lane_body_distance[li] = nearest_distance;
    ray_lane_body_slot[li] = nearest_slot;
    workgroupBarrier();
    if (li != 0u) {
        return;
    }
    var surface = vec2<f32>(3.0e38, f32(SURFACE_MISS));
    var hit_cell = vec2<i32>(0);
    for (var lane = 0u; lane < 64u; lane++) {
        if (u32(ray_lane_event[lane].y) != SURFACE_MISS) {
            surface = ray_lane_event[lane];
            hit_cell = ray_lane_cell[lane];
            break;
        }
    }
    // The nearest over the lanes; equal distances go to the lower slot.
    var body_distance = 3.0e38;
    var body_slot = BODY_NONE;
    for (var lane = 0u; lane < 64u; lane++) {
        let distance = ray_lane_body_distance[lane];
        let slot = ray_lane_body_slot[lane];
        if (slot != BODY_NONE && (distance < body_distance || (distance == body_distance && slot < body_slot))) {
            body_distance = distance;
            body_slot = slot;
        }
    }
    let kind = u32(surface.y);
    results[index].v0 = kind;
    results[index].v1 = bitcast<u32>(hit_cell.x);
    results[index].v2 = bitcast<u32>(hit_cell.y);
    results[index].v3 = bitcast<u32>(surface.x);
    results[index].v4 = BODY_NONE;
    if (body_slot != BODY_NONE && (kind != SURFACE_HIT || body_distance < surface.x)) {
        results[index].v4 = body_slot;
        results[index].v5 = load_body_state(state_base(body_slot)).generation;
        results[index].v6 = bitcast<u32>(body_distance);
    }
    if (command.kind == COMMAND_QUERY_RAY) {
        results[index].status = RESULT_RAY;
    } else if (kind != SURFACE_HIT) {
        results[index].status = RESULT_TARGET_MISSED;
    }
}

// The point a command acts at: its column's top or its ray's hit. `ok` is false (and the result
// already says why) when there is none.
struct TargetPoint {
    ok: bool,
    position: vec3<f32>,
}

fn target_point(index: u32, command: CommandRecord, column_top: bool, report: bool) -> TargetPoint {
    if (command.target_kind == TARGET_RAY) {
        if (results[index].v0 != SURFACE_HIT) {
            return TargetPoint(false, vec3<f32>(0.0));
        }
        let ray = command_ray(command);
        return TargetPoint(true, ray.origin + ray.direction * bitcast<f32>(results[index].v3));
    }
    let cell = vec2<i32>(command.cell_x, command.cell_y);
    let col = column(cell);
    if (!col.present) {
        if (report) {
            results[index].status = RESULT_TARGET_NOT_RESIDENT;
        }
        return TargetPoint(false, vec3<f32>(0.0));
    }
    var height = 0.0;
    if (column_top) {
        // The legacy driver's column point: the column's liquid head.
        height = f32(liquid_head(col.value)) * body_params.elevation_scale;
    }
    return TargetPoint(true, vec3<f32>(f32(cell.x), height, -f32(cell.y)));
}

fn store_target(index: u32, p: vec3<f32>) {
    results[index].v7 = bitcast<u32>(p.x);
    results[index].v8 = bitcast<u32>(p.y);
    results[index].v9 = bitcast<u32>(p.z);
}

// Whether `command` names a live body; answers STALE_BODY otherwise.
fn named_body(index: u32, command: CommandRecord) -> bool {
    let b = command.body_index;
    if (b >= body_params.capacity) {
        results[index].status = RESULT_STALE_BODY;
        return false;
    }
    let s = load_body_state(state_base(b));
    if ((s.flags & BODY_ALIVE) == 0u || s.generation != command.body_generation) {
        results[index].status = RESULT_STALE_BODY;
        return false;
    }
    return true;
}

@compute @workgroup_size(1, 1, 1)
fn spawn_body() {
    let index = dispatch_params.command_index;
    let command = commands[index];
    let b = command.body_index;
    if (command.kind != COMMAND_SPAWN || b >= body_params.capacity) {
        return;
    }
    var s: BodyState;
    s.generation = command.body_generation;
    if (results[index].status != RESULT_BODY_SPAWNED) {
        // The ray missed: the slot stays dead until the outcome frees it on the CPU.
        store_body_state(state_base(b), s);
        return;
    }
    let point = target_point(index, command, false, true);
    if (!point.ok) {
        store_body_state(state_base(b), s);
        return;
    }
    // spawn_sphere: drop height above the bedrock and loose material of the quad's column.
    let col = column(quad_cell(point.position));
    if (!col.present) {
        results[index].status = RESULT_TARGET_NOT_RESIDENT;
        store_body_state(state_base(b), s);
        return;
    }
    let floor_at = f32(cell_elevation(col.value) + cell_granular_volume(col.value)) * body_params.elevation_scale;
    let position = vec3<f32>(point.position.x, floor_at + command.d, point.position.z);
    s.anchor_x = tick.origin_key_x;
    s.anchor_y = tick.origin_key_y;
    _ = anchor_position(&s, position);
    s.flags = BODY_ALIVE;
    s.normal_y = 1.0;
    store_body_state(state_base(b), s);
    store_body_config(config_base(b), BodyConfig(command.a, command.b, command.c, 1.0 / command.a));
    results[index].word0 = bitcast<u32>(s.anchor_x);
    results[index].word1 = bitcast<u32>(s.anchor_y);
    results[index].v7 = bitcast<u32>(s.local_x);
    results[index].v8 = bitcast<u32>(s.local_y);
    results[index].v9 = bitcast<u32>(s.height);
}

@compute @workgroup_size(1, 1, 1)
fn remove_body() {
    let index = dispatch_params.command_index;
    let command = commands[index];
    if (command.kind != COMMAND_REMOVE || !named_body(index, command)) {
        return;
    }
    var s = load_body_state(state_base(command.body_index));
    results[index].word0 = s.landings;
    results[index].word1 = s.liquid_entries;
    results[index].slot_index = s.body_contacts;
    results[index].slot_generation = s.craters;
    results[index].v7 = s.splashes;
    results[index].v8 = s.absorbed_energy;
    s.flags = 0u;
    store_body_state(state_base(command.body_index), s);
}

fn body_velocity(s: BodyState) -> vec3<f32> {
    return vec3<f32>(s.velocity_x, s.velocity_y, s.velocity_z);
}

fn set_body_velocity(s: ptr<function, BodyState>, v: vec3<f32>) {
    (*s).velocity_x = v.x;
    (*s).velocity_y = v.y;
    (*s).velocity_z = v.z;
}

@compute @workgroup_size(1, 1, 1)
fn drive_body() {
    let index = dispatch_params.command_index;
    let command = commands[index];
    if (command.kind != COMMAND_DRIVE || !named_body(index, command)) {
        return;
    }
    var s = load_body_state(state_base(command.body_index));
    if ((s.flags & BODY_SUSPENDED) != 0u) {
        return;
    }
    // drive_sphere: accelerate along the direction up to the speed cap along it.
    let direction = vec3<f32>(command.a, 0.0, command.b);
    let v = body_velocity(s);
    let gain = max(min(command.c * body_params.tick_seconds, command.d - dot(v, direction)), 0.0);
    set_body_velocity(&s, v + direction * gain);
    store_body_state(state_base(command.body_index), s);
    atomicStore(&results[index].count, 1u);
}

@compute @workgroup_size(1, 1, 1)
fn jump_body() {
    let index = dispatch_params.command_index;
    let command = commands[index];
    if (command.kind != COMMAND_JUMP || !named_body(index, command)) {
        return;
    }
    var s = load_body_state(state_base(command.body_index));
    if ((s.flags & BODY_GROUNDED) == 0u || (s.flags & BODY_SUSPENDED) != 0u) {
        return;
    }
    let normal = vec3<f32>(s.normal_x, s.normal_y, s.normal_z);
    set_body_velocity(&s, body_velocity(s) + normal * JUMP_IMPULSE);
    store_body_state(state_base(command.body_index), s);
    atomicStore(&results[index].count, 1u);
}

// Every body accelerates toward the point for one tick (attract_sphere), or bodies near its
// column rise and drift toward it (lift_sphere). Suspended bodies are frozen and unaffected.
fn pull(attract: bool, id: vec3<u32>) {
    let index = dispatch_params.command_index;
    let command = commands[index];
    let expected = select(COMMAND_LIFT, COMMAND_ATTRACT, attract);
    if (command.kind != expected || (command.target_kind == TARGET_RAY && results[index].v0 != SURFACE_HIT)) {
        return;
    }
    let point = target_point(index, command, true, id.x == 0u);
    if (!point.ok) {
        return;
    }
    if (id.x == 0u) {
        store_target(index, point.position);
    }
    let b = id.x;
    if (b >= body_params.body_count) {
        return;
    }
    var s = load_body_state(state_base(b));
    if ((s.flags & BODY_ALIVE) == 0u || (s.flags & BODY_SUSPENDED) != 0u) {
        return;
    }
    let placed = place(s);
    if (placed.far) {
        return;
    }
    let dt = body_params.tick_seconds;
    let offset = point.position - placed.position;
    var v = body_velocity(s);
    if (attract) {
        if (dot(offset, offset) <= TARGET_EPSILON_SQUARED) {
            return;
        }
        v += normalize(offset) * ATTRACT_ACCELERATION * dt;
    } else {
        let horizontal = vec3<f32>(offset.x, 0.0, offset.z);
        let distance_squared = dot(horizontal, horizontal);
        if (distance_squared >= LIFT_INFLUENCE_RADIUS * LIFT_INFLUENCE_RADIUS) {
            return;
        }
        v.y += LIFT_ACCELERATION * dt;
        if (distance_squared > TARGET_EPSILON_SQUARED) {
            v += normalize(horizontal) * LIFT_CENTERING_ACCELERATION * dt;
        }
    }
    set_body_velocity(&s, v);
    store_body_state(state_base(b), s);
    atomicAdd(&results[index].count, 1u);
}

@compute @workgroup_size(64, 1, 1)
fn attract_bodies(@builtin(global_invocation_id) id: vec3<u32>) {
    pull(true, id);
}

@compute @workgroup_size(64, 1, 1)
fn lift_bodies(@builtin(global_invocation_id) id: vec3<u32>) {
    pull(false, id);
}
