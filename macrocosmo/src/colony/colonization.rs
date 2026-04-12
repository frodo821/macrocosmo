use bevy::prelude::*;

use crate::amount::Amt;
use crate::galaxy::{Planet, StarSystem, SystemAttributes};
use crate::modifier::ModifiedValue;
use crate::events::{GameEvent, GameEventKind};
use crate::species::{ColonyJobs, ColonyPopulation, ColonySpecies};
use crate::time_system::GameClock;

use super::{
    Buildings, BuildQueue, BuildingQueue, Colony, FoodConsumption, LastProductionTick,
    MaintenanceCost, Production, ProductionFocus, ResourceCapacity, ResourceStockpile,
    SystemBuildings, SystemBuildingQueue, DEFAULT_SYSTEM_BUILDING_SLOTS,
};

/// #114: Default cost/time to colonize a new planet from an existing colony in the same system.
///
/// #160: Canonical values live in `GameBalance` (Lua-defined). These
/// constants are retained as fallbacks when the `GameBalance` resource
/// isn't available (e.g. UI display paths, tests).
pub const COLONIZATION_MINERAL_COST: Amt = Amt::units(300);
pub const COLONIZATION_ENERGY_COST: Amt = Amt::units(200);
pub const COLONIZATION_BUILD_TIME: i64 = 90;
pub const COLONIZATION_POPULATION_TRANSFER: f64 = 10.0;
pub const COLONIZATION_MIN_POPULATION: f64 = 20.0;

/// #114: Queue for same-system colonization orders (attached to StarSystem entities).
#[derive(Component, Default)]
pub struct ColonizationQueue {
    pub orders: Vec<ColonizationOrder>,
}

/// #114: A single colonization order in the queue.
pub struct ColonizationOrder {
    pub target_planet: Entity,
    pub source_colony: Entity,
    pub minerals_remaining: Amt,
    pub energy_remaining: Amt,
    pub build_time_remaining: i64,
    pub initial_population: f64,
}

/// #114: Pending colonization order spawned by UI, consumed by `apply_pending_colonization_orders`.
#[derive(Component)]
pub struct PendingColonizationOrder {
    pub system_entity: Entity,
    pub target_planet: Entity,
    pub source_colony: Entity,
}

/// Create the capital colony scaffolding (Colony, Buildings, SystemBuildings, ResourceStockpile)
/// with EMPTY building slots. Buildings and initial ships are added by the faction's
/// on_game_start Lua callback (see `run_faction_on_game_start` in `scripting::game_start_ctx`).
pub fn spawn_capital_colony(
    mut commands: Commands,
    systems: Query<(Entity, &StarSystem)>,
    planets: Query<(Entity, &crate::galaxy::Planet, &SystemAttributes)>,
) {
    // Find the capital star system
    let capital_system = systems.iter().find(|(_, s)| s.is_capital);
    let Some((capital_entity, capital_star)) = capital_system else {
        warn!("No capital star system found; capital colony not created");
        return;
    };

    // Find the first planet of the capital system
    let capital_planet = planets.iter().find(|(_, p, _)| p.system == capital_entity);
    let Some((planet_entity, _, attributes)) = capital_planet else {
        warn!("No planet found for capital system; capital colony not created");
        return;
    };

    let num_slots = attributes.max_building_slots as usize;
    let slots = vec![None; num_slots];
    let system_slots = vec![None; DEFAULT_SYSTEM_BUILDING_SLOTS];

    commands.spawn((
        Colony {
            planet: planet_entity,
            population: 100.0,
            growth_rate: 0.01,
        },
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::units(5)),
            energy_per_hexadies: ModifiedValue::new(Amt::units(5)),
            research_per_hexadies: ModifiedValue::new(Amt::units(1)),
            food_per_hexadies: ModifiedValue::new(Amt::units(5)),
        },
        BuildQueue {
            queue: Vec::new(),
        },
        Buildings { slots },
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
        ColonyPopulation {
            species: vec![ColonySpecies {
                species_id: "human".to_string(),
                population: 100,
            }],
        },
        ColonyJobs::default(),
    ));
    // Add ResourceStockpile, ResourceCapacity, and SystemBuildings to the StarSystem entity
    commands.entity(capital_entity).insert((
        ResourceStockpile {
            minerals: Amt::units(500),
            energy: Amt::units(500),
            research: Amt::ZERO,
            food: Amt::units(200),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        SystemBuildings { slots: system_slots },
        SystemBuildingQueue::default(),
    ));
    info!("Capital colony scaffold created on {}", capital_star.name);
}

