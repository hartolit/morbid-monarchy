// Shared deformation and displacement: who may change a cell, and the crater and trail rules.
//
// Bodies change terrain and liquid in two rounds per substep (ground deformation, then liquid
// displacement). In each round a cell has at most one owner, so every changed cell has exactly
// one writer and two bodies can never spend the same material:
//
// claims_clear   each active body resets the owner word of every cell it might claim this
//                round (a box around its centre, or around its whole swept segment).
// *_prepare      each body claims the cells its rule may change with atomicMin of a key:
//                (role, squared distance, body index). Role 0 is a cell under the body (a
//                source, a crushed or pressed column, ice, foliage); role 1 a cell it would
//                deposit into (a rim, a berm, a splash ring). Lower keys win, so the body
//                standing on a cell owns it before one that would bury it, the nearer body
//                before the farther, and the lower index breaks ties; the outcome is the same
//                whatever order invocations run in.
// *_apply        each body runs the baseline's rule over the cells it owns, treating the others
//                as if outside the window (the baseline's own edge behaviour): material it could
//                not take stays where it was, room it could not use stays free. A body moves
//                material only between cells it owns, so the round conserves exactly.
//
// Owned cells are copied into workgroup memory, changed there in the baseline's order, and
// written back once; each change is counted (`BDIAG_*`) and marked edited, so the next tick's
// material phases wake it like a brush stroke.

const KEY_ROLE_SINK: u32 = 0x40000000u;
const CLAIM_FREE: u32 = 0xFFFFFFFFu;
const APPLY_SIDE: u32 = 27u;
const APPLY_CELLS: u32 = 729u;

// Bits of `cell_info`.
const INFO_OWNED: u32 = 1u;
const INFO_CHANGED: u32 = 2u;
const INFO_LOOSE_SHIFT: u32 = 8u;
const INFO_LOOSE_MATERIAL_SHIFT: u32 = 12u;
const INFO_HAS_LOOSE: u32 = 0x10000u;

fn claim_key(role: u32, distance_squared: f32, b: u32) -> u32 {
    let distance = min(u32(distance_squared * 64.0), 0x1FFFFu);
    return role | (distance << 13u) | b;
}

fn claim(cell: vec2<i32>, key: u32) {
    let slot = resident_slot(cell);
    if (slot != SLOT_NONE) {
        atomicMin(&claims[cell_index(slot, cell)], key);
    }
}

fn owns(cell: vec2<i32>, key: u32) -> bool {
    let slot = resident_slot(cell);
    return slot != SLOT_NONE && atomicLoad(&claims[cell_index(slot, cell)]) == key;
}

// The box of cells within `reach` of the horizontal point `p`: GridPhysics's `cells_within`
// iterates its rows then its columns.
struct CellBox {
    low: vec2<i32>,
    size: vec2<i32>,
}

fn cell_box(p: vec3<f32>, reach: f32) -> CellBox {
    let low = vec2<i32>(i32(floor(p.x - reach)), i32(floor(-p.z - reach)));
    let high = vec2<i32>(i32(ceil(p.x + reach)), i32(ceil(-p.z + reach)));
    return CellBox(low, high - low + vec2<i32>(1));
}

fn box_cell(bx: CellBox, i: u32) -> vec2<i32> {
    return bx.low + vec2<i32>(i32(i % u32(bx.size.x)), i32(i / u32(bx.size.x)));
}

fn horizontal_distance_squared(p: vec3<f32>, cell: vec2<i32>) -> f32 {
    let dx = f32(cell.x) - p.x;
    let dz = -f32(cell.y) - p.z;
    return dx * dx + dz * dz;
}

