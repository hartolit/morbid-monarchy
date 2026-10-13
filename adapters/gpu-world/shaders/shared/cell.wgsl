// Field access over a packed cell: two u32 words, `.x` holding bits 0-31 and `.y` bits 32-63.
// The generated declarations give each field's word, shift and mask (CELL_<FIELD>_WORD,
// _SHIFT, _MASK), derived from world_model's WorldCell::LAYOUT. A setter replaces one field
// and leaves every other bit of both words as it was.
//
// A setter is not a transaction: a kernel that changes several fields owns the whole cell for
// its phase and writes it once.

fn cell_get(cell: vec2<u32>, word: u32, shift: u32, mask: u32) -> u32 {
    return (cell[word] >> shift) & mask;
}

fn cell_set(cell: vec2<u32>, word: u32, shift: u32, mask: u32, value: u32) -> vec2<u32> {
    var out = cell;
    out[word] = (out[word] & ~(mask << shift)) | ((value & mask) << shift);
    return out;
}

fn cell_terrain_material(c: vec2<u32>) -> u32 {
    return cell_get(c, CELL_TERRAIN_MATERIAL_WORD, CELL_TERRAIN_MATERIAL_SHIFT, CELL_TERRAIN_MATERIAL_MASK);
}
fn cell_with_terrain_material(c: vec2<u32>, v: u32) -> vec2<u32> {
    return cell_set(c, CELL_TERRAIN_MATERIAL_WORD, CELL_TERRAIN_MATERIAL_SHIFT, CELL_TERRAIN_MATERIAL_MASK, v);
}

fn cell_surface_material(c: vec2<u32>) -> u32 {
    return cell_get(c, CELL_SURFACE_MATERIAL_WORD, CELL_SURFACE_MATERIAL_SHIFT, CELL_SURFACE_MATERIAL_MASK);
}
fn cell_with_surface_material(c: vec2<u32>, v: u32) -> vec2<u32> {
    return cell_set(c, CELL_SURFACE_MATERIAL_WORD, CELL_SURFACE_MATERIAL_SHIFT, CELL_SURFACE_MATERIAL_MASK, v);
}

fn cell_granular_material(c: vec2<u32>) -> u32 {
    return cell_get(c, CELL_GRANULAR_MATERIAL_WORD, CELL_GRANULAR_MATERIAL_SHIFT, CELL_GRANULAR_MATERIAL_MASK);
}
fn cell_with_granular_material(c: vec2<u32>, v: u32) -> vec2<u32> {
    return cell_set(c, CELL_GRANULAR_MATERIAL_WORD, CELL_GRANULAR_MATERIAL_SHIFT, CELL_GRANULAR_MATERIAL_MASK, v);
}

fn cell_fluid_material(c: vec2<u32>) -> u32 {
    return cell_get(c, CELL_FLUID_MATERIAL_WORD, CELL_FLUID_MATERIAL_SHIFT, CELL_FLUID_MATERIAL_MASK);
}
fn cell_with_fluid_material(c: vec2<u32>, v: u32) -> vec2<u32> {
    return cell_set(c, CELL_FLUID_MATERIAL_WORD, CELL_FLUID_MATERIAL_SHIFT, CELL_FLUID_MATERIAL_MASK, v);
}

fn cell_variants(c: vec2<u32>) -> u32 {
    return cell_get(c, CELL_VARIANTS_WORD, CELL_VARIANTS_SHIFT, CELL_VARIANTS_MASK);
}

fn cell_elevation(c: vec2<u32>) -> u32 {
    return cell_get(c, CELL_ELEVATION_WORD, CELL_ELEVATION_SHIFT, CELL_ELEVATION_MASK);
}
fn cell_with_elevation(c: vec2<u32>, v: u32) -> vec2<u32> {
    return cell_set(c, CELL_ELEVATION_WORD, CELL_ELEVATION_SHIFT, CELL_ELEVATION_MASK, v);
}

fn cell_fluid_volume(c: vec2<u32>) -> u32 {
    return cell_get(c, CELL_FLUID_VOLUME_WORD, CELL_FLUID_VOLUME_SHIFT, CELL_FLUID_VOLUME_MASK);
}
fn cell_with_fluid_volume(c: vec2<u32>, v: u32) -> vec2<u32> {
    return cell_set(c, CELL_FLUID_VOLUME_WORD, CELL_FLUID_VOLUME_SHIFT, CELL_FLUID_VOLUME_MASK, v);
}

fn cell_granular_volume(c: vec2<u32>) -> u32 {
    return cell_get(c, CELL_GRANULAR_VOLUME_WORD, CELL_GRANULAR_VOLUME_SHIFT, CELL_GRANULAR_VOLUME_MASK);
}
fn cell_with_granular_volume(c: vec2<u32>, v: u32) -> vec2<u32> {
    return cell_set(c, CELL_GRANULAR_VOLUME_WORD, CELL_GRANULAR_VOLUME_SHIFT, CELL_GRANULAR_VOLUME_MASK, v);
}

fn cell_surface_state(c: vec2<u32>) -> u32 {
    return cell_get(c, CELL_SURFACE_STATE_WORD, CELL_SURFACE_STATE_SHIFT, CELL_SURFACE_STATE_MASK);
}
fn cell_with_surface_state(c: vec2<u32>, v: u32) -> vec2<u32> {
    return cell_set(c, CELL_SURFACE_STATE_WORD, CELL_SURFACE_STATE_SHIFT, CELL_SURFACE_STATE_MASK, v);
}

fn cell_terrain_state(c: vec2<u32>) -> u32 {
    return cell_get(c, CELL_TERRAIN_STATE_WORD, CELL_TERRAIN_STATE_SHIFT, CELL_TERRAIN_STATE_MASK);
}

fn cell_compass(c: vec2<u32>) -> u32 {
    return cell_get(c, CELL_COMPASS_WORD, CELL_COMPASS_SHIFT, CELL_COMPASS_MASK);
}
fn cell_with_compass(c: vec2<u32>, v: u32) -> vec2<u32> {
    return cell_set(c, CELL_COMPASS_WORD, CELL_COMPASS_SHIFT, CELL_COMPASS_MASK, v);
}
