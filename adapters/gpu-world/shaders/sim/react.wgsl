// react: every awake cell of a process tile computes its new value into `scratch`, applying in
// the baseline's order its granular transfers, its liquid transfers (with bed erosion), and, when
// active, thermal reactions, biology and fire. Neighbours are read from `cells`, the tick's input,
// which no pass changes before `commit`. A donor drains exactly the amount its receiver's
// resolution accepted; a receiver adds exactly what it accepted.
//
// The workgroup then publishes the tile's changed and persisting cells for the next tick's wake
// pass, clears the bitmaps the wake pass of this tick consumed, marks the next tick's candidate
// tiles its cells reach, and adds the tile's accounting to the diagnostics.
//
// commit: copies the awake cells' new values into `cells` (every cell, in dense execution).

// Fault injection only: a pipeline built with this override drops what receivers accept of
// liquid while donors still drain it, the unmatched transfer the `cells` suite must detect.
override fault_unmatched_transfer: bool = false;

const SUM_FLUID_IN: u32 = 0u;
const SUM_FLUID_OUT: u32 = 1u;
const SUM_SOLID_IN: u32 = 2u;
const SUM_SOLID_OUT: u32 = 3u;
const SUM_QUENCH: u32 = 4u;
const SUM_BOIL: u32 = 5u;
const SUM_BURN: u32 = 6u;
const SUM_UPTAKE: u32 = 7u;
const SUM_EROSION: u32 = 8u;
// The sums above are added to 64-bit counters in this order, starting at DIAG_FLUID_IN_LO.
const WIDE_SUMS: u32 = 9u;
const SUM_AWAKE: u32 = 9u;
const SUM_ACTIVE: u32 = 10u;
const SUM_CHANGED: u32 = 11u;
const SUM_MISMATCH: u32 = 12u;
const SUM_STALE: u32 = 13u;
const SUMS: u32 = 14u;

var<workgroup> react_tile: u32;
var<workgroup> sums: array<atomic<u32>, SUMS>;
var<workgroup> changed_bits: array<atomic<u32>, 2>;
var<workgroup> persist_bits: array<atomic<u32>, 2>;
var<workgroup> marks: atomic<u32>;

struct Reacted {
    cell: vec2<u32>,
    quench: u32,
    boil: u32,
    burn: u32,
    uptake: u32,
    erosion: u32,
    mismatch: bool,
    stale: u32,
    phase_pending: bool,
}

fn erode_bed(c: vec2<u32>, incoming: u32, fluid: u32, place: vec2<i32>) -> Changed {
    let flux = incoming * fluid_erosion_factor(fluid);
    if (flux == 0u || (keyed_hash(place, tick.rule_tick, tick.rule_key) & EROSION_MASK) >= flux) {
        return Changed(c, 0u);
    }
    let sediment = terrain_to_granular(cell_terrain_material(c));
    let resting = cell_granular_material(c);
    if (sediment == MATERIAL_NONE || cell_elevation(c) == 0u || (resting != GRANULAR_EMPTY && resting != sediment)) {
        return Changed(c, 0u);
    }
    var out = cell_with_elevation(c, cell_elevation(c) - 1u);
    out = cell_with_granular_material(out, sediment);
    return Changed(with_granular_volume(out, cell_granular_volume(out) + 1u), 1u);
}

fn roll(place: vec2<i32>, threshold: u32) -> bool {
    return keyed_hash_extended(place, tick.rule_tick, tick.rule_key, 0u, PHASE_HASH_CADENCE) % CHANCE_RESOLUTION < threshold;
}

fn chance(place: vec2<i32>, salt: u32, odds: u32) -> bool {
    return ratio(keyed_draw(place, tick.rule_tick, tick.rule_key, salt), 1u, odds);
}

