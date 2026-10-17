// Activity scheduling: which tiles the material phases visit this tick, and each cell's wake level.
//
// scan_tiles  every window tile, one invocation each (workgroup = window chunk): reads and clears
//             the tile's candidate flag, resets its state, and appends it to the candidate list
//             when a flag, a chunk wake request, a whole-world wake or dense execution says so.
// *_args      one invocation: turns a list count into indirect dispatch arguments.
// wake_tiles  candidate tiles, one workgroup each: wake levels from the 3 x 3 tiles' changed and
//             persist bitmaps of the previous tick and edited bitmaps of this one; appends tiles
//             with an awake cell (every candidate, in dense execution) to the process list.
//
// The candidate flags were set by the previous tick's `react` (for its changed and persisting
// cells) and by this tick's edits; scan_tiles is their only reader and clears each after reading.

@group(1) @binding(0) var<storage, read_write> dispatch_args: array<u32>;

fn dense() -> bool {
    return (tick.activity_flags & ACTIVITY_DENSE) != 0u;
}

fn wake_all() -> bool {
    return (tick.activity_flags & ACTIVITY_WAKE_ALL) != 0u;
}

fn chunk_requested(chunk: vec2<i32>) -> bool {
    if (any(chunk < vec2<i32>(0)) || any(chunk >= vec2<i32>(i32(tick.window_width), i32(tick.window_height)))) {
        return false;
    }
    let index = u32(chunk.y) * tick.window_width + u32(chunk.x);
    return atomicLoad(&activity[tick.requests_base + index]) != 0u;
}

@compute @workgroup_size(64, 1, 1)
fn scan_tiles(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let chunk = wg.x;
    let tile = chunk * TILES_PER_CHUNK + li;
    let flagged = atomicExchange(&activity[tile_word(tile, TILE_CANDIDATE)], 0u);
    atomicStore(&activity[tile_word(tile, TILE_STATE)], 0u);
    if (window_slots[chunk] == SLOT_NONE) {
        return;
    }
    var candidate = flagged != 0u || wake_all() || dense();
    // A chunk wake request (an activation or a retirement) reaches its own tiles and the
    // bordering tiles of the adjacent chunks: three cells, less than one tile.
    let chunk_xy = vec2<i32>(i32(chunk % tick.window_width), i32(chunk / tick.window_width));
    let within = vec2<u32>(li % TILES_PER_CHUNK_EDGE, li / TILES_PER_CHUNK_EDGE);
    let low = within == vec2<u32>(0u);
    let high = within == vec2<u32>(TILES_PER_CHUNK_EDGE - 1u);
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let x_ok = dx == 0 || (dx < 0 && low.x) || (dx > 0 && high.x);
            let y_ok = dy == 0 || (dy < 0 && low.y) || (dy > 0 && high.y);
            if (x_ok && y_ok && chunk_requested(chunk_xy + vec2<i32>(dx, dy))) {
                candidate = true;
            }
        }
    }
    if (candidate) {
        let position = atomicAdd(&activity[ACTIVITY_CANDIDATE_COUNT], 1u);
        atomicStore(&activity[tick.candidates_base + position], tile);
    }
}

fn write_args(at: u32, count: u32) {
    dispatch_args[at] = min(count, DISPATCH_SPLIT);
    dispatch_args[at + 1u] = max((count + DISPATCH_SPLIT - 1u) / DISPATCH_SPLIT, 1u);
    dispatch_args[at + 2u] = 1u;
}

@compute @workgroup_size(1, 1, 1)
fn candidate_args() {
    let count = atomicLoad(&activity[ACTIVITY_CANDIDATE_COUNT]);
    write_args(ARGS_CANDIDATES, count);
    atomicStore(&diagnostics[DIAG_CANDIDATE_TILES], count);
}

@compute @workgroup_size(1, 1, 1)
fn process_args() {
    let count = atomicLoad(&activity[ACTIVITY_PROCESS_COUNT]);
    write_args(ARGS_PROCESS, count);
    atomicStore(&diagnostics[DIAG_PROCESS_TILES], count);
}

var<workgroup> wake_tile: u32;
// Per tile of the 3 x 3 neighbourhood (row-major from the lower left): the bits that wake as a
// change (changed last tick, edited or in a requested chunk) and the persisting bits, two words
// each.
var<workgroup> neighborhood: array<vec4<u32>, 9>;
// The neighbourhood as 24 rows of 24 bits, change bits and persist bits.
var<workgroup> change_rows: array<u32, 24>;
var<workgroup> persist_rows: array<u32, 24>;
var<workgroup> wake_words: array<atomic<u32>, 4>;
var<workgroup> wake_flags: atomic<u32>;

