use crate::engine::{
    utils::{FlowPattern, ShuffledDirs, spatial_hash},
    world::{
        cell::{CompassFlags, FluidMat, WorldCell},
        grid::GridReadView,
    },
};
use bevy::math::IVec2;

pub const EROSION_MASK: u32 = 511;

/// Calculates the exact volume of fluid to transfer between two cells,
/// accounting for the absolute floor elevation (terrain + granular).
#[inline(always)]
fn calc_liquid_transfer(
    source_cell: &WorldCell,
    dest_cell: &WorldCell,
    world_pos: IVec2,
    tick: u32,
) -> u16 {
    // The physical floor beneath the liquid includes solid terrain and granular volumes
    let source_floor = source_cell.elevation() as u32 + source_cell.granular_vol() as u32;
    let dest_floor = dest_cell.elevation() as u32 + dest_cell.granular_vol() as u32;

    let source_fluid = source_cell.fluid_vol() as u32;
    let dest_fluid = dest_cell.fluid_vol() as u32;

    let source_total = source_floor + source_fluid;
    let dest_total = dest_floor + dest_fluid;

    if dest_total >= source_total {
        return 0;
    }

    let diff = source_total - dest_total;
    let mut amount = diff / 2;

    // Micro-sloshing: Ensure liquids settle completely flat by bypassing integer truncation
    if amount == 0 && diff >= 1 {
        if spatial_hash(world_pos, tick) % 2 == 0 {
            amount = 1;
        }
    }

    amount = amount.min(source_fluid);
    amount = amount.min((WorldCell::MAX_FLUID_VOL as u32).saturating_sub(dest_fluid));

    amount as u16
}

/// Determines the best neighboring destination for liquid to flow into.
#[inline(always)]
fn get_preferred_destination(
    world_pos: IVec2,
    view: GridReadView,
    tick: u32,
) -> Option<(usize, IVec2)> {
    let (_, source_cell) = view.get_cell(world_pos)?;
    let source_mat = source_cell.fluid_mat();

    if source_mat == FluidMat::EMPTY {
        return None;
    }

    let pattern = if source_mat == FluidMat::FLUID_MAGMA {
        FlowPattern::Cardinal
    } else {
        FlowPattern::Omni
    };

    let shuffled = ShuffledDirs::new_deterministic_with_momentum(
        pattern,
        world_pos,
        tick,
        source_mat.0,
        0,
        source_cell.compass(),
    );

    let source_total = source_cell.elevation() as u32
        + source_cell.granular_vol() as u32
        + source_cell.fluid_vol() as u32;

    let mut best_dest = None;
    let mut best_total = source_total;

    for &dir in shuffled.get().iter() {
        let neighbor_pos = world_pos + dir;

        if let Some((neighbor_idx, neighbor_cell)) = view.get_cell(neighbor_pos) {
            let neighbor_total = neighbor_cell.elevation() as u32
                + neighbor_cell.granular_vol() as u32
                + neighbor_cell.fluid_vol() as u32;

            if neighbor_total < source_total {
                let is_empty = neighbor_cell.fluid_mat() == FluidMat::EMPTY;
                let is_same = neighbor_cell.fluid_mat() == source_mat;
                let can_overtake =
                    !is_empty && !is_same && source_cell.fluid_vol() > neighbor_cell.fluid_vol();

                if is_empty || is_same || can_overtake {
                    if neighbor_total < best_total {
                        best_total = neighbor_total;
                        best_dest = Some((neighbor_idx, neighbor_pos));
                    }
                }
            }
        }
    }

    best_dest
}

