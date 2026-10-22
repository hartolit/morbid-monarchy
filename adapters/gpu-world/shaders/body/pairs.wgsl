// Body-body contact: a complete spatial broadphase and a bounded Jacobi solver.
//
// bins_count, bins_scan, bins_scatter   a counting sort of every binned body (active, and
//              suspended within the window) into a hash table of 3-D bins, at the position its
//              integration ended at.
//
// Candidate pairs are generated once per substep, from those positions: two bodies are a
// candidate pair when they are closer than their radii, the contact slop, both substeps'
// travel and PAIR_MARGIN (room for the solver's own corrections). The solver's passes consider
// only candidate pairs; a contact that arises farther away than that, out of the corrections
// themselves, is generated in the next substep. The bin edge is the largest such distance
// (twice the largest live radius and the farthest travel, the slop and the margin), so two
// candidates lie in the same or adjacent bins and the 27 bins around a body hold every
// candidate; a bin has no capacity, so none is dropped, and hash collisions only add bodies the
// generation test rejects. The all-pairs broadphase applies the same generation test to every
// body, so both solve exactly the same pairs.
// pairs_count  the contacts each active body has (overlapping or within the slop, or swept):
//              the solver's scaling. It also records the body's candidates in its pair list
//              (PAIR_LIST_CAPACITY of them; the generation test depends only on where the
//              substep's integration left the bodies, so the candidates are the same in every
//              pass), and the passes below read the list instead of the bins; a body with more
//              candidates than the list holds walks the bins in every pass, as without a list.
//              This and the passes below run one workgroup per body, whose lanes share its
//              candidates (see "One workgroup per body").
// pairs_position, pairs_velocity   the session's position and velocity iterations
//              (`GpuWorldConfig::pair_iterations`), Jacobi passes each. A pass reads every
//              body from one half of the ping-pong buffers and writes the other, so no body
//              reads a value another invocation is writing. Each body gathers its partners'
//              corrections and sums them in fixed point, which makes the sum independent of the
//              order partners are found in, and so the result deterministic.
//
//              Position passes: each overlapping pair asks both bodies to part by its depth,
//              split equally (corrections are not momentum, and an equal split lets a light
//              body wedged between heavy ones push them apart instead of taking the whole
//              correction itself); a body moves by the mean of the corrections it is asked for
//              (the pairs still overlapping in this pass), within MAX_PAIR_CORRECTION of where
//              it was binned this substep.
//              Velocity passes: each touching pair is driven toward its parting speed (the
//              restitution of its approach at the start of the solve, none for a resting
//              approach), by an impulse split by inverse mass and scaled by 1 / max(contacts of
//              either body), the same for both, so impulses stay equal and opposite and
//              momentum is kept.
//              A body touching terrain slides along it rather than being pushed into it
//              (`mobility`).
//
//              Convergence limit: Jacobi averaging propagates a correction through a pile one
//              partner per pass, so a dense pile stays slightly compressed where light bodies
//              are wedged between heavy ones. The `bodies` suite's dense cluster (512 bodies of
//              radius 2 to 8) measures it: settled, at the default 16 and 16 passes, no pair
//              overlaps by more than 0.4 of its smaller radius.
// pairs_residual  the overlap and approach left after the last passes, and each body's
//              touching count.
//
// The all-pairs broadphase (a configuration, for comparison) visits every body instead.

// The farthest the position passes move a body from where it was binned in one substep, world
// units: deeper overlaps resolve over several substeps (at most 120 units a second at the
// 1/120 s substep), the way a maximum depenetration speed bounds them. Candidate generation
// allows for both bodies' corrections, so no contact can arise inside a substep's solve between
// bodies that are not a candidate pair.
const MAX_PAIR_CORRECTION: f32 = 0.5;
const PAIR_MARGIN: f32 = 2.0 * MAX_PAIR_CORRECTION;
// Fixed-point scales of the summed corrections: 2^-16 world units and 2^-12 units per second.
const POSITION_QUANTUM: f32 = 65536.0;
const VELOCITY_QUANTUM: f32 = 4096.0;

