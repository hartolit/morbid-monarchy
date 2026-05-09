use bevy::math::IVec2;

use crate::engine::{entities::EntityPhysicsConfig, world::grid::ActiveWorldGrid};

/// Safely resolves absolute physical floor height from the bit-packed CA grid.
#[inline(always)]
pub fn fetch_floor_height(
    grid: &ActiveWorldGrid,
    pos: IVec2,
    bounds_min: IVec2,
    bounds_max: IVec2,
    elevation_scale: f32,
) -> Option<f32> {
    if pos.x >= bounds_min.x
        && pos.x < bounds_max.x
        && pos.y >= bounds_min.y
        && pos.y < bounds_max.y
    {
        let cell = grid.get_cell(pos);
        Some((cell.elevation() as f32 + cell.granular_vol() as f32) * elevation_scale)
    } else {
        None
    }
}

/// Computes surrounding terrain resistance via sparse, outward cache-line aligned sampling.
#[inline(always)]
pub fn compute_outward_resistance(
    grid: &ActiveWorldGrid,
    center_pos: IVec2,
    center_height: f32,
    bounds_min: IVec2,
    bounds_max: IVec2,
    config: &EntityPhysicsConfig,
) -> f32 {
    let mut accumulated_height = 0.0;
    let mut valid_samples = 0;

    for ring in 1..=config.outward_sample_rings {
        let current_offset = (ring as i32) * config.outward_stride_step;

        // Sparse orthogonal/corner sampling radiating outward from the contact point.
        // Maximizes cache fetch economy without sweeping dense memory blocks.
        let sample_points = [
            center_pos + IVec2::new(current_offset, current_offset),
            center_pos + IVec2::new(-current_offset, current_offset),
            center_pos + IVec2::new(current_offset, -current_offset),
            center_pos + IVec2::new(-current_offset, -current_offset),
        ];

        for point in sample_points {
            if let Some(sampled_height) =
                fetch_floor_height(grid, point, bounds_min, bounds_max, config.elevation_scale)
            {
                // Ignore highly volatile heights (cliffs, ravines) to establish an organic structural plateau baseline
                if (sampled_height - center_height).abs() <= config.volatile_cliff_threshold {
                    accumulated_height += sampled_height;
                    valid_samples += 1;
                }
            }
        }
    }

    if valid_samples == 0 {
        return f32::MAX;
    }

    let baseline_average = accumulated_height / (valid_samples as f32);
    let depth_below_plateau = (baseline_average - center_height).max(0.0);

    1.0 + (depth_below_plateau * config.resistance_multiplier)
}
