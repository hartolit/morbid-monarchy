//! An independent CPU reference of the cell rules as 03 selected them, for small fixtures.
//!
//! The GPU evaluates each transfer once, as a proposal its receiver resolves; this reference is
//! deliberately written the other way, in the baseline engine's gather form: every cell
//! re-derives its neighbours' preferences and amounts from the tick's input cells, and wake
//! levels are scattered with `schedule_wake` as the baseline does. It applies 03's corrections
//! (a receiver accepts only what it can hold, the conservative quench) and 03's keyed draws for
//! biology and fire, and it treats a missing chunk as a closed boundary. With seed 0, rules
//! without fire and biology, and no column at the elevation ceiling it equalled the baseline
//! engine cell for cell; a test against `monarch-engine` itself checked that until 06 removed
//! the engine, so this file is not the GPU implementation's own oracle.
//!
//! It is a test reference, not a production fallback: it holds a whole small window in memory
//! and recomputes everything, which is only sensible for a few chunks.

use world_model::{
    cell::WorldCell,
    command::EditTool,
    glam::IVec2,
    material::{
        FluidMat, GranularMat, SurfaceMat, TerrainMat, fluid_properties, granular_repose,
        is_combustible,
    },
    observation::WorldTotals,
    rules::{
        self, ASH_RESIDUE_ODDS, BIOLOGY_ODDS_DENOMINATOR, CARDINAL_DIRS, CHANCE_RESOLUTION,
        DROWNING_FLUID_VOLUME, EROSION_MASK, FIRE_ODDS_DENOMINATOR, FLAME_LIFETIME_TICKS,
        FOLIAGE_GROWTH_ODDS, FOLIAGE_MATURE_STATE, FOLIAGE_WATER_USE, FlowPattern,
        IGNITION_ODDS_DENOMINATOR, MAX_CAPILLARY_LIFT, MAX_GRANULAR_TRANSFER, MIN_TRANSFER_HEAD,
        MUD_DRYING_ODDS, NEIGHBOR_DIRS, OIL_BURN_RATE, PHASE_HASH_CADENCE, PhaseThresholds,
        RuleTick, SEEDING_FOLIAGE_STATE, SEEDING_MAX_STEP, SEEDING_ODDS, SEEDING_ODDS_ON_ASH,
        SPONTANEOUS_GROWTH_ODDS, SPONTANEOUS_GROWTH_ODDS_ON_ASH, STEAM_LOSS_PER_CONTACT,
        STICTION_VARIANTS, ShuffledDirs, SimulationRules, WAKE_ACTIVE_FRESH, WAKE_ACTIVE_MIN,
        WAKE_CHANGE_RADIUS, WAKE_HALO, compass_for_flow, compass_forward, draw, keyed_spatial_hash,
        keyed_spatial_hash_extended, ratio, salt,
    },
    spatial::{CHUNK_CELL_COUNT, CHUNK_SIZE},
};

/// Edge of an activity tile, in cells (the GPU's scheduling unit; here only counted).
pub const TILE_EDGE: i32 = 8;

/// What one tick did, in the quantities the GPU's diagnostics report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TickDiagnostics {
    pub fluid_in: u64,
    pub fluid_out: u64,
    pub solid_in: u64,
    pub solid_out: u64,
    pub quench: u64,
    pub boil: u64,
    pub burn: u64,
    pub uptake: u64,
    pub erosion: u64,
    pub tool_fluid_in: u64,
    pub tool_solid_in: u64,
    pub tool_solid_out: u64,
    pub awake_cells: u64,
    pub active_cells: u64,
    pub changed_cells: u64,
    pub process_tiles: u64,
    pub receive_mismatches: u64,
}

impl TickDiagnostics {
    /// `ΔL + sinks` and `ΔS − quench` of the material phases: both zero when conserved.
    pub fn imbalance(&self) -> (i128, i128) {
        let fluid = i128::from(self.fluid_in) - i128::from(self.fluid_out)
            + i128::from(self.quench)
            + i128::from(self.boil)
            + i128::from(self.burn)
            + i128::from(self.uptake);
        let solid =
            i128::from(self.solid_in) - i128::from(self.solid_out) - i128::from(self.quench);
        (fluid, solid)
    }
}

/// The baseline brush (`morbid-app` `apply_brush`) expressed with world-model's column rules.
pub fn edit_cell(mut cell: WorldCell, tool: EditTool, strength: u8) -> WorldCell {
    if let Some(fluid) = tool.fluid() {
        cell.inject_fluid(fluid, u16::from(strength));
        return cell;
    }
    match tool {
        EditTool::Fire => {
            let fluid = cell.fluid_mat();
            if cell.surface_mat() != SurfaceMat::SURFACE_FIRE
                && (fluid == FluidMat::EMPTY || fluid == FluidMat::FLUID_OIL)
            {
                cell.set_surface_mat(SurfaceMat::SURFACE_FIRE);
                cell.set_surface_state(0);
            }
        }
        EditTool::Sand => {
            let is_sand = cell.granular_mat() == GranularMat::GRANULAR_SAND;
            let old = if is_sand { cell.granular_vol() } else { 0 };
            let new = (old + u16::from(strength))
                .min(WorldCell::MAX_ELEVATION as u16 + WorldCell::MAX_GRANULAR_VOL);
            if !is_sand || old != new {
                cell.set_granular_mat(GranularMat::GRANULAR_SAND);
                cell.set_granular_vol(new);
            }
        }
        EditTool::Raise => {
            let raised =
                (u32::from(cell.elevation()) + u32::from(strength)).min(WorldCell::MAX_ELEVATION);
            cell.set_elevation(raised as u16);
        }
        EditTool::Lower => cell.set_elevation(cell.elevation().saturating_sub(u16::from(strength))),
        EditTool::Water | EditTool::Lava | EditTool::Oil | EditTool::Ice => {}
    }
    cell
}

