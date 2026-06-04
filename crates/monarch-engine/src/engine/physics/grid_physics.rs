use crate::engine::{
    entities::EntityPhysicsConfig,
    utils::spatial_hash,
    world::{
        cell::{GranularMat, SurfaceMat, WorldCell},
        grid::ActiveWorldGrid,
    },
};
use bevy::math::IVec2;

/// A zero-allocation translation layer for physical grid mutations.
pub struct GridPhysicsApi<'a> {
    pub grid: &'a mut ActiveWorldGrid,
    pub config: &'a EntityPhysicsConfig,
    bounds_minimum: IVec2,
}

impl<'a> GridPhysicsApi<'a> {
    pub fn new(grid: &'a mut ActiveWorldGrid, config: &'a EntityPhysicsConfig) -> Self {
        let bounds_minimum = grid.spatial.window_origin;
        Self {
            grid,
            config,
            bounds_minimum,
        }
    }

    /// Evaluates grid inclusion utilizing integer underflow to eliminate branching logic.
    #[inline(always)]
    pub fn is_in_bounds(&self, cell_position: IVec2) -> bool {
        let local_x = (cell_position.x - self.bounds_minimum.x) as u32;
        let local_y = (cell_position.y - self.bounds_minimum.y) as u32;
        local_x < self.grid.spatial.width as u32 && local_y < self.grid.spatial.height as u32
    }

    pub fn get_bedrock_height(&self, cell_position: IVec2) -> Option<f32> {
        if !self.is_in_bounds(cell_position) {
            return None;
        }
        Some(self.grid.get_cell(cell_position).elevation() as f32 * self.config.elevation_scale)
    }

    pub fn get_floor_height(&self, cell_position: IVec2) -> Option<f32> {
        if !self.is_in_bounds(cell_position) {
            return None;
        }
        let cell = self.grid.get_cell(cell_position);
        Some((cell.elevation() as f32 + cell.granular_vol() as f32) * self.config.elevation_scale)
    }

    pub fn compute_outward_resistance(&self, center_position: IVec2, center_height: f32) -> f32 {
        let mut accumulated_height = 0.0;
        let mut valid_samples = 0;

        for ring in 1..=self.config.outward_sample_rings {
            let current_offset = (ring as i32) * self.config.outward_stride_step;
            let sample_points = [
                center_position + IVec2::new(current_offset, current_offset),
                center_position + IVec2::new(-current_offset, current_offset),
                center_position + IVec2::new(current_offset, -current_offset),
                center_position + IVec2::new(-current_offset, -current_offset),
            ];

            for point in sample_points {
                if let Some(sampled_height) = self.get_floor_height(point) {
                    if (sampled_height - center_height).abs()
                        <= self.config.volatile_cliff_threshold
                    {
                        accumulated_height += sampled_height;
                        valid_samples += 1;
                    }
                }
            }
        }

        if valid_samples == 0 {
            return f32::MAX;
        }
        let depth_below_plateau =
            (accumulated_height / valid_samples as f32 - center_height).max(0.0);
        1.0 + (depth_below_plateau * self.config.resistance_multiplier)
    }

