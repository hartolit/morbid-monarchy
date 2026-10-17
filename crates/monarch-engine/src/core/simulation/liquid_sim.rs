use crate::core::{simulation::DonorMask, world::grid::CellGridReadView};
use bevy::math::IVec2;
use world_model::{
    cell::WorldCell,
    material::{FluidMat, GranularMat, fluid_properties},
    rules::{
        EROSION_MASK, FlowPattern, NEIGHBOR_DIRS, RuleTick, ShuffledDirs, compass_for_flow,
        compass_forward, keyed_spatial_hash,
    },
};

/// Height of a column's free surface: bedrock, loose material and liquid. Ice counts towards
/// it too, so a frozen column is a raised floor to its liquid neighbours.
#[inline(always)]
fn column_head(cell: &WorldCell) -> u32 {
    cell.liquid_head()
}

/// Whether a column has a neighbour whose head is strictly below `head`, i.e. a unit dropped
/// into it can keep descending.
#[inline(always)]
fn drains_lower(view: CellGridReadView, world_pos: IVec2, head: u32) -> bool {
    NEIGHBOR_DIRS.iter().any(|&dir| {
        view.get_cell(world_pos + dir)
            .is_some_and(|(_, neighbor)| column_head(neighbor) < head)
    })
}

/// Whether a single unit may cross a head difference too small to divide.
///
/// Integer division strands such steps, which on their own freeze a surface into terraces.
/// A unit may step down when the receiver can pass it on to something lower, or, keeping the
/// momentum the source already carries, when the cell beyond the receiver is no higher. The
/// momentum rule carries units straight across flat terraces of any width until they find a
/// drop. A unit that meets a bank stops, and a cell without momentum only steps where it can
/// drain, so a level pool reaches a fixed point and falls asleep instead of dithering forever.
#[inline(always)]
fn allows_trickle(
    source_cell: &WorldCell,
    source_pos: IVec2,
    dest_pos: IVec2,
    dest_total: u32,
    view: CellGridReadView,
) -> bool {
    if drains_lower(view, dest_pos, dest_total) {
        return true;
    }

    let direction = dest_pos - source_pos;
    compass_forward(source_cell.compass()) == direction
        && view
            .get_cell(dest_pos + direction)
            .is_some_and(|(_, beyond)| column_head(beyond) <= dest_total)
}

/// Calculates the exact volume of fluid to transfer from `source_cell` at `source_pos` into
/// `dest_cell` at `dest_pos`.
///
/// Both the donor and the receiver evaluate this with identical inputs from the read-only
/// back buffer, which is what keeps the pull-based exchange mass-conserving.
#[inline(always)]
fn calc_liquid_transfer(
    source_cell: &WorldCell,
    source_pos: IVec2,
    dest_cell: &WorldCell,
    dest_pos: IVec2,
    view: CellGridReadView,
) -> u16 {
    let source_total = column_head(source_cell);
    let dest_total = column_head(dest_cell);

    if dest_total >= source_total
        || source_cell.fluid_mat().is_solid()
        || dest_cell.fluid_mat().is_solid()
    {
        return 0;
    }

    let fluid = fluid_properties(source_cell.fluid_mat());
    let mut amount = (source_total - dest_total) / fluid.flow_divisor;
    if amount == 0
        && fluid.levels_out
        && allows_trickle(source_cell, source_pos, dest_pos, dest_total, view)
    {
        amount = 1;
    }

    amount
        .min(source_cell.fluid_vol() as u32)
        .min((WorldCell::MAX_FLUID_VOL as u32).saturating_sub(dest_cell.fluid_vol() as u32))
        as u16
}

/// Determines the best neighboring destination for liquid to flow into.
///
/// Only neighbors that would receive a non-zero transfer qualify. A donor that claims a
/// destination it cannot actually move fluid into would still win arbitration in
/// `get_preferred_source` and starve every other donor bordering that cell.
#[inline(always)]
fn get_preferred_destination(
    world_pos: IVec2,
    view: CellGridReadView,
    tick: u32,
) -> Option<(usize, IVec2)> {
    let (_, source_cell) = view.get_cell(world_pos)?;
    let source_mat = source_cell.fluid_mat();

    if source_mat == FluidMat::EMPTY || source_mat.is_solid() {
        return None;
    }

    let pattern = if source_mat == FluidMat::FLUID_MAGMA {
        FlowPattern::Cardinal
    } else {
        FlowPattern::Omni
    };

    let shuffled = ShuffledDirs::with_momentum(
        pattern,
        world_pos,
        RuleTick::baseline(tick),
        source_mat.0,
        0,
        source_cell.compass(),
    );

    let source_total = column_head(source_cell);

    let mut best_dest = None;
    let mut best_total = source_total;

    for &dir in shuffled.get().iter() {
        let neighbor_pos = world_pos + dir;

        if let Some((neighbor_idx, neighbor_cell)) = view.get_cell(neighbor_pos) {
            let neighbor_total = column_head(neighbor_cell);

            // Any non-solid column with head below the source qualifies, whatever liquid it
            // holds; `WorldCell::receive_fluid` decides how two different liquids combine.
            if neighbor_total < best_total
                && calc_liquid_transfer(source_cell, world_pos, neighbor_cell, neighbor_pos, view)
                    > 0
            {
                best_total = neighbor_total;
                best_dest = Some((neighbor_idx, neighbor_pos));
            }
        }
    }

    best_dest
}