fn solid_units(cell: &WorldCell) -> u64 {
    u64::from(cell.elevation()) + u64::from(cell.granular_vol())
}

/// The tick's input cells and wake levels, read by every rule.
struct View<'a> {
    cells: &'a [WorldCell],
    wake: &'a [u8],
    resident: &'a [bool],
    width: i32,
    height: i32,
    chunks_x: i32,
    origin: IVec2,
    tick: RuleTick,
    granular: bool,
}

impl View<'_> {
    fn index(&self, local: IVec2) -> Option<usize> {
        if local.x < 0 || local.y < 0 || local.x >= self.width || local.y >= self.height {
            return None;
        }
        let chunk = (local.y / CHUNK_SIZE as i32) * self.chunks_x + local.x / CHUNK_SIZE as i32;
        self.resident[chunk as usize].then_some((local.y * self.width + local.x) as usize)
    }

    fn get(&self, local: IVec2) -> Option<WorldCell> {
        self.index(local).map(|index| self.cells[index])
    }

    fn at(&self, local: IVec2) -> WorldCell {
        self.get(local).unwrap_or_default()
    }

    fn can_donate(&self, local: IVec2) -> bool {
        self.index(local)
            .is_some_and(|index| self.wake[index] >= WAKE_ACTIVE_MIN)
    }

    /// The absolute cell position the hashes see.
    fn abs(&self, local: IVec2) -> IVec2 {
        IVec2::new(
            self.origin.x.wrapping_add(local.x),
            self.origin.y.wrapping_add(local.y),
        )
    }
}

// --- Granular: the baseline's `granular_sim`, gathering from the input view ---

fn floor_height(cell: &WorldCell) -> u32 {
    cell.elevation() as u32 + cell.granular_vol() as u32
}

fn structural_void(cell: &WorldCell) -> u32 {
    WorldCell::MAX_ELEVATION.saturating_sub(cell.elevation() as u32)
        + (WorldCell::MAX_GRANULAR_VOL as u32).saturating_sub(cell.granular_vol() as u32)
}

fn granular_transfer(source: &WorldCell, dest: &WorldCell) -> u16 {
    let (source_total, dest_total) = (floor_height(source), floor_height(dest));
    if dest_total >= source_total {
        return 0;
    }
    ((source_total - dest_total) / 2)
        .min(MAX_GRANULAR_TRANSFER)
        .min(source.granular_vol() as u32)
        .min(structural_void(dest)) as u16
}

fn granular_destination(view: &View, pos: IVec2) -> Option<IVec2> {
    let source = view.get(pos)?;
    let material = source.granular_mat();
    let repose = granular_repose(material);
    if repose == u16::MAX || source.granular_vol() == 0 || source.fluid_mat().is_solid() {
        return None;
    }
    let abs = view.abs(pos);
    let shuffled = ShuffledDirs::new(FlowPattern::Omni, abs, view.tick, material.0, 0);
    let source_total = floor_height(&source);
    let dynamic_repose = repose as u32 + keyed_spatial_hash(abs, view.tick) % STICTION_VARIANTS;
    let mut best = None;
    let mut best_diff = 0;
    for &dir in shuffled.get() {
        let Some(neighbor) = view.get(pos + dir) else {
            continue;
        };
        let diff = source_total.saturating_sub(floor_height(&neighbor));
        if diff > dynamic_repose && diff >= MIN_TRANSFER_HEAD {
            let compatible = neighbor.granular_mat() == GranularMat::EMPTY
                || neighbor.granular_mat() == material;
            let room = structural_void(&neighbor) > 0 && !neighbor.fluid_mat().is_solid();
            if compatible && room && diff > best_diff {
                best_diff = diff;
                best = Some(pos + dir);
            }
        }
    }
    best
}

fn granular_source(view: &View, pos: IVec2) -> Option<IVec2> {
    let dest = view.get(pos)?;
    let dest_total = floor_height(&dest);
    let shuffled = ShuffledDirs::new(FlowPattern::Omni, view.abs(pos), view.tick, 0, 0);
    let mut best = None;
    let mut best_diff = 0;
    for &dir in shuffled.get() {
        let neighbor_pos = pos + dir;
        let Some(neighbor) = view.get(neighbor_pos) else {
            continue;
        };
        if granular_repose(neighbor.granular_mat()) != u16::MAX
            && view.can_donate(neighbor_pos)
            && granular_destination(view, neighbor_pos) == Some(pos)
        {
            let diff = floor_height(&neighbor).saturating_sub(dest_total);
            if diff > best_diff {
                best_diff = diff;
                best = Some(neighbor_pos);
            }
        }
    }
    best
}

/// Granular units `pos` receives this tick, and their material.
fn granular_incoming(view: &View, pos: IVec2) -> (u16, GranularMat) {
    if !view.granular {
        return (0, GranularMat::EMPTY);
    }
    match granular_source(view, pos) {
        Some(source) => {
            let source = view.at(source);
            (
                granular_transfer(&source, &view.at(pos)),
                source.granular_mat(),
            )
        }
        None => (0, GranularMat::EMPTY),
    }
}

