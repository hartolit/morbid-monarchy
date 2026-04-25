use bevy::math::IVec2;

use crate::engine::{
    utils::{FlowPattern, ShuffledDirs, spatial_hash},
    world::cell::{MaterialId, PixelFlags, WorldCell},
};

// MUST be a 2^n - 1 value!
// Examples: 127 (1/128), 255 (1/256), 511 (1/512), 1023 (1/1024), 2047 (1/2048)
pub const EROSION_MASK: u32 = 511;

#[inline(always)]
fn is_liquid_mat(mat: MaterialId) -> bool {
    mat.0 >= 1 && mat.0 <= 31
}

#[inline(always)]
fn calc_transfer_amount(s_fluid: u8, t_fluid: u8, s_atmos: u8, t_atmos: u8) -> u8 {
    if t_atmos <= s_atmos {
        return 0;
    }

    let diff = t_atmos - s_atmos;
    let mut amount = (diff / 2).max(1);
    amount = amount.min(s_fluid);

    amount = amount.min(255u8.saturating_sub(t_fluid));
    amount = amount.min(t_atmos);
    amount = amount.min(255u8.saturating_sub(s_atmos));
    amount
}

#[inline(always)]
fn get_source_preference(
    sx: i32,
    sy: i32,
    read_buffer: &[WorldCell],
    width: i32,
    height: i32,
    tick: u32,
) -> Option<IVec2> {
    let my_idx = (sy * width + sx) as usize;
    let s_cell = &read_buffer[my_idx];
    let s_mat = s_cell.fluid.material;

    if !is_liquid_mat(s_mat) {
        return None;
    }

    let pattern = if s_mat == MaterialId::LIQUID_MAGMA {
        FlowPattern::Cardinal
    } else {
        FlowPattern::Omni
    };

    // Use the cell's own flags to drive its momentum bias
    let shuffled = ShuffledDirs::new_deterministic_with_momentum(
        pattern,
        IVec2::new(sx, sy),
        tick,
        s_mat,
        0,
        s_cell.fluid.flags,
    );
    let dirs = shuffled.get();

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
                let is_same = n_cell.fluid.material == s_mat;
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

#[inline(always)]
fn get_highest_priority_liquid_source(
    tx: i32,
    ty: i32,
    read_buffer: &[WorldCell],
    width: i32,
    height: i32,
    tick: u32,
) -> Option<IVec2> {
    let t_idx = (ty * width + tx) as usize;
    let t_cell = &read_buffer[t_idx];

    let pattern = if t_cell.fluid.material == MaterialId::LIQUID_MAGMA {
        FlowPattern::Cardinal
    } else {
        FlowPattern::Omni
    };

    // Use the target cell's fluid flags to break ties with momentum
    let shuffled = ShuffledDirs::new_deterministic_with_momentum(
        pattern,
        IVec2::new(tx, ty),
        tick,
        t_cell.fluid.material,
        0,
        t_cell.fluid.flags,
    );
    let dirs = shuffled.get();

    let mut best_source = None;
    let mut best_fluid = 0;

    for &dir in dirs.iter() {
        let sx = tx + dir.x;
        let sy = ty + dir.y;

        if sx >= 0 && sx < width && sy >= 0 && sy < height {
            let s_idx = (sy * width + sx) as usize;
            let s_cell = &read_buffer[s_idx];

            if is_liquid_mat(s_cell.fluid.material) {
                // Symmetrical Check: Cell T asks Cell S what Cell S wants to do
                if let Some(pref) = get_source_preference(sx, sy, read_buffer, width, height, tick)
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

// Inject momentum flags and erosion into step_liquid:
#[inline(always)]
pub fn step_liquid(
    cell: &mut WorldCell,
    old_cell: &WorldCell,
    read_buffer: &[WorldCell],
    width: i32,
    height: i32,
    pos: IVec2,
    tick: u32,
) {
    let x = pos.x;
    let y = pos.y;

    let old_fluid = old_cell.fluid.state;
    let mut old_atmos = old_cell.atmosphere.state;

    let mut incoming_amt = 0;
    let mut incoming_mat = MaterialId::EMPTY;
    let mut outgoing_amt = 0;
    let mut source_pos_cache = None;

    if let Some(source_pos) =
        get_highest_priority_liquid_source(x, y, read_buffer, width, height, tick)
    {
        source_pos_cache = Some(source_pos);
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

    if is_liquid_mat(old_cell.fluid.material) {
        if let Some(dest_pos) = get_source_preference(x, y, read_buffer, width, height, tick) {
            let winner = get_highest_priority_liquid_source(
                dest_pos.x,
                dest_pos.y,
                read_buffer,
                width,
                height,
                tick,
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

    if incoming_amt == 0 && outgoing_amt == 0 {
        return;
    }

    if incoming_amt > 0 {
        if old_cell.fluid.material == MaterialId::EMPTY || old_cell.fluid.material != incoming_mat {
            cell.fluid.material = incoming_mat;
            cell.fluid.variant = 0;
        }

        // --- HYDRAULIC MOMENTUM ---
        if let Some(sp) = source_pos_cache {
            let flow_dir = pos - sp;
            let mut new_flags = PixelFlags::WAKES_AWAKE;
            if flow_dir.y > 0 {
                new_flags.insert(PixelFlags::FACING_N);
            }
            if flow_dir.y < 0 {
                new_flags.insert(PixelFlags::FACING_S);
            }
            if flow_dir.x > 0 {
                new_flags.insert(PixelFlags::FACING_E);
            }
            if flow_dir.x < 0 {
                new_flags.insert(PixelFlags::FACING_W);
            }
            cell.fluid.flags = new_flags;
        }

        // --- EMERGENT EROSION ---
        // Fast deterministic bitwise check
        // By increasing the baseline atmosphere, we permanently deepen the terrain crater.
        if spatial_hash(pos, tick) & EROSION_MASK == 0 {
            old_atmos = old_atmos.saturating_add(1);
        }
    }

    cell.fluid.state = old_fluid
        .saturating_add(incoming_amt)
        .saturating_sub(outgoing_amt);

    cell.atmosphere.state = old_atmos
        .saturating_sub(incoming_amt)
        .saturating_add(outgoing_amt);

    if cell.fluid.state == 0 {
        cell.fluid.material = MaterialId::EMPTY;
        cell.fluid.flags = PixelFlags::NONE;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::world::cell::{MaterialId, WorldCell};
    use bevy::math::IVec2;
    use rayon::prelude::*;

    fn create_cell(atmos: u8, fluid_mat: MaterialId, fluid_state: u8) -> WorldCell {
        let mut cell = WorldCell::default();
        cell.atmosphere.state = atmos;
        cell.fluid.material = fluid_mat;
        cell.fluid.state = fluid_state;
        cell
    }

    #[test]
    fn test_parallel_mass_conservation() {
        let width = 20;
        let height = 20;
        let size = (width * height) as usize;

        let mut cells = vec![create_cell(255, MaterialId::EMPTY, 0); size];
        let mut back_buffer = vec![create_cell(255, MaterialId::EMPTY, 0); size];

        // Create a flat ground (atmosphere = 100)
        for c in cells.iter_mut() {
            c.atmosphere.state = 100;
        }

        // Drop a massive 4x4 block of dense liquid to force massive parallel collisions
        for y in 8..12 {
            for x in 8..12 {
                let idx = (y * width + x) as usize;
                cells[idx] = create_cell(0, MaterialId::LIQUID_WATER, 200);
            }
        }

        let initial_mass: u64 = cells.iter().map(|c| c.fluid.state as u64).sum();

        // Simulate 100 ticks using REAL parallel Rayon iteration
        for tick in 0..100 {
            // Swap buffers (mimicking the engine)
            std::mem::swap(&mut cells, &mut back_buffer);
            cells.copy_from_slice(&back_buffer);

            let read_buffer = &back_buffer;

            // Parallel lock-free execution (The Danger Zone)
            cells.par_iter_mut().enumerate().for_each(|(idx, cell)| {
                let pos = IVec2::new((idx as i32) % width, (idx as i32) / width);
                let old_cell = &read_buffer[idx];

                step_liquid(cell, old_cell, read_buffer, width, height, pos, tick);
            });

            // Verify exact mass conservation after parallel collisions
            let current_mass: u64 = cells.iter().map(|c| c.fluid.state as u64).sum();

            assert_eq!(
                initial_mass, current_mass,
                "PARALLEL MASS LEAK at tick {}! Started with {}, now {}",
                tick, initial_mass, current_mass
            );
        }
    }
}
