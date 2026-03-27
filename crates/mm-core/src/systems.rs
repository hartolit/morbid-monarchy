use bevy_ecs::prelude::{Query, Res, With};
use bevy_math::Vec3;
use bevy_transform::components::Transform;

use crate::player::{MovementConfig, MovementIntent, Player, SimulationStep};

pub fn apply_movement_intent(
    config: Res<MovementConfig>,
    step: Res<SimulationStep>,
    mut players: Query<(&MovementIntent, &mut Transform), With<Player>>,
) {
    let distance = config.units_per_second * step.delta_seconds;
    if distance == 0.0 {
        return;
    }

    for (intent, mut transform) in &mut players {
        let direction = normalized_planar_or_zero(intent.0);
        if direction == Vec3::ZERO {
            continue;
        }

        transform.translation += direction * distance;
    }
}

fn normalized_planar_or_zero(intent: Vec3) -> Vec3 {
    let planar = Vec3::new(intent.x, intent.y, 0.0);
    if planar.length_squared() > 1.0 {
        planar.normalize()
    } else {
        planar
    }
}

#[cfg(test)]
mod tests {
    use bevy_ecs::{schedule::Schedule, world::World};
    use bevy_math::Vec3;
    use bevy_transform::components::Transform;

    use super::apply_movement_intent;
    use crate::player::{MovementConfig, MovementIntent, Player, SimulationStep};

    #[test]
    fn player_moves_using_configured_speed_and_step() {
        let mut world = World::new();
        world.insert_resource(MovementConfig {
            units_per_second: 120.0,
        });
        world.insert_resource(SimulationStep { delta_seconds: 0.5 });

        let entity = world
            .spawn((Player, MovementIntent(Vec3::X), Transform::default()))
            .id();

        let mut schedule = Schedule::default();
        schedule.add_systems(apply_movement_intent);
        schedule.run(&mut world);

        let transform = world.entity(entity).get::<Transform>().unwrap();
        assert_eq!(transform.translation.x, 60.0);
        assert_eq!(transform.translation.y, 0.0);
    }

    #[test]
    fn diagonal_intent_is_normalized_before_movement() {
        let mut world = World::new();
        world.insert_resource(MovementConfig {
            units_per_second: 100.0,
        });
        world.insert_resource(SimulationStep { delta_seconds: 1.0 });

        let entity = world
            .spawn((
                Player,
                MovementIntent(Vec3::new(1.0, 1.0, 4.0)),
                Transform::default(),
            ))
            .id();

        let mut schedule = Schedule::default();
        schedule.add_systems(apply_movement_intent);
        schedule.run(&mut world);

        let transform = world.entity(entity).get::<Transform>().unwrap();
        let moved = transform.translation.truncate().length();
        assert!((moved - 100.0).abs() < 0.001);
        assert_eq!(transform.translation.z, 0.0);
    }

    #[test]
    fn existing_z_index_is_preserved_during_planar_movement() {
        let mut world = World::new();
        world.insert_resource(MovementConfig {
            units_per_second: 80.0,
        });
        world.insert_resource(SimulationStep {
            delta_seconds: 0.25,
        });

        let entity = world
            .spawn((
                Player,
                MovementIntent(Vec3::new(0.0, 1.0, 12.0)),
                Transform::from_xyz(5.0, 10.0, 7.0),
            ))
            .id();

        let mut schedule = Schedule::default();
        schedule.add_systems(apply_movement_intent);
        schedule.run(&mut world);

        let transform = world.entity(entity).get::<Transform>().unwrap();
        assert_eq!(transform.translation.x, 5.0);
        assert_eq!(transform.translation.y, 30.0);
        assert_eq!(transform.translation.z, 7.0);
    }
}
