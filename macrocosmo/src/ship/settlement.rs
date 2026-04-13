use bevy::prelude::*;

use crate::amount::Amt;
use crate::colony::{
    BuildQueue, Buildings, BuildingQueue, Colony, ColonyJobRates, FoodConsumption,
    MaintenanceCost, Production, ProductionFocus, ResourceCapacity, ResourceStockpile,
    SystemBuildings, SystemBuildingQueue,
};
use crate::components::Position;
use crate::knowledge::{
    record_world_event_fact, FactSysParam, KnowledgeFact, PlayerVantage,
};
use crate::player::{AboardShip, Player, StationedAt};
use crate::species::{ColonyJobs, ColonyPopulation, ColonySpecies};
use crate::events::{GameEvent, GameEventKind};
use crate::galaxy::{HostilePresence, StarSystem, SystemAttributes};
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
    mut ships: Query<(Entity, &Ship, &mut ShipState)>,
    systems: Query<(&StarSystem, &Position)>,
    planet_query: Query<(Entity, &crate::galaxy::Planet, &SystemAttributes)>,
    existing_colonies: Query<&Colony>,
    existing_stockpiles: Query<&ResourceStockpile, With<StarSystem>>,
    existing_system_buildings: Query<&SystemBuildings>,
    mut events: MessageWriter<GameEvent>,
    hostiles: Query<&HostilePresence>,
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
    for (ship_entity, ship, mut state) in &mut ships {
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
            let has_hostile = hostiles.iter().any(|h| h.system == system_entity);
            if has_hostile {
                info!(
                    "Colony Ship {} cannot settle at {} — hostile presence!",
                    ship.name, star_system.name
                );
                *state = ShipState::Docked { system: system_entity };
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
                    let comms = fact_sys
                        .empire_comms
                        .iter()
                        .next()
                        .cloned()
                        .unwrap_or_default();
                    let relays = fact_sys.relay_network.relays.clone();
                    let fact = KnowledgeFact::ColonyFailed {
                        event_id: Some(event_id),
                        system: system_entity,
                        name: star_system.name.clone(),
                        reason: "hostile presence".into(),
                    };
                    let _ = desc;
                    record_world_event_fact(
                        fact,
                        sys_pos_arr,
                        clock.elapsed,
                        &v,
                        &mut fact_sys.fact_queue,
                        &mut fact_sys.notifications,
                        &mut fact_sys.notified_ids,
                        &relays,
                        &comms,
                    );
                }
                continue;
            }

            // Collect planets that already have a colony
            let colonized_planets: Vec<Entity> = existing_colonies.iter()
                .map(|c| c.planet)
                .collect();

            // If a specific planet was targeted, try to use it
            let target_planet = if let Some(target_pe) = target_planet_entity {
                // Verify target planet is valid and not already colonized
                if colonized_planets.contains(&target_pe) {
                    info!("Target planet in {} is already colonized, settling aborted", star_system.name);
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
                info!("Colony Ship {} found no habitable planet at {}", ship.name, star_system.name);
                commands.entity(ship_entity).despawn();
                continue;
            };

            let system_name = star_system.name.clone();
            let num_slots = attrs.max_building_slots as usize;

            commands.spawn((
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
                BuildQueue {
                    queue: Vec::new(),
                },
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
            ));

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
                let comms = fact_sys
                    .empire_comms
                    .iter()
                    .next()
                    .cloned()
                    .unwrap_or_default();
                let relays = fact_sys.relay_network.relays.clone();
                let fact = KnowledgeFact::ColonyEstablished {
                    event_id: Some(event_id),
                    system: system_entity,
                    planet: planet_entity,
                    name: system_name.clone(),
                    detail: desc,
                };
                record_world_event_fact(
                    fact,
                    sys_pos_arr,
                    clock.elapsed,
                    &v,
                    &mut fact_sys.fact_queue,
                    &mut fact_sys.notifications,
                    &mut fact_sys.notified_ids,
                    &relays,
                    &comms,
                );
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
            ShipState::Refitting { system, completes_at, new_modules, target_revision, .. } => {
                (*system, *completes_at, new_modules.clone(), *target_revision)
            }
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
                let comms = fact_sys
                    .empire_comms
                    .iter()
                    .next()
                    .cloned()
                    .unwrap_or_default();
                let relays = fact_sys.relay_network.relays.clone();
                let fact = KnowledgeFact::StructureBuilt {
                    event_id: Some(event_id),
                    system: Some(system),
                    kind: "refit".into(),
                    name: ship.name.clone(),
                    destroyed: false,
                    detail: desc,
                };
                record_world_event_fact(
                    fact,
                    sys_pos_arr,
                    clock.elapsed,
                    &v,
                    &mut fact_sys.fact_queue,
                    &mut fact_sys.notifications,
                    &mut fact_sys.notified_ids,
                    &relays,
                    &comms,
                );
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