fn bin_edge() -> f32 {
    let radius = bitcast<f32>(atomicLoad(&bins[body_params.bounds_base]));
    let travel = bitcast<f32>(atomicLoad(&bins[body_params.bounds_base + 1u]));
    return max(2.0 * radius + 2.0 * travel + CONTACT_SLOP + PAIR_MARGIN + 0.01, 1.0);
}

fn bin_of(p: vec3<f32>, edge: f32) -> vec3<i32> {
    return vec3<i32>(floor(p / edge));
}

fn bin_slot(bin: vec3<i32>) -> u32 {
    let h = (u32(bin.x) * 73856093u) ^ (u32(bin.y) * 19349663u) ^ (u32(bin.z) * 83492791u);
    return h & body_params.hash_mask;
}

// Whether body `b` is in the bins: alive and either active or a suspended obstacle inside the
// window (a far suspended body is in neither).
fn binned(b: u32) -> bool {
    let s = load_body_state(state_base(b));
    if ((s.flags & BODY_ALIVE) == 0u) {
        return false;
    }
    return (bodies[work_word(b, WORK_FLAGS)] & WORK_ACTIVE) != 0u || !place(s).far;
}

fn is_active(b: u32) -> bool {
    return (bodies[work_word(b, WORK_FLAGS)] & WORK_ACTIVE) != 0u;
}

@compute @workgroup_size(64, 1, 1)
fn bins_count(@builtin(global_invocation_id) id: vec3<u32>) {
    let b = id.x;
    if (b >= body_params.body_count || !binned(b)) {
        return;
    }
    let slot = bin_slot(bin_of(work_vec3(b, WORK_BINNED), bin_edge()));
    atomicAdd(&bins[body_params.counts_base + slot], 1u);
}

var<workgroup> scan_totals: array<u32, 256>;

// One workgroup: an exclusive prefix sum of the bin counts into the bin starts.
@compute @workgroup_size(256, 1, 1)
fn bins_scan(@builtin(local_invocation_index) li: u32) {
    let slots = body_params.hash_mask + 1u;
    let per = (slots + 255u) / 256u;
    let first = li * per;
    var total = 0u;
    for (var slot = first; slot < min(first + per, slots); slot++) {
        total += atomicLoad(&bins[body_params.counts_base + slot]);
    }
    scan_totals[li] = total;
    workgroupBarrier();
    for (var stride = 1u; stride < 256u; stride = stride << 1u) {
        var value = scan_totals[li];
        if (li >= stride) {
            value += scan_totals[li - stride];
        }
        workgroupBarrier();
        scan_totals[li] = value;
        workgroupBarrier();
    }
    var running = scan_totals[li] - total;
    for (var slot = first; slot < min(first + per, slots); slot++) {
        atomicStore(&bins[body_params.starts_base + slot], running);
        running += atomicLoad(&bins[body_params.counts_base + slot]);
    }
}

@compute @workgroup_size(64, 1, 1)
fn bins_scatter(@builtin(global_invocation_id) id: vec3<u32>) {
    let b = id.x;
    if (b >= body_params.body_count || !binned(b)) {
        return;
    }
    let slot = bin_slot(bin_of(work_vec3(b, WORK_BINNED), bin_edge()));
    let position = atomicLoad(&bins[body_params.starts_base + slot])
        + atomicAdd(&bins[body_params.fill_base + slot], 1u);
    atomicStore(&bins[body_params.entries_base + position], b);
}

// One pair as the solver sees it.
struct Pair {
    // 0 none, 1 overlap or touching, 2 swept (they passed through each other this substep).
    kind: u32,
    normal: vec3<f32>,
    // Overlap (positive) or gap (negative), along `normal`.
    depth: f32,
}