@compute @workgroup_size(64, 1, 1)
fn claims_clear(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let b = wg.x;
    if (b >= body_params.body_count || (bodies[work_word(b, WORK_FLAGS)] & WORK_ACTIVE) == 0u) {
        return;
    }
    let radius = load_body_config(config_base(b)).radius;
    let center = work_vec3(b, WORK_POS_A);
    var low = center;
    var high = center;
    var reach = radius * SPLASH_RING_OUTER_RATIO + 1.0;
    if (pass_params.iteration == 0u) {
        // The terrain round may stop a sweep anywhere on the segment from the substep's start.
        let previous = work_vec3(b, WORK_PREV);
        low = min(center, previous);
        high = max(center, previous);
        reach = radius * RIM_EXPANSION + 1.0;
    }
    let first = vec2<i32>(i32(floor(low.x - reach)), i32(floor(-high.z - reach)));
    let last = vec2<i32>(i32(ceil(high.x + reach)), i32(ceil(-low.z + reach)));
    let size = last - first + vec2<i32>(1);
    let count = u32(size.x * size.y);
    for (var i = li; i < count; i += 64u) {
        let cell = first + vec2<i32>(i32(i % u32(size.x)), i32(i / u32(size.x)));
        let slot = resident_slot(cell);
        if (slot != SLOT_NONE) {
            atomicStore(&claims[cell_index(slot, cell)], CLAIM_FREE);
        }
    }
}

// Foliage taller than the underside of a body standing on its column is cleared
// (GridPhysicsApi::clear_surface_organics).
fn clears_foliage(c: vec2<u32>, underside: f32) -> bool {
    if (cell_surface_material(c) != SURFACE_FOLIAGE) {
        return false;
    }
    let scale = body_params.elevation_scale;
    let base = f32(cell_elevation(c) + cell_granular_volume(c) + cell_fluid_volume(c)) * scale;
    let top = base + max(1.0, f32(cell_surface_state(c))) * scale;
    return underside < top;
}

fn underside_at(center: vec3<f32>, radius: f32, distance_squared: f32) -> f32 {
    return center.y - sqrt(max(radius * radius - distance_squared, 0.0));
}

// Claims what the terrain round's decision and the foliage clearing may change (role 0 under
// the body, role 1 where it deposits).
fn claim_footprint(b: u32, f: Frame, li: u32) {
    let r = f.radius;
    let rim_outer = r * RIM_EXPANSION;
    let bx = cell_box(f.center, rim_outer);
    let count = u32(bx.size.x * bx.size.y);
    let heading = rut_heading(f.velocity);
    for (var i = li; i < count; i += 64u) {
        let cell = box_cell(bx, i);
        let d2 = horizontal_distance_squared(f.center, cell);
        let col = column(cell);
        if (!col.present) {
            continue;
        }
        var role = 2u;
        if (d2 <= r * r && clears_foliage(col.value, underside_at(f.center, r, d2))) {
            role = 0u;
        }
        if (f.decision == DECIDE_CRATER) {
            if (d2 <= r * r) {
                role = 0u;
            } else if (d2 <= rim_outer * rim_outer && d2 >= (r * RIM_INNER_RATIO) * (r * RIM_INNER_RATIO)) {
                role = min(role, 1u);
            }
        } else if (f.decision == DECIDE_RUT) {
            let width = r * RUT_WIDTH_RATIO;
            let inner = r * RUT_BERM_INNER_RATIO;
            let outer = r * RUT_BERM_OUTER_RATIO;
            if (d2 <= width * width) {
                role = 0u;
            } else if (d2 >= inner * inner && d2 <= outer * outer && berm_side(f.center, cell, heading)) {
                role = min(role, 1u);
            }
        }
        if (role == 0u) {
            claim(cell, claim_key(0u, d2, b));
        } else if (role == 1u) {
            claim(cell, claim_key(KEY_ROLE_SINK, d2, b));
        }
    }
}

// The direction a rolling body's berms form beside: its horizontal heading, or none (zero)
// below RUT_HEADING_MIN_SPEED.
fn rut_heading(velocity: vec3<f32>) -> vec2<f32> {
    let horizontal = vec2<f32>(velocity.x, velocity.z);
    if (length(horizontal) > RUT_HEADING_MIN_SPEED) {
        return normalize(horizontal);
    }
    return vec2<f32>(0.0);
}

// RimShape's heading filter: beside the direction of travel, or anywhere without a heading.
fn berm_side(center: vec3<f32>, cell: vec2<i32>, heading: vec2<f32>) -> bool {
    if (all(heading == vec2<f32>(0.0))) {
        return true;
    }
    let offset = vec2<f32>(f32(cell.x) - center.x, -f32(cell.y) - center.z);
    let length_offset = length(offset);
    var direction = vec2<f32>(0.0);
    if (length_offset > 0.0) {
        direction = offset / length_offset;
    }
    return abs(dot(direction, heading)) <= RUT_BERM_SIDE_COSINE;
}