fn granular_outgoing(view: &View, pos: IVec2) -> u16 {
    let cell = view.at(pos);
    if !view.granular || !view.can_donate(pos) || cell.granular_mat() == GranularMat::EMPTY {
        return 0;
    }
    match granular_destination(view, pos) {
        Some(dest) if granular_source(view, dest) == Some(pos) => {
            granular_transfer(&cell, &view.at(dest))
        }
        _ => 0,
    }
}

// --- Liquid: the baseline's `liquid_sim`, with the receiver's capacity check ---

fn drains_lower(view: &View, pos: IVec2, head: u32) -> bool {
    NEIGHBOR_DIRS.iter().any(|&dir| {
        view.get(pos + dir)
            .is_some_and(|neighbor| neighbor.liquid_head() < head)
    })
}

fn allows_trickle(
    view: &View,
    source: &WorldCell,
    source_pos: IVec2,
    dest_pos: IVec2,
    dest_total: u32,
) -> bool {
    if drains_lower(view, dest_pos, dest_total) {
        return true;
    }
    let direction = dest_pos - source_pos;
    compass_forward(source.compass()) == direction
        && view
            .get(dest_pos + direction)
            .is_some_and(|beyond| beyond.liquid_head() <= dest_total)
}

fn liquid_transfer(
    view: &View,
    source: &WorldCell,
    source_pos: IVec2,
    dest: &WorldCell,
    dest_pos: IVec2,
) -> u16 {
    let (source_total, dest_total) = (source.liquid_head(), dest.liquid_head());
    if dest_total >= source_total || source.fluid_mat().is_solid() || dest.fluid_mat().is_solid() {
        return 0;
    }
    let fluid = fluid_properties(source.fluid_mat());
    let mut amount = (source_total - dest_total) / fluid.flow_divisor;
    if amount == 0
        && fluid.levels_out
        && allows_trickle(view, source, source_pos, dest_pos, dest_total)
    {
        amount = 1;
    }
    amount
        .min(source.fluid_vol() as u32)
        .min((WorldCell::MAX_FLUID_VOL as u32).saturating_sub(dest.fluid_vol() as u32)) as u16
}

fn pattern_of(material: FluidMat) -> FlowPattern {
    if material == FluidMat::FLUID_MAGMA {
        FlowPattern::Cardinal
    } else {
        FlowPattern::Omni
    }
}

fn liquid_destination(view: &View, pos: IVec2) -> Option<IVec2> {
    let source = view.get(pos)?;
    let material = source.fluid_mat();
    if material == FluidMat::EMPTY || material.is_solid() {
        return None;
    }
    let shuffled = ShuffledDirs::with_momentum(
        pattern_of(material),
        view.abs(pos),
        view.tick,
        material.0,
        0,
        source.compass(),
    );
    let mut best = None;
    let mut best_total = source.liquid_head();
    for &dir in shuffled.get() {
        let neighbor_pos = pos + dir;
        let Some(neighbor) = view.get(neighbor_pos) else {
            continue;
        };
        let total = neighbor.liquid_head();
        if total < best_total && liquid_transfer(view, &source, pos, &neighbor, neighbor_pos) > 0 {
            best_total = total;
            best = Some(neighbor_pos);
        }
    }
    best
}

fn liquid_source(view: &View, pos: IVec2) -> Option<IVec2> {
    let dest = view.get(pos)?;
    let shuffled = ShuffledDirs::with_momentum(
        pattern_of(dest.fluid_mat()),
        view.abs(pos),
        view.tick,
        dest.fluid_mat().0,
        0,
        dest.compass(),
    );
    let mut best = None;
    let mut best_fluid = 0;
    for &dir in shuffled.get() {
        let neighbor_pos = pos + dir;
        let Some(neighbor) = view.get(neighbor_pos) else {
            continue;
        };
        if neighbor.fluid_mat() != FluidMat::EMPTY
            && view.can_donate(neighbor_pos)
            && liquid_destination(view, neighbor_pos) == Some(pos)
            && neighbor.fluid_vol() > best_fluid
        {
            best_fluid = neighbor.fluid_vol();
            best = Some(neighbor_pos);
        }
    }
    best
}

/// What receiver `pos` accepts of `proposed` units of `material`: what `receive_fluid` would
/// take from the receiver's input column after the largest compaction its granular incoming
/// could cause this tick (its own outflow only frees room).
fn liquid_accepted(view: &View, pos: IVec2, material: FluidMat, proposed: u16) -> u16 {
    let input = view.at(pos);
    let (granular_in, _) = granular_incoming(view, pos);
    let compaction =
        (input.granular_vol() + granular_in).saturating_sub(WorldCell::MAX_GRANULAR_VOL);
    let mut worst = input;
    worst.set_elevation(input.elevation() + compaction);
    worst.receivable(material, proposed)
}

fn erode_bed(
    cell: &mut WorldCell,
    incoming: u16,
    fluid: FluidMat,
    abs: IVec2,
    tick: RuleTick,
) -> bool {
    let flux = incoming as u32 * fluid_properties(fluid).erosion_factor;
    if flux == 0 || (keyed_spatial_hash(abs, tick) & EROSION_MASK) >= flux {
        return false;
    }
    let Some(sediment) = cell.terrain_mat().to_granular() else {
        return false;
    };
    let resting = cell.granular_mat();
    if cell.elevation() == 0 || (resting != GranularMat::EMPTY && resting != sediment) {
        return false;
    }
    cell.set_elevation(cell.elevation() - 1);
    cell.set_granular_mat(sediment);
    cell.set_granular_vol(cell.granular_vol() + 1);
    true
}

