// Liquid displacement, one workgroup per body: the baseline's `displace_liquids`, as the second
// round of owned cells (deform.wgsl). A splash crown when a body plunges in, a bow wave while it
// ploughs along the surface; displaced liquid is only moved, never removed, and the material
// phases let it flow back. Sources (the cavity, or the strip ahead of the waterline) and sinks
// (the ring around them) never share a cell, so each owned cell changes once, straight in
// `cells`.

struct DisplaceFrame {
    enabled: u32,
    decision: u32,
    radius: f32,
    mass: f32,
    center: vec3<f32>,
    velocity: vec3<f32>,
    fluid: u32,
    budget: u32,
    inner: f32,
    outer: f32,
    entry_energy: f32,
    heading: vec2<f32>,
    pending: f32,
}

var<workgroup> displace_frame: DisplaceFrame;

fn load_displace_frame(b: u32) -> DisplaceFrame {
    var d: DisplaceFrame;
    let s = load_body_state(state_base(b));
    let flags = bodies[work_word(b, WORK_FLAGS)];
    d.enabled = select(0u, 1u, (s.flags & BODY_ALIVE) != 0u && (flags & WORK_ACTIVE) != 0u);
    let config = load_body_config(config_base(b));
    d.radius = config.radius;
    d.mass = config.mass;
    d.center = work_vec3(b, WORK_POS_A);
    d.velocity = work_vec3(b, WORK_VEL_A);
    d.decision = bodies[work_word(b, WORK_DISPLACE)];
    d.fluid = bodies[work_word(b, WORK_FLUID)];
    d.budget = bodies[work_word(b, WORK_BUDGET)];
    d.inner = work_f32(b, WORK_INNER);
    d.outer = work_f32(b, WORK_OUTER);
    d.entry_energy = work_f32(b, WORK_ENTRY_ENERGY);
    d.heading = vec2<f32>(work_f32(b, WORK_WAKE_HEADING), work_f32(b, WORK_WAKE_HEADING + 1u));
    d.pending = s.pending_wake;
    return d;
}

// Units cell `cell` gives as a source, or takes as a sink (role 1), under the round's rule.
fn displace_units(d: DisplaceFrame, cell: vec2<i32>, c: vec2<u32>, sink: bool) -> u32 {
    let d2 = horizontal_distance_squared(d.center, cell);
    if (sink) {
        if (d2 <= d.inner * d.inner || d2 > d.outer * d.outer) {
            return 0u;
        }
        // fluid_room: dry, or holding the same liquid.
        let resting = cell_fluid_material(c);
        if (resting != FLUID_EMPTY && resting != d.fluid) {
            return 0u;
        }
        let cap = select(WAKE_MAX_RING_PER_CELL, SPLASH_MAX_PER_CELL, d.decision == DISPLACE_SPLASH);
        return min(CELL_MAX_FLUID_VOLUME - cell_fluid_volume(c), cap);
    }
    if (d2 > d.inner * d.inner) {
        return 0u;
    }
    let liquid = liquid_in(c);
    if (!liquid.present || liquid.material != d.fluid) {
        return 0u;
    }
    if (d.decision == DISPLACE_SPLASH) {
        // The cavity is scooped deepest under the centre and tapers to its edge.
        let taper = 1.0 - sqrt(d2) / d.inner;
        return u32(f32(liquid.volume) * taper);
    }
    // Only liquid ahead of the waterline is shoved aside.
    let offset = vec2<f32>(f32(cell.x) - d.center.x, -f32(cell.y) - d.center.z);
    if (dot(offset, d.heading) <= 0.0) {
        return 0u;
    }
    let underside = underside_at(d.center, d.radius, d2);
    let overlap = u32(max(floor((liquid.surface - underside) / body_params.elevation_scale), 0.0));
    return min(min(overlap, liquid.volume), WAKE_MAX_PER_CELL);
}

fn displace_key(d: DisplaceFrame, b: u32, cell: vec2<i32>, c: vec2<u32>) -> u32 {
    let d2 = horizontal_distance_squared(d.center, cell);
    if (displace_units(d, cell, c, false) > 0u) {
        return claim_key(0u, d2, b);
    }
    if (displace_units(d, cell, c, true) > 0u) {
        return claim_key(KEY_ROLE_SINK, d2, b);
    }
    return CLAIM_FREE;
}

@compute @workgroup_size(64, 1, 1)
fn displace_prepare(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let b = wg.x;
    if (li == 0u) {
        var d = load_displace_frame(b);
        d.decision = DISPLACE_NONE;
        if (d.enabled != 0u && b < body_params.body_count) {
            decide_displacement(b, &d);
        }
        displace_frame = d;
    }
    let d = workgroupUniformLoad(&displace_frame);
    if (d.enabled == 0u || d.decision == DISPLACE_NONE || b >= body_params.body_count) {
        return;
    }
    let bx = cell_box(d.center, d.outer);
    let count = u32(bx.size.x * bx.size.y);
    for (var i = li; i < count; i += 64u) {
        let cell = box_cell(bx, i);
        let col = column(cell);
        if (col.present) {
            let key = displace_key(d, b, cell, col.value);
            if (key != CLAIM_FREE) {
                claim(cell, key);
            }
        }
    }
}

