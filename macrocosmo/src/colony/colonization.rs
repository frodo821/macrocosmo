use bevy::prelude::*;

use crate::amount::Amt;
use crate::colony::ColonyJobRates;
use crate::components::Position;
use crate::events::GameEvent;
use crate::faction::FactionOwner;
use crate::galaxy::{Planet, StarSystem, SystemAttributes};
use crate::knowledge::{FactSysParam, KnowledgeFact, PlayerVantage};
use crate::modifier::ModifiedValue;
use crate::player::{AboardShip, Player, PlayerEmpire, StationedAt};
use crate::ship::{Ship, ShipState};
use crate::ship_design::ShipDesignRegistry;
use crate::species::{ColonyJobs, ColonyPopulation, ColonySpecies};
use crate::time_system::GameClock;

use super::{
    BuildQueue, BuildingQueue, Buildings, Colony, FoodConsumption,
    LastProductionTick, MaintenanceCost, Production, ProductionFocus, ResourceCapacity,
    ResourceStockpile, SlotAssignment, SystemBuildingQueue, SystemBuildings,
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
    // #297 (S-2): Resolve the player faction so Colony + StarSystem can be
    // tagged with `FactionOwner` at spawn time. `PlayerEmpire` carries the
    // canonical diplomatic identity for the human player in single-faction
    // mode; multi-faction expansion will rewrite this lookup without
    // changing the spawn-site shape.
    empire_q: Query<Entity, With<PlayerEmpire>>,
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

    // #297 (S-2): Resolve faction entity; warn + skip attachment if missing
    // (same defensive pattern used by `tick_build_queue` ship_owner).
    let faction_entity: Option<Entity> = empire_q.iter().next();
    if faction_entity.is_none() {
        warn!(
            "No PlayerEmpire found when spawning capital colony at {} — \
             Colony and StarSystem will not carry FactionOwner",
            capital_star.name
        );
    }

    let num_slots = attributes.max_building_slots as usize;
    let slots = vec![None; num_slots];

    let colony_entity = commands
        .spawn((
            Colony {
                planet: planet_entity,
                growth_rate: 0.01,
            },
            // #250: Production starts at zero; all output comes from buildings
            // (automation modifiers) + pop-driven job contributions, never from
            // a hidden base rate. Legacy code seeded this with +5 everywhere,
            // causing empty colonies to "self-produce" and masking the real
            // job-system pipeline.
            Production {
                minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
                energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
                research_per_hexadies: ModifiedValue::new(Amt::ZERO),
                food_per_hexadies: ModifiedValue::new(Amt::ZERO),
            },
            BuildQueue::default(),
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
                growth_accumulator: 0.0,
            },
            ColonyJobs::default(),
            ColonyJobRates::default(),
        ))
        .id();
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
        SystemBuildings::default(),
        SystemBuildingQueue::default(),
    ));
    // #297 (S-2): Tag Colony and StarSystem with their administrative
    // owner. StarSystem receives FactionOwner because `SystemBuildings` is
    // attached to the same entity (plan §2C). `Sovereignty.owner` is
    // derived separately from Core-ship presence (#295) and may disagree
    // when an enemy Core ship enters the system — that's intentional.
    if let Some(empire) = faction_entity {
        commands.entity(colony_entity).insert(FactionOwner(empire));
        commands.entity(capital_entity).insert(FactionOwner(empire));
    }
    info!("Capital colony scaffold created on {}", capital_star.name);
}