// The contact between active body `i` and body `j`, from `i`'s side; positions are read from
// half `half` of the buffers. `velocity_pass` adds the slop.
//
// A pair that was apart at the substep's start and meets during it takes the normal at its
// time of impact along the swept segments, and a pair whose centres pass each other during the
// substep keeps the side it started on: a fast body that ends past the other's centre is
// pushed back the way it came, never through.
fn pair_contact(i: u32, j: u32, half: u32, velocity_pass: bool) -> Pair {
    let none = Pair(0u, vec3<f32>(0.0), 0.0);
    let ri = load_body_config(config_base(i)).radius;
    let rj = load_body_config(config_base(j)).radius;
    let reach = ri + rj;
    let pi = work_vec3(i, WORK_POS_A + half * 4u);
    let pj = work_vec3(j, WORK_POS_A + half * 4u);
    let d1 = pi - pj;
    let dist = length(d1);
    var normal = vec3<f32>(select(-1.0, 1.0, i < j), 0.0, 0.0);
    if (dist > COINCIDENT_DISTANCE) {
        normal = d1 / dist;
    }
    // Swept: did the segments between the substep's start and end come within reach?
    let d0 = work_vec3(i, WORK_PREV) - work_vec3(j, WORK_PREV);
    let u = d1 - d0;
    let a = dot(u, u);
    let c = dot(d0, d0) - reach * reach;
    var impact = false;
    var impact_normal = normal;
    if (a > 1e-12 && c > 0.0) {
        let b = 2.0 * dot(d0, u);
        let discriminant = b * b - 4.0 * a * c;
        if (discriminant >= 0.0) {
            let t = (-b - sqrt(discriminant)) / (2.0 * a);
            if (t >= 0.0 && t <= 1.0) {
                impact = true;
                impact_normal = normalize_or(d0 + u * t, normal);
            }
        }
    }
    let crossed = !impact && dot(d0, d1) < 0.0 && length(d0) > COINCIDENT_DISTANCE;
    var side = normal;
    if (impact) {
        side = impact_normal;
    } else if (crossed) {
        side = normalize(d0);
    }
    if (dist < reach + select(0.0, CONTACT_SLOP, velocity_pass)) {
        return Pair(1u, side, reach - dot(d1, side));
    }
    if (impact || crossed) {
        let depth = reach - dot(d1, side);
        if (depth > 0.0) {
            return Pair(2u, side, depth);
        }
    }
    return none;
}

// How body `b` moves when a pair pushes it along `direction`, by inverse mass (`weighted`,
// impulses) or equally (position corrections). A suspended body does not move.
// A body that touched terrain at the end of the last substep (a ground or a wall), pushed into
// it, slides along it instead: the push's component into the terrain is removed (the terrain
// carries it). Terrain response runs after the pair solve, so without this a pile's bottom
// layer, or a body against a crater wall, would be pushed into the terrain only to be pushed
// back into its partners; with it two bodies side by side in a crater part along its walls.
// `motion` is the displacement per unit of the pair's correction, `weight` its component along
// `direction` (the body's share of the pair's effective inverse mass). Both bodies of a pair
// compute both values, so the pair's split is the same from either side.
struct Mobility {
    motion: vec3<f32>,
    weight: f32,
}

fn mobility(b: u32, direction: vec3<f32>, weighted: bool) -> Mobility {
    if (!is_active(b)) {
        return Mobility(vec3<f32>(0.0), 0.0);
    }
    let s = load_body_state(state_base(b));
    let inverse_mass = select(1.0, load_body_config(config_base(b)).inverse_mass, weighted);
    let terrain = decode_direction(s.terrain_normal);
    let into = dot(direction, terrain);
    if ((s.flags & BODY_TERRAIN) != 0u && into < 0.0) {
        let slide = direction - terrain * into;
        return Mobility(slide * inverse_mass, dot(slide, direction) * inverse_mass);
    }
    return Mobility(direction * inverse_mass, inverse_mass);
}