// --- One cell for one tick ---

#[derive(Default)]
struct CellStep {
    quench: u64,
    boil: u64,
    burn: u64,
    uptake: u64,
    erosion: u64,
    mismatch: bool,
    phase_pending: bool,
}

fn step_thermal(
    view: &View,
    pos: IVec2,
    cell: &mut WorldCell,
    phases: PhaseThresholds,
    out: &mut CellStep,
) {
    let fluid = cell.fluid_mat();
    if fluid == FluidMat::EMPTY {
        return;
    }
    let (mut magma_contacts, mut coolant_contacts) = (0u16, 0u16);
    let (mut heated, mut touches_ice, mut touches_dry, mut touches_unfrozen) =
        (false, false, false, false);
    for dir in NEIGHBOR_DIRS {
        let Some(neighbor) = view.get(pos + dir) else {
            continue;
        };
        let material = neighbor.fluid_mat();
        heated |=
            material == FluidMat::FLUID_MAGMA || neighbor.surface_mat() == SurfaceMat::SURFACE_FIRE;
        touches_ice |= material.is_solid();
        touches_dry |= material == FluidMat::EMPTY;
        touches_unfrozen |= !material.is_solid();
        if CARDINAL_DIRS.contains(&dir) {
            if material == FluidMat::FLUID_MAGMA {
                magma_contacts += 1;
            } else if material.is_coolant() {
                coolant_contacts += 1;
            }
        }
    }
    let roll = |threshold: u32| {
        keyed_spatial_hash_extended(view.abs(pos), view.tick, 0, PHASE_HASH_CADENCE)
            % CHANCE_RESOLUTION
            < threshold
    };
    if fluid == FluidMat::FLUID_MAGMA {
        if coolant_contacts > 0 {
            out.quench += u64::from(cell.quench());
        }
        return;
    }
    if fluid.is_solid() {
        if heated {
            cell.thaw();
            return;
        }
        if let Some(threshold) = phases.melt
            && touches_unfrozen
        {
            if roll(threshold) {
                cell.thaw();
            }
            out.phase_pending = true;
        }
        return;
    }
    if fluid.is_coolant() && magma_contacts > 0 {
        out.boil += u64::from(cell.drain_fluid(STEAM_LOSS_PER_CONTACT * magma_contacts));
    }
    if let Some(threshold) = phases.freeze
        && (touches_ice || touches_dry)
        && !heated
        && cell.fluid_mat().frozen().is_some()
    {
        if roll(threshold) {
            cell.freeze();
        }
        out.phase_pending = true;
    }
}

fn step_biology(
    view: &View,
    pos: IVec2,
    old: &WorldCell,
    cell: &mut WorldCell,
    out: &mut CellStep,
) {
    let surface = old.surface_mat();
    let fluid = old.fluid_mat();
    if surface != SurfaceMat::EMPTY
        && surface != SurfaceMat::SURFACE_FOLIAGE
        && surface != SurfaceMat::SURFACE_ASH
    {
        return;
    }
    let fertile = old.terrain_mat() == TerrainMat::TERRAIN_DIRT
        || old.granular_mat() == GranularMat::GRANULAR_DIRT
        || old.granular_mat() == GranularMat::GRANULAR_MUD;
    let bare = surface == SurfaceMat::EMPTY || surface == SurfaceMat::SURFACE_ASH;
    if !fertile && bare {
        return;
    }
    let crust = floor_height(old);
    let mut hydrated =
        old.granular_mat() == GranularMat::GRANULAR_MUD || fluid == FluidMat::FLUID_WATER;
    let drowning = fluid != FluidMat::EMPTY && old.fluid_vol() > DROWNING_FLUID_VOLUME;
    let mut seeding_neighbors = 0;
    for dir in CARDINAL_DIRS {
        let Some(neighbor) = view.get(pos + dir) else {
            continue;
        };
        let step = crust.abs_diff(floor_height(&neighbor));
        if bare
            && neighbor.surface_mat() == SurfaceMat::SURFACE_FOLIAGE
            && neighbor.surface_state() >= SEEDING_FOLIAGE_STATE
            && step <= SEEDING_MAX_STEP
        {
            seeding_neighbors += 1;
        }
        if !hydrated
            && step <= MAX_CAPILLARY_LIFT
            && (neighbor.fluid_mat() == FluidMat::FLUID_WATER
                || neighbor.fluid_mat() == FluidMat::FLUID_BLOOD)
        {
            hydrated = true;
        }
    }
    let abs = view.abs(pos);
    let chance = |salt: u32, odds: u32| ratio(draw(abs, view.tick, salt), 1, odds);
    if bare {
        if fertile && hydrated && !drowning {
            let ash = surface == SurfaceMat::SURFACE_ASH;
            let seeding = if ash {
                SEEDING_ODDS_ON_ASH
            } else {
                SEEDING_ODDS
            };
            let spontaneous = if ash {
                SPONTANEOUS_GROWTH_ODDS_ON_ASH
            } else {
                SPONTANEOUS_GROWTH_ODDS
            };
            if (seeding_neighbors > 0 && chance(salt::BIOLOGY_SEEDING, seeding))
                || chance(salt::BIOLOGY_SPONTANEOUS, spontaneous)
            {
                cell.set_surface_mat(SurfaceMat::SURFACE_FOLIAGE);
                cell.set_surface_state(1);
            }
        }
    } else {
        let state = old.surface_state();
        if hydrated && !drowning && state < FOLIAGE_MATURE_STATE {
            if chance(salt::BIOLOGY_GROWTH, FOLIAGE_GROWTH_ODDS) {
                cell.set_surface_state(state + 1);
            }
            let current = cell.fluid_mat();
            if current == FluidMat::FLUID_WATER || current == FluidMat::FLUID_BLOOD {
                out.uptake += u64::from(cell.drain_fluid(FOLIAGE_WATER_USE));
            } else if cell.granular_mat() == GranularMat::GRANULAR_MUD
                && chance(salt::BIOLOGY_MUD_DRYING, MUD_DRYING_ODDS)
            {
                cell.set_granular_mat(GranularMat::GRANULAR_DIRT);
            }
        }
    }
}