// GridPhysicsApi::granular_above: loose units wholly above `limit`, as (material, units).
fn granular_above(c: vec2<u32>, limit: f32) -> vec2<u32> {
    if (fluid_is_solid(cell_fluid_material(c))) {
        return vec2<u32>(0u);
    }
    let volume = cell_granular_volume(c);
    let scale = body_params.elevation_scale;
    let solid = f32(cell_elevation(c)) * scale;
    if (volume == 0u || solid + f32(volume) * scale <= limit) {
        return vec2<u32>(0u);
    }
    let kept = u32(max(ceil((limit - solid) / scale - QUANTIZATION_TOLERANCE), 0.0));
    if (volume <= kept) {
        return vec2<u32>(0u);
    }
    return vec2<u32>(cell_granular_material(c), volume - kept);
}

// GridPhysicsApi::granular_room.
fn granular_room(c: vec2<u32>, material: u32, ceiling: f32, max_per_cell: u32) -> u32 {
    let resting = cell_granular_material(c);
    if (resting != GRANULAR_EMPTY && resting != material) {
        return 0u;
    }
    let scale = body_params.elevation_scale;
    let top = f32(cell_elevation(c) + cell_granular_volume(c)) * scale;
    if (top >= ceiling) {
        return 0u;
    }
    let allowance = u32(min(floor((ceiling - top) / scale), 65535.0));
    return min(min(CELL_MAX_GRANULAR_VOLUME - cell_granular_volume(c), max_per_cell), allowance);
}

var<workgroup> work_cells: array<vec2<u32>, APPLY_CELLS>;
var<workgroup> cell_info: array<u32, APPLY_CELLS>;
var<workgroup> lane_sources: array<u32, 64>;
var<workgroup> lane_sinks: array<u32, 64>;
var<workgroup> material_totals: array<atomic<u32>, 16>;
var<workgroup> material_last: array<atomic<u32>, 16>;
// What the round decided, shared by every invocation: the dominant material, the source and
// sink totals, and the energy left after crushing.
var<workgroup> round_totals: vec4<u32>;
var<workgroup> round_sums: vec2<u32>;
var<workgroup> round_energy: f32;

struct ApplyFrame {
    enabled: u32,
    decision: u32,
    radius: f32,
    mass: f32,
    center: vec3<f32>,
    velocity: vec3<f32>,
    approach: f32,
    normal: vec3<f32>,
    energy: f32,
    extension: f32,
    crush_cost: f32,
    rim_ceiling: f32,
    berm_ceiling: f32,
    heading: vec2<f32>,
    contact: Frame,
}

var<workgroup> apply_frame: ApplyFrame;

// The box's run of cells each invocation handles in the prefix sums: contiguous, in box order.
fn lane_run(count: u32, li: u32) -> vec2<u32> {
    let per = (count + 63u) / 64u;
    let first = min(li * per, count);
    return vec2<u32>(first, min(first + per, count));
}

// This invocation's exclusive prefix of the two lane sums. The totals go to `round_sums`,
// which callers read with workgroupUniformLoad so they can steer barriers.
fn lane_prefix(li: u32) -> vec2<u32> {
    workgroupBarrier();
    var sources = 0u;
    var sinks = 0u;
    var total_sources = 0u;
    var total_sinks = 0u;
    for (var lane = 0u; lane < 64u; lane++) {
        if (lane < li) {
            sources += lane_sources[lane];
            sinks += lane_sinks[lane];
        }
        total_sources += lane_sources[lane];
        total_sinks += lane_sinks[lane];
    }
    workgroupBarrier();
    if (li == 0u) {
        round_sums = vec2<u32>(total_sources, total_sinks);
    }
    return vec2<u32>(sources, sinks);
}