fn neighborhood_bits(center: vec2<i32>, offset: vec2<i32>) -> vec4<u32> {
    let tile = tile_id(center + offset);
    if (tile == TILE_NONE) {
        return vec4<u32>(0u);
    }
    var change = vec2<u32>(0u);
    var persist = vec2<u32>(0u);
    if (tile_resident(tile)) {
        let previous = (tick.parity ^ 1u) * 2u;
        change = vec2<u32>(
            atomicLoad(&activity[tile_word(tile, TILE_CHANGED + previous)])
                | atomicLoad(&activity[tile_word(tile, TILE_EDITED)]),
            atomicLoad(&activity[tile_word(tile, TILE_CHANGED + previous + 1u)])
                | atomicLoad(&activity[tile_word(tile, TILE_EDITED + 1u)]),
        );
        persist = vec2<u32>(
            atomicLoad(&activity[tile_word(tile, TILE_PERSIST + previous)]),
            atomicLoad(&activity[tile_word(tile, TILE_PERSIST + previous + 1u)]),
        );
    }
    let chunk = vec2<i32>(tile_coords(tile)) / i32(TILES_PER_CHUNK_EDGE);
    if (chunk_requested(chunk)) {
        change = vec2<u32>(0xFFFFFFFFu);
    }
    return vec4<u32>(change, persist);
}

fn tile_row(bits: vec2<u32>, row: u32) -> u32 {
    let word = select(bits.x, bits.y, row >= 4u);
    return (word >> ((row % 4u) * 8u)) & 0xFFu;
}

fn span(low: u32, high: u32) -> u32 {
    return ((1u << (high - low + 1u)) - 1u) << low;
}

// Whether any change bit of rows `first..=last` lies in columns `low..=high`.
fn any_change(first: u32, last: u32, low: u32, high: u32) -> bool {
    for (var row = first; row <= last; row++) {
        if ((change_rows[row] & span(low, high)) != 0u) {
            return true;
        }
    }
    return false;
}

fn any_persist(first: u32, last: u32, low: u32, high: u32) -> bool {
    for (var row = first; row <= last; row++) {
        if ((persist_rows[row] & span(low, high)) != 0u) {
            return true;
        }
    }
    return false;
}

@compute @workgroup_size(64, 1, 1)
fn wake_tiles(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    if (li == 0u) {
        wake_tile = list_entry(ACTIVITY_CANDIDATE_COUNT, tick.candidates_base, wg);
    }
    let tile = workgroupUniformLoad(&wake_tile);
    if (tile == TILE_NONE) {
        return;
    }
    let center = tile_coords(tile);
    if (li < 9u) {
        neighborhood[li] = neighborhood_bits(center, vec2<i32>(i32(li % 3u) - 1, i32(li / 3u) - 1));
    }
    if (li < 4u) {
        atomicStore(&wake_words[li], 0u);
    }
    if (li == 0u) {
        atomicStore(&wake_flags, 0u);
    }
    workgroupBarrier();
    if (li < 48u) {
        let row = li % 24u;
        let kind = li / 24u;
        var bits = 0u;
        for (var column = 0u; column < 3u; column++) {
            let tile_bits = neighborhood[(row / 8u) * 3u + column];
            let words = select(tile_bits.xy, tile_bits.zw, kind == 1u);
            bits |= tile_row(words, row % 8u) << (column * 8u);
        }
        if (kind == 0u) {
            change_rows[row] = bits;
        } else {
            persist_rows[row] = bits;
        }
    }
    workgroupBarrier();

    // The baseline's schedule_wake as a gather: level 3 within 2 of a change, 2 for a persisting
    // cell, 1 within 3 of a change or 1 of a persisting cell.
    let x = li % TILE_EDGE + 8u;
    let y = li / TILE_EDGE + 8u;
    var level = 0u;
    if (wake_all() || any_change(y - 2u, y + 2u, x - 2u, x + 2u)) {
        level = WAKE_ACTIVE_FRESH;
    } else if ((persist_rows[y] & (1u << x)) != 0u) {
        level = WAKE_ACTIVE_FRESH - 1u;
    } else if (any_change(y - 3u, y + 3u, x - 3u, x + 3u) || any_persist(y - 1u, y + 1u, x - 1u, x + 1u)) {
        level = WAKE_HALO;
    }
    if (level > 0u) {
        atomicOr(&wake_words[li / 16u], level << ((li % 16u) * 2u));
        atomicOr(&wake_flags, select(TILE_AWAKE, TILE_AWAKE | TILE_ACTIVE, level >= WAKE_ACTIVE_MIN));
    }
    workgroupBarrier();
    if (li < 4u) {
        atomicStore(&activity[tile_word(tile, TILE_WAKE + li)], atomicLoad(&wake_words[li]));
    }
    if (li == 0u) {
        let flags = atomicLoad(&wake_flags);
        atomicStore(&activity[tile_word(tile, TILE_STATE)], flags);
        if ((flags & TILE_ACTIVE) != 0u) {
            atomicAdd(&diagnostics[DIAG_ACTIVE_TILES], 1u);
        }
        if (flags != 0u || dense()) {
            append(ACTIVITY_PROCESS_COUNT, tick.process_base, tile);
        }
    }
}
