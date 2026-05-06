use crate::engine::{
    utils::{FlowPattern, ShuffledDirs, spatial_hash},
    world::{
        cell::{CompassFlags, FluidMat, WorldCell},
        grid::GridReadView,
    },
};
use bevy::math::IVec2;

pub const EROSION_MASK: u32 = 511;

#[inline(always)]
fn calc_transfer_amount(
    s_fluid: u16,
    t_fluid: u16,
    s_elev: u16,
    t_elev: u16,
    world_pos: IVec2,
    tick: u32,
) -> u16 {
    let s_total = s_elev as u32 + s_fluid as u32;
    let t_total = t_elev as u32 + t_fluid as u32;

    if t_total >= s_total {
        return 0;
    }

    let diff = s_total - t_total;
    let mut amount = diff / 2;

    // Micro-Sloshing: If integer truncation halted the flow but a difference exists,
    // probabilistically allow 1 unit to transfer to keep fluids self-leveling.
    if amount == 0 && diff >= 1 {
        if spatial_hash(world_pos, tick) % 2 == 0 {
            amount = 1;
        }
    }

    amount = amount.min(s_fluid as u32);
    amount = amount.min((WorldCell::MAX_FLUID_VOL as u32).saturating_sub(t_fluid as u32));
    amount as u16
}