/// Determines the highest-volume neighboring source that wishes to flow into this cell.
/// Only cells allowed to donate this tick compete, so the winner always executes its side.
#[inline(always)]
fn get_preferred_source(
    world_pos: IVec2,
    view: CellGridReadView,
    donors: DonorMask,
    tick: u32,
) -> Option<(usize, IVec2)> {
    let (_, dest_cell) = view.get_cell(world_pos)?;

    let pattern = if dest_cell.fluid_mat() == FluidMat::FLUID_MAGMA {
        FlowPattern::Cardinal
    } else {
        FlowPattern::Omni
    };

    let shuffled = ShuffledDirs::with_momentum(
        pattern,
        world_pos,
        RuleTick::baseline(tick),
        dest_cell.fluid_mat().0,
        0,
        dest_cell.compass(),
    );

    let mut best_source = None;
    let mut best_fluid = 0;

    for &dir in shuffled.get().iter() {
        let neighbor_pos = world_pos + dir;

        if let Some((neighbor_idx, neighbor_cell)) = view.get_cell(neighbor_pos)
            && neighbor_cell.fluid_mat() != FluidMat::EMPTY
            && donors.can_donate(neighbor_idx)
            && let Some((_, pref_pos)) = get_preferred_destination(neighbor_pos, view, tick)
            && pref_pos == world_pos
            && neighbor_cell.fluid_vol() > best_fluid
        {
            best_fluid = neighbor_cell.fluid_vol();
            best_source = Some((neighbor_idx, neighbor_pos));
        }
    }

    best_source
}

/// Scours the bed beneath incoming flow.
///
/// The odds scale with the incoming flux, so still water never erodes while fast streams
/// cut channels. The removed rock is deposited in place as its loose equivalent rather than
/// deleted, keeping the solid mass of the column constant. An existing layer of a different
/// granular material armours the bed and blocks erosion.
#[inline(always)]
fn erode_bed(
    cell: &mut WorldCell,
    incoming_amt: u16,
    fluid: FluidMat,
    world_pos: IVec2,
    tick: u32,
) {
    let erosive_flux = incoming_amt as u32 * fluid_properties(fluid).erosion_factor;
    if erosive_flux == 0
        || (keyed_spatial_hash(world_pos, RuleTick::baseline(tick)) & EROSION_MASK) >= erosive_flux
    {
        return;
    }

    let Some(sediment) = cell.terrain_mat().to_granular() else {
        return;
    };

    let resting_granular = cell.granular_mat();
    if cell.elevation() == 0
        || (resting_granular != GranularMat::EMPTY && resting_granular != sediment)
    {
        return;
    }

    cell.set_elevation(cell.elevation() - 1);
    cell.set_granular_mat(sediment);
    cell.set_granular_vol(cell.granular_vol() + 1);
}

/// Executes a single simulation step for liquid physics.
/// `can_donate` is false for halo cells, which only complete transfers from active neighbours.
#[inline(always)]
pub fn step_liquid(
    cell: &mut WorldCell,
    old_cell: &WorldCell,
    view: CellGridReadView,
    donors: DonorMask,
    can_donate: bool,
    world_pos: IVec2,
    tick: u32,
) {
    let mut incoming_amt = 0;
    let mut incoming_mat = FluidMat::EMPTY;
    let mut outgoing_amt = 0;
    let mut source_pos_cache = None;

    // Receive liquid from the optimal incoming source
    if let Some((source_idx, source_pos)) = get_preferred_source(world_pos, view, donors, tick) {
        source_pos_cache = Some(source_pos);
        let source_cell = &view.cells[source_idx];

        incoming_amt = calc_liquid_transfer(source_cell, source_pos, old_cell, world_pos, view);
        incoming_mat = source_cell.fluid_mat();
    }

    // Donate liquid to the optimal outgoing destination
    if can_donate
        && old_cell.fluid_mat() != FluidMat::EMPTY
        && let Some((dest_idx, dest_pos)) = get_preferred_destination(world_pos, view, tick)
        && let Some((_, winner_pos)) = get_preferred_source(dest_pos, view, donors, tick)
        && winner_pos == world_pos
    {
        outgoing_amt =
            calc_liquid_transfer(old_cell, world_pos, &view.cells[dest_idx], dest_pos, view);
    }

    if incoming_amt == 0 && outgoing_amt == 0 {
        return;
    }

    // Outflow leaves first, so a column whose resident liquid drained away entirely hands
    // over to the incoming liquid instead of being judged by volume it no longer holds.
    cell.drain_fluid(outgoing_amt);

    if incoming_amt > 0 {
        cell.receive_fluid(incoming_mat, incoming_amt);

        // Apply momentum based on flow direction
        if let Some(sp) = source_pos_cache {
            cell.set_compass(compass_for_flow(world_pos - sp));
        }

        // Elevation is read from `cell`, not `old_cell`: the granular pass may already have
        // compressed overflow into bedrock this tick, and overwriting it would delete that mass.
        erode_bed(cell, incoming_amt, incoming_mat, world_pos, tick);
    }
}
