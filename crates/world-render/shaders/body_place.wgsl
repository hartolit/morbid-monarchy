// Where a body slot is, read from the GPU owner's body state against this frame's window
// (gpu-world's body/geometry.wgsl `place`): window-local render space, the anchor chunk's
// offset from the window's minimum chunk plus the position inside it. Needs `frame` and a
// `bodies` binding.

struct PlacedBody {
    alive: bool,
    position: vec3<f32>,
    radius: f32,
    state: BodyState,
    config: BodyConfig,
}

fn placed_body(slot: u32) -> PlacedBody {
    var placed: PlacedBody;
    placed.alive = false;
    if (slot >= frame.follow.z) {
        return placed;
    }
    let state = load_body_state(slot * BODY_STATE_WORDS);
    if ((state.flags & BODY_ALIVE) == 0u) {
        return placed;
    }
    let config = load_body_config(frame.offsets.x + slot * BODY_CONFIG_WORDS);
    let size = f32(CHUNK_SIZE);
    let cell_x = f32(state.anchor_x - frame.window.x) * size + state.local_x;
    let cell_y = f32(state.anchor_y - frame.window.y) * size + state.local_y;
    placed.alive = true;
    placed.position = vec3<f32>(cell_x, state.height, -cell_y);
    placed.radius = config.radius;
    placed.state = state;
    placed.config = config;
    return placed;
}
