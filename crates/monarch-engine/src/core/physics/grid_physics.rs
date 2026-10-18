use crate::core::{entities::GlobalPhysicsConfig, world::grid::ActiveWorldGrid};
use bevy::math::{IVec2, Vec3};
use world_model::{
    body::tuning::QUANTIZATION_TOLERANCE,
    cell::WorldCell,
    material::{FluidMat, GranularMat, SurfaceMat, ground_softness, terrain_crush_hardness},
    spatial::cell_render_xz,
};

/// One column's part in a displacement: how many units may be taken from it (a source) or
/// added to it (a sink).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplacementSlot {
    pub cell: IVec2,
    pub units: u16,
}

/// Splits `total` whole units across `slots` in proportion to their capacities using
/// largest-remainder rounding. Shares never exceed a slot's capacity and sum to exactly `total`,
/// which callers keep at or below the combined capacity.
fn apportion(total: u32, slots: &[DisplacementSlot]) -> Vec<u16> {
    let capacity: u64 = slots.iter().map(|slot| slot.units as u64).sum();
    if total == 0 || capacity == 0 {
        return vec![0; slots.len()];
    }

    let mut shares = Vec::with_capacity(slots.len());
    let mut remainders = Vec::with_capacity(slots.len());
    let mut assigned = 0u64;
    for (index, slot) in slots.iter().enumerate() {
        let exact = total as u64 * slot.units as u64;
        shares.push((exact / capacity) as u16);
        remainders.push((exact % capacity, index));
        assigned += exact / capacity;
    }

    // Ties resolve by slot order so the split is deterministic.
    remainders.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for &(_, index) in remainders.iter().take((total as u64 - assigned) as usize) {
        shares[index] += 1;
    }
    shares
}

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
        Some(cell.solid_top() as f32 * self.global_config.elevation_scale)
    }

    /// Returns the liquid occupying a column and the world-space height of its free surface.
    #[inline(always)]
    pub fn get_fluid_surface(&self, cell_position: IVec2) -> Option<(FluidMat, f32)> {
        if !self.is_in_bounds(cell_position) {
            return None;
        }
        let cell = self.grid.get_cell(cell_position);
        let material = cell.fluid_mat();
        if material == FluidMat::EMPTY || material.is_solid() || cell.fluid_vol() == 0 {
            return None;
        }
        let surface_units =
            cell.elevation() as f32 + cell.granular_vol() as f32 + cell.fluid_vol() as f32;
        Some((material, surface_units * self.global_config.elevation_scale))
    }

    /// World-space corners of the two triangles covering the quad between `cell` and
    /// `cell + (1, 1)`, exactly as the landscape mesh draws them: vertices sit at integer cell
    /// coordinates (grid y maps to world -Z) and the quad is split along the diagonal from
    /// `(x + 1, y)` to `(x, y + 1)`. Returns `None` when a corner lies outside the window.
    #[inline(always)]
    pub fn floor_quad_triangles(&self, cell: IVec2) -> Option<[[Vec3; 3]; 2]> {
        Self::quad_triangles_with(cell, |position| self.get_floor_height(position))
    }

    /// The same triangulation as `floor_quad_triangles`, with corner heights supplied by
    /// `height_of`, for collision against a modified floor (such as yielding soil).
    #[inline(always)]
    pub fn quad_triangles_with(
        cell: IVec2,
        height_of: impl Fn(IVec2) -> Option<f32>,
    ) -> Option<[[Vec3; 3]; 2]> {
        let corner = |offset: IVec2| -> Option<Vec3> {
            let position = cell + offset;
            let height = height_of(position)?;
            let (x, z) = cell_render_xz(position);
            Some(Vec3::new(x, height, z))
        };
        let v00 = corner(IVec2::new(0, 0))?;
        let v10 = corner(IVec2::new(1, 0))?;
        let v01 = corner(IVec2::new(0, 1))?;
        let v11 = corner(IVec2::new(1, 1))?;
        Some([[v00, v10, v01], [v10, v11, v01]])
    }

    /// Height of the floor mesh (bedrock plus loose material) at a world position, interpolated
    /// over the same triangles that `floor_quad_triangles` returns.
    #[inline(always)]
    pub fn get_interpolated_floor_height(&self, pos_x: f32, pos_z: f32) -> Option<f32> {
        let grid_x = pos_x.floor();
        let grid_y = (-pos_z).floor();
        let cell = IVec2::new(grid_x as i32, grid_y as i32);

        let h00 = self.get_floor_height(cell)?;
        let h10 = self.get_floor_height(cell + IVec2::new(1, 0))?;
        let h01 = self.get_floor_height(cell + IVec2::new(0, 1))?;
        let h11 = self.get_floor_height(cell + IVec2::new(1, 1))?;

        let tx = pos_x - grid_x;
        let ty = -pos_z - grid_y;

        Some(if tx + ty <= 1.0 {
            h00 + (h10 - h00) * tx + (h01 - h00) * ty
        } else {
            h11 + (h01 - h11) * (1.0 - tx) + (h10 - h11) * (1.0 - ty)
        })
    }

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
                if let Some(sampled_height) = self.get_floor_height(point)
                    && (sampled_height - center_height).abs() <= cliff_threshold
                {
                    accumulated_height += sampled_height;
                    valid_samples += 1;
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

    /// Crushes bedrock poking above `effective_bottom_height`, spending at most
    /// `available_energy`. `crush_cost` is the energy per elevation unit of packed dirt; harder
    /// materials scale it by their crush hardness.
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

        let unit_cost = crush_cost * terrain_crush_hardness(cell.terrain_mat());
        let required_crush = ((solid_height - effective_bottom_height)
            / self.global_config.elevation_scale)
            .ceil() as u16;
        let maximum_crush_allowance = (available_energy / unit_cost).floor() as u16;
        let final_crush_volume = required_crush
            .min(cell.elevation())
            .min(maximum_crush_allowance);

        if final_crush_volume > 0 {
            cell.set_elevation(cell.elevation() - final_crush_volume);
            self.grid.set_cell(cell_position, cell);
            self.grid.wake_cell(cell_position);
            return (
                final_crush_volume as u32,
                final_crush_volume as f32 * unit_cost,
            );
        }
        (0, 0.0)
    }

    /// Softness of the material at the top of a column; zero outside the window.
    pub fn ground_softness(&self, cell_position: IVec2) -> f32 {
        if !self.is_in_bounds(cell_position) {
            return 0.0;
        }
        ground_softness(&self.grid.get_cell(cell_position))
    }

    /// Shatters an ice sheet whose top lies above `height_limit`, melting the column back into
    /// water. Returns whether it broke.
    pub fn break_ice(&mut self, cell_position: IVec2, height_limit: f32) -> bool {
        if !self.is_in_bounds(cell_position) {
            return false;
        }
        let mut cell = self.grid.get_cell(cell_position);
        let top = cell.solid_top() as f32 * self.global_config.elevation_scale;
        if !cell.fluid_mat().is_solid() || top <= height_limit || !cell.thaw() {
            return false;
        }
        self.grid.set_cell(cell_position, cell);
        self.grid.wake_cell(cell_position);
        true
    }

    /// Presses bare bedrock down towards `target_height` by at most `max_units`, compacting the
    /// soil under a load. Returns the units removed; columns with loose material or ice on top
    /// are left to the caller.
    pub fn compact_bedrock(
        &mut self,
        cell_position: IVec2,
        target_height: f32,
        max_units: u16,
    ) -> u16 {
        if !self.is_in_bounds(cell_position) {
            return 0;
        }
        let mut cell = self.grid.get_cell(cell_position);
        if cell.granular_vol() > 0 || cell.fluid_mat().is_solid() {
            return 0;
        }
        let scale = self.global_config.elevation_scale;
        let excess = ((cell.elevation() as f32 * scale - target_height) / scale
            + QUANTIZATION_TOLERANCE)
            .floor();
        let pressed = (excess.max(0.0) as u16)
            .min(max_units)
            .min(cell.elevation());
        if pressed > 0 {
            cell.set_elevation(cell.elevation() - pressed);
            self.grid.set_cell(cell_position, cell);
            self.grid.wake_cell(cell_position);
        }
        pressed
    }

    /// Loose material in a column that sits above `height_limit`, in whole units.
    pub fn granular_above(
        &self,
        cell_position: IVec2,
        height_limit: f32,
    ) -> Option<(GranularMat, u16)> {
        if !self.is_in_bounds(cell_position) {
            return None;
        }
        let cell = self.grid.get_cell(cell_position);
        if cell.fluid_mat().is_solid() {
            return None;
        }
        let volume = cell.granular_vol();
        let scale = self.global_config.elevation_scale;
        let solid_height = cell.elevation() as f32 * scale;

        if volume == 0 || solid_height + volume as f32 * scale <= height_limit {
            return None;
        }

        // Round towards keeping material: a unit only goes once it sits wholly above the limit,
        // so excavation never undershoots its target by more than the rounding tolerance.
        let kept = ((height_limit - solid_height) / scale - QUANTIZATION_TOLERANCE)
            .ceil()
            .max(0.0) as u16;
        (volume > kept).then(|| (cell.granular_mat(), volume - kept))
    }

    /// Units of `material` a column can take before its loose layer is full, it reaches
    /// `ceiling_height`, or it exceeds `max_per_cell`.
    pub fn granular_room(
        &self,
        cell_position: IVec2,
        material: GranularMat,
        ceiling_height: f32,
        max_per_cell: u16,
    ) -> u16 {
        if !self.is_in_bounds(cell_position) {
            return 0;
        }
        let cell = self.grid.get_cell(cell_position);
        let resting = cell.granular_mat();
        if resting != GranularMat::EMPTY && resting != material {
            return 0;
        }

        let scale = self.global_config.elevation_scale;
        let top = (cell.elevation() as f32 + cell.granular_vol() as f32) * scale;
        if top >= ceiling_height {
            return 0;
        }

        let structural_allowance = ((ceiling_height - top) / scale).floor() as u16;
        WorldCell::MAX_GRANULAR_VOL
            .saturating_sub(cell.granular_vol())
            .min(max_per_cell)
            .min(structural_allowance)
    }

    /// The liquid in a column, its volume and the world-space height of its free surface.
    pub fn fluid_column(&self, cell_position: IVec2) -> Option<(FluidMat, u16, f32)> {
        let (material, surface) = self.get_fluid_surface(cell_position)?;
        Some((
            material,
            self.grid.get_cell(cell_position).fluid_vol(),
            surface,
        ))
    }

    /// Units of `material` a column can take: it must be dry or hold the same liquid.
    pub fn fluid_room(&self, cell_position: IVec2, material: FluidMat, max_per_cell: u16) -> u16 {
        if !self.is_in_bounds(cell_position) {
            return 0;
        }
        let cell = self.grid.get_cell(cell_position);
        let resting = cell.fluid_mat();
        if resting != FluidMat::EMPTY && resting != material {
            return 0;
        }
        WorldCell::MAX_FLUID_VOL
            .saturating_sub(cell.fluid_vol())
            .min(max_per_cell)
    }

    /// Moves loose `material` from `sources` into `sinks`, at most `limit` units.
    /// Every unit removed is deposited, so displacement never creates or destroys material.
    pub fn displace_granular(
        &mut self,
        material: GranularMat,
        sources: &[DisplacementSlot],
        sinks: &[DisplacementSlot],
        limit: u32,
    ) -> u32 {
        self.displace(
            sources,
            sinks,
            limit,
            |cell, units| {
                let remaining = cell.granular_vol() - units;
                cell.set_granular_vol(remaining);
                if remaining == 0 {
                    cell.set_granular_mat(GranularMat::EMPTY);
                }
            },
            |cell, units| {
                cell.set_granular_mat(material);
                cell.set_granular_vol(cell.granular_vol() + units);
            },
        )
    }

    /// Moves liquid `material` from `sources` into `sinks`, at most `limit` units.
    /// Every unit removed is deposited, so displacement never creates or destroys liquid.
    pub fn displace_fluid(
        &mut self,
        material: FluidMat,
        sources: &[DisplacementSlot],
        sinks: &[DisplacementSlot],
        limit: u32,
    ) -> u32 {
        self.displace(
            sources,
            sinks,
            limit,
            |cell, units| {
                cell.drain_fluid(units);
            },
            |cell, units| {
                cell.receive_fluid(material, units);
            },
        )
    }

    fn displace(
        &mut self,
        sources: &[DisplacementSlot],
        sinks: &[DisplacementSlot],
        limit: u32,
        remove: impl Fn(&mut WorldCell, u16),
        add: impl Fn(&mut WorldCell, u16),
    ) -> u32 {
        let available: u32 = sources.iter().map(|slot| slot.units as u32).sum();
        let room: u32 = sinks.iter().map(|slot| slot.units as u32).sum();
        let moved = available.min(room).min(limit);
        if moved == 0 {
            return 0;
        }

        for (slot, share) in sources.iter().zip(apportion(moved, sources)) {
            if share > 0 {
                let mut cell = self.grid.get_cell(slot.cell);
                remove(&mut cell, share);
                self.grid.set_cell(slot.cell, cell);
                self.grid.wake_cell(slot.cell);
            }
        }
        for (slot, share) in sinks.iter().zip(apportion(moved, sinks)) {
            if share > 0 {
                let mut cell = self.grid.get_cell(slot.cell);
                add(&mut cell, share);
                self.grid.set_cell(slot.cell, cell);
                self.grid.wake_cell(slot.cell);
            }
        }
        moved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slots(units: &[u16]) -> Vec<DisplacementSlot> {
        units
            .iter()
            .enumerate()
            .map(|(index, &units)| DisplacementSlot {
                cell: IVec2::new(index as i32, 0),
                units,
            })
            .collect()
    }

    #[test]
    fn apportion_is_exact_and_respects_capacity() {
        let capacities = slots(&[3, 1, 7, 0, 2, 5]);
        for total in 0..=18 {
            let shares = apportion(total, &capacities);
            assert_eq!(shares.iter().map(|&s| s as u32).sum::<u32>(), total);
            for (share, slot) in shares.iter().zip(&capacities) {
                assert!(*share <= slot.units);
            }
        }
    }
}
