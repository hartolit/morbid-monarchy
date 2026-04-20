use crate::engine::world::{cell::MaterialId, grid::ActiveWorldGrid};
use bevy::prelude::*;

// --- PHYSICS TUNING ---
// Minimum pressure difference required to flow (stops restless jitters and uphill crawls)
const SURFACE_TENSION: i32 = 4;
// How strongly the "phantom wave" pulls water into its line
const WAVE_STRENGTH: i32 = 50;
const WAVE_SPEED: f64 = 3.5;
const WAVE_FREQUENCY: f64 = 0.12;

pub fn simulate_water(time: Res<Time>, mut grid: ResMut<ActiveWorldGrid>) {
    let width = grid.width;
    let height = grid.height;
    let time_secs = time.elapsed_secs_f64();

    // Read-only snapshot of the current frame for deterministic neighbor lookups
    let old_cells = grid.cells.clone();

    // Delta buffers for True Displacement mass conservation
    let mut f_deltas = vec![0_i32; grid.cells.len()];
    let mut a_deltas = vec![0_i32; grid.cells.len()];

    // ==========================================
    // 1. CALCULATE FLOW & WAVE CONTRACTION
    // ==========================================
    for idx in 0..grid.cells.len() {
        let cell = &old_cells[idx];

        if cell.fluid.material == MaterialId::EMPTY || cell.fluid.state == 0 {
            continue;
        }

        let x = (idx as i32) % width;
        let y = (idx as i32) / width;

        // Add the Phantom Wave Contractor to the actual atmospheric pressure
        let my_wave_bonus = wave_contractor(x, y, time_secs);
        let my_perceived_pressure = cell.atmosphere.state as i32 + my_wave_bonus;

        let mut best_neighbor = None;
        let mut highest_pressure = my_perceived_pressure;

        // 4-Way Cardinal Flow
        let neighbors = [(0, -1), (0, 1), (-1, 0), (1, 0)];

        for (dx, dy) in neighbors.iter() {
            let nx = (x + dx + width) % width;
            let ny = (y + dy + height) % height;
            let n_idx = (ny * width + nx) as usize;

            let n_cell = &old_cells[n_idx];

            let n_wave_bonus = wave_contractor(nx, ny, time_secs);
            let n_perceived_pressure = n_cell.atmosphere.state as i32 + n_wave_bonus;

            // Water seeks the highest perceived pressure (deepest physical point + wave pull)
            if n_perceived_pressure > highest_pressure {
                highest_pressure = n_perceived_pressure;
                best_neighbor = Some(n_idx);
            }
        }

        // Execute Flow if it overcomes Surface Tension
        if let Some(n_idx) = best_neighbor {
            let pressure_diff = highest_pressure - my_perceived_pressure;

            if pressure_diff > SURFACE_TENSION {
                let n_cell = &old_cells[n_idx];

                // Calculate how much room the neighbor actually has to avoid destroying mass
                let space_in_neighbor = 255 - n_cell.fluid.state as i32;

                let flow_amount = (pressure_diff / 2)
                    .max(1)
                    .min(cell.fluid.state as i32)
                    .min(space_in_neighbor);

                if flow_amount > 0 {
                    // TRUE DISPLACEMENT: Water leaves us, Air enters us
                    f_deltas[idx] -= flow_amount;
                    a_deltas[idx] += flow_amount;

                    // Water enters neighbor, Air leaves neighbor
                    f_deltas[n_idx] += flow_amount;
                    a_deltas[n_idx] -= flow_amount;
                }
            }
        }
    }

    // ==========================================
    // 2. APPLY FLOWS & CULL GHOST WATER
    // ==========================================
    let mut changed = false;
    for idx in 0..grid.cells.len() {
        let f_delta = f_deltas[idx];
        let a_delta = a_deltas[idx];

        let cell = &mut grid.cells[idx];

        // GHOST WATER FIX: Aggressive cleanup of dry cells
        if cell.fluid.material != MaterialId::EMPTY && cell.fluid.state == 0 && f_delta <= 0 {
            cell.fluid.material = MaterialId::EMPTY;
            changed = true;
        }

        if f_delta == 0 && a_delta == 0 {
            continue;
        }

        // Apply Fluid
        let new_fluid = (cell.fluid.state as i32 + f_delta).clamp(0, 255) as u8;
        cell.fluid.state = new_fluid;

        if new_fluid == 0 {
            cell.fluid.material = MaterialId::EMPTY; // Secondary ghost-water cull
        } else if cell.fluid.material == MaterialId::EMPTY {
            cell.fluid.material = MaterialId::LIQUID_WATER; // Re-hydrate empty cell
        }

        // Apply Atmosphere (Equalize)
        let new_atmos = (cell.atmosphere.state as i32 + a_delta).clamp(0, 255) as u8;
        cell.atmosphere.state = new_atmos;

        changed = true;
    }

    // Only force a GPU upload if water actually moved or evaporated
    if changed {
        grid.cells_dirty = true;
    }
}

/// Generates a moving, deterministic wave line.
/// When the peak hits a coordinate, it temporarily spikes the perceived atmospheric
/// pressure, sucking nearby water into a wave crest.
#[inline(always)]
fn wave_contractor(x: i32, y: i32, time_secs: f64) -> i32 {
    let phase =
        (x as f64 * WAVE_FREQUENCY + y as f64 * WAVE_FREQUENCY - time_secs * WAVE_SPEED).sin();

    // Only the top 20% of the sine wave applies pull, creating distinct lines of effect
    // rather than a messy sloshing sea.
    if phase > 0.8 {
        ((phase - 0.8) * 5.0 * WAVE_STRENGTH as f64) as i32
    } else {
        0
    }
}
