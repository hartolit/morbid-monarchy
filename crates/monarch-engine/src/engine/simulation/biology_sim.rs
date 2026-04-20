use bevy::prelude::*;
use rand::RngExt;

use crate::engine::world::{cell::MaterialId, grid::ActiveWorldGrid};

pub fn simulate_biology(mut grid: ResMut<ActiveWorldGrid>) {
    let mut rng = rand::rng();
    let cell_count = grid.cells.len();

    // Randomly perturb 10% of the cells every single frame
    let ticks = cell_count / 10;

    for _ in 0..ticks {
        let idx = rng.random_range(0..cell_count);

        let current_gas = grid.cells[idx].atmosphere.state;

        // Randomly boil or condense the atmosphere.
        // Because Terrain Z = H_max - Gas - Fluid, increasing gas will crush the terrain down,
        // and decreasing gas will cause the terrain to violently spike upward.
        if rng.random_bool(0.5) {
            grid.cells[idx].atmosphere.state = current_gas.saturating_add(5);
        } else {
            grid.cells[idx].atmosphere.state = current_gas.saturating_sub(5);
        }

        // Ensure the atmosphere cell isn't totally EMPTY so the shader reads its state byte
        if grid.cells[idx].atmosphere.material == MaterialId::EMPTY {
            grid.cells[idx].atmosphere.material = MaterialId::GAS_STEAM;
        }
    }
}

pub fn old_biology(mut grid: ResMut<ActiveWorldGrid>) {
    let mut rng = rand::rng();
    let cell_count = grid.cells.len();
    let width = grid.width;
    let height = grid.height;

    let ticks = cell_count / 10;

    for _ in 0..ticks {
        let idx = rng.random_range(0..cell_count);
        let cell = grid.cells[idx];

        if cell.fluid.material != MaterialId::EMPTY || cell.surface.material != MaterialId::EMPTY {
            continue;
        }

        let terrain = cell.terrain;

        if terrain.material != MaterialId::LOOSE_SAND
            && terrain.material != MaterialId::ORGANIC_FOLIAGE
        {
            continue;
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
                let n_cell = grid.cells[n_idx];

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
                grid.cells[idx].terrain.material = MaterialId::ORGANIC_FOLIAGE;
                grid.cells[idx].terrain.state = 0;
            } else if rng.random_ratio(1, 10_000) {
                // Spontaneous low-probability spark to keep the grid alive
                grid.cells[idx].terrain.material = MaterialId::ORGANIC_FOLIAGE;
                grid.cells[idx].terrain.state = 0;
            }
        } else if terrain.material == MaterialId::ORGANIC_FOLIAGE {
            // Excited/Refractory phase: Automatically age and die back into sand
            if terrain.state < 10 {
                grid.cells[idx].terrain.state += 1;
            } else {
                grid.cells[idx].terrain.material = MaterialId::LOOSE_SAND;
                grid.cells[idx].terrain.state = 0;
            }
        }
    }
}