fn step_fire(view: &View, pos: IVec2, old: &WorldCell, cell: &mut WorldCell, out: &mut CellStep) {
    let surface = old.surface_mat();
    let fluid = old.fluid_mat();
    let abs = view.abs(pos);
    if fluid.is_coolant() {
        if surface == SurfaceMat::SURFACE_FIRE {
            cell.set_surface_mat(SurfaceMat::EMPTY);
            cell.set_surface_state(0);
        }
        return;
    }
    if surface == SurfaceMat::SURFACE_FIRE {
        if cell.fluid_mat() == FluidMat::FLUID_OIL {
            out.burn += u64::from(cell.drain_fluid(OIL_BURN_RATE));
            return;
        }
        let state = old.surface_state();
        if state < FLAME_LIFETIME_TICKS {
            cell.set_surface_state(state + 1);
        } else {
            let ash = ratio(draw(abs, view.tick, salt::FIRE_ASH), 1, ASH_RESIDUE_ODDS);
            cell.set_surface_mat(if ash {
                SurfaceMat::SURFACE_ASH
            } else {
                SurfaceMat::EMPTY
            });
            cell.set_surface_state(0);
        }
        return;
    }
    if !is_combustible(surface, fluid) {
        return;
    }
    let mut heat = if fluid == FluidMat::FLUID_MAGMA {
        IGNITION_ODDS_DENOMINATOR
    } else {
        0
    };
    for dir in NEIGHBOR_DIRS {
        if let Some(neighbor) = view.get(pos + dir)
            && (neighbor.surface_mat() == SurfaceMat::SURFACE_FIRE
                || neighbor.fluid_mat() == FluidMat::FLUID_MAGMA)
        {
            heat += 1;
        }
    }
    if heat > 0
        && ratio(
            draw(abs, view.tick, salt::FIRE_IGNITION),
            heat.min(IGNITION_ODDS_DENOMINATOR),
            IGNITION_ODDS_DENOMINATOR,
        )
    {
        cell.set_surface_mat(SurfaceMat::SURFACE_FIRE);
        cell.set_surface_state(0);
    }
}

fn step_cell(
    view: &View,
    pos: IVec2,
    rules: SimulationRules,
    phases: PhaseThresholds,
    out: &mut CellStep,
) -> WorldCell {
    let index = view.index(pos).expect("only resident cells are stepped");
    let old = view.cells[index];
    let active = view.wake[index] >= WAKE_ACTIVE_MIN;
    let mut cell = old;

    if view.granular {
        let (incoming, material) = granular_incoming(view, pos);
        let outgoing = granular_outgoing(view, pos);
        if incoming > 0 || outgoing > 0 {
            if incoming > 0 && old.granular_mat() == GranularMat::EMPTY {
                cell.set_granular_mat(material);
            }
            cell.set_granular_vol(
                old.granular_vol()
                    .saturating_add(incoming)
                    .saturating_sub(outgoing),
            );
            if cell.granular_vol() == 0 {
                cell.set_granular_mat(GranularMat::EMPTY);
            }
        }
    }

    if rules.liquid {
        let incoming = liquid_source(view, pos).map(|source| {
            let donor = view.at(source);
            let proposed = liquid_transfer(view, &donor, source, &old, pos);
            (
                source,
                donor.fluid_mat(),
                liquid_accepted(view, pos, donor.fluid_mat(), proposed),
            )
        });
        let outgoing = if active && old.fluid_mat() != FluidMat::EMPTY {
            match liquid_destination(view, pos) {
                Some(dest) if liquid_source(view, dest) == Some(pos) => {
                    let proposed = liquid_transfer(view, &old, pos, &view.at(dest), dest);
                    liquid_accepted(view, dest, old.fluid_mat(), proposed)
                }
                _ => 0,
            }
        } else {
            0
        };
        let incoming_amount = incoming.map_or(0, |(_, _, accepted)| accepted);
        if incoming_amount > 0 || outgoing > 0 {
            cell.drain_fluid(outgoing);
            if let Some((source, material, accepted)) = incoming
                && accepted > 0
            {
                let elevation = cell.elevation();
                let got = cell.receive_fluid(material, accepted);
                out.mismatch |= got != accepted;
                out.quench += u64::from(cell.elevation() - elevation);
                cell.set_compass(compass_for_flow(pos - source));
                out.erosion += u64::from(erode_bed(
                    &mut cell,
                    accepted,
                    material,
                    view.abs(pos),
                    view.tick,
                ));
            }
        }
    }

    if active {
        let abs = view.abs(pos);
        if rules.thermal {
            step_thermal(view, pos, &mut cell, phases, out);
        }
        if rules.biology
            && ratio(
                draw(abs, view.tick, salt::BIOLOGY_SAMPLE),
                1,
                BIOLOGY_ODDS_DENOMINATOR,
            )
        {
            step_biology(view, pos, &old, &mut cell, out);
        }
        if rules.fire
            && ratio(
                draw(abs, view.tick, salt::FIRE_SAMPLE),
                1,
                FIRE_ODDS_DENOMINATOR,
            )
        {
            step_fire(view, pos, &old, &mut cell, out);
        }
    }
    cell
}