// Marks a changed cell edited and counts what it gained and lost.
fn commit_cell(cell: vec2<i32>, before: vec2<u32>, after: vec2<u32>) {
    let slot = resident_slot(cell);
    cells[cell_index(slot, cell)] = after;
    mark_slot_changed(slot);
    let tile = tile_id(tile_of_cell(cell));
    let within = vec2<u32>(cell) % TILE_EDGE;
    atomicOr(&activity[tile_word(tile, TILE_EDITED + bitmap_word(within))], bitmap_bit(within));
    mark_candidates(tile, reach_mask(within, u32(WAKE_CHANGE_RADIUS) + 1u));
    let fluid_before = cell_fluid_volume(before);
    let fluid_after = cell_fluid_volume(after);
    add64_body(BDIAG_FLUID_IN_LO, max(fluid_after, fluid_before) - fluid_before);
    add64_body(BDIAG_FLUID_OUT_LO, max(fluid_after, fluid_before) - fluid_after);
    let solid_before = cell_elevation(before) + cell_granular_volume(before);
    let solid_after = cell_elevation(after) + cell_granular_volume(after);
    add64_body(BDIAG_SOLID_IN_LO, max(solid_after, solid_before) - solid_before);
    add64_body(BDIAG_SOLID_OUT_LO, max(solid_after, solid_before) - solid_after);
}

fn load_apply_frame(b: u32) -> ApplyFrame {
    var a: ApplyFrame;
    let f = load_contact_frame(b);
    a.enabled = f.enabled;
    a.decision = bodies[work_word(b, WORK_DECISION)];
    a.radius = f.radius;
    a.mass = f.mass;
    a.center = work_vec3(b, WORK_CENTER);
    a.velocity = f.velocity;
    a.approach = work_f32(b, WORK_APPROACH);
    a.normal = work_vec3(b, WORK_NORMAL);
    a.energy = work_f32(b, WORK_ENERGY);
    a.extension = work_f32(b, WORK_EXTENSION);
    a.crush_cost = work_f32(b, WORK_CRUSH_COST);
    a.rim_ceiling = work_f32(b, WORK_RIM_CEILING);
    a.berm_ceiling = work_f32(b, WORK_BERM_CEILING);
    a.heading = vec2<f32>(work_f32(b, WORK_HEADING), work_f32(b, WORK_HEADING + 1u));
    a.contact = f;
    return a;
}

// The key body `b` claimed `cell` with in the terrain round (CLAIM_FREE when none).
fn terrain_key(a: ApplyFrame, b: u32, cell: vec2<i32>, c: vec2<u32>) -> u32 {
    let r = a.radius;
    let d2 = horizontal_distance_squared(a.center, cell);
    var role = 2u;
    if (d2 <= r * r && clears_foliage(c, underside_at(a.center, r, d2))) {
        role = 0u;
    }
    if (a.decision == DECIDE_CRATER) {
        if (d2 <= r * r) {
            role = 0u;
        } else if (d2 <= (r * RIM_EXPANSION) * (r * RIM_EXPANSION) && d2 >= (r * RIM_INNER_RATIO) * (r * RIM_INNER_RATIO)) {
            role = min(role, 1u);
        }
    } else if (a.decision == DECIDE_RUT) {
        let width = r * RUT_WIDTH_RATIO;
        if (d2 <= width * width) {
            role = 0u;
        } else if (d2 >= (r * RUT_BERM_INNER_RATIO) * (r * RUT_BERM_INNER_RATIO)
            && d2 <= (r * RUT_BERM_OUTER_RATIO) * (r * RUT_BERM_OUTER_RATIO)
            && berm_side(a.center, cell, a.heading)) {
            role = min(role, 1u);
        }
    }
    if (role == 0u) {
        return claim_key(0u, d2, b);
    }
    if (role == 1u) {
        return claim_key(KEY_ROLE_SINK, d2, b);
    }
    return CLAIM_FREE;
}