// The baseline's step_thermal on the post-flow column, surveying the input neighbours.
fn react_thermal(local: vec2<i32>, place: vec2<i32>, out: ptr<function, Reacted>) {
    let fluid = cell_fluid_material((*out).cell);
    if (fluid == FLUID_EMPTY) {
        return;
    }
    var magma_contacts = 0u;
    var coolant_contacts = 0u;
    var heated = false;
    var touches_ice = false;
    var touches_dry = false;
    var touches_unfrozen = false;
    for (var dir = 0u; dir < 8u; dir++) {
        let neighbor = read_cell(local + DIRS[dir]);
        if (!neighbor.present) {
            continue;
        }
        let material = cell_fluid_material(neighbor.value);
        heated = heated || material == FLUID_MAGMA || cell_surface_material(neighbor.value) == SURFACE_FIRE;
        touches_ice = touches_ice || fluid_is_solid(material);
        touches_dry = touches_dry || material == FLUID_EMPTY;
        touches_unfrozen = touches_unfrozen || !fluid_is_solid(material);
        if (is_cardinal(dir)) {
            if (material == FLUID_MAGMA) {
                magma_contacts += 1u;
            } else if (fluid_is_coolant(material)) {
                coolant_contacts += 1u;
            }
        }
    }
    if (fluid == FLUID_MAGMA) {
        if (coolant_contacts > 0u) {
            let quenched = quench((*out).cell);
            (*out).cell = quenched.cell;
            (*out).quench += quenched.units;
        }
        return;
    }
    if (fluid_is_solid(fluid)) {
        if (heated) {
            (*out).cell = thaw((*out).cell);
            return;
        }
        if ((tick.phase_flags & PHASE_MELT) != 0u && touches_unfrozen) {
            if (roll(place, tick.melt_threshold)) {
                (*out).cell = thaw((*out).cell);
            }
            (*out).phase_pending = true;
        }
        return;
    }
    if (fluid_is_coolant(fluid) && magma_contacts > 0u) {
        let boiled = drain_fluid((*out).cell, STEAM_LOSS_PER_CONTACT * magma_contacts);
        (*out).cell = boiled.cell;
        (*out).boil += boiled.units;
    }
    if ((tick.phase_flags & PHASE_FREEZE) != 0u
        && (touches_ice || touches_dry)
        && !heated
        && fluid_frozen(cell_fluid_material((*out).cell)) != MATERIAL_NONE) {
        if (roll(place, tick.freeze_threshold)) {
            (*out).cell = freeze((*out).cell);
        }
        (*out).phase_pending = true;
    }
}

// The baseline's step_biology with keyed draws.
fn react_biology(local: vec2<i32>, place: vec2<i32>, old: vec2<u32>, out: ptr<function, Reacted>) {
    let surface = cell_surface_material(old);
    let fluid = cell_fluid_material(old);
    if (surface != SURFACE_EMPTY && surface != SURFACE_FOLIAGE && surface != SURFACE_ASH) {
        return;
    }
    let loose = cell_granular_material(old);
    let fertile = cell_terrain_material(old) == TERRAIN_DIRT || loose == GRANULAR_DIRT || loose == GRANULAR_MUD;
    let bare = surface == SURFACE_EMPTY || surface == SURFACE_ASH;
    if (!fertile && bare) {
        return;
    }
    let crust = floor_height(old);
    var hydrated = loose == GRANULAR_MUD || fluid == FLUID_WATER;
    let drowning = fluid != FLUID_EMPTY && cell_fluid_volume(old) > DROWNING_FLUID_VOLUME;
    var seeding_neighbors = 0u;
    for (var dir = 0u; dir < 8u; dir += 2u) {
        let neighbor = read_cell(local + DIRS[dir]);
        if (!neighbor.present) {
            continue;
        }
        let neighbor_crust = floor_height(neighbor.value);
        let rise = max(crust, neighbor_crust) - min(crust, neighbor_crust);
        if (bare
            && cell_surface_material(neighbor.value) == SURFACE_FOLIAGE
            && cell_surface_state(neighbor.value) >= SEEDING_FOLIAGE_STATE
            && rise <= SEEDING_MAX_STEP) {
            seeding_neighbors += 1u;
        }
        let neighbor_fluid = cell_fluid_material(neighbor.value);
        if (!hydrated && rise <= MAX_CAPILLARY_LIFT && (neighbor_fluid == FLUID_WATER || neighbor_fluid == FLUID_BLOOD)) {
            hydrated = true;
        }
    }
    if (bare) {
        if (fertile && hydrated && !drowning) {
            let ash = surface == SURFACE_ASH;
            let seeding = select(SEEDING_ODDS, SEEDING_ODDS_ON_ASH, ash);
            let spontaneous = select(SPONTANEOUS_GROWTH_ODDS, SPONTANEOUS_GROWTH_ODDS_ON_ASH, ash);
            if ((seeding_neighbors > 0u && chance(place, SALT_BIOLOGY_SEEDING, seeding))
                || chance(place, SALT_BIOLOGY_SPONTANEOUS, spontaneous)) {
                (*out).cell = cell_with_surface_state(cell_with_surface_material((*out).cell, SURFACE_FOLIAGE), 1u);
            }
        }
        return;
    }
    let state = cell_surface_state(old);
    if (hydrated && !drowning && state < FOLIAGE_MATURE_STATE) {
        if (chance(place, SALT_BIOLOGY_GROWTH, FOLIAGE_GROWTH_ODDS)) {
            (*out).cell = cell_with_surface_state((*out).cell, state + 1u);
        }
        let current = cell_fluid_material((*out).cell);
        if (current == FLUID_WATER || current == FLUID_BLOOD) {
            let drawn = drain_fluid((*out).cell, FOLIAGE_WATER_USE);
            (*out).cell = drawn.cell;
            (*out).uptake += drawn.units;
        } else if (cell_granular_material((*out).cell) == GRANULAR_MUD && chance(place, SALT_BIOLOGY_MUD_DRYING, MUD_DRYING_ODDS)) {
            (*out).cell = cell_with_granular_material((*out).cell, GRANULAR_DIRT);
        }
    }
}

