use bevy::prelude::*;

use crate::colony::{AuthorityParams, ConstructionParams};
use crate::communication::CommandLog;
use crate::components::Position;
use crate::galaxy::StarSystem;
use crate::knowledge::KnowledgeStore;
use crate::physics;
use crate::technology::{
    EmpireModifiers, GameFlags, GlobalParams, RecentlyResearched, ResearchPool, ResearchQueue,
    TechTree,
};

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_player_empire)
            .add_systems(Startup, spawn_player.after(crate::galaxy::generate_galaxy))
            .add_systems(Update, log_player_info);
    }
}

/// Spawn the player's empire entity with all empire-level components.
/// This must run before any system that queries for PlayerEmpire.
pub fn spawn_player_empire(mut commands: Commands) {
    commands.spawn((
        Empire {
            name: "Human Federation".into(),
        },
        PlayerEmpire,
        TechTree::default(),
        ResearchQueue::default(),
        ResearchPool::default(),
        RecentlyResearched::default(),
        AuthorityParams::default(),
        ConstructionParams::default(),
        EmpireModifiers::default(),
        GameFlags::default(),
        GlobalParams::default(),
        KnowledgeStore::default(),
        CommandLog::default(),
    ));
    info!("Player empire entity spawned");
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

/// An empire entity represents a faction/civilization.
#[derive(Component)]
pub struct Empire {
    pub name: String,
}

/// Marker component for the player's empire entity.
#[derive(Component)]
pub struct PlayerEmpire;

pub fn spawn_player(
    mut commands: Commands,
    capitals: Query<(Entity, &StarSystem)>,
) {
    for (entity, system) in &capitals {
        if system.is_capital {
            commands.spawn((Player, StationedAt { system: entity }));
            info!("Player starts at capital: {}", system.name);
            return;
        }
    }
    warn!("No capital system found!");
}

pub fn log_player_info(
    keys: Res<ButtonInput<KeyCode>>,
    player_q: Query<&StationedAt, With<Player>>,
    systems: Query<(&StarSystem, &Position)>,
    all_systems: Query<(Entity, &StarSystem, &Position)>,
) {
    if !keys.just_pressed(KeyCode::KeyI) {
        return;
    }

    if let Ok(stationed) = player_q.single() {
        if let Ok((current, current_pos)) = systems.get(stationed.system) {
            info!("=== Player Location: {} ===", current.name);
            info!(
                "Position: ({:.1}, {:.1}, {:.1}) ly",
                current_pos.x, current_pos.y, current_pos.z
            );

            info!("--- Nearby Systems ---");
            let mut nearby: Vec<(String, f64, bool)> = Vec::new();
            for (_entity, sys, sys_pos) in &all_systems {
                if sys.name == current.name {
                    continue;
                }
                let dist = physics::distance_ly(current_pos, sys_pos);
                nearby.push((sys.name.clone(), dist, sys.surveyed));
            }
            nearby.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

            for (name, dist, surveyed) in nearby.iter().take(10) {
                let survey_mark = if *surveyed { "+" } else { "?" };
                let delay_sd = physics::light_delay_hexadies(*dist);
                info!(
                    "  [{}] {} - {:.1} ly (light delay: {} sd / {:.1} yr)",
                    survey_mark, name, dist, delay_sd, dist
                );
            }
        }
    }
}
