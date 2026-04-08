use bevy::prelude::*;

use crate::galaxy::StarSystem;
use crate::physics;

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_player.after(crate::galaxy::generate_galaxy))
            .add_systems(Update, log_player_info);
    }
}

/// The player's current location
#[derive(Component)]
pub struct Player;

/// Player is stationed on a planet in a star system
#[derive(Component)]
pub struct StationedAt {
    pub system: Entity,
}

/// Player is aboard a ship (moving or stationary)
#[derive(Component)]
pub struct AboardShip {
    pub ship: Entity,
}

/// Represents the player's knowledge of a star system (delayed by distance)
#[derive(Component)]
pub struct KnownSystemState {
    /// The star system this knowledge refers to
    pub system: Entity,
    /// Game-year when this information was current (not when received)
    pub info_timestamp: f64,
    /// Snapshot of known data
    pub population: f64,
    pub production: f64,
}

fn spawn_player(
    mut commands: Commands,
    capitals: Query<(Entity, &StarSystem)>,
) {
    // Find the capital system
    for (entity, system) in &capitals {
        if system.is_capital {
            commands.spawn((Player, StationedAt { system: entity }));
            info!("Player starts at capital: {}", system.name);
            return;
        }
    }
    warn!("No capital system found!");
}

fn log_player_info(
    keys: Res<ButtonInput<KeyCode>>,
    player_q: Query<&StationedAt, With<Player>>,
    systems: Query<&StarSystem>,
    all_systems: Query<(Entity, &StarSystem)>,
) {
    // Press I for info
    if !keys.just_pressed(KeyCode::KeyI) {
        return;
    }

    if let Ok(stationed) = player_q.single() {
        if let Ok(current) = systems.get(stationed.system) {
            info!("=== Player Location: {} ===", current.name);
            info!(
                "Position: ({:.1}, {:.1}, {:.1}) ly",
                current.position[0], current.position[1], current.position[2]
            );

            // Show distances to other systems
            info!("--- Nearby Systems ---");
            let mut nearby: Vec<(String, f64, bool, bool)> = Vec::new();
            for (_entity, sys) in &all_systems {
                if sys.name == current.name {
                    continue;
                }
                let dist = physics::distance_ly(current.position, sys.position);
                let delay = physics::light_delay_years(dist);
                nearby.push((sys.name.clone(), dist, sys.surveyed, delay < 50.0));
            }
            nearby.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

            for (name, dist, surveyed, _) in nearby.iter().take(10) {
                let survey_mark = if *surveyed { "+" } else { "?" };
                let delay_sd = physics::light_delay_sexadies(*dist);
                info!(
                    "  [{}] {} - {:.1} ly (light delay: {} sd / {:.1} yr)",
                    survey_mark, name, dist, delay_sd, *dist
                );
            }
        }
    }
}