// What the body does to the liquid this substep: splash, wake or nothing, with its budget.
fn decide_displacement(b: u32, d: ptr<function, DisplaceFrame>) {
    var s = load_body_state(state_base(b));
    let r = (*d).radius;
    let liquid = liquid_at(nearest_cell((*d).center));
    if (!liquid.present) {
        s.pending_wake = 0.0;
        store_body_state(state_base(b), s);
        bodies[work_word(b, WORK_DISPLACE)] = DISPLACE_NONE;
        return;
    }
    (*d).fluid = liquid.material;
    let entering = s.previous_submerged <= 0.0 && s.submerged > 0.0;
    let plunge = -(*d).velocity.y;
    if (entering && plunge > SPLASH_MIN_SPEED) {
        let energy = 0.5 * (*d).mass * plunge * plunge;
        (*d).decision = DISPLACE_SPLASH;
        (*d).inner = r * SPLASH_CAVITY_RATIO;
        (*d).outer = r * SPLASH_RING_OUTER_RATIO;
        (*d).entry_energy = energy;
        (*d).budget = u32(min(energy * SPLASH_UNITS_PER_ENERGY, 4294967040.0));
    } else {
        // The bow wave comes from the waterline: the circle where the sphere cuts the surface.
        let above = (*d).center.y - liquid.surface;
        let horizontal = vec2<f32>((*d).velocity.x, (*d).velocity.z);
        let speed = length(horizontal);
        if (abs(above) >= r || speed < WAKE_MIN_SPEED) {
            s.pending_wake = 0.0;
        } else {
            let waterline = sqrt(r * r - above * above);
            s.pending_wake += 2.0 * waterline * (waterline / r) * speed * body_params.substep_seconds * WAKE_LAYER_UNITS;
            let budget = u32(floor(s.pending_wake));
            if (budget > 0u) {
                (*d).decision = DISPLACE_WAKE;
                (*d).inner = waterline;
                (*d).outer = waterline + WAKE_RING_WIDTH;
                (*d).budget = budget;
                (*d).heading = horizontal / speed;
            }
        }
    }
    (*d).pending = s.pending_wake;
    store_body_state(state_base(b), s);
    bodies[work_word(b, WORK_DISPLACE)] = (*d).decision;
    bodies[work_word(b, WORK_FLUID)] = (*d).fluid;
    bodies[work_word(b, WORK_BUDGET)] = (*d).budget;
    set_work_f32(b, WORK_INNER, (*d).inner);
    set_work_f32(b, WORK_OUTER, (*d).outer);
    set_work_f32(b, WORK_ENTRY_ENERGY, (*d).entry_energy);
    set_work_f32(b, WORK_WAKE_HEADING, (*d).heading.x);
    set_work_f32(b, WORK_WAKE_HEADING + 1u, (*d).heading.y);
}

@compute @workgroup_size(64, 1, 1)
fn displace_apply(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let b = wg.x;
    if (li == 0u) {
        displace_frame = load_displace_frame(b);
    }
    let d = workgroupUniformLoad(&displace_frame);
    if (d.enabled == 0u || d.decision == DISPLACE_NONE || b >= body_params.body_count) {
        return;
    }
    let bx = cell_box(d.center, d.outer);
    let count = u32(bx.size.x * bx.size.y);
    let run = lane_run(count, li);
    var sources = 0u;
    var sinks = 0u;
    for (var i = run.x; i < run.y; i++) {
        let cell = box_cell(bx, i);
        let col = column(cell);
        if (col.present && owns(cell, displace_key(d, b, cell, col.value))) {
            sources += displace_units(d, cell, col.value, false);
            sinks += displace_units(d, cell, col.value, true);
        }
    }
    lane_sources[li] = sources;
    lane_sinks[li] = sinks;
    let prefix = lane_prefix(li);
    let totals = workgroupUniformLoad(&round_sums);
    let moved = min(min(totals.x, totals.y), d.budget);
    if (moved > 0u) {
        var before_sources = prefix.x;
        var before_sinks = prefix.y;
        var taken_before = mul_div_floor(moved, before_sources, totals.x);
        var given_before = mul_div_floor(moved, before_sinks, totals.y);
        for (var i = run.x; i < run.y; i++) {
            let cell = box_cell(bx, i);
            let col = column(cell);
            if (!col.present || !owns(cell, displace_key(d, b, cell, col.value))) {
                continue;
            }
            let source = displace_units(d, cell, col.value, false);
            let sink = displace_units(d, cell, col.value, true);
            var c = col.value;
            if (source > 0u) {
                before_sources += source;
                let through = mul_div_floor(moved, before_sources, totals.x);
                c = drain_fluid(c, through - taken_before).cell;
                taken_before = through;
            } else if (sink > 0u) {
                before_sinks += sink;
                let through = mul_div_floor(moved, before_sinks, totals.y);
                c = receive_fluid(c, d.fluid, through - given_before).cell;
                given_before = through;
            }
            if (any(c != col.value)) {
                commit_cell(cell, col.value, c);
            }
        }
    }
    if (li == 0u) {
        var s = load_body_state(state_base(b));
        if (d.decision == DISPLACE_SPLASH) {
            let spent = min(f32(moved) * SPLASH_ENERGY_PER_UNIT, d.entry_energy);
            var velocity = d.velocity;
            velocity.y *= sqrt(max((d.entry_energy - spent) / d.entry_energy, 0.0));
            set_work_vec3(b, WORK_VEL_A, velocity);
            s.splashes += 1u;
            s.absorbed_energy += u32(min(spent * 16.0, 4294967040.0));
            s.tick_events |= TICK_SPLASH;
            s.tick_impact = max(s.tick_impact, -d.velocity.y);
            count_body(BDIAG_SPLASHES, 1u);
        } else {
            s.pending_wake = min(d.pending - f32(moved), WAKE_MAX_PENDING);
            count_body(BDIAG_WAKES, 1u);
        }
        store_body_state(state_base(b), s);
    }
}