fn pair_scale(i: u32, j: u32) -> f32 {
    return 1.0 / f32(max(max(bodies[work_word(i, WORK_PAIRS)], bodies[work_word(j, WORK_PAIRS)]), 1u));
}

struct Gathered {
    sum: vec3<i32>,
    candidates: u32,
    contacts: u32,
    swept: u32,
    // Partners overlapping or within the slop.
    touching: u32,
    // Partners whose correction is in `sum` (position passes: those still overlapping).
    correcting: u32,
    deepest: f32,
    fastest: f32,
}

// A frame whose candidates are its body's pair list (`PairFrame::groups`).
const LISTED: u32 = 0xffffffffu;

fn pair_count_word(b: u32) -> u32 {
    return body_params.pair_counts_base + b;
}

fn pair_list_word(b: u32, k: u32) -> u32 {
    return body_params.pair_lists_base + b * PAIR_LIST_CAPACITY + k;
}

// Whether `i` and `j` are a candidate pair this substep (see the top of this module).
fn generated(i: u32, j: u32) -> bool {
    let bi = work_vec3(i, WORK_BINNED);
    let bj = work_vec3(j, WORK_BINNED);
    let travel = distance(bi, work_vec3(i, WORK_PREV)) + distance(bj, work_vec3(j, WORK_PREV));
    let reach = load_body_config(config_base(i)).radius + load_body_config(config_base(j)).radius;
    return distance(bi, bj) < reach + CONTACT_SLOP + travel + PAIR_MARGIN;
}

// The speed a contact is to part at: the baseline's restitution of the approach the solve
// started from, and none for an approach slower than the baseline's settle speed (one
// substep of gravity, the grounding buffer, at least MINIMUM_BOUNCE_SPEED), as for the ground.
// Resting bodies then rest on each other instead of bouncing each substep, and every velocity
// pass drives a pair toward the same target instead of compounding a bounce per pass.
fn separation_target(i: u32, j: u32, normal: vec3<f32>) -> f32 {
    let approach = -dot(work_vec3(i, WORK_SOLVE_VELOCITY) - work_vec3(j, WORK_SOLVE_VELOCITY), normal);
    let settle = max(gravity_length() * body_params.substep_seconds + GROUNDING_DRIFT_BUFFER, MINIMUM_BOUNCE_SPEED);
    return select(0.0, IMPACT_RESTITUTION * approach, approach > settle);
}

// ---- One workgroup per body ------------------------------------------------------------
//
// The pair kernels run one 64-lane workgroup per body. Lane 0 finds the body's candidate groups
// (`pair_prepare`), the lanes share its candidates (`gather_lanes`), and each pair's
// contribution meets the others in workgroup atomics. Every contribution is an integer (the
// fixed-point sums and the counts) or a non-negative float compared as its bits (the maxima),
// so the result does not depend on which lane handled which pair, and the solve stays
// deterministic.

// The body a pair workgroup serves. `integrated`: active this substep. `gather`: it has
// candidates to visit in this pass. `groups` candidate groups hold `total` candidates, or
// `groups` is LISTED and the body's pair list holds them.
struct PairFrame {
    integrated: u32,
    gather: u32,
    groups: u32,
    total: u32,
}

var<workgroup> pair_frame: PairFrame;
// Each group's first bin entry, and the running candidate count at its end. With the all-pairs
// broadphase there is one group: every body slot.
var<workgroup> pair_first: array<u32, 27>;
var<workgroup> pair_end: array<u32, 27>;
var<workgroup> pair_sum: array<atomic<i32>, 3>;
// Candidates, contacts, swept, touching, correcting.
var<workgroup> pair_tally: array<atomic<u32>, 5>;
// Deepest overlap and fastest approach, as non-negative f32 bits.
var<workgroup> pair_extreme: array<atomic<u32>, 2>;