#[inline(always)]
fn get_source_preference(
    world_pos: IVec2,
    view: GridReadView,
    tick: u32,
) -> Option<(usize, IVec2)> {
    let (_, s_cell) = view.get_cell(world_pos)?;
    let s_mat = s_cell.fluid_mat();

    if s_mat == FluidMat::EMPTY {
        return None;
    }

    let pattern = if s_mat == FluidMat::MAGMA {
        FlowPattern::Cardinal
    } else {
        FlowPattern::Omni
    };
    let shuffled = ShuffledDirs::new_deterministic_with_momentum(
        pattern,
        world_pos,
        tick,
        s_mat.0,
        0,
        s_cell.compass(),
    );

    let s_total = s_cell.elevation() as u32 + s_cell.fluid_vol() as u32;
    let mut best_dest = None;
    let mut best_total = s_total;

    for &dir in shuffled.get().iter() {
        let n_pos = world_pos + dir;

        if let Some((n_idx, n_cell)) = view.get_cell(n_pos) {
            let n_total = n_cell.elevation() as u32 + n_cell.fluid_vol() as u32;

            if n_total < s_total {
                let is_empty = n_cell.fluid_mat() == FluidMat::EMPTY;
                let is_same = n_cell.fluid_mat() == s_mat;
                let can_overtake = !is_empty && !is_same && s_cell.fluid_vol() > n_cell.fluid_vol();

                if is_empty || is_same || can_overtake {
                    if n_total < best_total {
                        best_total = n_total;
                        best_dest = Some((n_idx, n_pos));
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

    let pattern = if t_cell.fluid_mat() == FluidMat::MAGMA {
        FlowPattern::Cardinal
    } else {
        FlowPattern::Omni
    };

    let shuffled = ShuffledDirs::new_deterministic_with_momentum(
        pattern,
        world_pos,
        tick,
        t_cell.fluid_mat().0,
        0,
        t_cell.compass(),
    );

    let mut best_source = None;
    let mut best_fluid = 0;

    for &dir in shuffled.get().iter() {
        let s_pos = world_pos + dir;

        if let Some((s_idx, s_cell)) = view.get_cell(s_pos) {
            if s_cell.fluid_mat() != FluidMat::EMPTY {
                if let Some((_, pref_pos)) = get_source_preference(s_pos, view, tick) {
                    if pref_pos == world_pos {
                        if s_cell.fluid_vol() > best_fluid {
                            best_fluid = s_cell.fluid_vol();
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
    let old_fluid = old_cell.fluid_vol();
    let mut old_elev = old_cell.elevation();

    let mut incoming_amt = 0;
    let mut incoming_mat = FluidMat::EMPTY;
    let mut outgoing_amt = 0;
    let mut source_pos_cache = None;

    if let Some((s_idx, source_pos)) = get_highest_priority_liquid_source(world_pos, view, tick) {
        source_pos_cache = Some(source_pos);
        let s_cell = &view.cells[s_idx];

        incoming_amt = calc_transfer_amount(
            s_cell.fluid_vol(),
            old_fluid,
            s_cell.elevation(),
            old_elev,
            source_pos,
            tick,
        );
        incoming_mat = s_cell.fluid_mat();
    }

    if old_cell.fluid_mat() != FluidMat::EMPTY {
        if let Some((_, dest_pos)) = get_source_preference(world_pos, view, tick) {
            if let Some((_, winner_pos)) = get_highest_priority_liquid_source(dest_pos, view, tick)
            {
                if winner_pos == world_pos {
                    if let Some((d_idx, _)) = view.get_cell(dest_pos) {
                        let d_cell = &view.cells[d_idx];
                        outgoing_amt = calc_transfer_amount(
                            old_fluid,
                            d_cell.fluid_vol(),
                            old_elev,
                            d_cell.elevation(),
                            world_pos,
                            tick,
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
        if old_cell.fluid_mat() == FluidMat::EMPTY || old_cell.fluid_mat() != incoming_mat {
            cell.set_fluid_mat(incoming_mat);
        }

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

        if spatial_hash(world_pos, tick) & EROSION_MASK == 0 {
            old_elev = old_elev.saturating_sub(1);
        }
    }

    cell.set_fluid_vol(
        old_fluid
            .saturating_add(incoming_amt)
            .saturating_sub(outgoing_amt),
    );
    cell.set_elevation(old_elev);

    if cell.fluid_vol() == 0 {
        cell.set_fluid_mat(FluidMat::EMPTY);
        cell.set_compass(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::world::{
        cell::{FluidMat, WorldCell},
        grid::GridReadView,
    };
    use bevy::math::IVec2;
    use rayon::prelude::*;

    fn create_cell(elev: u16, fluid_mat: FluidMat, fluid_vol: u16) -> WorldCell {
        let mut cell = WorldCell::default();
        cell.set_elevation(elev);
        cell.set_fluid_mat(fluid_mat);
        cell.set_fluid_vol(fluid_vol);
        cell
    }

    #[test]
    fn test_parallel_mass_conservation() {
        let width = 20;
        let height = 20;
        let size = (width * height) as usize;

        let mut cells = vec![create_cell(255, FluidMat::EMPTY, 0); size];
        let mut back_buffer = vec![create_cell(255, FluidMat::EMPTY, 0); size];

        // Create a flat ground (elevation = 100)
        for c in cells.iter_mut() {
            c.set_elevation(100);
        }

        // Drop a massive 4x4 block of dense liquid to force massive parallel collisions
        for y in 8..12 {
            for x in 8..12 {
                let idx = (y * width + x) as usize;
                cells[idx] = create_cell(0, FluidMat::WATER, 200);
            }
        }

        let initial_mass: u64 = cells.iter().map(|c| c.fluid_vol() as u64).sum();

        // Simulate 100 ticks using REAL parallel Rayon iteration
        for tick in 0..100 {
            std::mem::swap(&mut cells, &mut back_buffer);
            cells.copy_from_slice(&back_buffer);

            let read_buffer = &back_buffer;

            let view = GridReadView {
                cells: read_buffer,
                width,
                height,
                window_origin: IVec2::ZERO,
                buffer_head: IVec2::ZERO,
            };

            cells.par_iter_mut().enumerate().for_each(|(idx, cell)| {
                let pos = IVec2::new((idx as i32) % width, (idx as i32) / width);
                let old_cell = &read_buffer[idx];

                step_liquid(cell, old_cell, view, pos, tick);
            });

            let current_mass: u64 = cells.iter().map(|c| c.fluid_vol() as u64).sum();

            assert_eq!(
                initial_mass, current_mass,
                "PARALLEL MASS LEAK at tick {}! Started with {}, now {}",
                tick, initial_mass, current_mass
            );
        }
    }
}
