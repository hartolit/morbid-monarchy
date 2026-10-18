// apply_edit: one brush stroke, as the baseline tools apply it (morbid-app's apply_brush with
// world_model's column rules), around the command's centre cell or, for a ray-targeted stroke,
// around the quad cell `resolve_target` found the ray's hit in. A stroke whose ray missed has
// status RESULT_TARGET_MISSED and changes nothing.
//
// Dispatch: one invocation per cell of the stroke's (2r + 1)^2 bounding square, in 8 x 8
// workgroups; invocations past the square (partial edge workgroups) and outside the circle
// return at once, and no barrier follows. Each invocation owns exactly one cell, so a stroke
// has one writer per cell. Strokes of a tick run as separate dispatches in command order;
// wgpu places a barrier between dispatches that use the same read-write storage buffer, so a
// later stroke reads the earlier one's result. Counts go to the command's own result record
// and are read only after the pass ends. A changed cell is marked edited, so this tick's
// material phases see the stroke, and its tool quantities are added to the diagnostics.

@compute @workgroup_size(8, 8, 1)
fn apply_edit(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = dispatch_params.command_index;
    let command = commands[index];
    if (command.kind != COMMAND_EDIT || results[index].status != RESULT_EDITED) {
        return;
    }
    var center = vec2<i32>(command.center_x, command.center_y);
    if (command.target_kind == TARGET_RAY) {
        center = vec2<i32>(bitcast<i32>(results[index].v1), bitcast<i32>(results[index].v2));
    }
    if (all(id.xy == vec2<u32>(0u))) {
        results[index].word0 = bitcast<u32>(center.x);
        results[index].word1 = bitcast<u32>(center.y);
    }
    let radius = i32(command.radius);
    let offset = vec2<i32>(id.xy) - vec2<i32>(radius);
    if (offset.x > radius || offset.y > radius) {
        return;
    }
    if (offset.x * offset.x + offset.y * offset.y > radius * radius) {
        return;
    }
    let local = center + offset;
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
        mark_slot_changed(slot);
        atomicAdd(&results[index].changed, 1u);
        mark_edited(local);
        account_tool(before, after);
    }
}

// An edited cell wakes for this tick's material phases like a changed one: its tile records the
// edit, and the tiles within the wake reach of the cell become candidates.
fn mark_edited(local: vec2<i32>) {
    let tile = tile_id(tile_of_cell(local));
    let within = vec2<u32>(local) % TILE_EDGE;
    atomicOr(&activity[tile_word(tile, TILE_EDITED + bitmap_word(within))], bitmap_bit(within));
    mark_candidates(tile, reach_mask(within, u32(WAKE_CHANGE_RADIUS) + 1u));
}

// Tools are the only external sources and sinks: painted liquid only ever adds volume; sand,
// raise and lower change the solid column either way.
fn account_tool(before: vec2<u32>, after: vec2<u32>) {
    let fluid_before = cell_fluid_volume(before);
    let fluid_after = cell_fluid_volume(after);
    add64(DIAG_TOOL_FLUID_IN_LO, max(fluid_after, fluid_before) - fluid_before);
    let solid_before = cell_elevation(before) + cell_granular_volume(before);
    let solid_after = cell_elevation(after) + cell_granular_volume(after);
    add64(DIAG_TOOL_SOLID_IN_LO, max(solid_after, solid_before) - solid_before);
    add64(DIAG_TOOL_SOLID_OUT_LO, max(solid_after, solid_before) - solid_after);
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