// The workgroup describes body `i`'s pass: whether it takes part and where its candidates
// are, its pair list, the hash slots of the 27 bins around it (a slot two bins share is counted
// once) or every slot, and every lane gets the description. Lane 0 decides and the 27 bins are
// found by one lane each. `counting` visits every active body's candidates (the count pass,
// which fills the pair list); the others visit only a body that has contacts, from its list
// when its candidates fit.
var<workgroup> pair_slot: array<u32, 27>;
var<workgroup> pair_count: array<u32, 27>;

fn pair_prepare(i: u32, li: u32, counting: bool) -> PairFrame {
    if (li == 0u) {
        var frame = PairFrame(0u, 0u, 0u, 0u);
        if (i < body_params.body_count && is_active(i)) {
            frame.integrated = 1u;
            if (counting || bodies[work_word(i, WORK_PAIRS)] > 0u) {
                frame.gather = 1u;
                // The count pass walks the bins; the passes after it read the list it filled,
                // unless the body's candidates overflowed it.
                var candidates = PAIR_LIST_CAPACITY + 1u;
                if (!counting) {
                    candidates = atomicLoad(&bins[pair_count_word(i)]);
                }
                if (candidates <= PAIR_LIST_CAPACITY) {
                    frame.groups = LISTED;
                    frame.total = candidates;
                } else if (body_params.broadphase == BROADPHASE_ALL_PAIRS) {
                    pair_first[0] = 0u;
                    pair_end[0] = body_params.body_count;
                    frame.groups = 1u;
                    frame.total = body_params.body_count;
                }
            }
        }
        for (var k = 0u; k < 3u; k++) {
            atomicStore(&pair_sum[k], 0);
        }
        for (var k = 0u; k < 5u; k++) {
            atomicStore(&pair_tally[k], 0u);
        }
        atomicStore(&pair_extreme[0], 0u);
        atomicStore(&pair_extreme[1], 0u);
        pair_frame = frame;
    }
    let head = workgroupUniformLoad(&pair_frame);
    if (head.gather == 0u || head.groups != 0u) {
        return head;
    }
    if (li < 27u) {
        let k = i32(li);
        let center = bin_of(work_vec3(i, WORK_BINNED), bin_edge());
        pair_slot[li] = bin_slot(center + vec3<i32>(k % 3 - 1, (k / 3) % 3 - 1, k / 9 - 1));
    }
    workgroupBarrier();
    if (li < 27u) {
        let slot = pair_slot[li];
        var repeated = false;
        for (var m = 0u; m < li; m++) {
            repeated = repeated || pair_slot[m] == slot;
        }
        pair_first[li] = atomicLoad(&bins[body_params.starts_base + slot]);
        pair_count[li] = select(atomicLoad(&bins[body_params.counts_base + slot]), 0u, repeated);
    }
    workgroupBarrier();
    if (li == 0u) {
        var total = 0u;
        for (var g = 0u; g < 27u; g++) {
            total += pair_count[g];
            pair_end[g] = total;
        }
        pair_frame.groups = 27u;
        pair_frame.total = total;
    }
    return workgroupUniformLoad(&pair_frame);
}

// Candidate `k` of the workgroup's body `i`: a body slot, or BODY_NONE for an all-pairs slot
// that holds no binned body.
fn pair_candidate(frame: PairFrame, i: u32, k: u32) -> u32 {
    if (frame.groups == LISTED) {
        return atomicLoad(&bins[pair_list_word(i, k)]);
    }
    if (body_params.broadphase == BROADPHASE_ALL_PAIRS) {
        return select(BODY_NONE, k, binned(k));
    }
    var g = 0u;
    var begin = 0u;
    while (g + 1u < frame.groups && pair_end[g] <= k) {
        begin = pair_end[g];
        g++;
    }
    return atomicLoad(&bins[body_params.entries_base + pair_first[g] + k - begin]);
}

