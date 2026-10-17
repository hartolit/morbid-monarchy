// The bindings of the material phases (one bind group; the argument kernels add group 1).
//
// `flow` holds two words per cell, indexed like `cells`: the cell's proposal (word 2i) and its
// resolution (word 2i + 1), both stamped with the low eight bits of the tick. `scratch` holds the
// new value of every awake cell between `react` and `commit`. Tile records, lists and counters
// live in `activity`; per-tick accounting in `diagnostics`.

@group(0) @binding(0) var<uniform> tick: TickParams;
@group(0) @binding(1) var<storage, read_write> cells: array<vec2<u32>>;
@group(0) @binding(2) var<storage, read> window_slots: array<u32>;
@group(0) @binding(3) var<storage, read_write> flow: array<u32>;
@group(0) @binding(4) var<storage, read_write> scratch: array<vec2<u32>>;
@group(0) @binding(5) var<storage, read_write> activity: array<atomic<u32>>;
@group(0) @binding(6) var<storage, read_write> diagnostics: array<atomic<u32>>;

// The tile a workgroup of a list-driven pass processes: entry `wg.y * DISPATCH_SPLIT + wg.x` of
// the list whose count is at activity word `count_word`, or TILE_NONE past its end.
fn list_entry(count_word: u32, base: u32, wg: vec3<u32>) -> u32 {
    let position = wg.y * DISPATCH_SPLIT + wg.x;
    if (position >= atomicLoad(&activity[count_word])) {
        return TILE_NONE;
    }
    return atomicLoad(&activity[base + position]);
}

// Appends `tile` to the list whose count is at `count_word`. Lists hold every tile at most once
// and have room for every tile of the window, so an append cannot overflow.
fn append(count_word: u32, base: u32, tile: u32) {
    let position = atomicAdd(&activity[count_word], 1u);
    atomicStore(&activity[base + position], tile);
}

// A resident cell as the tick's material phases read it.
struct Read {
    present: bool,
    value: vec2<u32>,
    index: u32,
}

fn read_cell(local: vec2<i32>) -> Read {
    let slot = resident_slot(local);
    if (slot == SLOT_NONE) {
        return Read(false, vec2<u32>(0u), 0u);
    }
    let index = cell_index(slot, local);
    return Read(true, cells[index], index);
}

// The wake level of a window-local cell in this tick: 0 unless its tile was woken this tick.
fn wake_level(local: vec2<i32>) -> u32 {
    if (resident_slot(local) == SLOT_NONE) {
        return 0u;
    }
    let tile = tile_id(tile_of_cell(local));
    if ((atomicLoad(&activity[tile_word(tile, TILE_STATE)]) & TILE_AWAKE) == 0u) {
        return 0u;
    }
    let within = vec2<u32>(local) % TILE_EDGE;
    let position = within.y * TILE_EDGE + within.x;
    let word = atomicLoad(&activity[tile_word(tile, TILE_WAKE + position / 16u)]);
    return (word >> ((position % 16u) * 2u)) & 3u;
}

fn stamp() -> u32 {
    return (tick.tick_lo & 0xFFu) << FLOW_STAMP_SHIFT;
}

fn stamped(word: u32) -> bool {
    return (word & FLOW_STAMP_MASK) == stamp();
}

fn field(word: u32, shift: u32, mask: u32) -> u32 {
    return (word >> shift) & mask;
}

// A proposal or resolution: per family a direction (DIR_NONE when there is none) and an amount.
fn pack_flow(granular_dir: u32, granular_amount: u32, liquid_dir: u32, liquid_amount: u32) -> u32 {
    return stamp()
        | (granular_dir << FLOW_GRANULAR_DIR_SHIFT)
        | (granular_amount << FLOW_GRANULAR_AMOUNT_SHIFT)
        | (liquid_dir << FLOW_LIQUID_DIR_SHIFT)
        | (liquid_amount << FLOW_LIQUID_AMOUNT_SHIFT);
}

fn granular_dir(word: u32) -> u32 {
    return field(word, FLOW_GRANULAR_DIR_SHIFT, FLOW_DIR_MASK);
}

fn granular_amount(word: u32) -> u32 {
    return field(word, FLOW_GRANULAR_AMOUNT_SHIFT, FLOW_GRANULAR_AMOUNT_MASK);
}

fn liquid_dir(word: u32) -> u32 {
    return field(word, FLOW_LIQUID_DIR_SHIFT, FLOW_DIR_MASK);
}

fn liquid_amount(word: u32) -> u32 {
    return field(word, FLOW_LIQUID_AMOUNT_SHIFT, FLOW_LIQUID_AMOUNT_MASK);
}

fn runs(rule: u32) -> bool {
    return (tick.rules & rule) != 0u;
}

// The window-local cell a 64-invocation tile workgroup's invocation owns.
fn tile_cell(tile: u32, within: vec2<u32>) -> vec2<i32> {
    return tile_coords(tile) * i32(TILE_EDGE) + vec2<i32>(within);
}