    pub fn clear_surface_organics(&mut self, cell_position: IVec2, carving_bottom_height: f32) {
        if !self.is_in_bounds(cell_position) {
            return;
        }
        let mut cell = self.grid.get_cell(cell_position);

        if cell.surface_mat() == SurfaceMat::SURFACE_FOLIAGE {
            let base_height =
                (cell.elevation() as f32 + cell.granular_vol() as f32 + cell.fluid_vol() as f32)
                    * self.config.elevation_scale;
            let top_height = base_height
                + (1.0f32.max(cell.surface_state() as f32)) * self.config.elevation_scale;

            if carving_bottom_height < top_height {
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
        energy_pool: f32,
        crush_cost: f32,
    ) -> (u32, f32) {
        if energy_pool < self.config.min_deformation_energy || !self.is_in_bounds(cell_position) {
            return (0, 0.0);
        }

        let mut cell = self.grid.get_cell(cell_position);
        if cell.elevation() == 0 {
            return (0, 0.0);
        }

        let solid_height = cell.elevation() as f32 * self.config.elevation_scale;
        if solid_height <= effective_bottom_height {
            return (0, 0.0);
        }

        let needed_crush =
            ((solid_height - effective_bottom_height) / self.config.elevation_scale).ceil() as u16;
        let maximum_affordable_crush = (energy_pool / crush_cost).floor() as u16;
        let actual_crush = needed_crush
            .min(cell.elevation())
            .min(maximum_affordable_crush);

        if actual_crush > 0 {
            cell.set_elevation(cell.elevation() - actual_crush);
            self.grid.set_cell(cell_position, cell);
            self.grid.wake_cell(cell_position);
            return (actual_crush as u32, actual_crush as f32 * crush_cost);
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

        let solid_height = cell.elevation() as f32 * self.config.elevation_scale;
        let total_height = solid_height + (granular_volume as f32 * self.config.elevation_scale);

        if total_height > effective_bottom_height {
            let target_volume = if effective_bottom_height <= solid_height {
                0
            } else {
                ((effective_bottom_height - solid_height) / self.config.elevation_scale)
                    .floor()
                    .max(0.0) as u16
            };

            if granular_volume > target_volume {
                let actual_removal_volume = granular_volume - target_volume;
                let primary_material = cell.granular_mat();

                cell.set_granular_vol(granular_volume - actual_removal_volume);
                if cell.granular_vol() == 0 {
                    cell.set_granular_mat(GranularMat::EMPTY);
                }

                self.grid.set_cell(cell_position, cell);
                self.grid.wake_cell(cell_position);
                return (actual_removal_volume as u32, Some(primary_material));
            }
        }
        (0, None)
    }

    pub fn deposit_granular(
        &mut self,
        cell_position: IVec2,
        material: GranularMat,
        attempt_amount: u16,
        blade_ceiling_height: f32,
    ) -> u16 {
        if attempt_amount == 0 || !self.is_in_bounds(cell_position) {
            return 0;
        }

        let mut cell = self.grid.get_cell(cell_position);
        let current_material = cell.granular_mat();
        if current_material != GranularMat::EMPTY && current_material != material {
            return 0;
        }

        let total_height =
            (cell.elevation() as f32 + cell.granular_vol() as f32) * self.config.elevation_scale;
        if total_height >= blade_ceiling_height {
            return 0;
        }

        let volume_allowance =
            ((blade_ceiling_height - total_height) / self.config.elevation_scale).floor() as u16;
        let available_slot = WorldCell::MAX_GRANULAR_VOL.saturating_sub(cell.granular_vol());

        let actual_deposit_volume = attempt_amount
            .min(available_slot)
            .min(self.config.max_rim_deposit_per_cell)
            .min(volume_allowance);

        if actual_deposit_volume > 0 {
            cell.set_granular_mat(material);
            cell.set_granular_vol(cell.granular_vol() + actual_deposit_volume);
            self.grid.set_cell(cell_position, cell);
            self.grid.wake_cell(cell_position);
        }
        actual_deposit_volume
    }
}

/// Computes granular matter volume transfers including probabilistic remainder micro-sloshing.
#[inline(always)]
pub fn calc_granular_transfer(
    source_cell: &WorldCell,
    dest_cell: &WorldCell,
    world_position: IVec2,
    tick: u32,
) -> u16 {
    let source_total = source_cell.elevation() as u32 + source_cell.granular_vol() as u32;
    let dest_total = dest_cell.elevation() as u32 + dest_cell.granular_vol() as u32;

    if dest_total >= source_total {
        return 0;
    }

    let diff = source_total - dest_total;
    let mut amount = diff / 2;

    if amount == 0 && diff >= 1 && spatial_hash(world_position, tick) % 2 == 0 {
        amount = 1;
    }

    amount
        .min(source_cell.granular_vol() as u32)
        .min((WorldCell::MAX_GRANULAR_VOL as u32).saturating_sub(dest_cell.granular_vol() as u32))
        as u16
}

/// Computes exact fluid volume transfers across diverse underlying multi-layer structural floors.
#[inline(always)]
pub fn calc_liquid_transfer(
    source_cell: &WorldCell,
    dest_cell: &WorldCell,
    world_position: IVec2,
    tick: u32,
) -> u16 {
    let source_floor = source_cell.elevation() as u32 + source_cell.granular_vol() as u32;
    let dest_floor = dest_cell.elevation() as u32 + dest_cell.granular_vol() as u32;

    let source_total = source_floor + source_cell.fluid_vol() as u32;
    let dest_total = dest_floor + dest_cell.fluid_vol() as u32;

    if dest_total >= source_total {
        return 0;
    }

    let diff = source_total - dest_total;
    let mut amount = diff / 2;

    if amount == 0 && diff >= 1 && spatial_hash(world_position, tick) % 2 == 0 {
        amount = 1;
    }

    amount
        .min(source_cell.fluid_vol() as u32)
        .min((WorldCell::MAX_FLUID_VOL as u32).saturating_sub(dest_cell.fluid_vol() as u32))
        as u16
}
