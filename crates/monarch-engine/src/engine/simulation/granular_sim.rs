use bevy::math::IVec2;

use crate::engine::{
    physics::materials::get_granular_repose,
    utils::{FlowPattern, ShuffledDirs, spatial_hash},
    world::{
        cell::{GranularMat, WorldCell},
        grid::GridReadView,
    },
};

/// Calculates the exact volume of granular material to transfer between two cells.
#[inline(always)]
fn calc_granular_transfer(
    source_cell: &WorldCell,
    dest_cell: &WorldCell,
    world_pos: IVec2,
    tick: u32,
) -> u16 {
    let source_total = source_cell.elevation() as u32 + source_cell.granular_vol() as u32;
    let dest_total = dest_cell.elevation() as u32 + dest_cell.granular_vol() as u32;

    if dest_total >= source_total {
        return 0;
    }

    let diff = source_total - dest_total;
    let mut amount = diff / 2;

    // Micro-sloshing: Prevents perfect pyramids by probabilistically
    // transferring remainder units when integer division halts flow.
    if amount == 0 && diff >= 1 {
        if spatial_hash(world_pos, tick) % 2 == 0 {
            amount = 1;
        }
    }

    let source_vol = source_cell.granular_vol() as u32;
    let dest_vol = dest_cell.granular_vol() as u32;

    amount = amount.min(source_vol);
    amount = amount.min((WorldCell::MAX_GRANULAR_VOL as u32).saturating_sub(dest_vol));

    amount as u16
}

/// Determines the best neighboring destination for granular material to fall into.
#[inline(always)]
fn get_preferred_destination(
    world_pos: IVec2,
    view: GridReadView,
    tick: u32,
) -> Option<(usize, IVec2)> {
    let (_, source_cell) = view.get_cell(world_pos)?;
    let source_mat = source_cell.granular_mat();
    let repose = get_granular_repose(source_mat);

    if repose == u16::MAX || source_cell.granular_vol() == 0 {
        return None;
    }

    let shuffled =
        ShuffledDirs::new_deterministic(FlowPattern::Omni, world_pos, tick, source_mat.0, 0);
    let source_total = source_cell.elevation() as u32 + source_cell.granular_vol() as u32;

    let mut best_dest = None;
    let mut best_diff = 0;

    for &dir in shuffled.get().iter() {
        let neighbor_pos = world_pos + dir;

        if let Some((neighbor_idx, neighbor_cell)) = view.get_cell(neighbor_pos) {
            let neighbor_total =
                neighbor_cell.elevation() as u32 + neighbor_cell.granular_vol() as u32;

            // Validate the angle of repose is exceeded
            if source_total.saturating_sub(neighbor_total) > repose as u32 {
                let diff = source_total - neighbor_total;

                // Only flow into identical materials or empty granular slots
                let is_empty = neighbor_cell.granular_mat() == GranularMat::EMPTY;
                let is_same = neighbor_cell.granular_mat() == source_mat;

                if (is_empty || is_same) && diff > best_diff {
                    best_diff = diff;
                    best_dest = Some((neighbor_idx, neighbor_pos));
                }
            }
        }
    }

    best_dest
}

/// Determines the best neighboring source that wishes to drop granular material here.
#[inline(always)]
fn get_preferred_source(world_pos: IVec2, view: GridReadView, tick: u32) -> Option<(usize, IVec2)> {
    let shuffled = ShuffledDirs::new_deterministic(FlowPattern::Omni, world_pos, tick, 0, 0);

    let mut best_source = None;
    let mut best_diff = 0;

    for &dir in shuffled.get().iter() {
        let neighbor_pos = world_pos + dir;

        if let Some((neighbor_idx, neighbor_cell)) = view.get_cell(neighbor_pos) {
            if get_granular_repose(neighbor_cell.granular_mat()) != u16::MAX {
                if let Some((_, dest_pos)) = get_preferred_destination(neighbor_pos, view, tick) {
                    if dest_pos == world_pos {
                        let (_, dest_cell) = view.get_cell(world_pos)?;
                        let neighbor_total =
                            neighbor_cell.elevation() as u32 + neighbor_cell.granular_vol() as u32;
                        let dest_total =
                            dest_cell.elevation() as u32 + dest_cell.granular_vol() as u32;

                        let diff = neighbor_total.saturating_sub(dest_total);
                        if diff > best_diff {
                            best_diff = diff;
                            best_source = Some((neighbor_idx, neighbor_pos));
                        }
                    }
                }
            }
        }
    }

    best_source
}

/// Executes a single simulation step for granular physics.
#[inline(always)]
pub fn step_granular(
    cell: &mut WorldCell,
    old_cell: &WorldCell,
    view: GridReadView,
    world_pos: IVec2,
    tick: u32,
) {
    let old_vol = old_cell.granular_vol();
    let mut incoming_amt = 0;
    let mut incoming_mat = GranularMat::EMPTY;
    let mut outgoing_amt = 0;

    // Receive granular matter from a steeper neighbor
    if let Some((source_idx, source_pos)) = get_preferred_source(world_pos, view, tick) {
        let source_cell = &view.cells[source_idx];
        incoming_amt = calc_granular_transfer(source_cell, old_cell, source_pos, tick);
        incoming_mat = source_cell.granular_mat();
    }

    // Donate granular matter to a lower neighbor
    if old_cell.granular_mat() != GranularMat::EMPTY {
        if let Some((_, dest_pos)) = get_preferred_destination(world_pos, view, tick) {
            // Verify we are the destination's primary source to prevent race conditions
            if let Some((_, winner_pos)) = get_preferred_source(dest_pos, view, tick) {
                if winner_pos == world_pos {
                    if let Some((dest_idx, _)) = view.get_cell(dest_pos) {
                        let dest_cell = &view.cells[dest_idx];
                        outgoing_amt = calc_granular_transfer(old_cell, dest_cell, world_pos, tick);
                    }
                }
            }
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
