// The bindings of the body substeps (one bind group, one pipeline layout).
//
// `bodies` is one flat word buffer with the regions of `plan::BodyLayout` (state, configuration,
// per-substep work, samples, the observe list); generated `load_*`/`store_*` functions read and
// write its records. `bins` holds the broadphase (counts, starts, fill cursors, entries and the
// live bounds). `claims` holds one owner word per cell, indexed like `cells`, for the
// deformation and displacement rounds. `pass_params` names the substep and solver iteration of
// the dispatch (a dynamic offset into a table written once).

struct EventRing {
    header: EventHeader,
    records: array<EventRecord>,
}

@group(0) @binding(0) var<uniform> tick: TickParams;
@group(0) @binding(1) var<uniform> body_params: BodyParams;
@group(0) @binding(2) var<uniform> pass_params: BodyDispatch;
@group(0) @binding(3) var<storage, read_write> cells: array<vec2<u32>>;
@group(0) @binding(4) var<storage, read> window_slots: array<u32>;
@group(0) @binding(5) var<storage, read_write> bodies: array<u32>;
@group(0) @binding(6) var<storage, read_write> bins: array<atomic<u32>>;
@group(0) @binding(7) var<storage, read_write> claims: array<atomic<u32>>;
@group(0) @binding(8) var<storage, read_write> activity: array<atomic<u32>>;
@group(0) @binding(9) var<storage, read_write> diagnostics: array<atomic<u32>>;
@group(0) @binding(10) var<storage, read_write> events: EventRing;

// The work region's word offsets, flags and decision codes (`WORK_*`, `DECIDE_*`,
// `DISPLACE_*`) are generated from `gpu_world::layout::body_work`.

fn state_base(b: u32) -> u32 {
    return body_params.state_base + b * BODY_STATE_WORDS;
}

fn config_base(b: u32) -> u32 {
    return body_params.config_base + b * BODY_CONFIG_WORDS;
}

fn work_word(b: u32, field: u32) -> u32 {
    return body_params.work_base + b * BODY_WORK_WORDS + field;
}

fn work_f32(b: u32, field: u32) -> f32 {
    return bitcast<f32>(bodies[work_word(b, field)]);
}

fn set_work_f32(b: u32, field: u32, value: f32) {
    bodies[work_word(b, field)] = bitcast<u32>(value);
}

fn work_vec3(b: u32, field: u32) -> vec3<f32> {
    return vec3<f32>(work_f32(b, field), work_f32(b, field + 1u), work_f32(b, field + 2u));
}

fn set_work_vec3(b: u32, field: u32, value: vec3<f32>) {
    set_work_f32(b, field, value.x);
    set_work_f32(b, field + 1u, value.y);
    set_work_f32(b, field + 2u, value.z);
}

fn last_substep() -> bool {
    return pass_params.substep + 1u >= body_params.substeps;
}

// Adds `value` to the 64-bit body counter at diagnostics words `word` and `word + 1`.
fn add64_body(field: u32, value: u32) {
    add64(body_params.diagnostics_base + field, value);
}

fn count_body(field: u32, value: u32) {
    if (value > 0u) {
        atomicAdd(&diagnostics[body_params.diagnostics_base + field], value);
    }
}

// Raises a float-bits maximum (non-negative floats order like their bits).
fn max_body(field: u32, value: f32) {
    if (value > 0.0) {
        atomicMax(&diagnostics[body_params.diagnostics_base + field], bitcast<u32>(value));
    }
}

fn push_event(kind: u32, a: u32, b: u32, c: u32) {
    let index = atomicAdd(&events.header.count, 1u);
    if (index < events.header.capacity) {
        events.records[index] = EventRecord(kind, a, b, c);
    } else {
        atomicAdd(&events.header.dropped, 1u);
    }
}
