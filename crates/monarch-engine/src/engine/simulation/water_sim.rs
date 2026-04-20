use crate::engine::world::grid::ActiveWorldGrid;
use bevy::prelude::*;

#[derive(Resource)]
pub struct WaterSimulationConfig {
    pub surface_tension: i32,
    pub wave_strength: i32,
    pub wave_speed: f64,
    pub wave_frequency: f64,
}

impl Default for WaterSimulationConfig {
    fn default() -> Self {
        Self {
            surface_tension: 1,
            wave_strength: 20,
            wave_speed: 0.5,
            wave_frequency: 0.1,
        }
    }
}

// pub fn simulate_water(
//     time: Res<Time>,
//     config: Res<WaterSimulationConfig>,
//     mut grid: ResMut<ActiveWorldGrid>,
// ) {
// }

// #[inline(always)]
// fn wave_contractor(x: i32, y: i32, time_secs: f64, config: &WaterSimulationConfig) -> i32 {
//     0
// }
