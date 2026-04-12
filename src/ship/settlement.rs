use bevy::prelude::*;

use crate::amount::Amt;
use crate::colony::{
    BuildQueue, Buildings, BuildingQueue, Colony, FoodConsumption, MaintenanceCost,
    Production, ProductionFocus, ResourceCapacity, ResourceStockpile,
    SystemBuildings, SystemBuildingQueue,
};
use crate::events::{GameEvent, GameEventKind};
use crate::galaxy::{HostilePresence, StarSystem, SystemAttributes};
use crate::time_system::GameClock;

use super::{Ship, ShipState};

/// Duration of a colonization/settling operation in hexadies (60 hexadies = 1 year) (#32)
pub const SETTLING_DURATION_HEXADIES: i64 = 60;

/// System that processes ongoing settling operations. When the timer completes,
/// establishes a colony on the first habitable planet and despawns the colony ship.
pub fn process_settling(
    mut commands: Commands,
    clock: Res<GameClock>,
    mut ships: Query<(Entity, &Ship, &mut ShipState)>,
    systems: Query<&StarSystem>,
    planet_query: Query<(Entity, &crate::galaxy::Planet, &SystemAttributes)>,
    existing_colonies: Query<&Colony>,
    existing_stockpiles: Query<&ResourceStockpile, With<StarSystem>>,
    existing_system_buildings: Query<&SystemBuildings>,
    mut events: MessageWriter<GameEvent>,
    hostiles: Query<&HostilePresence>,
) {
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
            let Ok(star_system) = systems.get(system_entity) else {
                continue;
            };

            // #52/#56: Check for hostile presence — cannot colonize while hostiles remain
            let has_hostile = hostiles.iter().any(|h| h.system == system_entity);
            if has_hostile {
                info!(
                    "Colony Ship {} cannot settle at {} — hostile presence!",
                    ship.name, star_system.name
                );
                *state = ShipState::Docked { system: system_entity };
                events.write(GameEvent {
                    timestamp: clock.elapsed,
                    kind: GameEventKind::ColonyFailed,
                    description: format!(
                        "Cannot establish colony at {} — hostile presence must be eliminated first!",
                        star_system.name
                    ),
                    related_system: Some(system_entity),
                });
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
            let minerals_rate = resource_production_rate(attrs.mineral_richness);
            let energy_rate = resource_production_rate(attrs.energy_potential);
            let research_rate = resource_production_rate(attrs.research_potential);
            let num_slots = attrs.max_building_slots as usize;

            commands.spawn((
                Colony {
                    planet: planet_entity,
                    population: 10.0,
                    growth_rate: 0.005,
                },
                Production {
                    minerals_per_hexadies: crate::modifier::ModifiedValue::new(minerals_rate),
                    energy_per_hexadies: crate::modifier::ModifiedValue::new(energy_rate),
                    research_per_hexadies: crate::modifier::ModifiedValue::new(research_rate),
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

            events.write(GameEvent {
                timestamp: clock.elapsed,
                kind: GameEventKind::ColonyEstablished,
                description: format!("Colony established at {}", system_name),
                related_system: Some(system_entity),
            });

            info!("Colony established at {} (M:{}/E:{}/R:{} per sd)", system_name, minerals_rate, energy_rate, research_rate);

            // Consume the colony ship
            commands.entity(ship_entity).despawn();
        }
    }
}

/// #98: Process ships that are being refitted — when complete, swap modules and re-dock.
pub fn process_refitting(
    clock: Res<GameClock>,
    mut ships: Query<(Entity, &mut Ship, &mut ShipState)>,
    mut events: MessageWriter<GameEvent>,
    systems: Query<&StarSystem>,
) {
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

            let system_name = systems
                .get(system)
                .map(|s| s.name.clone())
                .unwrap_or_else(|_| "Unknown".to_string());
            events.write(GameEvent {
                timestamp: clock.elapsed,
                kind: GameEventKind::ShipBuilt,
                description: format!("{} refit completed at {}", ship.name, system_name),
                related_system: Some(system),
            });
        }
    }
}

// --- Colony ship arrival (#20) ---

/// Convert a continuous resource level (0.0..1.0) to a production rate in Amt.
/// Scales linearly: 0.0 -> 0, 1.0 -> 8 units per hexadies.
pub fn resource_production_rate(level: f64) -> crate::amount::Amt {
    if level <= 0.0 {
        Amt::ZERO
    } else {
        // Scale: 0.0->0, 0.4->3.2, 0.7->5.6, 1.0->8.0
        Amt::from_f64(level * 8.0)
    }
}
