use std::collections::HashSet;

use bevy::prelude::*;

use crate::colony::{AuthorityParams, ConstructionParams};
use crate::communication::CommandLog;
use crate::components::Position;
use crate::condition::ScopedFlags;
use crate::empire::CommsParams;
use crate::galaxy::StarSystem;
use crate::game_state::GameState;
use crate::knowledge::{KnowledgeStore, SystemVisibilityMap};
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

        // #439 Phase 3: player empire + Ruler spawn is a new-game
        // construction step, moved from Startup to OnEnter(NewGame).
        app.add_systems(
            OnEnter(GameState::NewGame),
            spawn_player_empire.run_if(not_in_observer_mode),
        )
        .add_systems(
            OnEnter(GameState::NewGame),
            (bevy::ecs::schedule::ApplyDeferred, spawn_player)
                .chain()
                .after(crate::galaxy::generate_galaxy)
                .after(spawn_player_empire)
                .run_if(not_in_observer_mode),
        )
        .add_systems(
            Update,
            update_ruler_location.after(crate::time_system::advance_game_time),
        )
        .add_systems(
            Update,
            sync_ruler_viewer_system.after(update_ruler_location),
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
            SystemVisibilityMap::default(),
            CommandLog::default(),
            ScopedFlags::default(),
            PendingColonyTechModifiers::default(),
            CommsParams::default(),
        ),
    ));
    info!("Player empire entity spawned");
}

/// The physical avatar of an empire's leader. Every empire gets one.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct Ruler {
    pub name: String,
    pub empire: Entity,
}

/// Forward-reference on empire entity pointing to its Ruler entity.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct EmpireRuler(pub Entity);

/// Marker: this Ruler is the human-controlled player.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct Player;

/// Player is stationed on a planet in a star system
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct StationedAt {
    pub system: Entity,
}

/// Player is aboard a ship (moving or stationary)
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct AboardShip {
    pub ship: Entity,
}

/// An empire entity represents a faction/civilization.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct Empire {
    pub name: String,
}

/// Marker component for the player's empire entity.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct PlayerEmpire;

/// The star system used as the light-speed reference point for an empire's
/// knowledge propagation. For the player empire this tracks `StationedAt`;
/// for NPC empires it is set to the capital system at spawn.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct EmpireViewerSystem(pub Entity);

/// Faction identity component. Defines which faction an empire belongs to.
/// The `id` matches a FactionDefinition loaded from Lua scripts.
///
/// Preset fields (`can_diplomacy`, `allowed_diplomatic_options`) are copied
/// from [`crate::scripting::faction_api::FactionDefinition`] at spawn time.
/// Runtime code reads these fields directly instead of looking up the
/// faction type registry.
#[derive(Component, Clone, Debug, Reflect)]
#[reflect(Component)]
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

/// Spawn a Ruler entity for an empire at the given system.
/// Returns the Ruler entity.
pub fn spawn_ruler_for_empire(
    commands: &mut Commands,
    empire_entity: Entity,
    system: Entity,
    name: String,
    is_player: bool,
) -> Entity {
    let mut ec = commands.spawn((
        Ruler {
            name: name.clone(),
            empire: empire_entity,
        },
        StationedAt { system },
    ));
    if is_player {
        ec.insert(Player);
    }
    let ruler_entity = ec.id();
    commands
        .entity(empire_entity)
        .insert(EmpireRuler(ruler_entity));
    ruler_entity
}

pub fn spawn_player(
    mut commands: Commands,
    capitals: Query<(Entity, &StarSystem)>,
    empire_q: Query<(Entity, &Faction), With<PlayerEmpire>>,
    home_assignments: Option<Res<crate::galaxy::HomeSystemAssignments>>,
) {
    let Ok((empire_entity, faction)) = empire_q.single() else {
        return;
    };

    // #429: Use HomeSystemAssignments to find the player faction's home system.
    // Fall back to is_capital for backward compat (tests without galaxy gen).
    let home = home_assignments
        .as_ref()
        .and_then(|ha| ha.assignments.get(&faction.id).copied());

    let capital_entity =
        home.or_else(|| capitals.iter().find(|(_, s)| s.is_capital).map(|(e, _)| e));

    let Some(entity) = capital_entity else {
        warn!("No home system found for player!");
        return;
    };

    let system_name = capitals
        .get(entity)
        .map(|(_, s)| s.name.clone())
        .unwrap_or_else(|_| "unknown".into());

    spawn_ruler_for_empire(&mut commands, empire_entity, entity, "Player".into(), true);
    info!("Player Ruler starts at home system: {}", system_name);
}

pub fn log_player_info(
    keys: Res<ButtonInput<KeyCode>>,
    keybindings: Option<Res<crate::input::KeybindingRegistry>>,
    player_q: Query<&StationedAt, With<Player>>,
    systems: Query<(&StarSystem, &Position)>,
    all_systems: Query<(Entity, &StarSystem, &Position)>,
) {
    let pressed = match keybindings.as_deref() {
        Some(kb) => kb.is_just_pressed(crate::input::actions::DEBUG_LOG_PLAYER_INFO, &keys),
        None => keys.just_pressed(KeyCode::KeyI),
    };
    if !pressed {
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

/// Sync `EmpireViewerSystem` on ALL empires that have a Ruler to match the
/// Ruler's current `StationedAt` system.
pub fn sync_ruler_viewer_system(
    rulers: Query<(&Ruler, &StationedAt)>,
    mut empires: Query<(&EmpireRuler, &mut EmpireViewerSystem)>,
) {
    for (empire_ruler, mut viewer) in &mut empires {
        if let Ok((_, stationed)) = rulers.get(empire_ruler.0) {
            viewer.0 = stationed.system;
        }
    }
}

/// Update Ruler's StationedAt when aboard a ship that docks at a new system.
/// Only updates on dock — while in transit, StationedAt stays at the last docked system.
/// Runs for ALL rulers (player and NPC).
pub fn update_ruler_location(
    mut ruler_q: Query<(&AboardShip, &mut StationedAt), With<Ruler>>,
    ships: Query<&ShipState>,
) {
    for (aboard, mut stationed) in &mut ruler_q {
        if let Ok(state) = ships.get(aboard.ship) {
            if let ShipState::InSystem { system } = state {
                stationed.system = *system;
            }
            // In transit states (SubLight, InFTL, etc.): keep StationedAt at last docked system
        }
    }
}