/// #114: Process colonization orders on star systems.
/// Deducts resources, counts down build time, and spawns a new colony on completion.
pub fn tick_colonization_queue(
    mut commands: Commands,
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    mut systems_with_queue: Query<(Entity, &mut ColonizationQueue, &mut ResourceStockpile)>,
    mut colonies: Query<&mut Colony>,
    planet_query: Query<(Entity, &Planet, &SystemAttributes)>,
    mut events: MessageWriter<GameEvent>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }

    for (system_entity, mut cq, mut stockpile) in &mut systems_with_queue {
        let mut completed: Vec<usize> = Vec::new();

        for (i, order) in cq.orders.iter_mut().enumerate() {
            for _ in 0..delta {
                let minerals_transfer = order.minerals_remaining.min(stockpile.minerals);
                order.minerals_remaining = order.minerals_remaining.sub(minerals_transfer);
                stockpile.minerals = stockpile.minerals.sub(minerals_transfer);

                let energy_transfer = order.energy_remaining.min(stockpile.energy);
                order.energy_remaining = order.energy_remaining.sub(energy_transfer);
                stockpile.energy = stockpile.energy.sub(energy_transfer);

                order.build_time_remaining -= 1;

                if order.minerals_remaining == Amt::ZERO
                    && order.energy_remaining == Amt::ZERO
                    && order.build_time_remaining <= 0
                {
                    completed.push(i);
                    break;
                }
            }
        }

        // Process completions in reverse to maintain indices
        for &idx in completed.iter().rev() {
            let order = cq.orders.remove(idx);

            // Transfer population from source colony
            if let Ok(mut source) = colonies.get_mut(order.source_colony) {
                let transfer = order.initial_population.min(source.population - 1.0);
                source.population -= transfer;
            }

            // Get planet attributes for production rates
            let (planet_name, minerals_rate, energy_rate, research_rate, num_slots) =
                if let Ok((_, planet, attrs)) = planet_query.get(order.target_planet) {
                    (
                        planet.name.clone(),
                        crate::ship::resource_production_rate(attrs.mineral_richness),
                        crate::ship::resource_production_rate(attrs.energy_potential),
                        crate::ship::resource_production_rate(attrs.research_potential),
                        attrs.max_building_slots as usize,
                    )
                } else {
                    continue;
                };

            // Spawn the new colony
            commands.spawn((
                Colony {
                    planet: order.target_planet,
                    population: order.initial_population,
                    growth_rate: 0.005,
                },
                Production {
                    minerals_per_hexadies: crate::modifier::ModifiedValue::new(minerals_rate),
                    energy_per_hexadies: crate::modifier::ModifiedValue::new(energy_rate),
                    research_per_hexadies: crate::modifier::ModifiedValue::new(research_rate),
                    food_per_hexadies: crate::modifier::ModifiedValue::new(Amt::ZERO),
                },
                BuildQueue { queue: Vec::new() },
                Buildings { slots: vec![None; num_slots] },
                BuildingQueue::default(),
                ProductionFocus::default(),
                MaintenanceCost::default(),
                FoodConsumption::default(),
            ));

            events.write(crate::events::GameEvent {
                timestamp: clock.elapsed,
                kind: crate::events::GameEventKind::ColonyEstablished,
                description: format!("New colony established on {}", planet_name),
                related_system: Some(system_entity),
            });

            info!("Colony established on {} via build queue colonization", planet_name);
        }
    }
}

/// #114: Consume PendingColonizationOrder entities and add them to the system's ColonizationQueue.
///
/// #160: Uses `GameBalance` for colonization costs and build time.
pub fn apply_pending_colonization_orders(
    mut commands: Commands,
    pending: Query<(Entity, &PendingColonizationOrder)>,
    mut queues: Query<&mut ColonizationQueue>,
    balance: Res<crate::technology::GameBalance>,
) {
    let mineral_cost = balance.colonization_mineral_cost();
    let energy_cost = balance.colonization_energy_cost();
    let build_time = balance.colonization_build_time();
    for (entity, order) in &pending {
        // Get or create the ColonizationQueue on the system
        if let Ok(mut cq) = queues.get_mut(order.system_entity) {
            cq.orders.push(ColonizationOrder {
                target_planet: order.target_planet,
                source_colony: order.source_colony,
                minerals_remaining: mineral_cost,
                energy_remaining: energy_cost,
                build_time_remaining: build_time,
                initial_population: COLONIZATION_POPULATION_TRANSFER,
            });
        } else {
            commands.entity(order.system_entity).insert(ColonizationQueue {
                orders: vec![ColonizationOrder {
                    target_planet: order.target_planet,
                    source_colony: order.source_colony,
                    minerals_remaining: mineral_cost,
                    energy_remaining: energy_cost,
                    build_time_remaining: build_time,
                    initial_population: COLONIZATION_POPULATION_TRANSFER,
                }],
            });
        }
        commands.entity(entity).despawn();
    }
}
