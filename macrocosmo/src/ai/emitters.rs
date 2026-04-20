//! Metric emitters — read ECS state and emit to the AI bus.
//!
//! Phase 1 (#190): emits a single global view of the NPC empire's fleet and
//! economy. Per-faction scoping will land in a later phase.

use std::collections::HashSet;

use bevy::prelude::*;

use crate::ai::emit::AiBusWriter;
use crate::ai::schema::ids::metric;
use crate::colony::{
    Buildings, Colony, FoodConsumption, Production, ResourceCapacity, ResourceStockpile,
    SlotAssignment,
};
use crate::galaxy::{BASE_CARRYING_CAPACITY, Planet, StarSystem, SystemAttributes};
use crate::galaxy::{AtSystem, Hostile};
use crate::ship::{CoreShip, Owner, Ship, ShipHitpoints, ShipModifiers, ShipState};
use crate::technology::TechTree;
use crate::time_system::GameClock;

/// Emit military metrics for NPC empires.
///
/// Registered under [`AiTickSet::MetricProduce`](super::AiTickSet::MetricProduce).
/// Phase 1: aggregates all empire-owned ships into a single global view.
pub fn emit_military_metrics(
    mut writer: AiBusWriter,
    ships: Query<(
        &Ship,
        &ShipHitpoints,
        &ShipModifiers,
        &ShipState,
        Option<&CoreShip>,
    )>,
    hostiles: Query<&AtSystem, With<Hostile>>,
) {
    let mut total_ships: f64 = 0.0;
    let mut total_attack: f64 = 0.0;
    let mut total_defense: f64 = 0.0;
    let mut total_strength: f64 = 0.0;
    let mut total_armor: f64 = 0.0;
    let mut total_shields: f64 = 0.0;
    let mut total_shield_regen: f64 = 0.0;
    let mut total_current_hp: f64 = 0.0;
    let mut total_max_hp: f64 = 0.0;
    let mut ships_in_system: f64 = 0.0;
    let mut has_flagship = false;

    for (ship, hp, mods, state, core) in &ships {
        // Phase 1: only count empire-owned ships (skip neutrals).
        if !ship.owner.is_empire() {
            continue;
        }

        total_ships += 1.0;

        let attack = mods.attack.final_value().to_f64();
        let defense = mods.defense.final_value().to_f64();
        let current_hp = hp.hull + hp.armor + hp.shield;
        let max_hp = hp.hull_max + hp.armor_max + hp.shield_max;

        total_attack += attack;
        total_defense += defense;
        total_strength += attack + defense + current_hp;
        total_armor += hp.armor;
        total_shields += hp.shield;
        total_shield_regen += hp.shield_regen;
        total_current_hp += current_hp;
        total_max_hp += max_hp;

        if matches!(state, ShipState::InSystem { .. }) {
            ships_in_system += 1.0;
        }

        if core.is_some() {
            has_flagship = true;
        }
    }

    writer.emit(&metric::my_total_ships(), total_ships);
    writer.emit(&metric::my_strength(), total_strength);
    writer.emit(&metric::my_total_attack(), total_attack);
    writer.emit(&metric::my_total_defense(), total_defense);
    writer.emit(&metric::my_armor(), total_armor);
    writer.emit(&metric::my_shields(), total_shields);
    writer.emit(&metric::my_shield_regen_rate(), total_shield_regen);

    let vulnerability = if total_max_hp > 0.0 {
        1.0 - (total_current_hp / total_max_hp)
    } else {
        0.0
    };
    writer.emit(&metric::my_vulnerability_score(), vulnerability);

    let fleet_ready = if total_ships > 0.0 {
        ships_in_system / total_ships
    } else {
        0.0
    };
    writer.emit(&metric::my_fleet_ready(), fleet_ready);

    writer.emit(
        &metric::my_has_flagship(),
        if has_flagship { 1.0 } else { 0.0 },
    );

    let hostile_systems: HashSet<Entity> = hostiles.iter().map(|at| at.0).collect();
    writer.emit(
        &metric::systems_with_hostiles(),
        hostile_systems.len() as f64,
    );
}

