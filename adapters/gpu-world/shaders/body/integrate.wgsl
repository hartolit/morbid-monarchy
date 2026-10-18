// The start and end of a body substep, and the tick's samples.
//
// body_begin   one invocation per body slot: the residency-edge check, then the baseline's
//              semi-implicit Euler step for gravity, buoyancy and drag (`integrate_body_forces`),
//              the speed cap, and the substep's starting point; reads the cells as the previous
//              substep (or the material phases) left them.
// body_end     one invocation per body slot: re-anchors the position, records the substep's body
//              contacts and, at the tick's last substep, the coalesced cosmetic event.
// body_observe one invocation per selected handle: copies its state into the tick's samples.
//
// A suspended body is frozen with its state: it neither moves nor reacts, and active bodies
// meet it as an immovable obstacle. It resumes, with its velocity, once every column it can
// reach in a substep is resident again.

fn load_frame_state(b: u32) -> BodyState {
    return load_body_state(state_base(b));
}

// Whether every chunk the box of `reach` around `p` touches is resident.
fn region_resident(p: vec3<f32>, reach: f32) -> bool {
    let low = vec2<i32>(i32(floor(p.x - reach)), i32(floor(-p.z - reach)));
    let high = vec2<i32>(i32(floor(p.x + reach)), i32(floor(-p.z + reach)));
    let extent = vec2<i32>(vec2<u32>(tick.window_width, tick.window_height));
    if (any(low < vec2<i32>(0)) || any(high >= extent * i32(CHUNK_SIZE))) {
        return false;
    }
    let first = low >> vec2<u32>(6u);
    let last = high >> vec2<u32>(6u);
    for (var cy = first.y; cy <= last.y; cy++) {
        for (var cx = first.x; cx <= last.x; cx++) {
            if (window_slots[u32(cy) * tick.window_width + u32(cx)] == SLOT_NONE) {
                return false;
            }
        }
    }
    return true;
}

fn set_bounds(radius: f32, travel: f32) {
    atomicMax(&bins[body_params.bounds_base], bitcast<u32>(radius));
    if (travel > 0.0) {
        atomicMax(&bins[body_params.bounds_base + 1u], bitcast<u32>(travel));
    }
}

@compute @workgroup_size(64, 1, 1)
fn bins_clear(@builtin(global_invocation_id) id: vec3<u32>) {
    let slot = id.x;
    if (slot < body_params.hash_mask + 1u) {
        atomicStore(&bins[body_params.counts_base + slot], 0u);
        atomicStore(&bins[body_params.fill_base + slot], 0u);
    }
    if (slot < 4u) {
        atomicStore(&bins[body_params.bounds_base + slot], 0u);
    }
}

@compute @workgroup_size(64, 1, 1)
fn body_begin(@builtin(global_invocation_id) id: vec3<u32>) {
    let b = id.x;
    if (b >= body_params.body_count) {
        return;
    }
    var s = load_frame_state(b);
    if ((s.flags & BODY_ALIVE) == 0u) {
        bodies[work_word(b, WORK_FLAGS)] = 0u;
        return;
    }
    let config = load_body_config(config_base(b));
    let placed = place(s);
    var velocity = vec3<f32>(s.velocity_x, s.velocity_y, s.velocity_z);
    let dt = body_params.substep_seconds;
    let reach = config.radius * SPLASH_RING_OUTER_RATIO + 2.0 + length(velocity) * dt;
    if (placed.far || !region_resident(placed.position, reach)) {
        if ((s.flags & BODY_SUSPENDED) == 0u) {
            s.flags |= BODY_SUSPENDED;
            store_body_state(state_base(b), s);
        }
        bodies[work_word(b, WORK_FLAGS)] = 0u;
        bodies[work_word(b, WORK_PAIRS)] = 0u;
        if (!placed.far) {
            // An obstacle that does not move: both halves of the solver's buffers hold it.
            for (var half = 0u; half < 2u; half++) {
                set_work_vec3(b, WORK_POS_A + half * 4u, placed.position);
                set_work_vec3(b, WORK_VEL_A + half * 4u, vec3<f32>(0.0));
            }
            set_work_vec3(b, WORK_PREV, placed.position);
            set_work_vec3(b, WORK_BINNED, placed.position);
            set_work_vec3(b, WORK_SOLVE_VELOCITY, vec3<f32>(0.0));
            set_bounds(config.radius, 0.0);
        }
        return;
    }
    s.flags &= ~BODY_SUSPENDED;

    // integrate_body_forces: gravity, Archimedes, and drag blended by the submerged share.
    let position = placed.position;
    let gravity = vec3<f32>(0.0, GRAVITY_Y, 0.0);
    var acceleration = gravity;
    var drag_rate = AIR_DRAG_RATE;
    var submerged = 0.0;
    let liquid = liquid_at(nearest_cell(position));
    if (liquid.present) {
        let depth = liquid.surface - (position.y - config.radius);
        submerged = submerged_volume_fraction(depth, config.radius);
        if (submerged > 0.0) {
            let ratio = fluid_relative_density(liquid.material)
                / max(config.relative_density, MIN_BODY_RELATIVE_DENSITY);
            acceleration -= gravity * (submerged * ratio);
            drag_rate = drag_rate * (1.0 - submerged) + fluid_drag_rate(liquid.material) * submerged;
        }
    }
    velocity += acceleration * dt;
    velocity *= exp(-drag_rate * dt);
    let speed = length(velocity);
    if (speed > MAX_BODY_SPEED) {
        velocity *= MAX_BODY_SPEED / speed;
        count_body(BDIAG_SPEED_LIMITED, 1u);
    }
    s.previous_submerged = s.submerged;
    s.submerged = submerged;
    s.liquid = select(FLUID_EMPTY, liquid.material, liquid.present);
    if (s.previous_submerged <= 0.0 && submerged > 0.0) {
        s.liquid_entries += 1u;
    }
    store_body_state(state_base(b), s);

    let next = position + velocity * dt;
    set_work_vec3(b, WORK_POS_A, next);
    set_work_vec3(b, WORK_VEL_A, velocity);
    set_work_vec3(b, WORK_PREV, position);
    set_work_vec3(b, WORK_BINNED, next);
    set_work_vec3(b, WORK_SOLVE_VELOCITY, velocity);
    var flags = WORK_ACTIVE;
    if ((s.flags & BODY_GROUNDED) != 0u) {
        flags |= WORK_WAS_GROUNDED;
    }
    bodies[work_word(b, WORK_FLAGS)] = flags;
    bodies[work_word(b, WORK_TOUCHING)] = 0u;
    bodies[work_word(b, WORK_PAIRS)] = 0u;
    set_bounds(config.radius, distance(next, position));
}