// The baseline's step_fire with keyed draws.
fn react_fire(local: vec2<i32>, place: vec2<i32>, old: vec2<u32>, out: ptr<function, Reacted>) {
    let surface = cell_surface_material(old);
    let fluid = cell_fluid_material(old);
    if (fluid_is_coolant(fluid)) {
        if (surface == SURFACE_FIRE) {
            (*out).cell = cell_with_surface_state(cell_with_surface_material((*out).cell, SURFACE_EMPTY), 0u);
        }
        return;
    }
    if (surface == SURFACE_FIRE) {
        if (cell_fluid_material((*out).cell) == FLUID_OIL) {
            let burned = drain_fluid((*out).cell, OIL_BURN_RATE);
            (*out).cell = burned.cell;
            (*out).burn += burned.units;
            return;
        }
        let state = cell_surface_state(old);
        if (state < FLAME_LIFETIME_TICKS) {
            (*out).cell = cell_with_surface_state((*out).cell, state + 1u);
        } else {
            let ash = chance(place, SALT_FIRE_ASH, ASH_RESIDUE_ODDS);
            let residue = select(SURFACE_EMPTY, SURFACE_ASH, ash);
            (*out).cell = cell_with_surface_state(cell_with_surface_material((*out).cell, residue), 0u);
        }
        return;
    }
    if (!(surface_is_combustible(surface) || fluid_is_fuel(fluid))) {
        return;
    }
    var heat = select(0u, IGNITION_ODDS_DENOMINATOR, fluid == FLUID_MAGMA);
    for (var dir = 0u; dir < 8u; dir++) {
        let neighbor = read_cell(local + DIRS[dir]);
        if (neighbor.present
            && (cell_surface_material(neighbor.value) == SURFACE_FIRE || cell_fluid_material(neighbor.value) == FLUID_MAGMA)) {
            heat += 1u;
        }
    }
    if (heat > 0u
        && ratio(keyed_draw(place, tick.rule_tick, tick.rule_key, SALT_FIRE_IGNITION), min(heat, IGNITION_ODDS_DENOMINATOR), IGNITION_ODDS_DENOMINATOR)) {
        (*out).cell = cell_with_surface_state(cell_with_surface_material((*out).cell, SURFACE_FIRE), 0u);
    }
}

// The accepted amount a destination's resolution grants this donor along `dir`, for a family.
fn granted(local: vec2<i32>, dir: u32, liquid: bool, out: ptr<function, Reacted>) -> u32 {
    let dest = read_cell(local + DIRS[dir]);
    let resolution = flow[2u * dest.index + 1u];
    if (!stamped(resolution)) {
        (*out).stale += 1u;
        return 0u;
    }
    if (liquid) {
        return select(0u, liquid_amount(resolution), liquid_dir(resolution) == opposite(dir));
    }
    return select(0u, granular_amount(resolution), granular_dir(resolution) == opposite(dir));
}