@compute @workgroup_size(64, 1, 1)
fn contact_apply(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let b = wg.x;
    if (li == 0u) {
        apply_frame = load_apply_frame(b);
    }
    let a = workgroupUniformLoad(&apply_frame);
    if (a.enabled == 0u || b >= body_params.body_count) {
        return;
    }
    let r = a.radius;
    let bx = cell_box(a.center, r * RIM_EXPANSION);
    let count = u32(bx.size.x * bx.size.y);

    // Load the owned cells; count the ones another body took.
    var lost = 0u;
    for (var i = li; i < count; i += 64u) {
        let cell = box_cell(bx, i);
        let col = column(cell);
        var info = 0u;
        if (col.present) {
            let key = terrain_key(a, b, cell, col.value);
            if (key != CLAIM_FREE) {
                if (owns(cell, key)) {
                    info = INFO_OWNED;
                    work_cells[i] = col.value;
                } else {
                    lost += 1u;
                }
            }
        }
        cell_info[i] = info;
    }
    count_body(BDIAG_CLAIMS_LOST, lost);
    if (li < 16u) {
        atomicStore(&material_totals[li], 0u);
        atomicStore(&material_last[li], 0u);
    }
    workgroupBarrier();

    // Foliage the body's underside reaches is cleared.
    var cleared = 0u;
    for (var i = li; i < count; i += 64u) {
        let cell = box_cell(bx, i);
        let d2 = horizontal_distance_squared(a.center, cell);
        if ((cell_info[i] & INFO_OWNED) != 0u && d2 <= r * r
            && clears_foliage(work_cells[i], underside_at(a.center, r, d2))) {
            work_cells[i] = cell_with_surface_state(cell_with_surface_material(work_cells[i], SURFACE_EMPTY), 0u);
            cell_info[i] |= INFO_CHANGED;
            cleared += 1u;
        }
    }
    count_body(BDIAG_ORGANICS_CLEARED, cleared);
    workgroupBarrier();

    if (a.decision == DECIDE_CRATER) {
        apply_crater(b, a, bx, count, li);
    } else if (a.decision == DECIDE_RUT) {
        apply_rut(b, a, bx, count, li);
    }
    workgroupBarrier();

    for (var i = li; i < count; i += 64u) {
        if ((cell_info[i] & INFO_CHANGED) != 0u) {
            let cell = box_cell(bx, i);
            let before = column(cell).value;
            if (any(before != work_cells[i])) {
                commit_cell(cell, before, work_cells[i]);
            }
        }
    }
}

// Picks the dominant loose material (Iterator::max_by_key over the loose entries in order: the
// largest total, ties going to the material whose last entry comes last) and stores it in
// `round_totals.x`, MATERIAL_NONE when there is no loose material.
fn choose_dominant(li: u32) {
    workgroupBarrier();
    if (li == 0u) {
        var best = MATERIAL_NONE;
        var best_total = 0u;
        var best_last = 0u;
        for (var m = 0u; m < 16u; m++) {
            let total = atomicLoad(&material_totals[m]);
            let last = atomicLoad(&material_last[m]);
            if (last > 0u && (best == MATERIAL_NONE || total > best_total || (total == best_total && last > best_last))) {
                best = m;
                best_total = total;
                best_last = last;
            }
        }
        round_totals = vec4<u32>(best, 0u, 0u, 0u);
    }
    workgroupBarrier();
}

fn loose_units(info: u32) -> u32 {
    return (info >> INFO_LOOSE_SHIFT) & 0xFu;
}

fn loose_material(info: u32) -> u32 {
    return (info >> INFO_LOOSE_MATERIAL_SHIFT) & 0xFu;
}

// Records loose material a body would shove from cell `i`.
fn note_loose(i: u32, material: u32, units: u32) {
    cell_info[i] |= INFO_HAS_LOOSE | (min(units, 15u) << INFO_LOOSE_SHIFT) | (material << INFO_LOOSE_MATERIAL_SHIFT);
    atomicAdd(&material_totals[material], units);
    atomicMax(&material_last[material], i + 1u);
}

// shove_into_rim: moves the dominant loose material from the noted cells into the sinks the
// `sink_room` rule gives, at most `limit` units, apportioned exactly. Returns the units moved.
fn shove(a: ApplyFrame, bx: CellBox, count: u32, li: u32, limit: u32, crater: bool) -> u32 {
    choose_dominant(li);
    let dominant = workgroupUniformLoad(&round_totals).x;
    if (dominant == MATERIAL_NONE) {
        return 0u;
    }
    let run = lane_run(count, li);
    var sources = 0u;
    var sinks = 0u;
    for (var i = run.x; i < run.y; i++) {
        sources += shove_source(i, dominant);
        sinks += shove_sink(a, bx, i, dominant, crater);
    }
    lane_sources[li] = sources;
    lane_sinks[li] = sinks;
    let prefix = lane_prefix(li);
    let totals = workgroupUniformLoad(&round_sums);
    let moved = min(min(totals.x, totals.y), limit);
    if (moved == 0u) {
        return 0u;
    }
    var before_sources = prefix.x;
    var before_sinks = prefix.y;
    var taken_before = mul_div_floor(moved, before_sources, totals.x);
    var given_before = mul_div_floor(moved, before_sinks, totals.y);
    for (var i = run.x; i < run.y; i++) {
        let source = shove_source(i, dominant);
        let sink = shove_sink(a, bx, i, dominant, crater);
        var c = work_cells[i];
        if (source > 0u) {
            before_sources += source;
            let through = mul_div_floor(moved, before_sources, totals.x);
            let share = through - taken_before;
            taken_before = through;
            if (share > 0u) {
                let remaining = cell_granular_volume(c) - share;
                c = cell_with_granular_volume(c, remaining);
                if (remaining == 0u) {
                    c = cell_with_granular_material(c, GRANULAR_EMPTY);
                }
                cell_info[i] |= INFO_CHANGED;
            }
        }
        if (sink > 0u) {
            before_sinks += sink;
            let through = mul_div_floor(moved, before_sinks, totals.y);
            let share = through - given_before;
            given_before = through;
            if (share > 0u) {
                c = cell_with_granular_material(c, dominant);
                c = cell_with_granular_volume(c, cell_granular_volume(c) + share);
                cell_info[i] |= INFO_CHANGED;
            }
        }
        work_cells[i] = c;
    }
    workgroupBarrier();
    return moved;
}

