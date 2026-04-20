use bevy::prelude::*;
use rand::RngExt;
use rayon::prelude::*;

use crate::engine::world::{cell::MaterialId, grid::ActiveWorldGrid};

pub fn simulate_biology(mut grid: ResMut<ActiveWorldGrid>) {
    let width = grid.width;
    let height = grid.height;

    // We clone the cells to create a "frozen" read-only state of the grid for this frame.
    // This allows safe, lock-free neighbor lookups across threads without violating borrow rules.
    let old_cells = grid.cells.clone();

    // Iterate over every cell in parallel
    grid.cells
        .par_iter_mut()
        .enumerate()
        .for_each(|(idx, cell)| {
            // rand::rng() in rand 0.10 provides a fast, thread-local generator
            // so we must instantiate it inside the closure per-thread.
            let mut rng = rand::rng();

            // Randomly perturb ~10% of the cells every single frame to simulate
            // the old `ticks = cell_count / 10` Monte Carlo approach.
            if !rng.random_ratio(1, 10) {
                return;
            }

            // ==========================================
            // 1. Atmosphere Simulation
            // ==========================================
            let current_gas = cell.atmosphere.state;

            // Randomly boil or condense the atmosphere.
            if rng.random_bool(0.5) {
                cell.atmosphere.state = current_gas.saturating_add(5);
            } else {
                cell.atmosphere.state = current_gas.saturating_sub(5);
            }

            // Ensure the atmosphere cell isn't totally EMPTY so the shader reads its state byte
            if cell.atmosphere.material == MaterialId::EMPTY {
                cell.atmosphere.material = MaterialId::GAS_STEAM;
            }

            // ==========================================
            // 2. Terrain Biology Simulation
            // ==========================================

            // Read from the frozen `old_cells` to decide logic, but write to `cell`
            let old_cell = &old_cells[idx];

            if old_cell.fluid.material != MaterialId::EMPTY
                || old_cell.surface.material != MaterialId::EMPTY
            {
                return;
            }

            let terrain = old_cell.terrain;

            if terrain.material != MaterialId::LOOSE_SAND
                && terrain.material != MaterialId::ORGANIC_FOLIAGE
            {
                return;
            }

            let x = (idx as i32) % width;
            let y = (idx as i32) / width;

            let mut wave_front_neighbors = 0;

            for dy in -1..=1 {
                for dx in -1..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }

                    let nx = (x + dx + width) % width;
                    let ny = (y + dy + height) % height;
                    let n_idx = (ny * width + nx) as usize;

                    // Safely read the neighbor's state from the frozen grid
                    let n_cell = &old_cells[n_idx];

                    // Only fresh, newly sprouted grass (state < 2) acts as an active wave front
                    if n_cell.terrain.material == MaterialId::ORGANIC_FOLIAGE
                        && n_cell.terrain.state < 2
                    {
                        wave_front_neighbors += 1;
                    }
                }
            }

            if terrain.material == MaterialId::LOOSE_SAND {
                // Resting phase: Catch the wave if an active front touches it
                if wave_front_neighbors > 0 {
                    cell.terrain.material = MaterialId::ORGANIC_FOLIAGE;
                    cell.terrain.state = 0;
                } else if rng.random_ratio(1, 10_000) {
                    // Spontaneous low-probability spark to keep the grid alive
                    cell.terrain.material = MaterialId::ORGANIC_FOLIAGE;
                    cell.terrain.state = 0;
                }
            } else if terrain.material == MaterialId::ORGANIC_FOLIAGE {
                // Excited/Refractory phase: Automatically age and die back into sand
                if terrain.state < 10 {
                    cell.terrain.state += 1;
                } else {
                    cell.terrain.material = MaterialId::LOOSE_SAND;
                    cell.terrain.state = 0;
                }
            }
        });

    grid.cells_dirty = true;
}