/// Determines the highest-volume neighboring source that wishes to flow into this cell.
#[inline(always)]
fn get_preferred_source(world_pos: IVec2, view: GridReadView, tick: u32) -> Option<(usize, IVec2)> {
    let (_, dest_cell) = view.get_cell(world_pos)?;

    let pattern = if dest_cell.fluid_mat() == FluidMat::FLUID_MAGMA {
        FlowPattern::Cardinal
    } else {
        FlowPattern::Omni
    };

    let shuffled = ShuffledDirs::new_deterministic_with_momentum(
        pattern,
        world_pos,
        tick,
        dest_cell.fluid_mat().0,
        0,
        dest_cell.compass(),
    );

    let mut best_source = None;
    let mut best_fluid = 0;

    for &dir in shuffled.get().iter() {
        let neighbor_pos = world_pos + dir;

        if let Some((neighbor_idx, neighbor_cell)) = view.get_cell(neighbor_pos) {
            if neighbor_cell.fluid_mat() != FluidMat::EMPTY {
                if let Some((_, pref_pos)) = get_preferred_destination(neighbor_pos, view, tick) {
                    if pref_pos == world_pos {
                        if neighbor_cell.fluid_vol() > best_fluid {
                            best_fluid = neighbor_cell.fluid_vol();
                            best_source = Some((neighbor_idx, neighbor_pos));
                        }
                    }
                }
            }
        }
    }

    best_source
}

/// Executes a single simulation step for liquid physics.
#[inline(always)]
pub fn step_liquid(
    cell: &mut WorldCell,
    old_cell: &WorldCell,
    view: GridReadView,
    world_pos: IVec2,
    tick: u32,
) {
    let old_fluid = old_cell.fluid_vol();
    let mut old_elev = old_cell.elevation();

    let mut incoming_amt = 0;
    let mut incoming_mat = FluidMat::EMPTY;
    let mut outgoing_amt = 0;
    let mut source_pos_cache = None;

    // Receive liquid from the optimal incoming source
    if let Some((source_idx, source_pos)) = get_preferred_source(world_pos, view, tick) {
        source_pos_cache = Some(source_pos);
        let source_cell = &view.cells[source_idx];

        incoming_amt = calc_liquid_transfer(source_cell, old_cell, source_pos, tick);
        incoming_mat = source_cell.fluid_mat();
    }

    // Donate liquid to the optimal outgoing destination
    if old_cell.fluid_mat() != FluidMat::EMPTY {
        if let Some((_, dest_pos)) = get_preferred_destination(world_pos, view, tick) {
            // Verify we are the destination's optimal source
            if let Some((_, winner_pos)) = get_preferred_source(dest_pos, view, tick) {
                if winner_pos == world_pos {
                    if let Some((dest_idx, _)) = view.get_cell(dest_pos) {
                        let dest_cell = &view.cells[dest_idx];
                        outgoing_amt = calc_liquid_transfer(old_cell, dest_cell, world_pos, tick);
                    }
                }
            }
        }
    }

    if incoming_amt == 0 && outgoing_amt == 0 {
        return;
    }

    // Apply material transmutations if receiving into an empty cell
    if incoming_amt > 0 {
        if old_cell.fluid_mat() == FluidMat::EMPTY || old_cell.fluid_mat() != incoming_mat {
            cell.set_fluid_mat(incoming_mat);
        }

        // Apply hydraulic momentum based on flow direction
        if let Some(sp) = source_pos_cache {
            let flow_dir = world_pos - sp;
            let mut new_flags = 0;
            if flow_dir.y > 0 {
                new_flags |= CompassFlags::FACING_N;
            }
            if flow_dir.y < 0 {
                new_flags |= CompassFlags::FACING_S;
            }
            if flow_dir.x > 0 {
                new_flags |= CompassFlags::FACING_E;
            }
            if flow_dir.x < 0 {
                new_flags |= CompassFlags::FACING_W;
            }
            cell.set_compass(new_flags);
        }

        // Apply rare erosion to solid terrain directly beneath heavy flow
        if spatial_hash(world_pos, tick) & EROSION_MASK == 0 {
            old_elev = old_elev.saturating_sub(1);
        }
    }

    // Apply exact volume differentials
    cell.set_fluid_vol(
        old_fluid
            .saturating_add(incoming_amt)
            .saturating_sub(outgoing_amt),
    );
    cell.set_elevation(old_elev);

    // Clean up completely depleted liquid columns
    if cell.fluid_vol() == 0 {
        cell.set_fluid_mat(FluidMat::EMPTY);
        cell.set_compass(0);
    }
}
