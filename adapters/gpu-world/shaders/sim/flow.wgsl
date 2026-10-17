// Transfer proposals and their resolution: the baseline's granular_sim and liquid_sim decisions,
// each evaluated once.
//
// propose  awake cells of process tiles. An active cell chooses its preferred granular and liquid
//          destination from `cells` (the tick's input, read up to two cells away) and the amount
//          it offers; every awake cell writes its proposal word, empty when it offers nothing.
// resolve  awake cells of process tiles. A cell picks, per family, the baseline's winning source
//          among its active neighbours whose proposal names it, and accepts at most what its
//          column can hold; it writes its resolution word.
//
// Both write only their own word of `flow`; a resolution is read by its winner in `react`.

var<workgroup> flow_tile: u32;

fn granular_transfer(source: vec2<u32>, dest: vec2<u32>) -> u32 {
    let source_total = floor_height(source);
    let dest_total = floor_height(dest);
    if (dest_total >= source_total) {
        return 0u;
    }
    return min(
        min((source_total - dest_total) / 2u, MAX_GRANULAR_TRANSFER),
        min(cell_granular_volume(source), structural_void(dest)),
    );
}

// The preferred granular destination of an active cell and the units it offers (DIR_NONE, 0).
fn granular_destination(pos: vec2<i32>, source: vec2<u32>) -> vec2<u32> {
    let material = cell_granular_material(source);
    let repose = granular_repose(material);
    if (repose == REPOSE_NONE || cell_granular_volume(source) == 0u || fluid_is_solid(cell_fluid_material(source))) {
        return vec2<u32>(DIR_NONE, 0u);
    }
    let place = absolute(pos);
    let order = shuffled(false, place, tick.rule_tick, tick.rule_key, material, 0u);
    let source_total = floor_height(source);
    let dynamic_repose = repose + keyed_hash(place, tick.rule_tick, tick.rule_key) % STICTION_VARIANTS;
    var best = DIR_NONE;
    var best_diff = 0u;
    var best_cell = vec2<u32>(0u);
    for (var k = 0u; k < order.count; k++) {
        let dir = order.dirs[k];
        let neighbor = read_cell(pos + DIRS[dir]);
        if (!neighbor.present) {
            continue;
        }
        let neighbor_total = floor_height(neighbor.value);
        let diff = select(0u, source_total - neighbor_total, source_total > neighbor_total);
        if (diff > dynamic_repose && diff >= MIN_TRANSFER_HEAD) {
            let neighbor_material = cell_granular_material(neighbor.value);
            let compatible = neighbor_material == GRANULAR_EMPTY || neighbor_material == material;
            let room = structural_void(neighbor.value) > 0u && !fluid_is_solid(cell_fluid_material(neighbor.value));
            if (compatible && room && diff > best_diff) {
                best_diff = diff;
                best = dir;
                best_cell = neighbor.value;
            }
        }
    }
    if (best == DIR_NONE) {
        return vec2<u32>(DIR_NONE, 0u);
    }
    return vec2<u32>(best, granular_transfer(source, best_cell));
}

fn drains_lower(pos: vec2<i32>, head: u32) -> bool {
    for (var dir = 0u; dir < 8u; dir++) {
        let neighbor = read_cell(pos + DIRS[dir]);
        if (neighbor.present && liquid_head(neighbor.value) < head) {
            return true;
        }
    }
    return false;
}

// A single unit may cross a head difference too small to divide when the receiver drains lower
// or the source's momentum carries it onto a terrace no higher than the receiver.
fn allows_trickle(source: vec2<u32>, source_pos: vec2<i32>, dest_pos: vec2<i32>, dest_total: u32) -> bool {
    if (drains_lower(dest_pos, dest_total)) {
        return true;
    }
    let direction = dest_pos - source_pos;
    if (any(compass_forward(cell_compass(source)) != direction)) {
        return false;
    }
    let beyond = read_cell(dest_pos + direction);
    return beyond.present && liquid_head(beyond.value) <= dest_total;
}

fn liquid_transfer(source: vec2<u32>, source_pos: vec2<i32>, dest: vec2<u32>, dest_pos: vec2<i32>) -> u32 {
    let source_total = liquid_head(source);
    let dest_total = liquid_head(dest);
    let material = cell_fluid_material(source);
    if (dest_total >= source_total || fluid_is_solid(material) || fluid_is_solid(cell_fluid_material(dest))) {
        return 0u;
    }
    var amount = (source_total - dest_total) / fluid_flow_divisor(material);
    if (amount == 0u && fluid_levels_out(material) && allows_trickle(source, source_pos, dest_pos, dest_total)) {
        amount = 1u;
    }
    return min(min(amount, cell_fluid_volume(source)), CELL_MAX_FLUID_VOLUME - cell_fluid_volume(dest));
}

// The preferred liquid destination of an active cell and the units it offers (DIR_NONE, 0).
fn liquid_destination(pos: vec2<i32>, source: vec2<u32>) -> vec2<u32> {
    let material = cell_fluid_material(source);
    if (material == FLUID_EMPTY || fluid_is_solid(material)) {
        return vec2<u32>(DIR_NONE, 0u);
    }
    let order = with_momentum(
        shuffled(material == FLUID_MAGMA, absolute(pos), tick.rule_tick, tick.rule_key, material, 0u),
        cell_compass(source),
    );
    var best = DIR_NONE;
    var best_total = liquid_head(source);
    var best_amount = 0u;
    for (var k = 0u; k < order.count; k++) {
        let dir = order.dirs[k];
        let neighbor_pos = pos + DIRS[dir];
        let neighbor = read_cell(neighbor_pos);
        if (!neighbor.present) {
            continue;
        }
        let total = liquid_head(neighbor.value);
        if (total < best_total) {
            let amount = liquid_transfer(source, pos, neighbor.value, neighbor_pos);
            if (amount > 0u) {
                best_total = total;
                best = dir;
                best_amount = amount;
            }
        }
    }
    return vec2<u32>(best, best_amount);
}

