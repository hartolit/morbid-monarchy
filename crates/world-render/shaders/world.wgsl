// The resident world as the renderer reads it: window-local cells through the window map, as
// the GPU owner addresses them (gpu-world's shared/window.wgsl), against this frame's window.
// A chunk that is not resident has no cells; it is never the contents of a recycled slot.
//
// Sampling a neighbour that is not resident (beyond the window, or a missing chunk) reads the
// nearest column of the chunk being drawn instead, so layer edges close against the gap the
// way the baseline clamped them at its grid edge.

const LAYER_TERRAIN: u32 = 0u;
const LAYER_GRANULAR: u32 = 1u;
const LAYER_FLUID: u32 = 2u;
const LAYER_SURFACE: u32 = 3u;

// The chunk the current vertex belongs to, set once at the top of a terrain vertex.
var<private> home_chunk: vec2<i32>;
var<private> home_slot: u32;

fn window_extent_cells() -> vec2<i32> {
    return frame.window.zw * i32(CHUNK_SIZE);
}

// The slot holding window-local cell `local`, or SLOT_NONE.
fn slot_of(local: vec2<i32>) -> u32 {
    let extent = window_extent_cells();
    if (any(local < vec2<i32>(0)) || any(local >= extent)) {
        return SLOT_NONE;
    }
    let chunk = vec2<u32>(local) / CHUNK_SIZE;
    return window_slots[chunk.y * u32(frame.window.z) + chunk.x];
}

fn cell_in(slot: u32, local: vec2<i32>) -> vec2<u32> {
    let within = vec2<u32>(local) % CHUNK_SIZE;
    return cells[slot * CHUNK_CELLS + within.y * CHUNK_SIZE + within.x];
}

// The column at `local`, or the nearest column of the home chunk when `local` is not resident.
fn column_near(local: vec2<i32>) -> vec2<u32> {
    let slot = slot_of(local);
    if (slot != SLOT_NONE) {
        return cell_in(slot, local);
    }
    let low = home_chunk * i32(CHUNK_SIZE);
    let clamped = clamp(local, low, low + vec2<i32>(i32(CHUNK_SIZE) - 1));
    return cell_in(home_slot, clamped);
}

struct CellData {
    terrain_mat: u32,
    granular_mat: u32,
    fluid_mat: u32,
    surface_mat: u32,
    variants: u32,
    surface_state: f32,
    terrain_h: f32,
    granular_h: f32,
    fluid_h: f32,
    surface_h: f32,
}

fn decode(c: vec2<u32>) -> CellData {
    let scale = frame.window_cells.z;
    var data: CellData;
    data.terrain_mat = cell_terrain_material(c);
    data.surface_mat = cell_surface_material(c);
    data.granular_mat = cell_granular_material(c);
    data.fluid_mat = cell_fluid_material(c);
    data.variants = cell_variants(c);
    data.surface_state = f32(cell_surface_state(c));
    data.terrain_h = f32(cell_elevation(c)) * scale;
    data.granular_h = data.terrain_h + f32(cell_granular_volume(c)) * scale;
    data.fluid_h = data.granular_h + f32(cell_fluid_volume(c)) * scale;
    data.surface_h = data.granular_h + max(1.0, data.surface_state) * scale;
    return data;
}

fn cell_data(local: vec2<i32>) -> CellData {
    return decode(column_near(local));
}

// Height of the solid column (bedrock, loose material and ice): what casts heightfield shadows
// and occlusion. `WorldCell::solid_top`, as the body kernels' `solid_top` reads it.
fn solid_height_of(c: vec2<u32>) -> f32 {
    var frozen = 0u;
    if (cell_fluid_material(c) == FLUID_ICE) {
        frozen = cell_fluid_volume(c);
    }
    return f32(cell_elevation(c) + cell_granular_volume(c) + frozen) * frame.window_cells.z;
}

fn solid_height(local: vec2<i32>) -> f32 {
    return solid_height_of(column_near(local));
}

// The activity state bits (`TILE_AWAKE`, `TILE_ACTIVE`) of the tile holding window-local cell
// `local`, as the last tick's scheduling left them; 0 outside the window.
fn tile_state(local: vec2<i32>) -> u32 {
    let extent = window_extent_cells();
    if (any(local < vec2<i32>(0)) || any(local >= extent)) {
        return 0u;
    }
    let tile = vec2<u32>(local) / TILE_EDGE;
    let chunk = tile / TILES_PER_CHUNK_EDGE;
    let within = tile % TILES_PER_CHUNK_EDGE;
    let id = (chunk.y * u32(frame.window.z) + chunk.x) * TILES_PER_CHUNK + within.y * TILES_PER_CHUNK_EDGE + within.x;
    return activity[frame.offsets.y + id * TILE_RECORD_WORDS + TILE_STATE];
}