@compute @workgroup_size(64, 1, 1)
fn body_end(@builtin(global_invocation_id) id: vec3<u32>) {
    let b = id.x;
    if (b >= body_params.body_count) {
        return;
    }
    var s = load_frame_state(b);
    if ((s.flags & BODY_ALIVE) == 0u) {
        return;
    }
    let moving = (bodies[work_word(b, WORK_FLAGS)] & WORK_ACTIVE) != 0u;
    if (moving) {
        let shift = anchor_position(&s, work_vec3(b, WORK_POS_A));
        s.plane_origin_x -= shift.x;
        s.plane_origin_y -= shift.y;
        let velocity = work_vec3(b, WORK_VEL_A);
        s.velocity_x = velocity.x;
        s.velocity_y = velocity.y;
        s.velocity_z = velocity.z;
        let touching = bodies[work_word(b, WORK_TOUCHING)];
        if (touching > 0u && s.touching == 0u) {
            s.body_contacts += 1u;
            s.tick_events |= TICK_CONTACT;
        }
        s.touching = touching;
    }
    if (last_substep()) {
        count_body(BDIAG_LIVE, 1u);
        if ((s.flags & BODY_SUSPENDED) != 0u) {
            count_body(BDIAG_SUSPENDED, 1u);
        }
        if (s.tick_events != 0u) {
            let impact = u32(clamp(round(s.tick_impact), 0.0, 16777215.0));
            push_event(EVENT_BODY, b, s.generation, (s.tick_events & 0xFFu) | (impact << 8u));
            s.tick_events = 0u;
            s.tick_impact = 0.0;
        }
    }
    store_body_state(state_base(b), s);
}

@compute @workgroup_size(64, 1, 1)
fn body_observe(@builtin(global_invocation_id) id: vec3<u32>) {
    let entry = id.x;
    if (entry >= body_params.observe_count) {
        return;
    }
    let index = bodies[body_params.observe_base + entry * 2u];
    let generation = bodies[body_params.observe_base + entry * 2u + 1u];
    var out: BodySampleRecord;
    out.status = SAMPLE_GONE;
    out.index = index;
    out.generation = generation;
    if (index < body_params.capacity) {
        let s = load_frame_state(index);
        if ((s.flags & BODY_ALIVE) != 0u && s.generation == generation) {
            out.status = SAMPLE_OK;
            out.flags = s.flags;
            out.anchor_x = s.anchor_x;
            out.anchor_y = s.anchor_y;
            out.local_x = s.local_x;
            out.local_y = s.local_y;
            out.height = s.height;
            out.velocity_x = s.velocity_x;
            out.velocity_y = s.velocity_y;
            out.velocity_z = s.velocity_z;
            out.normal_x = s.normal_x;
            out.normal_y = s.normal_y;
            out.normal_z = s.normal_z;
            out.submerged = s.submerged;
            out.liquid = s.liquid;
            out.touching = s.touching;
            out.landings = s.landings;
            out.liquid_entries = s.liquid_entries;
            out.body_contacts = s.body_contacts;
            out.craters = s.craters;
            out.splashes = s.splashes;
            out.absorbed_energy = s.absorbed_energy;
        }
    }
    store_body_sample_record(body_params.samples_base + entry * BODY_SAMPLE_WORDS, out);
}
