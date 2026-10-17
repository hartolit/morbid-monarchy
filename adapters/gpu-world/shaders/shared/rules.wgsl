// Keyed hashes, draws and neighbour shuffles: world_model::rules, line for line.
//
// Every pseudo-random choice of a rule is a pure function of a cell's absolute position, the rule
// tick (the logical tick's low word) and the rule key (the world's rule seed mixed with the tick's
// high word), plus a salt. With key 0 these are the baseline engine's hashes.

// The eight neighbour steps in the order every shuffle starts from; index k and k + 4 (mod 8)
// are opposite steps.
const DIRS = array<vec2<i32>, 8>(
    vec2<i32>(0, 1),
    vec2<i32>(1, 1),
    vec2<i32>(1, 0),
    vec2<i32>(1, -1),
    vec2<i32>(0, -1),
    vec2<i32>(-1, -1),
    vec2<i32>(-1, 0),
    vec2<i32>(-1, 1),
);
const DIR_NONE: u32 = 15u;

fn opposite(dir: u32) -> u32 {
    return (dir + 4u) % 8u;
}

fn is_cardinal(dir: u32) -> bool {
    return dir % 2u == 0u;
}

fn fmix32(value: u32) -> u32 {
    var h = value;
    h ^= h >> 16u;
    h *= 0x85EBCA6Bu;
    h ^= h >> 13u;
    h *= 0xC2B2AE35u;
    h ^= h >> 16u;
    return h;
}

// world_model::rules::keyed_spatial_hash.
fn keyed_hash(pos: vec2<i32>, rule_tick: u32, rule_key: u32) -> u32 {
    return fmix32(
        (bitcast<u32>(pos.x) * 0x736A153Du)
            ^ (bitcast<u32>(pos.y) * 0x9E3779B1u)
            ^ (rule_tick * 0x11C64E6Du)
            ^ rule_key,
    );
}

// world_model::rules::keyed_spatial_hash_extended.
fn keyed_hash_extended(pos: vec2<i32>, rule_tick: u32, rule_key: u32, material: u32, cadence: u32) -> u32 {
    var h = keyed_hash(pos, rule_tick, rule_key) + material * 0x27D4EB2Fu + cadence * 0x165667B1u;
    h ^= h >> 13u;
    h *= 0x85EBCA6Bu;
    h ^= h >> 16u;
    return h;
}

// world_model::rules::draw: one keyed draw per decision.
fn keyed_draw(pos: vec2<i32>, rule_tick: u32, rule_key: u32, salt: u32) -> u32 {
    return keyed_hash_extended(pos, rule_tick, rule_key, 0u, salt);
}

fn ratio(value: u32, numerator: u32, denominator: u32) -> bool {
    return value % denominator < numerator;
}

// The rise a column's compass flags point along, or (0, 0).
fn compass_forward(flags: u32) -> vec2<i32> {
    var forward = vec2<i32>(0, 0);
    if ((flags & COMPASS_N) != 0u) {
        forward.y += 1;
    }
    if ((flags & COMPASS_S) != 0u) {
        forward.y -= 1;
    }
    if ((flags & COMPASS_E) != 0u) {
        forward.x += 1;
    }
    if ((flags & COMPASS_W) != 0u) {
        forward.x -= 1;
    }
    return forward;
}

// Compass flags of a flow along `flow` (receiver minus source).
fn compass_for_flow(flow: vec2<i32>) -> u32 {
    var flags = 0u;
    if (flow.y > 0) {
        flags |= COMPASS_N;
    }
    if (flow.y < 0) {
        flags |= COMPASS_S;
    }
    if (flow.x > 0) {
        flags |= COMPASS_E;
    }
    if (flow.x < 0) {
        flags |= COMPASS_W;
    }
    return flags;
}

// Neighbour directions (indices into DIRS) in a cell's shuffled order.
struct Order {
    dirs: array<u32, 8>,
    count: u32,
}

// world_model::rules::ShuffledDirs::new: Fisher-Yates driven by the extended hash.
fn shuffled(cardinal: bool, pos: vec2<i32>, rule_tick: u32, rule_key: u32, material: u32, cadence: u32) -> Order {
    var order: Order;
    if (cardinal) {
        order.dirs = array<u32, 8>(0u, 2u, 4u, 6u, 0u, 0u, 0u, 0u);
        order.count = 4u;
    } else {
        order.dirs = array<u32, 8>(0u, 1u, 2u, 3u, 4u, 5u, 6u, 7u);
        order.count = 8u;
    }
    var h = keyed_hash_extended(pos, rule_tick, rule_key, material, cadence);
    for (var i = order.count - 1u; i >= 1u; i--) {
        let j = h % (i + 1u);
        let held = order.dirs[i];
        order.dirs[i] = order.dirs[j];
        order.dirs[j] = held;
        h = h * 0x9E3779B1u + 1u;
    }
    return order;
}

// world_model::rules::ShuffledDirs::with_momentum: the compass rise moved to the front.
fn with_momentum(order: Order, flags: u32) -> Order {
    var out = order;
    let forward = compass_forward(flags);
    if (all(forward == vec2<i32>(0, 0))) {
        return out;
    }
    for (var i = 0u; i < out.count; i++) {
        if (all(DIRS[out.dirs[i]] == forward)) {
            let held = out.dirs[0];
            out.dirs[0] = out.dirs[i];
            out.dirs[i] = held;
            break;
        }
    }
    return out;
}