// Every lane adds its share of body `i`'s pairs to the workgroup's sums. mode 0: count
// contacts and fill the pair list; 1: position correction; 2: velocity impulses; 3: residuals.
// Positions and velocities are read from half `half`. A listed candidate passed the generation
// test when it was listed.
fn gather_lanes(i: u32, half: u32, mode: u32, frame: PairFrame, li: u32) {
    let listed = frame.groups == LISTED;
    for (var k = li; k < frame.total; k += 64u) {
        let j = pair_candidate(frame, i, k);
        if (!listed && (j == i || j == BODY_NONE || !generated(i, j))) {
            continue;
        }
        let candidate = atomicAdd(&pair_tally[0], 1u);
        if (mode == 0u && candidate < PAIR_LIST_CAPACITY) {
            atomicStore(&bins[pair_list_word(i, candidate)], j);
        }
        let pair = pair_contact(i, j, half, mode != 1u);
        if (pair.kind == 0u) {
            continue;
        }
        atomicAdd(&pair_tally[1], 1u);
        atomicAdd(&pair_tally[select(3u, 2u, pair.kind == 2u)], 1u);
        if (mode == 0u) {
            continue;
        }
        let weighted = mode != 1u;
        let move_i = mobility(i, pair.normal, weighted);
        let weight = move_i.weight + mobility(j, -pair.normal, weighted).weight;
        if (weight <= 1e-6) {
            continue;
        }
        var change = vec3<f32>(0.0);
        var quantum = POSITION_QUANTUM;
        if (mode == 1u) {
            if (pair.depth <= 0.0) {
                continue;
            }
            change = move_i.motion * (pair.depth / weight);
            atomicAdd(&pair_tally[4], 1u);
        } else {
            let vi = work_vec3(i, WORK_VEL_A + half * 4u);
            let vj = work_vec3(j, WORK_VEL_A + half * 4u);
            // Positive apart, negative closing.
            let normal_speed = dot(vi - vj, pair.normal);
            if (pair.kind == 1u && pair.depth > 0.0) {
                atomicMax(&pair_extreme[0], bitcast<u32>(pair.depth));
            }
            if (mode == 3u) {
                if (normal_speed < 0.0) {
                    atomicMax(&pair_extreme[1], bitcast<u32>(-normal_speed));
                }
                continue;
            }
            let parting = separation_target(i, j, pair.normal);
            if (normal_speed >= parting) {
                continue;
            }
            let impulse = (parting - normal_speed) / weight;
            change = move_i.motion * (impulse * pair_scale(i, j));
            quantum = VELOCITY_QUANTUM;
        }
        let fixed = vec3<i32>(round(change * quantum));
        atomicAdd(&pair_sum[0], fixed.x);
        atomicAdd(&pair_sum[1], fixed.y);
        atomicAdd(&pair_sum[2], fixed.z);
    }
}

// The workgroup's sums, once every lane has added its share.
fn gathered() -> Gathered {
    return Gathered(
        vec3<i32>(atomicLoad(&pair_sum[0]), atomicLoad(&pair_sum[1]), atomicLoad(&pair_sum[2])),
        atomicLoad(&pair_tally[0]),
        atomicLoad(&pair_tally[1]),
        atomicLoad(&pair_tally[2]),
        atomicLoad(&pair_tally[3]),
        atomicLoad(&pair_tally[4]),
        bitcast<f32>(atomicLoad(&pair_extreme[0])),
        bitcast<f32>(atomicLoad(&pair_extreme[1])),
    );
}

