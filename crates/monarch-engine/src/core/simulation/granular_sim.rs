use bevy::math::IVec2;

use crate::core::{
    physics::materials::get_granular_repose,
    simulation::DonorMask,
    utils::{FlowPattern, ShuffledDirs, spatial_hash},
    world::{
        cell::{GranularMat, WorldCell},
        grid::CellGridReadView,
    },
};

/// Upper bound on granular volume moved across one edge per tick; keeps avalanches gradual.
const MAX_GRANULAR_TRANSFER: u32 = 2;
/// Smallest head difference that moves a whole unit (`diff / 2 >= 1`). Below it a slope is
/// left at rest rather than dithered, so piles reach a fixed point.
const MIN_TRANSFER_HEAD: u32 = 2;
/// Range of the per-tick stiction added to the angle of repose.
const STICTION_VARIANTS: u32 = 3;

#[inline(always)]
fn floor_height(cell: &WorldCell) -> u32 {
    cell.elevation() as u32 + cell.granular_vol() as u32
}

/// Remaining room in a column before it hits the elevation ceiling.
#[inline(always)]
fn structural_void(cell: &WorldCell) -> u32 {
    ((WorldCell::MAX_ELEVATION).saturating_sub(cell.elevation() as u32))
        + ((WorldCell::MAX_GRANULAR_VOL as u32).saturating_sub(cell.granular_vol() as u32))
}

/// Calculates the exact volume of granular material to transfer between two cells.
#[inline(always)]
fn calc_granular_transfer(source_cell: &WorldCell, dest_cell: &WorldCell) -> u16 {
    let source_total = floor_height(source_cell);
    let dest_total = floor_height(dest_cell);

    if dest_total >= source_total {
        return 0;
    }

    let amount = ((source_total - dest_total) / 2)
        .min(MAX_GRANULAR_TRANSFER)
        .min(source_cell.granular_vol() as u32)
        .min(structural_void(dest_cell));

    amount as u16
}

/// Determines the best neighboring destination for granular material to fall into.
#[inline(always)]
fn get_preferred_destination(
    world_pos: IVec2,
    view: CellGridReadView,
    tick: u32,
) -> Option<(usize, IVec2)> {
    let (_, source_cell) = view.get_cell(world_pos)?;
    let source_mat = source_cell.granular_mat();
    let base_repose = get_granular_repose(source_mat);

    // Loose material under ice is frozen in place.
    if base_repose == u16::MAX
        || source_cell.granular_vol() == 0
        || source_cell.fluid_mat().is_solid()
    {
        return None;
    }

    let shuffled =
        ShuffledDirs::new_deterministic(FlowPattern::Omni, world_pos, tick, source_mat.0, 0);
    let source_total = floor_height(source_cell);

    let mut best_dest = None;
    let mut best_diff = 0;

    let stiction = spatial_hash(world_pos, tick) % STICTION_VARIANTS;
    let dynamic_repose = base_repose as u32 + stiction;

    for &dir in shuffled.get().iter() {
        let neighbor_pos = world_pos + dir;

        if let Some((neighbor_idx, neighbor_cell)) = view.get_cell(neighbor_pos) {
            let diff = source_total.saturating_sub(floor_height(neighbor_cell));

            if diff > dynamic_repose && diff >= MIN_TRANSFER_HEAD {
                let is_empty = neighbor_cell.granular_mat() == GranularMat::EMPTY;
                let is_same = neighbor_cell.granular_mat() == source_mat;
                // Nothing slides underneath an ice sheet.
                let has_room =
                    structural_void(neighbor_cell) > 0 && !neighbor_cell.fluid_mat().is_solid();

                if (is_empty || is_same) && has_room && diff > best_diff {
                    best_diff = diff;
                    best_dest = Some((neighbor_idx, neighbor_pos));
                }
            }
        }
    }

    best_dest
}

/// Determines the best neighboring source that wishes to drop granular material here.
/// Only cells allowed to donate this tick compete, so the winner always executes its side.
#[inline(always)]
fn get_preferred_source(
    world_pos: IVec2,
    view: CellGridReadView,
    donors: DonorMask,
    tick: u32,
) -> Option<(usize, IVec2)> {
    let (_, dest_cell) = view.get_cell(world_pos)?;
    let dest_total = floor_height(dest_cell);
    let shuffled = ShuffledDirs::new_deterministic(FlowPattern::Omni, world_pos, tick, 0, 0);

    let mut best_source = None;
    let mut best_diff = 0;

    for &dir in shuffled.get().iter() {
        let neighbor_pos = world_pos + dir;

        if let Some((neighbor_idx, neighbor_cell)) = view.get_cell(neighbor_pos)
            && get_granular_repose(neighbor_cell.granular_mat()) != u16::MAX
            && donors.can_donate(neighbor_idx)
            && let Some((_, dest_pos)) = get_preferred_destination(neighbor_pos, view, tick)
            && dest_pos == world_pos
        {
            let diff = floor_height(neighbor_cell).saturating_sub(dest_total);
            if diff > best_diff {
                best_diff = diff;
                best_source = Some((neighbor_idx, neighbor_pos));
            }
        }
    }

    best_source
}

/// Executes a single simulation step for granular physics.
/// `can_donate` is false for halo cells, which only complete transfers from active neighbours.
#[inline(always)]
pub fn step_granular(
    cell: &mut WorldCell,
    old_cell: &WorldCell,
    view: CellGridReadView,
    donors: DonorMask,
    can_donate: bool,
    world_pos: IVec2,
    tick: u32,
) {
    let old_vol = old_cell.granular_vol();
    let mut incoming_amt = 0;
    let mut incoming_mat = GranularMat::EMPTY;
    let mut outgoing_amt = 0;

    // Receive granular matter from a steeper neighbor
    if let Some((source_idx, _)) = get_preferred_source(world_pos, view, donors, tick) {
        let source_cell = &view.cells[source_idx];
        incoming_amt = calc_granular_transfer(source_cell, old_cell);
        incoming_mat = source_cell.granular_mat();
    }

    // Donate granular matter to a lower neighbor
    if can_donate
        && old_cell.granular_mat() != GranularMat::EMPTY
        && let Some((dest_idx, dest_pos)) = get_preferred_destination(world_pos, view, tick)
    {
        // Verify we are the destination's primary source to prevent race conditions
        if let Some((_, winner_pos)) = get_preferred_source(dest_pos, view, donors, tick)
            && winner_pos == world_pos
        {
            outgoing_amt = calc_granular_transfer(old_cell, &view.cells[dest_idx]);
        }
    }

    if incoming_amt == 0 && outgoing_amt == 0 {
        return;
    }

    // Apply material transmutations if receiving into an empty cell
    if incoming_amt > 0 && old_cell.granular_mat() == GranularMat::EMPTY {
        cell.set_granular_mat(incoming_mat);
    }

    // Apply exact volume differentials
    cell.set_granular_vol(
        old_vol
            .saturating_add(incoming_amt)
            .saturating_sub(outgoing_amt),
    );

    // Clean up completely depleted granular columns
    if cell.granular_vol() == 0 {
        cell.set_granular_mat(GranularMat::EMPTY);
    }
}