@compute @workgroup_size(8, 8, 1)
fn propose(
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(local_invocation_index) li: u32,
) {
    if (li == 0u) {
        flow_tile = list_entry(ACTIVITY_PROCESS_COUNT, tick.process_base, wg);
    }
    let tile = workgroupUniformLoad(&flow_tile);
    if (tile == TILE_NONE) {
        return;
    }
    let local = tile_cell(tile, lid.xy);
    let level = wake_level(local);
    if (level == 0u) {
        return;
    }
    let here = read_cell(local);
    var granular = vec2<u32>(DIR_NONE, 0u);
    var liquid = vec2<u32>(DIR_NONE, 0u);
    if (level >= WAKE_ACTIVE_MIN) {
        if (runs(RULE_GRANULAR)) {
            granular = granular_destination(local, here.value);
        }
        if (runs(RULE_LIQUID)) {
            liquid = liquid_destination(local, here.value);
        }
    }
    flow[2u * here.index] = pack_flow(granular.x, granular.y, liquid.x, liquid.y);
}

@compute @workgroup_size(8, 8, 1)
fn resolve(
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(local_invocation_index) li: u32,
) {
    if (li == 0u) {
        flow_tile = list_entry(ACTIVITY_PROCESS_COUNT, tick.process_base, wg);
    }
    let tile = workgroupUniformLoad(&flow_tile);
    if (tile == TILE_NONE) {
        return;
    }
    let local = tile_cell(tile, lid.xy);
    if (wake_level(local) == 0u) {
        return;
    }
    let here = read_cell(local);
    let place = absolute(local);
    var granular_source = DIR_NONE;
    var granular_accepted = 0u;
    if (runs(RULE_GRANULAR)) {
        let order = shuffled(false, place, tick.rule_tick, tick.rule_key, 0u, 0u);
        let dest_total = floor_height(here.value);
        var best_diff = 0u;
        for (var k = 0u; k < order.count; k++) {
            let dir = order.dirs[k];
            let neighbor_pos = local + DIRS[dir];
            let neighbor = read_cell(neighbor_pos);
            if (!neighbor.present || granular_repose(cell_granular_material(neighbor.value)) == REPOSE_NONE) {
                continue;
            }
            if (wake_level(neighbor_pos) < WAKE_ACTIVE_MIN) {
                continue;
            }
            let proposal = flow[2u * neighbor.index];
            if (!stamped(proposal)) {
                atomicAdd(&diagnostics[DIAG_STALE_FLOW], 1u);
                continue;
            }
            if (granular_dir(proposal) != opposite(dir)) {
                continue;
            }
            let neighbor_total = floor_height(neighbor.value);
            let diff = select(0u, neighbor_total - dest_total, neighbor_total > dest_total);
            if (diff > best_diff) {
                best_diff = diff;
                granular_source = dir;
                granular_accepted = granular_amount(proposal);
            }
        }
    }
    var liquid_source = DIR_NONE;
    var liquid_accepted = 0u;
    if (runs(RULE_LIQUID)) {
        let resident = cell_fluid_material(here.value);
        let order = with_momentum(
            shuffled(resident == FLUID_MAGMA, place, tick.rule_tick, tick.rule_key, resident, 0u),
            cell_compass(here.value),
        );
        var best_volume = 0u;
        var proposed = 0u;
        var material = FLUID_EMPTY;
        for (var k = 0u; k < order.count; k++) {
            let dir = order.dirs[k];
            let neighbor_pos = local + DIRS[dir];
            let neighbor = read_cell(neighbor_pos);
            if (!neighbor.present || cell_fluid_material(neighbor.value) == FLUID_EMPTY) {
                continue;
            }
            if (wake_level(neighbor_pos) < WAKE_ACTIVE_MIN) {
                continue;
            }
            let proposal = flow[2u * neighbor.index];
            if (!stamped(proposal)) {
                atomicAdd(&diagnostics[DIAG_STALE_FLOW], 1u);
                continue;
            }
            if (liquid_dir(proposal) != opposite(dir)) {
                continue;
            }
            let volume = cell_fluid_volume(neighbor.value);
            if (volume > best_volume) {
                best_volume = volume;
                liquid_source = dir;
                proposed = liquid_amount(proposal);
                material = cell_fluid_material(neighbor.value);
            }
        }
        if (liquid_source != DIR_NONE) {
            // The column as it can be at worst when the liquid arrives: after the largest
            // compaction this tick's granular inflow could cause. Its own outflow only frees room.
            let compaction = max(cell_granular_volume(here.value) + granular_accepted, CELL_MAX_GRANULAR_VOLUME) - CELL_MAX_GRANULAR_VOLUME;
            let worst = cell_with_elevation(here.value, cell_elevation(here.value) + compaction);
            liquid_accepted = receive_fluid(worst, material, proposed).units;
            if (liquid_accepted < proposed) {
                atomicAdd(&diagnostics[DIAG_REJECTED_TRANSFERS], 1u);
            }
        }
    }
    flow[2u * here.index + 1u] = pack_flow(granular_source, granular_accepted, liquid_source, liquid_accepted);
}