@compute @workgroup_size(64, 1, 1)
fn pairs_count(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let i = wg.x;
    let frame = pair_prepare(i, li, true);
    if (frame.gather == 0u) {
        return;
    }
    gather_lanes(i, 0u, 0u, frame, li);
    workgroupBarrier();
    if (li == 0u) {
        let found = gathered();
        add64_body(BDIAG_PAIR_VISITS_LO, frame.total);
        bodies[work_word(i, WORK_PAIRS)] = found.contacts;
        atomicStore(&bins[pair_count_word(i)], found.candidates);
        if (found.candidates > PAIR_LIST_CAPACITY) {
            count_body(BDIAG_PAIR_LIST_OVERFLOWS, 1u);
        }
        add64_body(BDIAG_CANDIDATE_PAIRS_LO, found.candidates);
        count_body(BDIAG_CONTACT_PAIRS, found.contacts);
        count_body(BDIAG_SWEPT_CONTACTS, found.swept);
    }
}

@compute @workgroup_size(64, 1, 1)
fn pairs_position(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let i = wg.x;
    let frame = pair_prepare(i, li, false);
    if (frame.integrated == 0u) {
        return;
    }
    let read = pass_params.parity;
    let write = 1u - read;
    if (frame.gather != 0u) {
        gather_lanes(i, read, 1u, frame, li);
    }
    workgroupBarrier();
    if (li != 0u) {
        return;
    }
    var position = work_vec3(i, WORK_POS_A + read * 4u);
    if (frame.gather != 0u) {
        let found = gathered();
        add64_body(BDIAG_PAIR_VISITS_LO, frame.total);
        add64_body(BDIAG_CANDIDATE_PAIRS_LO, found.candidates);
        // Jacobi averaging: the mean of the corrections still needed this pass, within the
        // substep's correction bound around the binned position.
        position += vec3<f32>(found.sum) / (POSITION_QUANTUM * f32(max(found.correcting, 1u)));
        let binned_at = work_vec3(i, WORK_BINNED);
        let moved = position - binned_at;
        let reach = length(moved);
        if (reach > MAX_PAIR_CORRECTION) {
            position = binned_at + moved * (MAX_PAIR_CORRECTION / reach);
        }
    }
    set_work_vec3(i, WORK_POS_A + write * 4u, position);
    // Velocities pass through position iterations unchanged.
    set_work_vec3(i, WORK_VEL_A + write * 4u, work_vec3(i, WORK_VEL_A + read * 4u));
}

@compute @workgroup_size(64, 1, 1)
fn pairs_velocity(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let i = wg.x;
    let frame = pair_prepare(i, li, false);
    if (frame.integrated == 0u) {
        return;
    }
    let read = pass_params.parity;
    let write = 1u - read;
    if (frame.gather != 0u) {
        gather_lanes(i, read, 2u, frame, li);
    }
    workgroupBarrier();
    if (li != 0u) {
        return;
    }
    var velocity = work_vec3(i, WORK_VEL_A + read * 4u);
    if (frame.gather != 0u) {
        let found = gathered();
        add64_body(BDIAG_PAIR_VISITS_LO, frame.total);
        add64_body(BDIAG_CANDIDATE_PAIRS_LO, found.candidates);
        velocity += vec3<f32>(found.sum) / VELOCITY_QUANTUM;
    }
    set_work_vec3(i, WORK_VEL_A + write * 4u, velocity);
    set_work_vec3(i, WORK_POS_A + write * 4u, work_vec3(i, WORK_POS_A + read * 4u));
}

@compute @workgroup_size(64, 1, 1)
fn pairs_residual(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let i = wg.x;
    let frame = pair_prepare(i, li, false);
    if (frame.gather == 0u) {
        return;
    }
    gather_lanes(i, 0u, 3u, frame, li);
    workgroupBarrier();
    if (li == 0u) {
        let found = gathered();
        max_body(BDIAG_RESIDUAL_OVERLAP, found.deepest);
        max_body(BDIAG_RESIDUAL_APPROACH, found.fastest);
        bodies[work_word(i, WORK_TOUCHING)] = found.touching;
    }
}
