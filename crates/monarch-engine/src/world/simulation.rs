use bevy::prelude::*;
use rand::RngExt;

use crate::world::grid::ActiveWorldGrid;
use crate::world::types::MaterialId;

pub fn simulate_biology(mut grid: ResMut<ActiveWorldGrid>) {
    let mut rng = rand::rng();
    let cell_count = grid.cells.len();
    let width = grid.width;
    let height = grid.height;

    // Simulate ~1% of the total grid per frame
    let ticks = cell_count / 100;

    for _ in 0..ticks {
        let idx = rng.random_range(0..cell_count);
        let cell = grid.cells[idx];

        if cell.fluid.material != MaterialId::EMPTY || cell.surface.material != MaterialId::EMPTY {
            continue;
        }

        let terrain = cell.terrain;

        if terrain.material != MaterialId::SAND && terrain.material != MaterialId::GRASS {
            continue;
        }

        let x = (idx as i32) % width;
        let y = (idx as i32) / width;

        let mut grass_neighbors = 0;
        let mut sand_neighbors = 0;

        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }

                let nx = (x + dx + width) % width;
                let ny = (y + dy + height) % height;
                let n_idx = (ny * width + nx) as usize;
                let n_cell = grid.cells[n_idx];

                if n_cell.terrain.material == MaterialId::GRASS {
                    grass_neighbors += 1;
                } else if n_cell.terrain.material == MaterialId::SAND {
                    sand_neighbors += 1;
                }
            }
        }

        let is_grass = terrain.material == MaterialId::GRASS;
        let same_neighbors = if is_grass {
            grass_neighbors
        } else {
            sand_neighbors
        };
        let opposite_neighbors = if is_grass {
            sand_neighbors
        } else {
            grass_neighbors
        };

        // --- VIOLENT EXPLOSION MECHANIC ---
        // Triggered only by heavily fortified "inner cells" at max strength
        if terrain.state >= 10 && same_neighbors == 8 {
            if rng.random_ratio(1, 50) {
                // Launch a seed bomb 10 to 25 tiles away
                let dist_x = rng.random_range(-25..=25);
                let dist_y = rng.random_range(-25..=25);

                let tx = (x + dist_x + width) % width;
                let ty = (y + dist_y + height) % height;

                // Explode and forcefully convert a 5x5 area of the OPPOSITE material
                for sy in -2..=2 {
                    for sx in -2..=2 {
                        let cx = (tx + sx + width) % width;
                        let cy = (ty + sy + height) % height;
                        let c_idx = (cy * width + cx) as usize;
                        let c_cell = grid.cells[c_idx];

                        if c_cell.fluid.material == MaterialId::EMPTY
                            && c_cell.surface.material == MaterialId::EMPTY
                        {
                            if is_grass && c_cell.terrain.material == MaterialId::SAND {
                                grid.cells[c_idx].terrain.material = MaterialId::GRASS;
                                grid.cells[c_idx].terrain.state = 5; // Spawn strong enough to survive
                            } else if !is_grass && c_cell.terrain.material == MaterialId::GRASS {
                                grid.cells[c_idx].terrain.material = MaterialId::SAND;
                                grid.cells[c_idx].terrain.state = 5; // Spawn strong enough to survive
                            }
                        }
                    }
                }

                // The inner cell expends all its energy to launch the bomb
                grid.cells[idx].terrain.state = 0;
                continue;
            }
        }

        // --- GROUP-FOCUSED GROWTH & DECAY ---
        if same_neighbors >= 5 {
            // Supported by a large collection: Grow stronger
            if rng.random_ratio(1, 4) && terrain.state < 10 {
                grid.cells[idx].terrain.state += 1;
            }
        } else if same_neighbors <= 3 {
            // Weakened by isolation or encroaching enemy cells
            if rng.random_ratio(1, 4) {
                if terrain.state > 0 {
                    grid.cells[idx].terrain.state -= 1;
                } else if opposite_neighbors > same_neighbors {
                    // Fully overwhelmed, convert to the attacking type
                    grid.cells[idx].terrain.material = if is_grass {
                        MaterialId::SAND
                    } else {
                        MaterialId::GRASS
                    };
                    grid.cells[idx].terrain.state = 1;
                }
            }
        }
    }
}
