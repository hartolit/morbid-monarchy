// The column rules of world_model::cell::WorldCell, as pure functions of a packed cell. Each
// returns the new cell and the units it moved, so a caller can account for them. The
// `cell-rules-match` case of the `cells` suite runs these against world-model on boundary values.

struct Changed {
    cell: vec2<u32>,
    units: u32,
}

fn liquid_head(c: vec2<u32>) -> u32 {
    return cell_elevation(c) + cell_granular_volume(c) + cell_fluid_volume(c);
}

// Bedrock plus loose material: the surface loose material slides over.
fn floor_height(c: vec2<u32>) -> u32 {
    return cell_elevation(c) + cell_granular_volume(c);
}

// Room left before the column's bedrock and loose layer reach their ceilings.
fn structural_void(c: vec2<u32>) -> u32 {
    return (CELL_MAX_ELEVATION - cell_elevation(c)) + (CELL_MAX_GRANULAR_VOLUME - cell_granular_volume(c));
}

// WorldCell::drain_fluid: removes up to `amount`; a column that runs dry loses its liquid state.
fn drain_fluid(c: vec2<u32>, amount: u32) -> Changed {
    let volume = cell_fluid_volume(c);
    let removed = min(amount, volume);
    var out = cell_with_fluid_volume(c, volume - removed);
    if (volume == removed) {
        out = cell_with_fluid_material(out, FLUID_EMPTY);
        out = cell_with_compass(out, 0u);
    }
    return Changed(out, removed);
}

// WorldCell::raise_bedrock: as many units as fit under the ceiling; a raise of nothing changes
// nothing.
fn raise_bedrock(c: vec2<u32>, material: u32, units: u32) -> Changed {
    let raised = min(units, CELL_MAX_ELEVATION - cell_elevation(c));
    var out = c;
    if (raised > 0u) {
        out = cell_with_elevation(out, cell_elevation(c) + raised);
        out = cell_with_terrain_material(out, material);
    }
    return Changed(out, raised);
}

// WorldCell::quench: resident magma becomes stone bedrock as far as the ceiling allows; the rest
// stays liquid magma.
fn quench(c: vec2<u32>) -> Changed {
    if (cell_fluid_material(c) != FLUID_MAGMA) {
        return Changed(c, 0u);
    }
    let raised = raise_bedrock(c, TERRAIN_STONE, cell_fluid_volume(c));
    return Changed(drain_fluid(raised.cell, raised.units).cell, raised.units);
}

// WorldCell::receive_fluid: the units accepted of `amount` units of `material`.
fn receive_fluid(c: vec2<u32>, material: u32, amount: u32) -> Changed {
    let resident = cell_fluid_material(c);
    if (amount == 0u || material == FLUID_EMPTY || fluid_is_solid(resident)) {
        return Changed(c, 0u);
    }
    if (material == FLUID_MAGMA && fluid_is_coolant(resident)) {
        return raise_bedrock(c, TERRAIN_STONE, amount);
    }
    var out = c;
    if (resident == FLUID_MAGMA && fluid_is_coolant(material)) {
        out = quench(out).cell;
        if (cell_fluid_material(out) == FLUID_MAGMA) {
            return Changed(out, 0u);
        }
    }
    let now = cell_fluid_material(out);
    let volume = cell_fluid_volume(out);
    let accepted = min(amount, CELL_MAX_FLUID_VOLUME - volume);
    if (now == material || now == FLUID_EMPTY || accepted > volume) {
        out = cell_with_fluid_material(out, material);
    }
    return Changed(cell_with_fluid_volume(out, volume + accepted), accepted);
}

// WorldCell::freeze and WorldCell::thaw: phase swaps that keep the volume.
fn freeze(c: vec2<u32>) -> vec2<u32> {
    let solid = fluid_frozen(cell_fluid_material(c));
    if (solid == MATERIAL_NONE) {
        return c;
    }
    return cell_with_compass(cell_with_fluid_material(c, solid), 0u);
}

fn thaw(c: vec2<u32>) -> vec2<u32> {
    let liquid = fluid_melted(cell_fluid_material(c));
    if (liquid == MATERIAL_NONE) {
        return c;
    }
    return cell_with_fluid_material(c, liquid);
}

// WorldCell::set_granular_vol: loose volume beyond the field's capacity compacts into bedrock of
// the matching solid, up to the elevation ceiling.
fn with_granular_volume(c: vec2<u32>, volume: u32) -> vec2<u32> {
    if (volume <= CELL_MAX_GRANULAR_VOLUME) {
        return cell_with_granular_volume(c, volume);
    }
    let excess = volume - CELL_MAX_GRANULAR_VOLUME;
    var out = cell_with_terrain_material(c, granular_to_terrain(cell_granular_material(c)));
    out = cell_with_elevation(out, min(cell_elevation(c) + excess, CELL_MAX_ELEVATION));
    return cell_with_granular_volume(out, CELL_MAX_GRANULAR_VOLUME);
}
