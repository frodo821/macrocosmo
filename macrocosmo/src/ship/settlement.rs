use bevy::prelude::*;

use crate::amount::Amt;
use crate::colony::{
    BuildQueue, BuildingQueue, Buildings, Colony, ColonyJobRates, FoodConsumption, MaintenanceCost,
    Production, ProductionFocus, ResourceCapacity, ResourceStockpile, SystemBuildingQueue,
    SystemBuildings,
};
use crate::components::Position;
use crate::events::{GameEvent, GameEventKind};
use crate::galaxy::{AtSystem, Hostile, StarSystem, SystemAttributes};
use crate::knowledge::{FactSysParam, KnowledgeFact, PlayerVantage};
use crate::player::{AboardShip, Player, StationedAt};
use crate::species::{ColonyJobs, ColonyPopulation, ColonySpecies};
use crate::time_system::GameClock;

use super::{Ship, ShipState};

/// Default duration of a colonization/settling operation in hexadies (60 hexadies = 1 year) (#32).
///
/// #160: Canonical value is `GameBalance.settling_duration` (Lua-defined).
/// Retained as fallback for helpers/tests without ECS access.
pub const SETTLING_DURATION_HEXADIES: i64 = 60;

/// System that processes ongoing settling operations. When the timer completes,
/// establishes a colony on the first habitable planet and despawns the colony ship.
#[allow(clippy::too_many_arguments)]
pub fn process_settling(
    mut commands: Commands,
    clock: Res<GameClock>,
    // #297 (S-2): Ships now optionally carry `FactionOwner` in addition
    // to the legacy `Ship.owner: Owner` enum. Prefer the component; fall
    // back to `Ship.owner = Owner::Empire(e)` for test-spawned or
    // legacy-save ships.
    mut ships: Query<(
        Entity,
        &Ship,
        &mut ShipState,
        Option<&crate::faction::FactionOwner>,
    )>,
    systems: Query<(&StarSystem, &Position)>,
    planet_query: Query<(Entity, &crate::galaxy::Planet, &SystemAttributes)>,
    existing_colonies: Query<&Colony>,
    existing_stockpiles: Query<&ResourceStockpile, With<StarSystem>>,
    existing_system_buildings: Query<&SystemBuildings>,
    mut events: MessageWriter<GameEvent>,
    hostiles: Query<(&AtSystem, Option<&crate::faction::FactionOwner>), With<Hostile>>,
    cores: Query<(&AtSystem, &crate::faction::FactionOwner), With<crate::ship::CoreShip>>,
    faction_relations: Res<crate::faction::FactionRelations>,
    empire_entity_q: Query<Entity, With<crate::player::PlayerEmpire>>,
    player_q: Query<&StationedAt, Without<Ship>>,
    player_aboard_q: Query<&AboardShip, With<Player>>,
    mut fact_sys: FactSysParam,
) {
    let player_system = player_q.iter().next().map(|s| s.system);
    let player_pos: Option<[f64; 3]> = player_system
        .and_then(|s| systems.get(s).ok())
        .map(|(_, p)| p.as_array());
    let player_aboard = player_aboard_q.iter().next().is_some();
    let vantage = player_pos.map(|pos| PlayerVantage {
        player_pos: pos,
        player_aboard,
    });
    for (ship_entity, ship, mut state, ship_faction_owner) in &mut ships {
        let (system_entity, target_planet_entity, completes_at) = match *state {
            ShipState::Settling {
                system,
                planet,
                completes_at,
                ..
            } => (system, planet, completes_at),
            _ => continue,
        };

        if clock.elapsed >= completes_at {
            let Ok((star_system, sys_pos)) = systems.get(system_entity) else {
                continue;
            };
            let sys_pos_arr = sys_pos.as_array();

            // #52/#56: Check for hostile presence — cannot colonize while hostiles remain
            // #293: Only block on hostiles the viewing empire considers
            // aggressive. Hostiles without `FactionOwner` (tests that skip
            // `setup_test_hostile_factions`, or pre-faction-backfill frames)
            // default to blocking — preserves legacy behavior.
            let viewer = empire_entity_q.iter().next();
            let has_hostile = hostiles.iter().any(|(at_system, owner)| {
                if at_system.0 != system_entity {
                    return false;
                }
                match (viewer, owner) {
                    (Some(v), Some(o)) => faction_relations
                        .get_or_default(v, o.0)
                        .can_attack_aggressive(),
                    _ => true,
                }
            });
            if has_hostile {
                info!(
                    "Colony Ship {} cannot settle at {} — hostile presence!",
                    ship.name, star_system.name
                );
                *state = ShipState::Docked {
                    system: system_entity,
                };
                // #249: Dual-write ColonyFailed.
                let event_id = fact_sys.allocate_event_id();
                let desc = format!(
                    "Cannot establish colony at {} — hostile presence must be eliminated first!",
                    star_system.name
                );
                events.write(GameEvent {
                    id: event_id,
                    timestamp: clock.elapsed,
                    kind: GameEventKind::ColonyFailed,
                    description: desc.clone(),
                    related_system: Some(system_entity),
                });
                if let Some(v) = vantage {
                    let fact = KnowledgeFact::ColonyFailed {
                        event_id: Some(event_id),
                        system: system_entity,
                        name: star_system.name.clone(),
                        reason: "hostile presence".into(),
                    };
                    let _ = desc;
                    fact_sys.record(fact, sys_pos_arr, clock.elapsed, &v);
                }
                continue;
            }

            // #299 (S-5): Safety net — verify that a Core owned by the
            // settling ship's faction still exists in the system. If the
            // Core was destroyed mid-settle, abort and return to Docked.
            // Neutral ships (no faction) bypass the gate for backward
            // compatibility with pre-faction test setups.
            let settling_faction: Option<Entity> =
                ship_faction_owner
                    .map(|fo| fo.0)
                    .or_else(|| match ship.owner {
                        crate::ship::Owner::Empire(e) => Some(e),
                        crate::ship::Owner::Neutral => None,
                    });
            if let Some(faction) = settling_faction {
                let has_own_core = cores
                    .iter()
                    .any(|(at, fo)| at.0 == system_entity && fo.0 == faction);
                if !has_own_core {
                    warn!(
                        "Colony Ship {} settling at {} aborted — sovereignty core removed!",
                        ship.name, star_system.name
                    );
                    *state = ShipState::Docked {
                        system: system_entity,
                    };
                    continue;
                }
            }

            // Collect planets that already have a colony
            let colonized_planets: Vec<Entity> =
                existing_colonies.iter().map(|c| c.planet).collect();

            // If a specific planet was targeted, try to use it
            let target_planet = if let Some(target_pe) = target_planet_entity {
                // Verify target planet is valid and not already colonized
                if colonized_planets.contains(&target_pe) {
                    info!(
                        "Target planet in {} is already colonized, settling aborted",
                        star_system.name
                    );
                    commands.entity(ship_entity).despawn();
                    continue;
                }
                planet_query.get(target_pe).ok()
            } else {
                // Auto-select: find the first habitable, uncolonized planet in this system
                planet_query.iter().find(|(entity, p, attrs)| {
                    p.system == system_entity
                        && crate::galaxy::is_habitable(attrs.habitability)
                        && !colonized_planets.contains(entity)
                })
            };

            let Some((planet_entity, _, attrs)) = target_planet else {
                info!(
                    "Colony Ship {} found no habitable planet at {}",
                    ship.name, star_system.name
                );
                commands.entity(ship_entity).despawn();
                continue;
            };

            let system_name = star_system.name.clone();
            let num_slots = attrs.max_building_slots as usize;

            // #297 (S-2): Resolve administrative owner for the new colony /
            // SystemBuildings. Prefer the `FactionOwner` component on the
            // settling ship; fall back to `Ship.owner = Owner::Empire(e)`
            // so pre-S-2 test ships still produce an owned colony. Neutral
            // ships spawn an un-owned colony (matches "no diplomatic
            // identity" semantics on the ship side).
            let new_owner: Option<Entity> =
                ship_faction_owner
                    .map(|fo| fo.0)
                    .or_else(|| match ship.owner {
                        crate::ship::Owner::Empire(e) => Some(e),
                        crate::ship::Owner::Neutral => None,
                    });

            let new_colony = commands
                .spawn((
                    Colony {
                        planet: planet_entity,
                        population: 10.0,
                        growth_rate: 0.005,
                    },
                    // #250: zero-base production; all output comes from building/
                    // job modifiers. Planet attributes (mineral_richness etc.) are
                    // still available for future building/job modifiers to consume.
                    Production {
                        minerals_per_hexadies: crate::modifier::ModifiedValue::new(Amt::ZERO),
                        energy_per_hexadies: crate::modifier::ModifiedValue::new(Amt::ZERO),
                        research_per_hexadies: crate::modifier::ModifiedValue::new(Amt::ZERO),
                        food_per_hexadies: crate::modifier::ModifiedValue::new(Amt::ZERO),
                    },
                    BuildQueue::default(),
                    Buildings {
                        slots: vec![None; num_slots],
                    },
                    BuildingQueue::default(),
                    ProductionFocus::default(),
                    MaintenanceCost::default(),
                    FoodConsumption::default(),
                    ColonyPopulation {
                        species: vec![ColonySpecies {
                            species_id: "human".to_string(),
                            population: 10,
                        }],
                    },
                    ColonyJobs::default(),
                    ColonyJobRates::default(),
                ))
                .id();
            if let Some(e) = new_owner {
                commands
                    .entity(new_colony)
                    .insert(crate::faction::FactionOwner(e));
            }

            // Add ResourceStockpile and ResourceCapacity to the StarSystem if not already present
            if existing_stockpiles.get(system_entity).is_err() {
                commands.entity(system_entity).insert((
                    ResourceStockpile {
                        minerals: Amt::units(100),
                        energy: Amt::units(100),
                        research: Amt::ZERO,
                        food: Amt::units(50),
                        authority: Amt::ZERO,
                    },
                    ResourceCapacity::default(),
                ));
            }

            // Add SystemBuildings and SystemBuildingQueue if not already present
            if existing_system_buildings.get(system_entity).is_err() {
                commands.entity(system_entity).insert((
                    SystemBuildings {
                        slots: vec![None; crate::colony::DEFAULT_SYSTEM_BUILDING_SLOTS],
                    },
                    SystemBuildingQueue::default(),
                ));
                // #297 (S-2): When this settling created SystemBuildings on a
                // previously-unowned StarSystem, also tag it with
                // `FactionOwner` so the "administrative owner" of the system
                // matches the colony we just spawned (plan §2C).
                if let Some(e) = new_owner {
                    commands
                        .entity(system_entity)
                        .insert(crate::faction::FactionOwner(e));
                }
            }

            // #249: Dual-write ColonyEstablished.
            let event_id = fact_sys.allocate_event_id();
            let desc = format!("Colony established at {}", system_name);
            events.write(GameEvent {
                id: event_id,
                timestamp: clock.elapsed,
                kind: GameEventKind::ColonyEstablished,
                description: desc.clone(),
                related_system: Some(system_entity),
            });
            if let Some(v) = vantage {
                let fact = KnowledgeFact::ColonyEstablished {
                    event_id: Some(event_id),
                    system: system_entity,
                    planet: planet_entity,
                    name: system_name.clone(),
                    detail: desc,
                };
                fact_sys.record(fact, sys_pos_arr, clock.elapsed, &v);
            }

            info!("Colony established at {}", system_name);

            // Consume the colony ship
            commands.entity(ship_entity).despawn();
        }
    }
}