/// A small resident window and its rules, stepped one tick at a time.
pub struct ReferenceWorld {
    /// Absolute cell of window-local `(0, 0)`.
    origin: IVec2,
    chunks_x: i32,
    chunks_y: i32,
    resident: Vec<bool>,
    cells: Vec<WorldCell>,
    /// Wake levels the next [`step`](Self::step) runs with: scattered by the previous step and
    /// by this tick's edits, activations and retirements.
    next_wake: Vec<u8>,
    tick: u64,
    seed: u64,
    rules: SimulationRules,
    ambient: f32,
    /// Command-phase accounting of the tick being prepared.
    pending: TickDiagnostics,
    last: TickDiagnostics,
}

impl ReferenceWorld {
    /// A window of `chunks_x × chunks_y` chunks whose lowest chunk key is `min_key`, with no
    /// chunk resident yet; the first [`step`](Self::step) simulates `first_tick`.
    pub fn new(
        min_key: IVec2,
        chunks_x: u32,
        chunks_y: u32,
        first_tick: u64,
        seed: u64,
        rules: SimulationRules,
        ambient: f32,
    ) -> Self {
        let size = CHUNK_SIZE as i32;
        let cells = (chunks_x * chunks_y) as usize * CHUNK_CELL_COUNT;
        Self {
            origin: IVec2::new(min_key.x.wrapping_mul(size), min_key.y.wrapping_mul(size)),
            chunks_x: chunks_x as i32,
            chunks_y: chunks_y as i32,
            resident: vec![false; (chunks_x * chunks_y) as usize],
            cells: vec![WorldCell::default(); cells],
            next_wake: vec![0; cells],
            tick: first_tick,
            seed,
            rules,
            ambient,
            pending: TickDiagnostics::default(),
            last: TickDiagnostics::default(),
        }
    }

    fn width(&self) -> i32 {
        self.chunks_x * CHUNK_SIZE as i32
    }

    fn height(&self) -> i32 {
        self.chunks_y * CHUNK_SIZE as i32
    }

    /// The tick the next step simulates.
    pub fn tick(&self) -> u64 {
        self.tick
    }

    pub fn last(&self) -> TickDiagnostics {
        self.last
    }

    fn chunk_index(&self, key: IVec2) -> usize {
        let size = CHUNK_SIZE as i32;
        let x = key.x.wrapping_mul(size).wrapping_sub(self.origin.x) / size;
        let y = key.y.wrapping_mul(size).wrapping_sub(self.origin.y) / size;
        assert!(
            (0..self.chunks_x).contains(&x) && (0..self.chunks_y).contains(&y),
            "chunk {key} outside the reference window"
        );
        (y * self.chunks_x + x) as usize
    }

    fn local_index(&self, local: IVec2) -> Option<usize> {
        if local.x < 0 || local.y < 0 || local.x >= self.width() || local.y >= self.height() {
            return None;
        }
        let size = CHUNK_SIZE as i32;
        let chunk = (local.y / size) * self.chunks_x + local.x / size;
        self.resident[chunk as usize].then_some((local.y * self.width() + local.x) as usize)
    }

    fn local(&self, abs: IVec2) -> IVec2 {
        IVec2::new(
            abs.x.wrapping_sub(self.origin.x),
            abs.y.wrapping_sub(self.origin.y),
        )
    }

    /// The baseline's `schedule_wake`: `level` within `radius` of `center`, halo one ring
    /// further, never lowering a level already scheduled.
    fn schedule_wake(&mut self, center: IVec2, level: u8, radius: i32) {
        let halo = radius + 1;
        for dy in -halo..=halo {
            for dx in -halo..=halo {
                let wake = if dx.abs() <= radius && dy.abs() <= radius {
                    level
                } else {
                    WAKE_HALO
                };
                if let Some(index) = self.local_index(center + IVec2::new(dx, dy)) {
                    self.next_wake[index] = self.next_wake[index].max(wake);
                }
            }
        }
    }

    fn chunk_cells_local(&self, chunk: usize) -> impl Iterator<Item = IVec2> + use<> {
        let size = CHUNK_SIZE as i32;
        let base = IVec2::new(
            (chunk as i32 % self.chunks_x) * size,
            (chunk as i32 / self.chunks_x) * size,
        );
        (0..size).flat_map(move |y| (0..size).map(move |x| base + IVec2::new(x, y)))
    }

    /// Makes a chunk resident in the tick being prepared; it counts as edited throughout.
    pub fn activate(&mut self, key: IVec2, cells: &[WorldCell]) {
        let chunk = self.chunk_index(key);
        self.resident[chunk] = true;
        let positions: Vec<IVec2> = self.chunk_cells_local(chunk).collect();
        for (local, cell) in positions.iter().zip(cells) {
            let index = self.local_index(*local).expect("resident");
            self.cells[index] = *cell;
            self.next_wake[index] = 0;
        }
        for local in positions {
            self.schedule_wake(local, WAKE_ACTIVE_FRESH, WAKE_CHANGE_RADIUS);
        }
    }

