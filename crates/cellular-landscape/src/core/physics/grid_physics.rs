use crate::core::{
    entities::GlobalPhysicsConfig,
    world::{
        cell::{GranularMat, SurfaceMat, WorldCell},
        grid::ActiveWorldGrid,
    },
};
use bevy::math::IVec2;

/// A zero-allocation translation boundary for cellular topological physics.
/// Executes deterministic memory reads/writes, strictly agnostic to the initiating entity.
pub struct GridPhysicsApi<'a> {
    pub grid: &'a mut ActiveWorldGrid,
    pub global_config: &'a GlobalPhysicsConfig,
    bounds_minimum: IVec2,
}

impl<'a> GridPhysicsApi<'a> {
    pub fn new(grid: &'a mut ActiveWorldGrid, global_config: &'a GlobalPhysicsConfig) -> Self {
        let bounds_minimum = grid.spatial.window_origin;
        Self {
            grid,
            global_config,
            bounds_minimum,
        }
    }

    #[inline(always)]
    pub fn is_in_bounds(&self, cell_position: IVec2) -> bool {
        let local_x = (cell_position.x - self.bounds_minimum.x) as u32;
        let local_y = (cell_position.y - self.bounds_minimum.y) as u32;
        local_x < self.grid.spatial.width as u32 && local_y < self.grid.spatial.height as u32
    }

    #[inline(always)]
    pub fn get_bedrock_height(&self, cell_position: IVec2) -> Option<f32> {
        if !self.is_in_bounds(cell_position) {
            return None;
        }
        Some(
            self.grid.get_cell(cell_position).elevation() as f32
                * self.global_config.elevation_scale,
        )
    }

    #[inline(always)]
    pub fn get_floor_height(&self, cell_position: IVec2) -> Option<f32> {
        if !self.is_in_bounds(cell_position) {
            return None;
        }
        let cell = self.grid.get_cell(cell_position);
        Some(
            (cell.elevation() as f32 + cell.granular_vol() as f32)
                * self.global_config.elevation_scale,
        )
    }

    /// Fetches a bilinearly interpolated, continuous floor height to eliminate discrete cellular stepping.
    #[inline(always)]
    pub fn get_interpolated_floor_height(&self, pos_x: f32, pos_z: f32) -> Option<f32> {
        let gx = pos_x.floor() as i32;
        let gy = (-pos_z).floor() as i32;

        let base_h = self.get_floor_height(IVec2::new(gx, gy))?;
        let h10 = self
            .get_floor_height(IVec2::new(gx + 1, gy))
            .unwrap_or(base_h);
        let h01 = self
            .get_floor_height(IVec2::new(gx, gy + 1))
            .unwrap_or(base_h);
        let h11 = self
            .get_floor_height(IVec2::new(gx + 1, gy + 1))
            .unwrap_or(base_h);

        let tx = pos_x - pos_x.floor();
        let ty = (-pos_z) - (-pos_z).floor();

        let h0 = base_h * (1.0 - tx) + h10 * tx;
        let h1 = h01 * (1.0 - tx) + h11 * tx;

        Some(h0 * (1.0 - ty) + h1 * ty)
    }