fn shove_source(i: u32, dominant: u32) -> u32 {
    let info = cell_info[i];
    if ((info & INFO_HAS_LOOSE) == 0u || loose_material(info) != dominant) {
        return 0u;
    }
    return loose_units(info);
}

fn shove_sink(a: ApplyFrame, bx: CellBox, i: u32, dominant: u32, crater: bool) -> u32 {
    if ((cell_info[i] & INFO_OWNED) == 0u) {
        return 0u;
    }
    let cell = box_cell(bx, i);
    let d2 = horizontal_distance_squared(a.center, cell);
    let r = a.radius;
    if (crater) {
        let inner = r * RIM_INNER_RATIO;
        let outer = r * RIM_EXPANSION;
        if (d2 < inner * inner || d2 > outer * outer) {
            return 0u;
        }
        return granular_room(work_cells[i], dominant, a.rim_ceiling, MAX_RIM_DEPOSIT);
    }
    let inner = r * RUT_BERM_INNER_RATIO;
    let outer = r * RUT_BERM_OUTER_RATIO;
    if (d2 < inner * inner || d2 > outer * outer || !berm_side(a.center, cell, a.heading)) {
        return 0u;
    }
    return granular_room(work_cells[i], dominant, a.berm_ceiling, MAX_RIM_DEPOSIT);
}

// carve_impact_crater: ice under the extended underside breaks, bedrock above it is crushed
// in footprint order while the impact energy lasts, and loose material above it is shoved into
// the rim, as much as the remaining energy pays for.
fn apply_crater(b: u32, a: ApplyFrame, bx: CellBox, count: u32, li: u32) {
    let r = a.radius;
    let scale = body_params.elevation_scale;
    if (li == 0u) {
        var remaining = a.energy;
        var crushed = 0u;
        var broken = 0u;
        for (var i = 0u; i < count; i++) {
            let cell = box_cell(bx, i);
            let d2 = horizontal_distance_squared(a.center, cell);
            if ((cell_info[i] & INFO_OWNED) == 0u || d2 > r * r) {
                continue;
            }
            let underside = underside_at(a.center, r, d2) - a.extension;
            var c = work_cells[i];
            if (fluid_is_solid(cell_fluid_material(c)) && top_height(c) > underside) {
                let melted = thaw(c);
                if (any(melted != c)) {
                    c = melted;
                    broken += 1u;
                    cell_info[i] |= INFO_CHANGED;
                }
            }
            let solid = f32(cell_elevation(c)) * scale;
            if (remaining > 0.0 && solid > underside && cell_elevation(c) > 0u) {
                let unit_cost = a.crush_cost * terrain_crush_hardness(cell_terrain_material(c));
                let required = u32(min(ceil((solid - underside) / scale), 65535.0));
                let allowance = u32(min(floor(remaining / unit_cost), 65535.0));
                let units = min(min(required, cell_elevation(c)), allowance);
                if (units > 0u) {
                    c = cell_with_elevation(c, cell_elevation(c) - units);
                    remaining = max(remaining - f32(units) * unit_cost, 0.0);
                    crushed += units;
                    cell_info[i] |= INFO_CHANGED;
                }
            }
            work_cells[i] = c;
        }
        round_energy = remaining;
        add64_body(BDIAG_COMPACTION_LO, crushed);
        count_body(BDIAG_ICE_BROKEN, broken);
    }
    workgroupBarrier();
    for (var i = li; i < count; i += 64u) {
        let cell = box_cell(bx, i);
        let d2 = horizontal_distance_squared(a.center, cell);
        if ((cell_info[i] & INFO_OWNED) != 0u && d2 <= r * r) {
            let loose = granular_above(work_cells[i], underside_at(a.center, r, d2) - a.extension);
            if (loose.y > 0u) {
                note_loose(i, loose.x, loose.y);
            }
        }
    }
    let remaining = workgroupUniformLoad(&round_energy);
    let affordable = u32(min(floor(remaining / COST_DISPLACE_GRANULAR), 4294967040.0));
    let moved = shove(a, bx, count, li, affordable, true);
    if (li == 0u) {
        let left = max(remaining - f32(moved) * COST_DISPLACE_GRANULAR, 0.0);
        let absorbed = a.energy - left;
        let retained = sqrt(max((a.energy - absorbed) / a.energy, 0.0));
        let velocity = a.velocity + a.normal * (a.approach * (1.0 - retained));
        set_work_vec3(b, WORK_VEL_A, velocity);
        var s = load_body_state(state_base(b));
        s.craters += 1u;
        s.absorbed_energy += u32(min(absorbed * 16.0, 4294967040.0));
        s.tick_events |= TICK_CRATER;
        s.tick_impact = max(s.tick_impact, a.approach);
        store_body_state(state_base(b), s);
        count_body(BDIAG_CRATERS, 1u);
    }
}

