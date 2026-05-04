use bevy::prelude::*;
use monarch_engine::prelude::{GridEvent, SimulationEventQueue};

pub fn process_grid_events(mut commands: Commands, event_queue: Res<SimulationEventQueue>) {
    while let Ok(event) = event_queue.rx.try_recv() {
        match event {
            GridEvent::SpawnTerrainParticle { pos, material } => {
                // commands.spawn(ParticleBundle { ... });
            }
            GridEvent::SpawnFluidParticle { pos, material } => {
                // commands.spawn(ParticleBundle { ... });
            }
            GridEvent::ApplyDamage { pos, amount } => {
                // Apply damage to any ECS characters standing at this coordinate
            }
            GridEvent::PlaySound { pos, sound_id } => {
                // Play positional audio
            }
        }
    }
}
