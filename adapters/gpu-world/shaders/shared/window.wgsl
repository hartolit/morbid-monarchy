// Addressing of resident cells and activity tiles, shared by the command and material kernels.
// Needs the globals `tick` (TickParams), `window_slots`, `activity` and `diagnostics`, which each
// assembly's bindings declare under these names.
//
// Coordinates are window-local cells. The window map holds one slot index per window chunk,
// row-major, or SLOT_NONE for a chunk that is not resident; such a chunk is a closed boundary,
// never the contents of a recycled slot. Tiles are 8 x 8 cells, 64 per chunk; a tile's identity
// is its window chunk index times 64 plus its row-major position in the chunk, so tile records
// follow the window, not the slot.

// The slot holding window-local cell `local`, or SLOT_NONE.
fn resident_slot(local: vec2<i32>) -> u32 {
    let extent = vec2<i32>(vec2<u32>(tick.window_width, tick.window_height) * CHUNK_SIZE);
    if (any(local < vec2<i32>(0)) || any(local >= extent)) {
        return SLOT_NONE;
    }
    let chunk = vec2<u32>(local) / CHUNK_SIZE;
    return window_slots[chunk.y * tick.window_width + chunk.x];
}

// Index of window-local cell `local` inside `slot`.
fn cell_index(slot: u32, local: vec2<i32>) -> u32 {
    let within = vec2<u32>(local) % CHUNK_SIZE;
    return slot * CHUNK_CELLS + within.y * CHUNK_SIZE + within.x;
}

// The absolute position of a window-local cell, as the rules' hashes see it (wrapping).
fn absolute(local: vec2<i32>) -> vec2<i32> {
    let origin = vec2<i32>(tick.origin_key_x, tick.origin_key_y) * i32(CHUNK_SIZE);
    return origin + local;
}

// Window tile coordinates of the tile containing window-local cell `local` (inside the window).
fn tile_of_cell(local: vec2<i32>) -> vec2<i32> {
    return local / i32(TILE_EDGE);
}

// The identity of the tile at window tile coordinates `t`, or TILE_NONE outside the window.
fn tile_id(t: vec2<i32>) -> u32 {
    let extent = vec2<i32>(vec2<u32>(tick.window_width, tick.window_height) * TILES_PER_CHUNK_EDGE);
    if (any(t < vec2<i32>(0)) || any(t >= extent)) {
        return TILE_NONE;
    }
    let chunk = vec2<u32>(t) / TILES_PER_CHUNK_EDGE;
    let within = vec2<u32>(t) % TILES_PER_CHUNK_EDGE;
    return (chunk.y * tick.window_width + chunk.x) * TILES_PER_CHUNK + within.y * TILES_PER_CHUNK_EDGE + within.x;
}

fn tile_chunk(tile: u32) -> u32 {
    return tile / TILES_PER_CHUNK;
}

// Window tile coordinates of a tile identity.
fn tile_coords(tile: u32) -> vec2<i32> {
    let chunk = tile / TILES_PER_CHUNK;
    let within = tile % TILES_PER_CHUNK;
    let chunk_xy = vec2<u32>(chunk % tick.window_width, chunk / tick.window_width);
    return vec2<i32>(chunk_xy * TILES_PER_CHUNK_EDGE + vec2<u32>(within % TILES_PER_CHUNK_EDGE, within / TILES_PER_CHUNK_EDGE));
}

fn tile_resident(tile: u32) -> bool {
    return tile != TILE_NONE && window_slots[tile_chunk(tile)] != SLOT_NONE;
}

// Word `field` of a tile's record in the activity buffer.
fn tile_word(tile: u32, field: u32) -> u32 {
    return tick.records_base + tile * TILE_RECORD_WORDS + field;
}

// Bit of in-tile cell `within` (x, y in 0..8) in a two-word tile bitmap: rows of eight bits.
fn bitmap_word(within: vec2<u32>) -> u32 {
    return within.y / 4u;
}

fn bitmap_bit(within: vec2<u32>) -> u32 {
    return 1u << ((within.y % 4u) * 8u + within.x);
}

// Which of the 3 x 3 tiles around a cell's tile lie within `reach` cells of it: bit
// (dy + 1) * 3 + (dx + 1). The cell's own tile (bit 4) is always included.
fn reach_mask(within: vec2<u32>, reach: u32) -> u32 {
    let low = within < vec2<u32>(reach);
    let high = within >= vec2<u32>(TILE_EDGE - reach);
    var mask = 0u;
    for (var dy = 0u; dy < 3u; dy++) {
        for (var dx = 0u; dx < 3u; dx++) {
            let x_ok = dx == 1u || (dx == 0u && low.x) || (dx == 2u && high.x);
            let y_ok = dy == 1u || (dy == 0u && low.y) || (dy == 2u && high.y);
            if (x_ok && y_ok) {
                mask |= 1u << (dy * 3u + dx);
            }
        }
    }
    return mask;
}

// Marks the resident tiles of `mask` around `tile` as candidates for the next scan.
fn mark_candidates(tile: u32, mask: u32) {
    let center = tile_coords(tile);
    for (var bit = 0u; bit < 9u; bit++) {
        if ((mask & (1u << bit)) == 0u) {
            continue;
        }
        let neighbor = tile_id(center + vec2<i32>(i32(bit % 3u) - 1, i32(bit / 3u) - 1));
        if (tile_resident(neighbor)) {
            atomicOr(&activity[tile_word(neighbor, TILE_CANDIDATE)], 1u);
        }
    }
}

// Adds `value` to the 64-bit counter at diagnostics words `word` (low) and `word + 1` (high).
// Each wrap of the low word is counted exactly once, by the add that caused it.
fn add64(word: u32, value: u32) {
    if (value == 0u) {
        return;
    }
    let before = atomicAdd(&diagnostics[word], value);
    if (before + value < before) {
        atomicAdd(&diagnostics[word + 1u], 1u);
    }
}
