use bevy::math::IVec2;
use rand::rngs::ThreadRng;

use crate::engine::{
    utils::{FlowPattern, ShuffledDirs},
    world::cell::{MaterialId, PixelFlags, WorldCell},
};

/// Liquids are any materials between 1 and 31.
#[inline(always)]
fn is_liquid_mat(mat: MaterialId) -> bool {
    mat.0 >= 1 && mat.0 <= 31
}

/// Calculates the exact lock-free transfer amount.
/// Guaranteed to perfectly level two cells without over-correcting (ping-ponging).
#[inline(always)]
fn calc_transfer_amount(s_fluid: u8, t_fluid: u8, s_atmos: u8, t_atmos: u8) -> u8 {
    // Water ONLY flows downhill.
    // higher atmos = lower physical elevation.
    if t_atmos <= s_atmos {
        return 0;
    }

    let diff = t_atmos - s_atmos;

    let mut amount = (diff / 2).max(1);
    amount = amount.min(s_fluid);

    // Safety constraints (Physical Back-pressure)
    amount = amount.min(255u8.saturating_sub(t_fluid)); // Target fluid capacity
    amount = amount.min(t_atmos); // Target atmos available to displace
    amount = amount.min(255u8.saturating_sub(s_atmos)); // Source atmos available to fill void
    amount
}

/// Evaluates all 8 neighbors to find the steepest downhill path.
#[inline(always)]
fn get_source_preference(
    sx: i32,
    sy: i32,
    read_buffer: &[WorldCell],
    width: i32,
    height: i32,
    dirs: &[IVec2],
) -> Option<IVec2> {
    let my_idx = (sy * width + sx) as usize;
    let s_cell = &read_buffer[my_idx];

    if !is_liquid_mat(s_cell.fluid.material) {
        return None;
    }

    let mut best_dest = None;
    let mut best_atmos = s_cell.atmosphere.state;

    for &dir in dirs.iter() {
        let nx = sx + dir.x;
        let ny = sy + dir.y;

        if nx >= 0 && nx < width && ny >= 0 && ny < height {
            let n_idx = (ny * width + nx) as usize;
            let n_cell = &read_buffer[n_idx];
            let n_atmos = n_cell.atmosphere.state;

            if n_atmos > s_cell.atmosphere.state {
                let is_empty = n_cell.fluid.material == MaterialId::EMPTY;
                let is_same = n_cell.fluid.material == s_cell.fluid.material;
                let can_overtake = !is_empty && !is_same && s_cell.fluid.state > n_cell.fluid.state;

                if is_empty || is_same || can_overtake {
                    if n_atmos > best_atmos {
                        best_atmos = n_atmos;
                        best_dest = Some(IVec2::new(nx, ny));
                    }
                }
            }
        }
    }
    best_dest
}

/// The Lock-Free Arbitrator.
#[inline(always)]
fn get_highest_priority_liquid_source(
    tx: i32,
    ty: i32,
    read_buffer: &[WorldCell],
    width: i32,
    height: i32,
    dirs: &[IVec2],
) -> Option<IVec2> {
    let mut best_source = None;
    let mut best_fluid = 0;

    for &dir in dirs.iter() {
        let sx = tx + dir.x;
        let sy = ty + dir.y;

        if sx >= 0 && sx < width && sy >= 0 && sy < height {
            let s_idx = (sy * width + sx) as usize;
            let s_cell = &read_buffer[s_idx];

            if is_liquid_mat(s_cell.fluid.material) {
                if let Some(pref) = get_source_preference(sx, sy, read_buffer, width, height, dirs)
                {
                    if pref.x == tx && pref.y == ty {
                        if s_cell.fluid.state > best_fluid {
                            best_fluid = s_cell.fluid.state;
                            best_source = Some(IVec2::new(sx, sy));
                        }
                    }
                }
            }
        }
    }
    best_source
}

/// Main entry point. A cell can simultaneously pull from above and give to below.
#[inline(always)]
pub fn step_liquid(
    cell: &mut WorldCell,
    old_cell: &WorldCell,
    read_buffer: &[WorldCell],
    width: i32,
    height: i32,
    rng: &mut ThreadRng,
    pos: IVec2,
) {
    let x = pos.x;
    let y = pos.y;

    // Generate the shuffled pattern once per cell execution
    let shuffled = ShuffledDirs::new(FlowPattern::Omni, rng);
    let dirs = shuffled.get();

    let old_fluid = old_cell.fluid.state;
    let old_atmos = old_cell.atmosphere.state;

    let mut incoming_amt = 0;
    let mut incoming_mat = MaterialId::EMPTY;
    let mut outgoing_amt = 0;

    // --- PHASE 1: PULL (Am I a Target?) ---
    if let Some(source_pos) =
        get_highest_priority_liquid_source(x, y, read_buffer, width, height, dirs)
    {
        let s_idx = (source_pos.y * width + source_pos.x) as usize;
        let s_cell = &read_buffer[s_idx];

        incoming_amt = calc_transfer_amount(
            s_cell.fluid.state,
            old_fluid,
            s_cell.atmosphere.state,
            old_atmos,
        );
        incoming_mat = s_cell.fluid.material;
    }

    // --- PHASE 2: LEAVE (Am I a Source?) ---
    if is_liquid_mat(old_cell.fluid.material) {
        if let Some(dest_pos) = get_source_preference(x, y, read_buffer, width, height, dirs) {
            let winner = get_highest_priority_liquid_source(
                dest_pos.x,
                dest_pos.y,
                read_buffer,
                width,
                height,
                dirs,
            );
            if winner == Some(pos) {
                let d_idx = (dest_pos.y * width + dest_pos.x) as usize;
                let d_cell = &read_buffer[d_idx];

                outgoing_amt = calc_transfer_amount(
                    old_fluid,
                    d_cell.fluid.state,
                    old_atmos,
                    d_cell.atmosphere.state,
                );
            }
        }
    }

    // --- PHASE 3: APPLY MUTATIONS ---
    if incoming_amt == 0 && outgoing_amt == 0 {
        return;
    }

    if incoming_amt > 0 {
        // Overtake Material
        if old_cell.fluid.material == MaterialId::EMPTY || old_cell.fluid.material != incoming_mat {
            cell.fluid.material = incoming_mat;
            cell.fluid.variant = 0; // Reset visual variants on overtake
        }
    }

    // Apply exact mass transfers safely
    cell.fluid.state = old_fluid
        .saturating_add(incoming_amt)
        .saturating_sub(outgoing_amt);

    cell.atmosphere.state = old_atmos
        .saturating_sub(incoming_amt)
        .saturating_add(outgoing_amt);

    // Clean up if we drained completely
    if cell.fluid.state == 0 {
        cell.fluid.material = MaterialId::EMPTY;
        cell.fluid.flags = PixelFlags::NONE;
    }
}
