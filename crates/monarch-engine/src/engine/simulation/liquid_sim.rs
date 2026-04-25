use bevy::math::IVec2;

use crate::engine::{
    utils::{FlowPattern, ShuffledDirs, spatial_hash},
    world::{
        cell::{MaterialId, PixelFlags, WorldCell},
        grid::GridReadView,
    },
};

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
    world_pos: IVec2,
    view: GridReadView,
    tick: u32,
) -> Option<(usize, IVec2)> {
    let (_, s_cell) = view.get_cell(world_pos)?;
    let s_mat = s_cell.fluid.material;

    if !is_liquid_mat(s_mat) {
        return None;
    }

    let pattern = if s_mat == MaterialId::LIQUID_MAGMA {
        FlowPattern::Cardinal
    } else {
        FlowPattern::Omni
    };

    let shuffled = ShuffledDirs::new_deterministic_with_momentum(
        pattern,
        world_pos,
        tick,
        s_mat,
        0,
        s_cell.fluid.flags,
    );

    let mut best_dest = None;
    let mut best_atmos = s_cell.atmosphere.state;

    for &dir in shuffled.get().iter() {
        let n_pos = world_pos + dir;

        // Zero-cost bounds check and torodial mapping via the view
        if let Some((n_idx, n_cell)) = view.get_cell(n_pos) {
            let n_atmos = n_cell.atmosphere.state;

            if n_atmos > s_cell.atmosphere.state {
                let is_empty = n_cell.fluid.material == MaterialId::EMPTY;
                let is_same = n_cell.fluid.material == s_mat;
                let can_overtake = !is_empty && !is_same && s_cell.fluid.state > n_cell.fluid.state;

                if is_empty || is_same || can_overtake {
                    if n_atmos > best_atmos {
                        best_atmos = n_atmos;
                        best_dest = Some((n_idx, n_pos)); // Cache both the raw index and pos
                    }
                }
            }
        }
    }
    best_dest
}

#[inline(always)]
fn get_highest_priority_liquid_source(
    world_pos: IVec2,
    view: GridReadView,
    tick: u32,
) -> Option<(usize, IVec2)> {
    let (_, t_cell) = view.get_cell(world_pos)?;

    let pattern = if t_cell.fluid.material == MaterialId::LIQUID_MAGMA {
        FlowPattern::Cardinal
    } else {
        FlowPattern::Omni
    };

    let shuffled = ShuffledDirs::new_deterministic_with_momentum(
        pattern,
        world_pos,
        tick,
        t_cell.fluid.material,
        0,
        t_cell.fluid.flags,
    );

    let mut best_source = None;
    let mut best_fluid = 0;

    for &dir in shuffled.get().iter() {
        let s_pos = world_pos + dir;

        if let Some((s_idx, s_cell)) = view.get_cell(s_pos) {
            if is_liquid_mat(s_cell.fluid.material) {
                // Symmetrical Check
                if let Some((_, pref_pos)) = get_source_preference(s_pos, view, tick) {
                    if pref_pos == world_pos {
                        if s_cell.fluid.state > best_fluid {
                            best_fluid = s_cell.fluid.state;
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
pub fn step_liquid(
    cell: &mut WorldCell,
    old_cell: &WorldCell,
    view: GridReadView,
    world_pos: IVec2,
    tick: u32,
) {
    let old_fluid = old_cell.fluid.state;
    let mut old_atmos = old_cell.atmosphere.state;

    let mut incoming_amt = 0;
    let mut incoming_mat = MaterialId::EMPTY;
    let mut outgoing_amt = 0;
    let mut source_pos_cache = None;

    // Destructures the raw index directly without division operations.
    if let Some((s_idx, source_pos)) = get_highest_priority_liquid_source(world_pos, view, tick) {
        source_pos_cache = Some(source_pos);
        let s_cell = &view.cells[s_idx];

        incoming_amt = calc_transfer_amount(
            s_cell.fluid.state,
            old_fluid,
            s_cell.atmosphere.state,
            old_atmos,
        );
        incoming_mat = s_cell.fluid.material;
    }

    if is_liquid_mat(old_cell.fluid.material) {
        if let Some((_, dest_pos)) = get_source_preference(world_pos, view, tick) {
            if let Some((_, winner_pos)) = get_highest_priority_liquid_source(dest_pos, view, tick)
            {
                if winner_pos == world_pos {
                    if let Some((d_idx, _)) = view.get_cell(dest_pos) {
                        let d_cell = &view.cells[d_idx];

                        outgoing_amt = calc_transfer_amount(
                            old_fluid,
                            d_cell.fluid.state,
                            old_atmos,
                            d_cell.atmosphere.state,
                        );
                    }
                }
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

        // --- Hydraulic Momentum ---
        if let Some(sp) = source_pos_cache {
            let flow_dir = world_pos - sp;
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

        // Use world_pos for spatial hash so the noise grid doesn't slide around during camera panning
        if spatial_hash(world_pos, tick) & EROSION_MASK == 0 {
            old_atmos = old_atmos.saturating_add(1);
        }
    }

    cell.fluid.state = old_fluid
        .saturating_add(incoming_amt)
        .saturating_sub(outgoing_amt);

    cell.atmosphere.state = old_atmos
        .saturating_sub(incoming_amt)
        .saturating_add(outgoing_amt);

    // --- Emergent Erosion ---
    // Fast deterministic bitwise check...
    // Increases atmosphere to permanently deepen terrain crater.
    if cell.fluid.state == 0 {
        cell.fluid.material = MaterialId::EMPTY;
        cell.fluid.flags = PixelFlags::NONE;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::world::{
        cell::{MaterialId, WorldCell},
        grid::GridReadView,
    };
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

            // Construct the zero-cost view inline
            let view = GridReadView {
                cells: read_buffer,
                width,
                height,
                window_origin: IVec2::ZERO,
                buffer_head: IVec2::ZERO,
            };

            // Parallel lock-free execution (The Danger Zone)
            cells.par_iter_mut().enumerate().for_each(|(idx, cell)| {
                let pos = IVec2::new((idx as i32) % width, (idx as i32) / width);
                let old_cell = &read_buffer[idx];

                step_liquid(
                    cell, old_cell, view, // Safely copied into the Rayon closure
                    pos, tick,
                );
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