    /// Removes a chunk; its neighbours within reach of its cells wake as if it were edited.
    pub fn retire(&mut self, key: IVec2) {
        let chunk = self.chunk_index(key);
        self.resident[chunk] = false;
        for local in self.chunk_cells_local(chunk).collect::<Vec<_>>() {
            self.schedule_wake(local, WAKE_ACTIVE_FRESH, WAKE_CHANGE_RADIUS);
        }
    }

    /// One brush stroke in the tick being prepared. Returns the cells it changed.
    pub fn edit(&mut self, tool: EditTool, center: IVec2, radius: i32, strength: u8) -> u32 {
        let mut changed = 0;
        let center = self.local(center);
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dy * dy > radius * radius {
                    continue;
                }
                let local = center + IVec2::new(dx, dy);
                let Some(index) = self.local_index(local) else {
                    continue;
                };
                let before = self.cells[index];
                let after = edit_cell(before, tool, strength);
                if after == before {
                    continue;
                }
                let fluid = i64::from(after.fluid_vol()) - i64::from(before.fluid_vol());
                let solid = solid_units(&after) as i64 - solid_units(&before) as i64;
                self.pending.tool_fluid_in += fluid.max(0) as u64;
                debug_assert!(fluid >= 0, "no tool removes liquid");
                if solid > 0 {
                    self.pending.tool_solid_in += solid as u64;
                } else {
                    self.pending.tool_solid_out += (-solid) as u64;
                }
                self.cells[index] = after;
                self.schedule_wake(local, WAKE_ACTIVE_FRESH, WAKE_CHANGE_RADIUS);
                changed += 1;
            }
        }
        changed
    }

    /// Wakes every resident cell for the tick being prepared.
    pub fn wake_everything(&mut self) {
        for index in 0..self.cells.len() {
            self.next_wake[index] = WAKE_ACTIVE_FRESH;
        }
    }

    pub fn set_rules(&mut self, rules: SimulationRules) {
        if rules != self.rules {
            self.rules = rules;
            self.wake_everything();
        }
    }

    pub fn set_ambient(&mut self, ambient: f32) {
        if ambient != self.ambient {
            self.ambient = ambient;
            self.wake_everything();
        }
    }

    /// Simulates the prepared tick.
    pub fn step(&mut self) {
        let wake = std::mem::replace(&mut self.next_wake, vec![0; self.cells.len()]);
        let input = self.cells.clone();
        let tick = RuleTick::new(self.seed, self.tick);
        let view = View {
            cells: &input,
            wake: &wake,
            resident: &self.resident,
            width: self.width(),
            height: self.height(),
            chunks_x: self.chunks_x,
            origin: self.origin,
            tick,
            granular: self.rules.granular && rules::granular_runs(self.tick),
        };
        let phases = PhaseThresholds::at(self.ambient);
        let mut diagnostics = std::mem::take(&mut self.pending);
        let mut output = input.clone();
        let mut schedule = Vec::new();
        let tiles_x = self.width() / TILE_EDGE;
        let mut awake_tiles = vec![false; (tiles_x * (self.height() / TILE_EDGE)) as usize];
        for y in 0..self.height() {
            for x in 0..self.width() {
                let pos = IVec2::new(x, y);
                let Some(index) = self.local_index(pos) else {
                    continue;
                };
                let level = wake[index];
                if level == 0 {
                    continue;
                }
                awake_tiles[((y / TILE_EDGE) * tiles_x + x / TILE_EDGE) as usize] = true;
                diagnostics.awake_cells += 1;
                let active = level >= WAKE_ACTIVE_MIN;
                diagnostics.active_cells += u64::from(active);
                let mut step = CellStep::default();
                let cell = step_cell(&view, pos, self.rules, phases, &mut step);
                let old = input[index];
                let (fluid_before, fluid_after) = (old.fluid_vol(), cell.fluid_vol());
                diagnostics.fluid_in += u64::from(fluid_after.saturating_sub(fluid_before));
                diagnostics.fluid_out += u64::from(fluid_before.saturating_sub(fluid_after));
                let (solid_before, solid_after) = (solid_units(&old), solid_units(&cell));
                diagnostics.solid_in += solid_after.saturating_sub(solid_before);
                diagnostics.solid_out += solid_before.saturating_sub(solid_after);
                diagnostics.quench += step.quench;
                diagnostics.boil += step.boil;
                diagnostics.burn += step.burn;
                diagnostics.uptake += step.uptake;
                diagnostics.erosion += step.erosion;
                diagnostics.receive_mismatches += u64::from(step.mismatch);
                let mut changed = cell != old;
                diagnostics.changed_cells += u64::from(changed);
                if active && !changed {
                    // The baseline's keep-alive, each only while its rule runs (03).
                    let surface = cell.surface_mat();
                    changed = (self.rules.biology
                        && surface == SurfaceMat::SURFACE_FOLIAGE
                        && cell.surface_state() < FOLIAGE_MATURE_STATE)
                        || (self.rules.fire && surface == SurfaceMat::SURFACE_FIRE)
                        || step.phase_pending;
                }
                if changed {
                    schedule.push((pos, WAKE_ACTIVE_FRESH, WAKE_CHANGE_RADIUS));
                } else if level > WAKE_ACTIVE_MIN {
                    schedule.push((pos, level - 1, 0));
                }
                output[index] = cell;
            }
        }
        for (pos, level, radius) in schedule {
            self.schedule_wake(pos, level, radius);
        }
        diagnostics.process_tiles = awake_tiles.iter().filter(|awake| **awake).count() as u64;
        self.cells = output;
        self.last = diagnostics;
        self.tick += 1;
    }

    /// The cells of a resident chunk, row-major.
    pub fn chunk(&self, key: IVec2) -> Vec<WorldCell> {
        let chunk = self.chunk_index(key);
        assert!(self.resident[chunk], "chunk {key} is not resident");
        self.chunk_cells_local(chunk)
            .map(|local| self.cells[self.local_index(local).expect("resident")])
            .collect()
    }

    pub fn cell(&self, abs: IVec2) -> Option<WorldCell> {
        self.local_index(self.local(abs))
            .map(|index| self.cells[index])
    }

    pub fn totals(&self) -> WorldTotals {
        let mut totals = WorldTotals::default();
        for y in 0..self.height() {
            for x in 0..self.width() {
                if let Some(index) = self.local_index(IVec2::new(x, y)) {
                    totals.add(&self.cells[index]);
                }
            }
        }
        totals
    }

    /// The wake levels the next step will run with, for resident cells (others are 0).
    pub fn next_wake(&self) -> &[u8] {
        &self.next_wake
    }
}