fn react_cell(local: vec2<i32>, here: Read, level: u32) -> Reacted {
    let old = here.value;
    var out = Reacted(old, 0u, 0u, 0u, 0u, 0u, false, 0u, false);
    let is_active = level >= WAKE_ACTIVE_MIN;
    let place = absolute(local);
    let proposal = flow[2u * here.index];
    let resolution = flow[2u * here.index + 1u];

    if (runs(RULE_GRANULAR)) {
        var incoming = 0u;
        var material = GRANULAR_EMPTY;
        let source = granular_dir(resolution);
        if (source != DIR_NONE) {
            incoming = granular_amount(resolution);
            material = cell_granular_material(read_cell(local + DIRS[source]).value);
        }
        var outgoing = 0u;
        let dest = granular_dir(proposal);
        if (is_active && cell_granular_material(old) != GRANULAR_EMPTY && dest != DIR_NONE) {
            outgoing = granted(local, dest, false, &out);
        }
        if (incoming > 0u || outgoing > 0u) {
            if (incoming > 0u && cell_granular_material(old) == GRANULAR_EMPTY) {
                out.cell = cell_with_granular_material(out.cell, material);
            }
            out.cell = with_granular_volume(out.cell, cell_granular_volume(old) + incoming - outgoing);
            if (cell_granular_volume(out.cell) == 0u) {
                out.cell = cell_with_granular_material(out.cell, GRANULAR_EMPTY);
            }
        }
    }

    if (runs(RULE_LIQUID)) {
        var incoming = 0u;
        var material = FLUID_EMPTY;
        let source = liquid_dir(resolution);
        if (source != DIR_NONE) {
            incoming = liquid_amount(resolution);
            material = cell_fluid_material(read_cell(local + DIRS[source]).value);
        }
        var outgoing = 0u;
        let dest = liquid_dir(proposal);
        if (is_active && cell_fluid_material(old) != FLUID_EMPTY && dest != DIR_NONE) {
            outgoing = granted(local, dest, true, &out);
        }
        if (fault_unmatched_transfer) {
            incoming = 0u;
        }
        if (incoming > 0u || outgoing > 0u) {
            out.cell = drain_fluid(out.cell, outgoing).cell;
            if (incoming > 0u) {
                let elevation = cell_elevation(out.cell);
                let received = receive_fluid(out.cell, material, incoming);
                out.mismatch = received.units != incoming;
                out.cell = received.cell;
                out.quench += cell_elevation(out.cell) - elevation;
                out.cell = cell_with_compass(out.cell, compass_for_flow(-DIRS[source]));
                let eroded = erode_bed(out.cell, incoming, material, place);
                out.cell = eroded.cell;
                out.erosion += eroded.units;
            }
        }
    }

    if (is_active) {
        if (runs(RULE_THERMAL)) {
            react_thermal(local, place, &out);
        }
        if (runs(RULE_BIOLOGY) && chance(place, SALT_BIOLOGY_SAMPLE, BIOLOGY_ODDS_DENOMINATOR)) {
            react_biology(local, place, old, &out);
        }
        if (runs(RULE_FIRE) && chance(place, SALT_FIRE_SAMPLE, FIRE_ODDS_DENOMINATOR)) {
            react_fire(local, place, old, &out);
        }
    }
    return out;
}