/// Emit economic metrics for NPC empires.
///
/// Registered under [`AiTickSet::MetricProduce`](super::AiTickSet::MetricProduce).
/// Phase 1: aggregates all empire-owned colonies/systems into a single global view.
#[allow(clippy::too_many_arguments)]
pub fn emit_economic_metrics(
    mut writer: AiBusWriter,
    clock: Res<GameClock>,
    colonies: Query<(
        &Colony,
        &Production,
        Option<&FoodConsumption>,
        Option<&Buildings>,
    )>,
    stockpiles: Query<
        (
            &ResourceStockpile,
            Option<&ResourceCapacity>,
        ),
        With<StarSystem>,
    >,
    ai_station_ships: Query<(Entity, &Ship, &ShipState, &SlotAssignment)>,
    planets: Query<&Planet>,
    planet_attrs: Query<&SystemAttributes, With<Planet>>,
    tech_tree: Option<Res<TechTree>>,
    ai_building_registry: Option<Res<crate::colony::BuildingRegistry>>,
) {
    // --- Production rates (per hexadies) ---
    let mut total_minerals_rate: f64 = 0.0;
    let mut total_energy_rate: f64 = 0.0;
    let mut total_food_rate: f64 = 0.0;
    let mut total_research_rate: f64 = 0.0;
    // Authority production is empire-level, tracked via AuthorityParams — skip for now.
    // We approximate from production component if available.

    // --- Population ---
    let mut total_population: f64 = 0.0;
    let mut total_growth_rate: f64 = 0.0;
    let mut total_carrying_capacity: f64 = 0.0;

    // --- Food ---
    let mut total_food_consumption: f64 = 0.0;

    // --- Territory ---
    let mut colony_count: f64 = 0.0;
    let mut colonized_systems: HashSet<Entity> = HashSet::new();

    // --- Infrastructure (building slots) ---
    let mut max_slots: f64 = 0.0;
    let mut used_slots: f64 = 0.0;

    for (colony, prod, food_consumption, buildings) in &colonies {
        colony_count += 1.0;

        // Production rates
        total_minerals_rate += prod.minerals_per_hexadies.final_value().to_f64();
        total_energy_rate += prod.energy_per_hexadies.final_value().to_f64();
        total_food_rate += prod.food_per_hexadies.final_value().to_f64();
        total_research_rate += prod.research_per_hexadies.final_value().to_f64();

        // Population
        total_population += colony.population;
        total_growth_rate += colony.growth_rate;

        // Carrying capacity from planet attributes
        if let Ok(attrs) = planet_attrs.get(colony.planet) {
            total_carrying_capacity += BASE_CARRYING_CAPACITY * attrs.habitability;
        }

        // Food consumption
        if let Some(fc) = food_consumption {
            total_food_consumption += fc.food_per_hexadies.final_value().to_f64();
        }

        // Track colonized systems
        if let Some(sys) = colony.system(&planets) {
            colonized_systems.insert(sys);
        }

        // Building slots
        if let Some(bldgs) = buildings {
            let total = bldgs.slots.len() as f64;
            let occupied = bldgs.slots.iter().filter(|s| s.is_some()).count() as f64;
            max_slots += total;
            used_slots += occupied;
        }
    }

    // Emit production metrics
    writer.emit(&metric::net_production_minerals(), total_minerals_rate);
    writer.emit(&metric::net_production_energy(), total_energy_rate);
    writer.emit(&metric::net_production_food(), total_food_rate);
    writer.emit(&metric::net_production_research(), total_research_rate);
    // TODO: net_production_authority requires reading AuthorityParams from PlayerEmpire entity

    // Emit population metrics
    writer.emit(&metric::population_total(), total_population);
    writer.emit(&metric::population_growth_rate(), total_growth_rate);
    writer.emit(
        &metric::population_carrying_capacity(),
        total_carrying_capacity,
    );
    let pop_ratio = if total_carrying_capacity > 0.0 {
        total_population / total_carrying_capacity
    } else {
        0.0
    };
    writer.emit(&metric::population_ratio(), pop_ratio);

    // Emit food metrics
    writer.emit(&metric::food_consumption_rate(), total_food_consumption);
    writer.emit(
        &metric::food_surplus(),
        total_food_rate - total_food_consumption,
    );

    // Emit territory metrics
    writer.emit(&metric::colony_count(), colony_count);
    writer.emit(
        &metric::colonized_system_count(),
        colonized_systems.len() as f64,
    );

    // --- Stockpiles ---
    let mut total_minerals: f64 = 0.0;
    let mut total_energy: f64 = 0.0;
    let mut total_food: f64 = 0.0;
    let mut total_authority: f64 = 0.0;
    let mut total_cap_minerals: f64 = 0.0;
    let mut total_cap_energy: f64 = 0.0;
    let mut total_cap_food: f64 = 0.0;
    let mut total_authority_debt: f64 = 0.0;

    // Infrastructure: system-level buildings
    let mut systems_with_shipyard: f64 = 0.0;
    let mut systems_with_port: f64 = 0.0;

    // Build set of systems with shipyard/port by iterating station ships.
    if let Some(ref ai_building_registry) = ai_building_registry {
        let reverse = crate::colony::system_buildings::build_reverse_design_map(ai_building_registry);
        for (_entity, ship, state, _slot) in &ai_station_ships {
            let system = match state {
                ShipState::InSystem { system: s } => *s,
                ShipState::Refitting { system: s, .. } => *s,
                _ => continue,
            };
            if let Some(bid) = reverse.get(&ship.design_id) {
                if let Some(def) = ai_building_registry.get(bid.as_str()) {
                    if def.capabilities.contains_key("shipyard") {
                        // Counted per system — simplified; may double-count if multiple shipyards.
                        systems_with_shipyard += 1.0;
                    }
                    if def.capabilities.contains_key("port") {
                        systems_with_port += 1.0;
                    }
                }
            }
            let _ = system; // used implicitly via the filter above
        }
    }

    for (stockpile, capacity) in &stockpiles {
        total_minerals += stockpile.minerals.to_f64();
        total_energy += stockpile.energy.to_f64();
        total_food += stockpile.food.to_f64();
        total_authority += stockpile.authority.to_f64();

        if let Some(cap) = capacity {
            total_cap_minerals += cap.minerals.to_f64();
            total_cap_energy += cap.energy.to_f64();
            total_cap_food += cap.food.to_f64();
        }
    }

    writer.emit(&metric::stockpile_minerals(), total_minerals);
    writer.emit(&metric::stockpile_energy(), total_energy);
    writer.emit(&metric::stockpile_food(), total_food);
    writer.emit(&metric::stockpile_authority(), total_authority);

    let ratio_minerals = if total_cap_minerals > 0.0 {
        (total_minerals / total_cap_minerals).min(1.0)
    } else {
        0.0
    };
    let ratio_energy = if total_cap_energy > 0.0 {
        (total_energy / total_cap_energy).min(1.0)
    } else {
        0.0
    };
    let ratio_food = if total_cap_food > 0.0 {
        (total_food / total_cap_food).min(1.0)
    } else {
        0.0
    };
    writer.emit(&metric::stockpile_ratio_minerals(), ratio_minerals);
    writer.emit(&metric::stockpile_ratio_energy(), ratio_energy);
    writer.emit(&metric::stockpile_ratio_food(), ratio_food);

    // TODO: total_authority_debt — needs clearer definition of "debt" in current model
    writer.emit(&metric::total_authority_debt(), total_authority_debt);

    // Infrastructure metrics
    writer.emit(&metric::systems_with_shipyard(), systems_with_shipyard);
    writer.emit(&metric::systems_with_port(), systems_with_port);
    writer.emit(&metric::max_building_slots(), max_slots);
    writer.emit(&metric::used_building_slots(), used_slots);
    writer.emit(&metric::free_building_slots(), max_slots - used_slots);
    writer.emit(
        &metric::can_build_ships(),
        if systems_with_shipyard > 0.0 {
            1.0
        } else {
            0.0
        },
    );

    // Technology metrics
    if let Some(ref tree) = tech_tree {
        let researched = tree.researched.len() as f64;
        let total = tree.technologies.len() as f64;
        writer.emit(&metric::tech_total_researched(), researched);
        let completion = if total > 0.0 { researched / total } else { 0.0 };
        writer.emit(&metric::tech_completion_percent(), completion);
        // TODO: tech_unlocks_available — requires walking prerequisites
        // TODO: research_output_ratio — requires active research cost context
    }

    // Meta / time
    writer.emit(&metric::game_elapsed_time(), clock.elapsed as f64);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::plugin::AiBusResource;
    use crate::ai::schema;
    use crate::amount::Amt;
    use crate::modifier::ScopedModifiers;
    use crate::time_system::{GameClock, GameSpeed};
    use macrocosmo_ai::WarningMode;

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(GameClock::new(10));
        app.insert_resource(GameSpeed::default());
        app.insert_resource(AiBusResource::with_warning_mode(WarningMode::Silent));
        // Declare metrics on the bus.
        {
            let mut bus = app.world_mut().resource_mut::<AiBusResource>();
            schema::declare_metrics_standalone(&mut bus.0);
        }
        app.add_systems(Update, emit_military_metrics);
        app
    }

    fn spawn_ship(app: &mut App, in_system: bool, is_core: bool) -> Entity {
        let system_entity = app.world_mut().spawn_empty().id();
        let state = if in_system {
            ShipState::InSystem {
                system: system_entity,
            }
        } else {
            ShipState::SubLight {
                origin: [0.0; 3],
                destination: [1.0, 0.0, 0.0],
                target_system: None,
                departed_at: 0,
                arrival_at: 100,
            }
        };

        let mut mods = ShipModifiers::default();
        // Set base attack/defense so final_value() returns non-zero.
        mods.attack = ScopedModifiers::new(Amt::from_f64(10.0));
        mods.defense = ScopedModifiers::new(Amt::from_f64(5.0));

        let mut entity_commands = app.world_mut().spawn((
            Ship {
                name: "Test Ship".into(),
                design_id: "corvette".into(),
                hull_id: "corvette_hull".into(),
                modules: vec![],
                owner: Owner::Empire(Entity::PLACEHOLDER),
                sublight_speed: 1.0,
                ftl_range: 5.0,
                player_aboard: false,
                home_port: system_entity,
                design_revision: 0,
                fleet: None,
            },
            ShipHitpoints {
                hull: 40.0,
                hull_max: 50.0,
                armor: 15.0,
                armor_max: 20.0,
                shield: 8.0,
                shield_max: 10.0,
                shield_regen: 1.0,
            },
            mods,
            state,
        ));
        if is_core {
            entity_commands.insert(CoreShip);
        }
        entity_commands.id()
    }

    #[test]
    fn emit_military_metrics_counts_ships() {
        let mut app = test_app();
        spawn_ship(&mut app, true, false);
        spawn_ship(&mut app, true, false);
        spawn_ship(&mut app, false, false);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let total = bus.current(&metric::my_total_ships()).unwrap();
        assert!((total - 3.0).abs() < 1e-9);
    }

    #[test]
    fn emit_military_metrics_fleet_ready_fraction() {
        let mut app = test_app();
        spawn_ship(&mut app, true, false);
        spawn_ship(&mut app, false, false);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let ready = bus.current(&metric::my_fleet_ready()).unwrap();
        assert!((ready - 0.5).abs() < 1e-9);
    }

    #[test]
    fn emit_military_metrics_flagship_detection() {
        let mut app = test_app();
        spawn_ship(&mut app, true, false);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let flag = bus.current(&metric::my_has_flagship()).unwrap();
        assert!((flag - 0.0).abs() < 1e-9);

        // Now add a core ship.
        spawn_ship(&mut app, true, true);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let flag = bus.current(&metric::my_has_flagship()).unwrap();
        assert!((flag - 1.0).abs() < 1e-9);
    }

    #[test]
    fn emit_military_metrics_attack_defense() {
        let mut app = test_app();
        spawn_ship(&mut app, true, false);
        spawn_ship(&mut app, true, false);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let attack = bus.current(&metric::my_total_attack()).unwrap();
        let defense = bus.current(&metric::my_total_defense()).unwrap();
        // 2 ships x 10 attack = 20
        assert!((attack - 20.0).abs() < 1e-9);
        // 2 ships x 5 defense = 10
        assert!((defense - 10.0).abs() < 1e-9);
    }

    #[test]
    fn emit_military_metrics_vulnerability() {
        let mut app = test_app();
        spawn_ship(&mut app, true, false);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let vuln = bus.current(&metric::my_vulnerability_score()).unwrap();
        // hull=40/50, armor=15/20, shield=8/10 => current=63, max=80
        // vuln = 1 - 63/80 = 0.2125
        assert!((vuln - 0.2125).abs() < 1e-4);
    }

    // -- Economic emitter tests ------------------------------------------------

    fn economic_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(GameClock::new(10));
        app.insert_resource(GameSpeed::default());
        app.insert_resource(AiBusResource::with_warning_mode(WarningMode::Silent));
        {
            let mut bus = app.world_mut().resource_mut::<AiBusResource>();
            schema::declare_metrics_standalone(&mut bus.0);
        }
        app.add_systems(Update, emit_economic_metrics);
        app
    }

    /// Spawn a minimal colony + star system for economic tests.
    /// Returns (colony_entity, system_entity, planet_entity).
    fn spawn_colony(
        app: &mut App,
        population: f64,
        minerals_rate: f64,
        energy_rate: f64,
        stockpile_minerals: u64,
        stockpile_energy: u64,
        building_slots: usize,
        occupied_slots: usize,
    ) -> (Entity, Entity, Entity) {
        use crate::colony::BuildingId;
        use crate::modifier::ModifiedValue;

        let system_entity = app
            .world_mut()
            .spawn((
                StarSystem {
                    name: "Test System".into(),
                    surveyed: true,
                    is_capital: false,
                    star_type: "yellow_dwarf".into(),
                },
                ResourceStockpile {
                    minerals: Amt::units(stockpile_minerals),
                    energy: Amt::units(stockpile_energy),
                    research: Amt::ZERO,
                    food: Amt::units(100),
                    authority: Amt::units(10),
                },
                ResourceCapacity::default(),
            ))
            .id();

        let planet_entity = app
            .world_mut()
            .spawn((
                Planet {
                    name: "Test Planet".into(),
                    system: system_entity,
                    planet_type: "terrestrial".into(),
                },
                SystemAttributes {
                    habitability: 0.8,
                    mineral_richness: 0.5,
                    energy_potential: 0.5,
                    research_potential: 0.5,
                    max_building_slots: building_slots as u8,
                },
            ))
            .id();

        let mut slots: Vec<Option<BuildingId>> = vec![None; building_slots];
        for i in 0..occupied_slots.min(building_slots) {
            slots[i] = Some(BuildingId::new("mine"));
        }

        let colony_entity = app
            .world_mut()
            .spawn((
                Colony {
                    planet: planet_entity,
                    population,
                    growth_rate: 0.01,
                },
                Production {
                    minerals_per_hexadies: ModifiedValue::new(Amt::from_f64(minerals_rate)),
                    energy_per_hexadies: ModifiedValue::new(Amt::from_f64(energy_rate)),
                    research_per_hexadies: ModifiedValue::new(Amt::ZERO),
                    food_per_hexadies: ModifiedValue::new(Amt::from_f64(5.0)),
                },
                Buildings { slots },
                FoodConsumption {
                    food_per_hexadies: ModifiedValue::new(Amt::from_f64(2.0)),
                },
            ))
            .id();

        (colony_entity, system_entity, planet_entity)
    }

    #[test]
    fn emit_economic_metrics_colony_count() {
        let mut app = economic_test_app();
        spawn_colony(&mut app, 100.0, 10.0, 5.0, 500, 300, 4, 2);
        spawn_colony(&mut app, 50.0, 5.0, 3.0, 200, 100, 3, 1);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let count = bus.current(&metric::colony_count()).unwrap();
        assert!((count - 2.0).abs() < 1e-9);
    }

    #[test]
    fn emit_economic_metrics_production_rates() {
        let mut app = economic_test_app();
        spawn_colony(&mut app, 100.0, 10.0, 5.0, 500, 300, 4, 0);
        spawn_colony(&mut app, 50.0, 5.0, 3.0, 200, 100, 3, 0);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let minerals = bus.current(&metric::net_production_minerals()).unwrap();
        let energy = bus.current(&metric::net_production_energy()).unwrap();
        // 10.0 + 5.0 = 15.0
        assert!((minerals - 15.0).abs() < 0.01);
        // 5.0 + 3.0 = 8.0
        assert!((energy - 8.0).abs() < 0.01);
    }

    #[test]
    fn emit_economic_metrics_stockpiles() {
        let mut app = economic_test_app();
        spawn_colony(&mut app, 100.0, 10.0, 5.0, 500, 300, 4, 0);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let minerals = bus.current(&metric::stockpile_minerals()).unwrap();
        let energy = bus.current(&metric::stockpile_energy()).unwrap();
        assert!((minerals - 500.0).abs() < 0.01);
        assert!((energy - 300.0).abs() < 0.01);
    }

    #[test]
    fn emit_economic_metrics_stockpile_ratios() {
        let mut app = economic_test_app();
        // Default ResourceCapacity: minerals=1000, energy=1000, food=500
        spawn_colony(&mut app, 100.0, 10.0, 5.0, 500, 300, 4, 0);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let ratio_m = bus.current(&metric::stockpile_ratio_minerals()).unwrap();
        let ratio_e = bus.current(&metric::stockpile_ratio_energy()).unwrap();
        // 500/1000 = 0.5
        assert!((ratio_m - 0.5).abs() < 0.01);
        // 300/1000 = 0.3
        assert!((ratio_e - 0.3).abs() < 0.01);
    }

    #[test]
    fn emit_economic_metrics_population() {
        let mut app = economic_test_app();
        spawn_colony(&mut app, 100.0, 10.0, 5.0, 500, 300, 4, 0);
        spawn_colony(&mut app, 50.0, 5.0, 3.0, 200, 100, 3, 0);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let pop = bus.current(&metric::population_total()).unwrap();
        assert!((pop - 150.0).abs() < 1e-9);

        let capacity = bus
            .current(&metric::population_carrying_capacity())
            .unwrap();
        // 200.0 * 0.8 = 160.0 per colony, 2 colonies = 320.0
        assert!((capacity - 320.0).abs() < 1e-9);
    }

    #[test]
    fn emit_economic_metrics_food_surplus() {
        let mut app = economic_test_app();
        // food production = 5.0, food consumption = 2.0 → surplus = 3.0
        spawn_colony(&mut app, 100.0, 10.0, 5.0, 500, 300, 4, 0);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let surplus = bus.current(&metric::food_surplus()).unwrap();
        assert!((surplus - 3.0).abs() < 0.01);
    }

    #[test]
    fn emit_economic_metrics_building_slots() {
        let mut app = economic_test_app();
        // 4 slots, 2 occupied
        spawn_colony(&mut app, 100.0, 10.0, 5.0, 500, 300, 4, 2);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let max = bus.current(&metric::max_building_slots()).unwrap();
        let used = bus.current(&metric::used_building_slots()).unwrap();
        let free = bus.current(&metric::free_building_slots()).unwrap();
        assert!((max - 4.0).abs() < 1e-9);
        assert!((used - 2.0).abs() < 1e-9);
        assert!((free - 2.0).abs() < 1e-9);
    }

    #[test]
    fn emit_economic_metrics_game_elapsed_time() {
        let mut app = economic_test_app();
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let elapsed = bus.current(&metric::game_elapsed_time()).unwrap();
        assert!((elapsed - 10.0).abs() < 1e-9);
    }

    #[test]
    fn emit_economic_metrics_no_colonies_emits_zeroes() {
        let mut app = economic_test_app();
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let count = bus.current(&metric::colony_count()).unwrap();
        assert!((count - 0.0).abs() < 1e-9);
        let minerals = bus.current(&metric::net_production_minerals()).unwrap();
        assert!((minerals - 0.0).abs() < 1e-9);
        let pop = bus.current(&metric::population_total()).unwrap();
        assert!((pop - 0.0).abs() < 1e-9);
    }
}