/// #114: Process colonization orders on star systems.
/// Deducts resources, counts down build time, and spawns a new colony on completion.
#[allow(clippy::too_many_arguments)]
pub fn tick_colonization_queue(
    mut commands: Commands,
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    mut systems_with_queue: Query<(Entity, &mut ColonizationQueue, &mut ResourceStockpile)>,
    mut colonies: Query<&mut Colony>,
    mut colony_pops: Query<&mut ColonyPopulation>,
    // #297 (S-2): Read FactionOwner off the source colony so the new
    // colony inherits administrative ownership. Separate read-only query
    // to avoid a mutable-vs-immutable conflict with `colonies`.
    source_owners: Query<&FactionOwner>,
    planet_query: Query<(Entity, &Planet, &SystemAttributes)>,
    positions: Query<&Position>,
    player_q: Query<&StationedAt, With<Player>>,
    ruler_aboard_q: Query<&AboardShip, With<Player>>,
    mut events: MessageWriter<GameEvent>,
    mut fact_sys: FactSysParam,
    building_registry: Res<super::BuildingRegistry>,
    design_registry: Res<ShipDesignRegistry>,
    ship_q: Query<(&Ship, &ShipState)>,
) {
    let player_system = player_q.iter().next().map(|s| s.system);
    let player_pos: Option<[f64; 3]> = player_system
        .and_then(|s| positions.get(s).ok())
        .map(|p| p.as_array());
    let ruler_aboard = ruler_aboard_q.iter().next().is_some();
    let vantage = player_pos.map(|pos| PlayerVantage {
        player_pos: pos,
        ruler_aboard,
    });
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
            if let Ok(mut source_pop) = colony_pops.get_mut(order.source_colony) {
                let transfer = (order.initial_population.round() as u32)
                    .min(source_pop.total().saturating_sub(1));
                // Remove `transfer` pops from the first species that has enough
                if let Some(sp) = source_pop.species.first_mut() {
                    sp.population = sp.population.saturating_sub(transfer);
                }
            }

            // Get planet attributes.
            let planet_name = if let Ok((_, planet, _)) = planet_query.get(order.target_planet) {
                planet.name.clone()
            } else {
                continue;
            };

            // #280: Determine slot count from colony_hub_t1's fixed_slots capability.
            // Falls back to planet max_building_slots if hub definition is missing.
            let (num_slots, hub_building) =
                crate::colony::hub_slots_for_new_colony(&building_registry, || {
                    planet_query
                        .get(order.target_planet)
                        .ok()
                        .map(|(_, _, a)| a.max_building_slots as usize)
                        .unwrap_or(4)
                });

            // Spawn the new colony
            let pop_count = order.initial_population.round().max(0.0) as u32;
            // #297 (S-2): Inherit administrative ownership from the source
            // colony that funded this expansion. If the source lacks a
            // FactionOwner (legacy save, test spawn, etc.) the new colony
            // also goes un-tagged — warn-and-skip rather than guess.
            let inherited_owner: Option<FactionOwner> =
                source_owners.get(order.source_colony).ok().copied();
            if inherited_owner.is_none() {
                warn!(
                    "Colonization order from source {:?} has no FactionOwner; \
                     new colony will not carry one either",
                    order.source_colony
                );
            }
            // #280: Build slots vec with hub pre-placed at slot 0.
            let mut slots = vec![None; num_slots];
            if let Some(hub_id) = hub_building {
                if !slots.is_empty() {
                    slots[0] = Some(hub_id);
                }
            }
            let new_colony = commands
                .spawn((
                    Colony {
                        planet: order.target_planet,
                        growth_rate: 0.005,
                    },
                    // #250: see comment in spawn_capital_colony above.
                    Production {
                        minerals_per_hexadies: crate::modifier::ModifiedValue::new(Amt::ZERO),
                        energy_per_hexadies: crate::modifier::ModifiedValue::new(Amt::ZERO),
                        research_per_hexadies: crate::modifier::ModifiedValue::new(Amt::ZERO),
                        food_per_hexadies: crate::modifier::ModifiedValue::new(Amt::ZERO),
                    },
                    BuildQueue::default(),
                    Buildings { slots },
                    BuildingQueue::default(),
                    ProductionFocus::default(),
                    MaintenanceCost::default(),
                    FoodConsumption::default(),
                    ColonyPopulation {
                        species: vec![ColonySpecies {
                            species_id: "human".to_string(),
                            population: pop_count,
                        }],
                        growth_accumulator: 0.0,
                    },
                    ColonyJobs::default(),
                    ColonyJobRates::default(),
                ))
                .id();
            if let Some(owner) = inherited_owner {
                commands.entity(new_colony).insert(owner);
            }

            // #249: Dual-write ColonyEstablished.
            let event_id = fact_sys.allocate_event_id();
            let desc = format!("New colony established on {}", planet_name);
            events.write(crate::events::GameEvent {
                id: event_id,
                timestamp: clock.elapsed,
                kind: crate::events::GameEventKind::ColonyEstablished,
                description: desc.clone(),
                related_system: Some(system_entity),
            });
            let origin_pos: Option<[f64; 3]> =
                positions.get(system_entity).ok().map(|p| p.as_array());
            if let (Some(v), Some(op)) = (vantage, origin_pos) {
                let fact = KnowledgeFact::ColonyEstablished {
                    event_id: Some(event_id),
                    system: system_entity,
                    planet: order.target_planet,
                    name: planet_name.clone(),
                    detail: desc,
                };
                fact_sys.record(fact, op, clock.elapsed, &v);
            }

            // #387: Auto-spawn a Shipyard station if none exists in this system.
            if !crate::ship::system_has_station_ship("station_shipyard_v1", system_entity, &ship_q)
            {
                let owner = source_owners
                    .get(order.source_colony)
                    .ok()
                    .map(|fo| crate::ship::Owner::Empire(fo.0))
                    .unwrap_or(crate::ship::Owner::Neutral);
                let sys_pos = positions
                    .get(system_entity)
                    .copied()
                    .unwrap_or(Position::from([0.0, 0.0, 0.0]));
                let ship_entity = crate::ship::spawn_ship(
                    &mut commands,
                    "station_shipyard_v1",
                    "Shipyard".to_string(),
                    system_entity,
                    sys_pos,
                    owner,
                    &design_registry,
                );
                // Assign the first free slot (or slot 0 if no SystemBuildings yet).
                commands
                    .entity(ship_entity)
                    .insert(SlotAssignment(0));
                info!(
                    "Auto-spawned Shipyard station at {} on colonization",
                    planet_name
                );
            }

            info!(
                "Colony established on {} via build queue colonization",
                planet_name
            );
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
            commands
                .entity(order.system_entity)
                .insert(ColonizationQueue {
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