@compute @workgroup_size(8, 8, 1)
fn react(
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(local_invocation_index) li: u32,
) {
    if (li == 0u) {
        react_tile = list_entry(ACTIVITY_PROCESS_COUNT, tick.process_base, wg);
    }
    let tile = workgroupUniformLoad(&react_tile);
    if (tile == TILE_NONE) {
        return;
    }
    if (li < SUMS) {
        atomicStore(&sums[li], 0u);
    }
    if (li < 2u) {
        atomicStore(&changed_bits[li], 0u);
        atomicStore(&persist_bits[li], 0u);
    }
    if (li == 0u) {
        atomicStore(&marks, 0u);
    }
    workgroupBarrier();

    let within = lid.xy;
    let local = tile_cell(tile, within);
    let level = wake_level(local);
    let here = read_cell(local);
    if (level > 0u) {
        let reacted = react_cell(local, here, level);
        let old = here.value;
        let updated = reacted.cell;
        scratch[here.index] = updated;
        let fluid_before = cell_fluid_volume(old);
        let fluid_after = cell_fluid_volume(updated);
        let solid_before = floor_height(old);
        let solid_after = floor_height(updated);
        atomicAdd(&sums[SUM_FLUID_IN], max(fluid_after, fluid_before) - fluid_before);
        atomicAdd(&sums[SUM_FLUID_OUT], max(fluid_after, fluid_before) - fluid_after);
        atomicAdd(&sums[SUM_SOLID_IN], max(solid_after, solid_before) - solid_before);
        atomicAdd(&sums[SUM_SOLID_OUT], max(solid_after, solid_before) - solid_after);
        atomicAdd(&sums[SUM_QUENCH], reacted.quench);
        atomicAdd(&sums[SUM_BOIL], reacted.boil);
        atomicAdd(&sums[SUM_BURN], reacted.burn);
        atomicAdd(&sums[SUM_UPTAKE], reacted.uptake);
        atomicAdd(&sums[SUM_EROSION], reacted.erosion);
        atomicAdd(&sums[SUM_AWAKE], 1u);
        atomicAdd(&sums[SUM_STALE], reacted.stale);
        let is_active = level >= WAKE_ACTIVE_MIN;
        if (is_active) {
            atomicAdd(&sums[SUM_ACTIVE], 1u);
        }
        if (reacted.mismatch) {
            atomicAdd(&sums[SUM_MISMATCH], 1u);
        }
        let value_changed = any(updated != old);
        if (value_changed) {
            atomicAdd(&sums[SUM_CHANGED], 1u);
        }
        // An active cell stays awake while foliage grows, fire burns or a phase change waits,
        // each only while its rule runs: a rule that is off cannot advance what the cell waits
        // for, and switching it back on wakes every cell.
        let surface = cell_surface_material(updated);
        let growing = runs(RULE_BIOLOGY) && surface == SURFACE_FOLIAGE && cell_surface_state(updated) < FOLIAGE_MATURE_STATE;
        let burning = runs(RULE_FIRE) && surface == SURFACE_FIRE;
        let kept = is_active && (growing || burning || reacted.phase_pending);
        let word = bitmap_word(within);
        let bit = bitmap_bit(within);
        if (value_changed || kept) {
            atomicOr(&changed_bits[word], bit);
            atomicOr(&marks, reach_mask(within, u32(WAKE_CHANGE_RADIUS) + 1u));
        } else if (level == WAKE_ACTIVE_FRESH) {
            atomicOr(&persist_bits[word], bit);
            atomicOr(&marks, reach_mask(within, 1u));
        }
    } else if (dense()) {
        scratch[here.index] = here.value;
    }
    workgroupBarrier();

    if (li < WIDE_SUMS) {
        add64(DIAG_FLUID_IN_LO + 2u * li, atomicLoad(&sums[li]));
    } else if (li < SUMS) {
        let counter = array<u32, 5>(DIAG_AWAKE_CELLS, DIAG_ACTIVE_CELLS, DIAG_CHANGED_CELLS, DIAG_RECEIVE_MISMATCHES, DIAG_STALE_FLOW);
        let value = atomicLoad(&sums[li]);
        if (value > 0u) {
            atomicAdd(&diagnostics[counter[li - WIDE_SUMS]], value);
        }
    }
    if (li == 0u) {
        let parity = tick.parity * 2u;
        let other = (tick.parity ^ 1u) * 2u;
        for (var w = 0u; w < 2u; w++) {
            atomicStore(&activity[tile_word(tile, TILE_CHANGED + parity + w)], atomicLoad(&changed_bits[w]));
            atomicStore(&activity[tile_word(tile, TILE_PERSIST + parity + w)], atomicLoad(&persist_bits[w]));
            atomicStore(&activity[tile_word(tile, TILE_CHANGED + other + w)], 0u);
            atomicStore(&activity[tile_word(tile, TILE_PERSIST + other + w)], 0u);
            atomicStore(&activity[tile_word(tile, TILE_EDITED + w)], 0u);
        }
        mark_candidates(tile, atomicLoad(&marks));
        // `commit` writes exactly the cells whose value changed here.
        if (atomicLoad(&sums[SUM_CHANGED]) > 0u) {
            mark_slot_changed(resident_slot(tile_cell(tile, vec2<u32>(0u))));
        }
    }
}

@compute @workgroup_size(8, 8, 1)
fn commit(
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(local_invocation_index) li: u32,
) {
    if (li == 0u) {
        react_tile = list_entry(ACTIVITY_PROCESS_COUNT, tick.process_base, wg);
    }
    let tile = workgroupUniformLoad(&react_tile);
    if (tile == TILE_NONE) {
        return;
    }
    let local = tile_cell(tile, lid.xy);
    let index = cell_index(resident_slot(local), local);
    if (dense()) {
        cells[index] = scratch[index];
        return;
    }
    if (wake_level(local) == 0u) {
        return;
    }
    let value = scratch[index];
    if (any(value != cells[index])) {
        cells[index] = value;
    }
}