    /// Computes surrounding terrain resistance via sparse, outward cache-line aligned sampling.
    #[inline(always)]
    pub fn compute_outward_resistance(
        &self,
        center_pos: IVec2,
        center_height: f32,
        sample_rings: usize,
        stride_step: i32,
        cliff_threshold: f32,
        resistance_multiplier: f32,
    ) -> f32 {
        let mut accumulated_height = 0.0;
        let mut valid_samples = 0;

        for ring in 1..=sample_rings {
            let offset = (ring as i32) * stride_step;
            let points = [
                center_pos + IVec2::new(offset, offset),
                center_pos + IVec2::new(-offset, offset),
                center_pos + IVec2::new(offset, -offset),
                center_pos + IVec2::new(-offset, -offset),
            ];

            for point in points {
                if let Some(sampled_height) = self.get_floor_height(point) {
                    if (sampled_height - center_height).abs() <= cliff_threshold {
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
        1.0 + (depth_below_plateau * resistance_multiplier)
    }

    pub fn clear_surface_organics(&mut self, cell_position: IVec2, clearance_height: f32) {
        if !self.is_in_bounds(cell_position) {
            return;
        }
        let mut cell = self.grid.get_cell(cell_position);

        if cell.surface_mat() == SurfaceMat::SURFACE_FOLIAGE {
            let base_height =
                (cell.elevation() as f32 + cell.granular_vol() as f32 + cell.fluid_vol() as f32)
                    * self.global_config.elevation_scale;
            let top_height = base_height
                + (1.0f32.max(cell.surface_state() as f32)) * self.global_config.elevation_scale;

            if clearance_height < top_height {
                cell.set_surface_mat(SurfaceMat::EMPTY);
                cell.set_surface_state(0);
                self.grid.set_cell(cell_position, cell);
                self.grid.wake_cell(cell_position);
            }
        }
    }

    pub fn crush_bedrock(
        &mut self,
        cell_position: IVec2,
        effective_bottom_height: f32,
        available_energy: f32,
        crush_cost: f32,
    ) -> (u32, f32) {
        if available_energy <= 0.0 || !self.is_in_bounds(cell_position) {
            return (0, 0.0);
        }

        let mut cell = self.grid.get_cell(cell_position);
        let solid_height = cell.elevation() as f32 * self.global_config.elevation_scale;

        if solid_height <= effective_bottom_height || cell.elevation() == 0 {
            return (0, 0.0);
        }

        let required_crush = ((solid_height - effective_bottom_height)
            / self.global_config.elevation_scale)
            .ceil() as u16;
        let maximum_crush_allowance = (available_energy / crush_cost).floor() as u16;
        let final_crush_volume = required_crush
            .min(cell.elevation())
            .min(maximum_crush_allowance);

        if final_crush_volume > 0 {
            cell.set_elevation(cell.elevation() - final_crush_volume);
            self.grid.set_cell(cell_position, cell);
            self.grid.wake_cell(cell_position);
            return (
                final_crush_volume as u32,
                final_crush_volume as f32 * crush_cost,
            );
        }
        (0, 0.0)
    }

    pub fn excavate_granular(
        &mut self,
        cell_position: IVec2,
        effective_bottom_height: f32,
    ) -> (u32, Option<GranularMat>) {
        if !self.is_in_bounds(cell_position) {
            return (0, None);
        }

        let mut cell = self.grid.get_cell(cell_position);
        let granular_volume = cell.granular_vol();
        if granular_volume == 0 {
            return (0, None);
        }

        let solid_height = cell.elevation() as f32 * self.global_config.elevation_scale;
        let total_height =
            solid_height + (granular_volume as f32 * self.global_config.elevation_scale);

        if total_height > effective_bottom_height {
            let target_volume = if effective_bottom_height <= solid_height {
                0
            } else {
                ((effective_bottom_height - solid_height) / self.global_config.elevation_scale)
                    .floor()
                    .max(0.0) as u16
            };

            if granular_volume > target_volume {
                let extracted_volume = granular_volume - target_volume;
                let primary_material = cell.granular_mat();

                cell.set_granular_vol(granular_volume - extracted_volume);
                if cell.granular_vol() == 0 {
                    cell.set_granular_mat(GranularMat::EMPTY);
                }

                self.grid.set_cell(cell_position, cell);
                self.grid.wake_cell(cell_position);
                return (extracted_volume as u32, Some(primary_material));
            }
        }
        (0, None)
    }

    pub fn deposit_granular(
        &mut self,
        cell_position: IVec2,
        material: GranularMat,
        attempt_amount: u16,
        ceiling_height: f32,
        max_deposit_per_cell: u16,
    ) -> u16 {
        if attempt_amount == 0 || !self.is_in_bounds(cell_position) {
            return 0;
        }

        let mut cell = self.grid.get_cell(cell_position);
        let current_material = cell.granular_mat();
        if current_material != GranularMat::EMPTY && current_material != material {
            return 0;
        }

        let total_height = (cell.elevation() as f32 + cell.granular_vol() as f32)
            * self.global_config.elevation_scale;
        if total_height >= ceiling_height {
            return 0;
        }

        let structural_allowance =
            ((ceiling_height - total_height) / self.global_config.elevation_scale).floor() as u16;

        let mathematical_allowance = ((WorldCell::MAX_ELEVATION as u16)
            .saturating_sub(cell.elevation()))
            + WorldCell::MAX_GRANULAR_VOL.saturating_sub(cell.granular_vol());

        let exact_deposit = attempt_amount
            .min(mathematical_allowance)
            .min(max_deposit_per_cell)
            .min(structural_allowance);

        if exact_deposit > 0 {
            cell.set_granular_mat(material);
            cell.set_granular_vol(cell.granular_vol() + exact_deposit);
            self.grid.set_cell(cell_position, cell);
            self.grid.wake_cell(cell_position);
        }
        exact_deposit
    }
}