// press_rut: columns the underside has sunk into are pressed to the lower of the underside and
// their yield floor; loose material is shoved into berms beside the trail and bare soft bedrock
// compacts. The work costs kinetic energy.
fn apply_rut(b: u32, a: ApplyFrame, bx: CellBox, count: u32, li: u32) {
    let r = a.radius;
    let width = r * RUT_WIDTH_RATIO;
    let scale = body_params.elevation_scale;
    var compacted = 0u;
    for (var i = li; i < count; i += 64u) {
        let cell = box_cell(bx, i);
        let d2 = horizontal_distance_squared(a.center, cell);
        if ((cell_info[i] & INFO_OWNED) == 0u || d2 > width * width) {
            continue;
        }
        let c = work_cells[i];
        let sinkage = column_sinkage(a.contact, c);
        if (sinkage <= 0.0) {
            continue;
        }
        let top = top_height(c);
        let yield_floor = plane_at(a.contact, cell) - sinkage;
        let pressed_to = max(underside_at(a.center, r, d2), yield_floor);
        if (top <= pressed_to) {
            continue;
        }
        let loose = granular_above(c, pressed_to);
        if (loose.y > 0u) {
            note_loose(i, loose.x, min(loose.y, RUT_PRESS_PER_SUBSTEP));
        } else if (cell_granular_volume(c) == 0u && !fluid_is_solid(cell_fluid_material(c))) {
            // compact_bedrock
            let excess = floor((f32(cell_elevation(c)) * scale - pressed_to) / scale + QUANTIZATION_TOLERANCE);
            let pressed = min(min(u32(max(excess, 0.0)), RUT_PRESS_PER_SUBSTEP), cell_elevation(c));
            if (pressed > 0u) {
                work_cells[i] = cell_with_elevation(c, cell_elevation(c) - pressed);
                cell_info[i] |= INFO_CHANGED;
                compacted += pressed;
            }
        }
    }
    lane_sources[li] = compacted;
    lane_sinks[li] = 0u;
    _ = lane_prefix(li);
    let compaction = workgroupUniformLoad(&round_sums).x;
    let shoved = shove(a, bx, count, li, 0xFFFFFFFFu, false);
    if (li == 0u) {
        add64_body(BDIAG_COMPACTION_LO, compaction);
        let spent = f32(shoved + compaction) * RUT_COST_PER_UNIT;
        let kinetic = 0.5 * a.mass * dot(a.velocity, a.velocity);
        if (spent > 0.0 && kinetic > 0.0) {
            set_work_vec3(b, WORK_VEL_A, a.velocity * sqrt(max(kinetic - spent, 0.0) / kinetic));
            count_body(BDIAG_RUTS, 1u);
        }
    }
}