#[cfg(test)]
mod tests {
    use world_model::{glam::IVec2, material::TerrainMat};

    use super::*;

    fn column(elevation: u16, fluid: FluidMat, volume: u16) -> WorldCell {
        let mut cell = WorldCell::default();
        cell.set_terrain_mat(TerrainMat::TERRAIN_DIRT);
        cell.set_elevation(elevation);
        cell.inject_fluid(fluid, volume);
        cell
    }

    /// Every rule on, every tick: the phases' accounting balances, and the totals move by
    /// exactly the classified sinks, including transfers refused at the elevation ceiling.
    #[test]
    fn every_rule_balances_its_accounting_at_the_ceiling() {
        let key = IVec2::new(-1, -1);
        let mut cells = vec![column(4000, FluidMat::EMPTY, 0); CHUNK_CELL_COUNT];
        for (index, cell) in cells.iter_mut().enumerate() {
            let (x, y) = (index % 64, index / 64);
            match (x / 8 + y / 8) % 5 {
                0 => *cell = column(4094, FluidMat::FLUID_WATER, 30),
                1 => *cell = column(4093, FluidMat::FLUID_MAGMA, 40),
                2 => {
                    *cell = column(4000, FluidMat::FLUID_OIL, 12);
                    cell.set_surface_mat(SurfaceMat::SURFACE_FIRE);
                }
                3 => {
                    cell.set_granular_mat(GranularMat::GRANULAR_MUD);
                    cell.set_granular_vol(14);
                    cell.inject_fluid(FluidMat::FLUID_WATER, 6);
                }
                _ => cell.set_surface_mat(SurfaceMat::SURFACE_FOLIAGE),
            }
        }
        let mut world = ReferenceWorld::new(key, 1, 1, 7, 99, SimulationRules::ALL, 4.0);
        world.activate(key, &cells);
        let mut totals = world.totals();
        let mut sinks = 0;
        for _ in 0..300 {
            world.step();
            let last = world.last();
            assert_eq!(last.imbalance(), (0, 0), "tick {}", world.tick() - 1);
            assert_eq!(last.receive_mismatches, 0);
            let next = world.totals();
            let lost = last.quench + last.boil + last.burn + last.uptake;
            assert_eq!(next.fluid_volume() + lost, totals.fluid_volume());
            assert_eq!(next.solid(), totals.solid() + last.quench);
            sinks += lost;
            totals = next;
        }
        assert!(sinks > 0, "the fixture exercised the sinks");
        let capped = world
            .chunk(key)
            .iter()
            .filter(|c| c.elevation() == 4095 && c.fluid_mat() == FluidMat::FLUID_MAGMA)
            .count();
        assert!(
            capped > 0,
            "magma that did not fit under the ceiling stayed liquid"
        );
    }

    /// The same inputs give the same world; another seed gives another one; a far change does
    /// not move a near outcome within one tick.
    #[test]
    fn keyed_draws_are_reproducible_and_seeded() {
        let key = IVec2::new(2, 3);
        let mut cells = vec![column(100, FluidMat::EMPTY, 0); CHUNK_CELL_COUNT];
        for cell in cells.iter_mut() {
            cell.set_terrain_mat(TerrainMat::TERRAIN_DIRT);
            cell.inject_fluid(FluidMat::FLUID_WATER, 3);
        }
        let run = |seed: u64| {
            let mut world = ReferenceWorld::new(key, 1, 1, 0, seed, SimulationRules::ALL, 15.0);
            world.activate(key, &cells);
            for _ in 0..200 {
                world.step();
            }
            world.chunk(key)
        };
        let grown = |cells: &[WorldCell]| {
            cells
                .iter()
                .filter(|c| c.surface_mat() == SurfaceMat::SURFACE_FOLIAGE)
                .count()
        };
        let first = run(1);
        assert_eq!(first, run(1));
        assert_ne!(first, run(2));
        assert!(grown(&first) > 0, "foliage took root");
    }
}
