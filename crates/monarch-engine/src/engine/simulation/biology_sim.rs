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
