// apply_edit: one brush stroke, as the baseline tools apply it (morbid-app's apply_brush with
// world_model's column rules).
//
// Dispatch: one invocation per cell of the stroke's (2r + 1)^2 bounding square, in 8 x 8
// workgroups; invocations past the square (partial edge workgroups) and outside the circle
// return at once, and no barrier follows. Each invocation owns exactly one cell, so a stroke
// has one writer per cell. Strokes of a tick run as separate dispatches in command order;
// wgpu places a barrier between dispatches that use the same read-write storage buffer, so a
// later stroke reads the earlier one's result. Counts go to the command's own result record
// and are read only after the pass ends.

@compute @workgroup_size(8, 8, 1)
fn apply_edit(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = dispatch_params.command_index;
    let command = commands[index];
    if (command.kind != COMMAND_EDIT) {
        return;
    }
    let radius = i32(command.radius);
    let offset = vec2<i32>(id.xy) - vec2<i32>(radius);
    if (offset.x > radius || offset.y > radius) {
        return;
    }
    if (offset.x * offset.x + offset.y * offset.y > radius * radius) {
        return;
    }
    let local = vec2<i32>(command.center_x, command.center_y) + offset;
    let slot = resident_slot(local);
    if (slot == SLOT_NONE) {
        atomicAdd(&results[index].not_resident, 1u);
        return;
    }
    let at = cell_index(slot, local);
    let before = cells[at];
    let after = apply_tool(before, command.tool, command.strength);
    if (any(after != before)) {
        cells[at] = after;
        atomicAdd(&results[index].changed, 1u);
    }
}

fn apply_tool(cell: vec2<u32>, tool: u32, strength: u32) -> vec2<u32> {
    let fluid = tool_fluid(tool);
    if (fluid != FLUID_EMPTY) {
        return inject_fluid(cell, fluid, strength);
    }
    switch tool {
        case TOOL_FIRE: {
            return ignite(cell);
        }
        case TOOL_SAND: {
            return add_sand(cell, strength);
        }
        case TOOL_RAISE: {
            return cell_with_elevation(cell, min(cell_elevation(cell) + strength, CELL_MAX_ELEVATION));
        }
        case TOOL_LOWER: {
            let elevation = cell_elevation(cell);
            return cell_with_elevation(cell, elevation - min(elevation, strength));
        }
        default: {
            return cell;
        }
    }
}

// WorldCell::inject_fluid: the painted liquid takes the column over and keeps the volume
// already there, up to the column's capacity.
fn inject_fluid(cell: vec2<u32>, material: u32, amount: u32) -> vec2<u32> {
    var out = cell;
    let volume = cell_fluid_volume(out);
    let added = min(amount, CELL_MAX_FLUID_VOLUME - volume);
    if (cell_fluid_material(out) != material) {
        out = cell_with_fluid_material(out, material);
        out = cell_with_compass(out, 0u);
    }
    return cell_with_fluid_volume(out, volume + added);
}

// Fire takes hold only on a dry or oil-covered column that is not already burning.
fn ignite(cell: vec2<u32>) -> vec2<u32> {
    let fluid = cell_fluid_material(cell);
    if (cell_surface_material(cell) == SURFACE_FIRE || (fluid != FLUID_EMPTY && fluid != FLUID_OIL)) {
        return cell;
    }
    return cell_with_surface_state(cell_with_surface_material(cell, SURFACE_FIRE), 0u);
}

// The sand brush: adds `strength` units of sand. Loose material of another kind is replaced,
// as in the baseline tool. Beyond the loose layer's capacity, sand compacts into sandstone
// bedrock (WorldCell::set_granular_vol), up to the elevation ceiling.
fn add_sand(cell: vec2<u32>, strength: u32) -> vec2<u32> {
    let is_sand = cell_granular_material(cell) == GRANULAR_SAND;
    let old_volume = select(0u, cell_granular_volume(cell), is_sand);
    let new_volume = min(old_volume + strength, CELL_MAX_ELEVATION + CELL_MAX_GRANULAR_VOLUME);
    if (is_sand && old_volume == new_volume) {
        return cell;
    }
    var out = cell_with_granular_material(cell, GRANULAR_SAND);
    if (new_volume > CELL_MAX_GRANULAR_VOLUME) {
        let excess = new_volume - CELL_MAX_GRANULAR_VOLUME;
        out = cell_with_terrain_material(out, SAND_COMPACTS_TO);
        out = cell_with_elevation(out, min(cell_elevation(out) + excess, CELL_MAX_ELEVATION));
        return cell_with_granular_volume(out, CELL_MAX_GRANULAR_VOLUME);
    }
    return cell_with_granular_volume(out, new_volume);
}