/// #98: Process ships that are being refitted — when complete, swap modules and re-dock.
#[allow(clippy::too_many_arguments)]
pub fn process_refitting(
    clock: Res<GameClock>,
    mut ships: Query<(Entity, &mut Ship, &mut ShipState)>,
    mut events: MessageWriter<GameEvent>,
    systems: Query<(&StarSystem, &Position)>,
    player_q: Query<&StationedAt, Without<Ship>>,
    player_aboard_q: Query<&AboardShip, With<Player>>,
    mut fact_sys: FactSysParam,
) {
    let player_system = player_q.iter().next().map(|s| s.system);
    let player_pos: Option<[f64; 3]> = player_system
        .and_then(|s| systems.get(s).ok())
        .map(|(_, p)| p.as_array());
    let player_aboard = player_aboard_q.iter().next().is_some();
    let vantage = player_pos.map(|pos| PlayerVantage {
        player_pos: pos,
        player_aboard,
    });

    for (_entity, mut ship, mut state) in &mut ships {
        let (system, completes_at, new_modules, target_revision) = match &*state {
            ShipState::Refitting {
                system,
                completes_at,
                new_modules,
                target_revision,
                ..
            } => (
                *system,
                *completes_at,
                new_modules.clone(),
                *target_revision,
            ),
            _ => continue,
        };

        if clock.elapsed >= completes_at {
            ship.modules = new_modules;
            // #123: Mark ship as in sync with the design revision we refit to.
            ship.design_revision = target_revision;
            *state = ShipState::Docked { system };

            let (system_name, sys_pos_arr) = systems
                .get(system)
                .map(|(s, p)| (s.name.clone(), p.as_array()))
                .unwrap_or_else(|_| ("Unknown".to_string(), [0.0; 3]));
            // #249: Dual-write refit completion.
            let event_id = fact_sys.allocate_event_id();
            let desc = format!("{} refit completed at {}", ship.name, system_name);
            events.write(GameEvent {
                id: event_id,
                timestamp: clock.elapsed,
                kind: GameEventKind::ShipBuilt,
                description: desc.clone(),
                related_system: Some(system),
            });
            if let Some(v) = vantage {
                let fact = KnowledgeFact::StructureBuilt {
                    event_id: Some(event_id),
                    system: Some(system),
                    kind: "refit".into(),
                    name: ship.name.clone(),
                    destroyed: false,
                    detail: desc,
                };
                fact_sys.record(fact, sys_pos_arr, clock.elapsed, &v);
            }
        }
    }
}

// --- Colony ship arrival (#20) ---

/// Convert a continuous resource level (0.0..1.0) to a production rate in Amt.
/// Scales linearly: 0.0 -> 0, 1.0 -> 8 units per hexadies.
///
/// #250: No longer wired into colony spawn — production now flows through
/// building + job modifiers. Kept because planet attributes (mineral_richness
/// etc.) are a likely input for a future attribute-scaled modifier system.
#[allow(dead_code)]
pub fn resource_production_rate(level: f64) -> crate::amount::Amt {
    if level <= 0.0 {
        Amt::ZERO
    } else {
        // Scale: 0.0->0, 0.4->3.2, 0.7->5.6, 1.0->8.0
        Amt::from_f64(level * 8.0)
    }
}
