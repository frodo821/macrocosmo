use std::collections::HashSet;

use bevy::prelude::*;

use crate::colony::{AuthorityParams, ConstructionParams};
use crate::communication::CommandLog;
use crate::components::Position;
use crate::condition::ScopedFlags;
use crate::empire::CommsParams;
use crate::galaxy::StarSystem;
use crate::knowledge::KnowledgeStore;
use crate::physics;
use crate::ship::{Ship, ShipState};
use crate::technology::{
    EmpireModifiers, GameFlags, GlobalParams, PendingColonyTechModifiers, RecentlyResearched,
    ResearchPool, ResearchQueue, TechTree,
};

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        use crate::observer::not_in_observer_mode;

        app.add_systems(Startup, spawn_player_empire.run_if(not_in_observer_mode))
            .add_systems(
                Startup,
                spawn_player
                    .after(crate::galaxy::generate_galaxy)
                    .run_if(not_in_observer_mode),
            )
            .add_systems(
                Update,
                update_player_location
                    .after(crate::time_system::advance_game_time)
                    .run_if(not_in_observer_mode),
            )
            .add_systems(Update, log_player_info.run_if(not_in_observer_mode));
    }
}

/// Spawn the player's empire entity with all empire-level components.
/// This must run before any system that queries for PlayerEmpire.
pub fn spawn_player_empire(mut commands: Commands) {
    commands.spawn((
        (
            Empire {
                name: "Human Federation".into(),
            },
            PlayerEmpire,
            Faction::new("humanity_empire", "Terran Federation"),
            TechTree::default(),
            ResearchQueue::default(),
            ResearchPool::default(),
            RecentlyResearched::default(),
            AuthorityParams::default(),
            ConstructionParams::default(),
        ),
        (
            EmpireModifiers::default(),
            GameFlags::default(),
            GlobalParams::default(),
            KnowledgeStore::default(),
            CommandLog::default(),
            ScopedFlags::default(),
            PendingColonyTechModifiers::default(),
            CommsParams::default(),
        ),
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

/// Faction identity component. Defines which faction an empire belongs to.
/// The `id` matches a FactionDefinition loaded from Lua scripts.
///
/// Preset fields (`can_diplomacy`, `allowed_diplomatic_options`) are copied
/// from [`crate::scripting::faction_api::FactionDefinition`] at spawn time.
/// Runtime code reads these fields directly instead of looking up the
/// faction type registry.
#[derive(Component, Clone, Debug)]
pub struct Faction {
    pub id: String,
    pub name: String,
    /// Whether this faction can engage in formal diplomacy (treaties,
    /// declarations, etc.). Copied from `FactionDefinition.can_diplomacy`
    /// at spawn time. Defaults to `false`.
    pub can_diplomacy: bool,
    /// Set of diplomatic option ids available to this faction.
    /// Populated from `FactionDefinition.allowed_diplomatic_options`
    /// at spawn time. Empty by default.
    pub allowed_diplomatic_options: HashSet<String>,
}

impl Faction {
    /// Convenience constructor with default preset fields.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            can_diplomacy: false,
            allowed_diplomatic_options: HashSet::new(),
        }
    }
}

pub fn spawn_player(mut commands: Commands, capitals: Query<(Entity, &StarSystem)>) {
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

/// Update player's StationedAt when aboard a ship that docks at a new system.
/// Only updates on dock — while in transit, StationedAt stays at the last docked system.
pub fn update_player_location(
    mut player_q: Query<(&AboardShip, &mut StationedAt), With<Player>>,
    ships: Query<&ShipState>,
) {
    for (aboard, mut stationed) in &mut player_q {
        if let Ok(state) = ships.get(aboard.ship) {
            if let ShipState::InSystem { system } = state {
                stationed.system = *system;
            }
            // In transit states (SubLight, InFTL, etc.): keep StationedAt at last docked system
        }
    }
}
