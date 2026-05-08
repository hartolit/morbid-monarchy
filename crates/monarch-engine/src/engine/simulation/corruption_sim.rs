use bevy::math::IVec2;

use crate::engine::{
    utils::{FlowPattern, ShuffledDirs},
    world::{
        cell::{TerrainMat, WorldCell},
        grid::GridReadView,
    },
};

#[inline(always)]
fn get_angle_of_repose(mat: TerrainMat) -> u16 {
    match mat {
        TerrainMat::TERRAIN_SANDSTONE => 2,
        TerrainMat::TERRAIN_ICE => 3,
        TerrainMat::GRAVEL => 4,
        _ => u16::MAX, // Non-granular, will not flow
    }
}

#[inline(always)]
fn get_granular_dest(world_pos: IVec2, view: GridReadView, tick: u32) -> Option<(usize, IVec2)> {
    let (_, s_cell) = view.get_cell(world_pos)?;
    let s_mat = s_cell.terrain_mat();
    let repose = get_angle_of_repose(s_mat);

    if repose == u16::MAX || s_cell.elevation() == 0 {
        return None;
    }

    // Sand/Gravel flows mostly omni-directionally
    let shuffled = ShuffledDirs::new_deterministic(FlowPattern::Omni, world_pos, tick, s_mat.0, 0);

    let mut best_dest = None;
    let mut best_elev = s_cell.elevation();

    for &dir in shuffled.get().iter() {
        let n_pos = world_pos + dir;

        if let Some((n_idx, n_cell)) = view.get_cell(n_pos) {
            // Angle of repose condition based purely on solid terrain elevation
            if s_cell.elevation().saturating_sub(n_cell.elevation()) > repose {
                if n_cell.elevation() < best_elev {
                    best_elev = n_cell.elevation();
                    best_dest = Some((n_idx, n_pos));
                }
            }
        }
    }
    best_dest
}

#[inline(always)]
fn get_granular_source(world_pos: IVec2, view: GridReadView, tick: u32) -> Option<(usize, IVec2)> {
    let shuffled = ShuffledDirs::new_deterministic(FlowPattern::Omni, world_pos, tick, 0, 0);

    let mut best_source = None;
    let mut best_elev = 0;

    for &dir in shuffled.get().iter() {
        let s_pos = world_pos + dir;

        if let Some((s_idx, s_cell)) = view.get_cell(s_pos) {
            if get_angle_of_repose(s_cell.terrain_mat()) != u16::MAX {
                if let Some((_, dest_pos)) = get_granular_dest(s_pos, view, tick) {
                    if dest_pos == world_pos {
                        if s_cell.elevation() > best_elev {
                            best_elev = s_cell.elevation();
                            best_source = Some((s_idx, s_pos));
                        }
                    }
                }
            }
        }
    }
    best_source
}

#[inline(always)]
pub fn step_granular(
    cell: &mut WorldCell,
    old_cell: &WorldCell,
    view: GridReadView,
    world_pos: IVec2,
    tick: u32,
) {
    let mut old_elev = old_cell.elevation();
    let mut incoming_mat = None;
    let mut is_receiving = false;
    let mut is_giving = false;

    // Receive elevation from a steeper granular neighbor
    if let Some((s_idx, _)) = get_granular_source(world_pos, view, tick) {
        let s_cell = &view.cells[s_idx];
        incoming_mat = Some(s_cell.terrain_mat());
        is_receiving = true;
    }

    // Give elevation to a lower neighbor
    if get_angle_of_repose(old_cell.terrain_mat()) != u16::MAX {
        if let Some((_, dest_pos)) = get_granular_dest(world_pos, view, tick) {
            if let Some((_, winner_pos)) = get_granular_source(dest_pos, view, tick) {
                if winner_pos == world_pos {
                    is_giving = true;
                }
            }
        }
    }

    if is_receiving {
        old_elev = old_elev.saturating_add(1);
        if let Some(mat) = incoming_mat {
            cell.set_terrain_mat(mat);
        }
    }

    if is_giving {
        old_elev = old_elev.saturating_sub(1);
    }

    if is_receiving || is_giving {
        cell.set_elevation(old_elev);
    }
}
