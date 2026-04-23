//! Metric emitters — read ECS state and emit to the AI bus.
//!
//! Each emitter iterates per-empire and writes faction-suffixed metrics
//! (e.g. `my_total_ships.faction_42`) so NPC policies read their own
//! empire's data rather than a global aggregate (#422).

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use crate::ai::convert::to_ai_faction;
use crate::ai::emit::AiBusWriter;
use crate::ai::schema::foreign::foreign_metric_id;
use crate::ai::schema::ids::metric;
use crate::colony::{
    Buildings, Colony, FoodConsumption, Production, ResourceCapacity, ResourceStockpile,
    SlotAssignment,
};
use crate::faction::FactionOwner;
use crate::galaxy::{BASE_CARRYING_CAPACITY, Planet, StarSystem, SystemAttributes};
use crate::galaxy::{AtSystem, Hostile, Sovereignty};
use crate::knowledge::KnowledgeStore;
use crate::player::Empire;
use crate::ship::{CoreShip, Owner, Ship, ShipHitpoints, ShipModifiers, ShipState};
use crate::technology::TechTree;
use crate::time_system::GameClock;

/// Emit per-faction military metrics for all empires.
///
/// Registered under [`AiTickSet::MetricProduce`](super::AiTickSet::MetricProduce).
/// Each empire gets its own set of faction-suffixed metrics
/// (e.g. `my_total_ships.faction_42`). `systems_with_hostiles` stays
/// global since it is not faction-scoped.
pub fn emit_military_metrics(
    mut writer: AiBusWriter,
    empires: Query<Entity, With<Empire>>,
    ships: Query<(
        &Ship,
        &ShipHitpoints,
        &ShipModifiers,
        &ShipState,
        Option<&CoreShip>,
    )>,
    hostiles: Query<&AtSystem, With<Hostile>>,
) {
    for empire_entity in &empires {
        let faction_id = to_ai_faction(empire_entity);

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
            if ship.owner != Owner::Empire(empire_entity) {
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

        writer.emit(&metric::for_faction("my_total_ships", faction_id), total_ships);
        writer.emit(&metric::for_faction("my_strength", faction_id), total_strength);
        writer.emit(&metric::for_faction("my_total_attack", faction_id), total_attack);
        writer.emit(&metric::for_faction("my_total_defense", faction_id), total_defense);
        writer.emit(&metric::for_faction("my_armor", faction_id), total_armor);
        writer.emit(&metric::for_faction("my_shields", faction_id), total_shields);
        writer.emit(
            &metric::for_faction("my_shield_regen_rate", faction_id),
            total_shield_regen,
        );

        let vulnerability = if total_max_hp > 0.0 {
            1.0 - (total_current_hp / total_max_hp)
        } else {
            0.0
        };
        writer.emit(
            &metric::for_faction("my_vulnerability_score", faction_id),
            vulnerability,
        );

        let fleet_ready = if total_ships > 0.0 {
            ships_in_system / total_ships
        } else {
            0.0
        };
        writer.emit(&metric::for_faction("my_fleet_ready", faction_id), fleet_ready);

        writer.emit(
            &metric::for_faction("my_has_flagship", faction_id),
            if has_flagship { 1.0 } else { 0.0 },
        );
    }

    // systems_with_hostiles is global — not per-faction.
    let hostile_systems: HashSet<Entity> = hostiles.iter().map(|at| at.0).collect();
    writer.emit(
        &metric::systems_with_hostiles(),
        hostile_systems.len() as f64,
    );
}

/// Emit per-faction economic metrics for all empires.
///
/// Registered under [`AiTickSet::MetricProduce`](super::AiTickSet::MetricProduce).
/// Each empire gets its own set of faction-suffixed metrics.
/// Truly global metrics (`game_elapsed_time`) remain un-suffixed.
#[allow(clippy::too_many_arguments)]
pub fn emit_economic_metrics(
    mut writer: AiBusWriter,
    clock: Res<GameClock>,
    empires: Query<Entity, With<Empire>>,
    colonies: Query<(
        Entity,
        &Colony,
        &Production,
        Option<&FoodConsumption>,
        Option<&Buildings>,
        Option<&crate::species::ColonyPopulation>,
        Option<&FactionOwner>,
    )>,
    stockpiles: Query<
        (
            Entity,
            &ResourceStockpile,
            Option<&ResourceCapacity>,
            Option<&Sovereignty>,
        ),
        With<StarSystem>,
    >,
    ai_station_ships: Query<(Entity, &Ship, &ShipState, &SlotAssignment)>,
    planets: Query<&Planet>,
    planet_attrs: Query<&SystemAttributes, With<Planet>>,
    tech_tree: Option<Res<TechTree>>,
    ai_building_registry: Option<Res<crate::colony::BuildingRegistry>>,
    core_ships: Query<
        (&crate::galaxy::AtSystem, &FactionOwner),
        With<crate::ship::CoreShip>,
    >,
) {
    // Per-empire: set of systems with at least one Core-equipped ship.
    // Used to gate system-building construction (Infrastructure Core is
    // required to place shipyards / ports / research labs — see #370).
    let mut core_systems_per_empire: HashMap<Entity, HashSet<Entity>> = HashMap::new();
    for (at, owner) in &core_ships {
        core_systems_per_empire
            .entry(owner.0)
            .or_default()
            .insert(at.0);
    }

    // Pre-compute system-level building info per owner empire, keyed by empire entity.
    let mut shipyard_counts: HashMap<Entity, f64> = HashMap::new();
    let mut port_counts: HashMap<Entity, f64> = HashMap::new();
    if let Some(ref registry) = ai_building_registry {
        let reverse = crate::colony::system_buildings::build_reverse_design_map(registry);
        for (_entity, ship, state, _slot) in &ai_station_ships {
            let _system = match state {
                ShipState::InSystem { system: s } => *s,
                ShipState::Refitting { system: s, .. } => *s,
                _ => continue,
            };
            if let Owner::Empire(owner) = ship.owner {
                if let Some(bid) = reverse.get(&ship.design_id) {
                    if let Some(def) = registry.get(bid.as_str()) {
                        if def.capabilities.contains_key("shipyard") {
                            *shipyard_counts.entry(owner).or_default() += 1.0;
                        }
                        if def.capabilities.contains_key("port") {
                            *port_counts.entry(owner).or_default() += 1.0;
                        }
                    }
                }
            }
        }
    }

    for empire_entity in &empires {
        let fid = to_ai_faction(empire_entity);

        // --- Production rates (per hexadies) ---
        let mut total_minerals_rate: f64 = 0.0;
        let mut total_energy_rate: f64 = 0.0;
        let mut total_food_rate: f64 = 0.0;
        let mut total_research_rate: f64 = 0.0;

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

        for (_entity, colony, prod, food_consumption, buildings, col_pop, faction_owner) in
            &colonies
        {
            // Filter: only count colonies owned by this empire.
            let owned = faction_owner.is_some_and(|fo| fo.0 == empire_entity);
            if !owned {
                continue;
            }

            colony_count += 1.0;

            // Production rates
            total_minerals_rate += prod.minerals_per_hexadies.final_value().to_f64();
            total_energy_rate += prod.energy_per_hexadies.final_value().to_f64();
            total_food_rate += prod.food_per_hexadies.final_value().to_f64();
            total_research_rate += prod.research_per_hexadies.final_value().to_f64();

            // Population
            total_population += col_pop.map(|p| p.total() as f64).unwrap_or(0.0);
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
        writer.emit(&metric::for_faction("net_production_minerals", fid), total_minerals_rate);
        writer.emit(&metric::for_faction("net_production_energy", fid), total_energy_rate);
        writer.emit(&metric::for_faction("net_production_food", fid), total_food_rate);
        writer.emit(&metric::for_faction("net_production_research", fid), total_research_rate);

        // Emit population metrics
        writer.emit(&metric::for_faction("population_total", fid), total_population);
        writer.emit(&metric::for_faction("population_growth_rate", fid), total_growth_rate);
        writer.emit(
            &metric::for_faction("population_carrying_capacity", fid),
            total_carrying_capacity,
        );
        let pop_ratio = if total_carrying_capacity > 0.0 {
            total_population / total_carrying_capacity
        } else {
            0.0
        };
        writer.emit(&metric::for_faction("population_ratio", fid), pop_ratio);

        // Emit food metrics
        writer.emit(&metric::for_faction("food_consumption_rate", fid), total_food_consumption);
        writer.emit(
            &metric::for_faction("food_surplus", fid),
            total_food_rate - total_food_consumption,
        );

        // Emit territory metrics
        writer.emit(&metric::for_faction("colony_count", fid), colony_count);
        writer.emit(
            &metric::for_faction("colonized_system_count", fid),
            colonized_systems.len() as f64,
        );

        // --- Stockpiles ---
        // Filter stockpiles by sovereignty: only systems owned by this empire.
        let mut total_minerals: f64 = 0.0;
        let mut total_energy: f64 = 0.0;
        let mut total_food: f64 = 0.0;
        let mut total_authority: f64 = 0.0;
        let mut total_cap_minerals: f64 = 0.0;
        let mut total_cap_energy: f64 = 0.0;
        let mut total_cap_food: f64 = 0.0;
        let mut total_authority_debt: f64 = 0.0;

        for (_sys_entity, stockpile, capacity, sovereignty) in &stockpiles {
            let is_owned = sovereignty.is_some_and(|sov| {
                sov.owner == Some(Owner::Empire(empire_entity))
            });
            if !is_owned {
                continue;
            }

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

        writer.emit(&metric::for_faction("stockpile_minerals", fid), total_minerals);
        writer.emit(&metric::for_faction("stockpile_energy", fid), total_energy);
        writer.emit(&metric::for_faction("stockpile_food", fid), total_food);
        writer.emit(&metric::for_faction("stockpile_authority", fid), total_authority);

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
        writer.emit(&metric::for_faction("stockpile_ratio_minerals", fid), ratio_minerals);
        writer.emit(&metric::for_faction("stockpile_ratio_energy", fid), ratio_energy);
        writer.emit(&metric::for_faction("stockpile_ratio_food", fid), ratio_food);

        writer.emit(&metric::for_faction("total_authority_debt", fid), total_authority_debt);

        // Infrastructure metrics
        let sys_shipyard = shipyard_counts.get(&empire_entity).copied().unwrap_or(0.0);
        let sys_port = port_counts.get(&empire_entity).copied().unwrap_or(0.0);
        writer.emit(&metric::for_faction("systems_with_shipyard", fid), sys_shipyard);
        writer.emit(&metric::for_faction("systems_with_port", fid), sys_port);
        let sys_core = core_systems_per_empire
            .get(&empire_entity)
            .map(|s| s.len() as f64)
            .unwrap_or(0.0);
        writer.emit(&metric::for_faction("systems_with_core", fid), sys_core);
        writer.emit(&metric::for_faction("max_building_slots", fid), max_slots);
        writer.emit(&metric::for_faction("used_building_slots", fid), used_slots);
        writer.emit(&metric::for_faction("free_building_slots", fid), max_slots - used_slots);
        writer.emit(
            &metric::for_faction("can_build_ships", fid),
            if sys_shipyard > 0.0 { 1.0 } else { 0.0 },
        );

        // Technology metrics — currently global TechTree, emitted for each empire.
        // TODO: per-empire tech trees when multiple empires have independent research.
        if let Some(ref tree) = tech_tree {
            let researched = tree.researched.len() as f64;
            let total = tree.technologies.len() as f64;
            writer.emit(&metric::for_faction("tech_total_researched", fid), researched);
            let completion = if total > 0.0 { researched / total } else { 0.0 };
            writer.emit(&metric::for_faction("tech_completion_percent", fid), completion);
        }
    }

    // Meta / time — global, not per-faction.
    writer.emit(&metric::game_elapsed_time(), clock.elapsed as f64);
}

/// Emit foreign-faction metrics from each empire's KnowledgeStore.
///
/// For each empire, we estimate other factions' strength, fleet count, and
/// colony count using ground-truth data (Sovereignty, Ship.owner) as a
/// temporary proxy until KnowledgeStore snapshots carry owner information.
///
/// The estimates are intentionally imprecise — in the future, they should
/// be derived purely from light-speed-delayed KnowledgeStore observations.
///
/// Registered under [`AiTickSet::MetricProduce`](super::AiTickSet::MetricProduce).
pub fn emit_foreign_metrics(
    mut writer: AiBusWriter,
    empires: Query<(Entity, Option<&KnowledgeStore>), With<Empire>>,
    ships: Query<(&Ship, &ShipHitpoints, &ShipModifiers)>,
    sovereignties: Query<&Sovereignty, With<StarSystem>>,
    colonies: Query<(&Colony, Option<&FactionOwner>)>,
) {
    // Pre-compute per-empire ship counts, strength, and colony counts.
    let mut ship_counts: HashMap<Entity, f64> = HashMap::new();
    let mut strength_totals: HashMap<Entity, f64> = HashMap::new();
    for (ship, hp, mods) in &ships {
        let owner_entity = match ship.owner {
            Owner::Empire(e) => e,
            _ => continue,
        };
        *ship_counts.entry(owner_entity).or_default() += 1.0;

        let attack = mods.attack.final_value().to_f64();
        let defense = mods.defense.final_value().to_f64();
        let current_hp = hp.hull + hp.armor + hp.shield;
        *strength_totals.entry(owner_entity).or_default() += attack + defense + current_hp;
    }

    // Pre-compute per-empire colony counts from Sovereignty.
    let mut colony_counts: HashMap<Entity, f64> = HashMap::new();
    for (_colony, faction_owner) in &colonies {
        if let Some(fo) = faction_owner {
            *colony_counts.entry(fo.0).or_default() += 1.0;
        }
    }

    // For each observing empire, emit foreign metrics for every other empire.
    let empire_entities: Vec<Entity> = empires.iter().map(|(e, _)| e).collect();
    for &observer in &empire_entities {
        let observer_fid = to_ai_faction(observer);

        for &target in &empire_entities {
            if target == observer {
                continue;
            }
            let target_fid = to_ai_faction(target);

            let fleet_count = ship_counts.get(&target).copied().unwrap_or(0.0);
            let strength = strength_totals.get(&target).copied().unwrap_or(0.0);
            let col_count = colony_counts.get(&target).copied().unwrap_or(0.0);

            // Emit using the foreign metric id convention:
            // "foreign.<metric>.faction_<target>" — but we must emit from the
            // observer's perspective. The bus currently has one global namespace
            // so we compose: "foreign.<metric>.faction_<target>.observer_<observer>"
            // would be ideal, but the existing schema declares slots as
            // "foreign.<metric>.faction_<target_id>". Since the bus is shared,
            // we emit global estimates (same for all observers for now).
            writer.emit(
                &foreign_metric_id("foreign.strength", target_fid),
                strength,
            );
            writer.emit(
                &foreign_metric_id("foreign.fleet_count", target_fid),
                fleet_count,
            );
            writer.emit(
                &foreign_metric_id("foreign.colony_count", target_fid),
                col_count,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::convert::to_ai_faction;
    use crate::ai::plugin::AiBusResource;
    use crate::ai::schema;
    use crate::amount::Amt;
    use crate::modifier::ScopedModifiers;
    use crate::player::Faction;
    use crate::time_system::{GameClock, GameSpeed};
    use macrocosmo_ai::WarningMode;

    /// Spawn an empire entity and declare its per-faction metric slots.
    /// Returns the empire entity.
    fn spawn_empire(app: &mut App) -> Entity {
        let entity = app.world_mut().spawn((
            Empire { name: "Test Empire".into() },
            Faction::new("test_empire", "Test Empire"),
        )).id();
        // Declare per-faction metric slots on the bus.
        let fid = to_ai_faction(entity);
        let mut bus = app.world_mut().resource_mut::<AiBusResource>();
        for base in crate::ai::schema::ids::metric::PER_FACTION_METRIC_BASES {
            let id = metric::for_faction(base, fid);
            bus.0.declare_metric(id, macrocosmo_ai::MetricSpec::gauge(macrocosmo_ai::Retention::Medium, "per-faction self metric"));
        }
        entity
    }

    fn test_app() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(GameClock::new(10));
        app.insert_resource(GameSpeed::default());
        app.insert_resource(AiBusResource::with_warning_mode(WarningMode::Silent));
        // Declare global metrics on the bus.
        {
            let mut bus = app.world_mut().resource_mut::<AiBusResource>();
            schema::declare_metrics_standalone(&mut bus.0);
        }
        app.add_systems(Update, emit_military_metrics);
        let empire = spawn_empire(&mut app);
        (app, empire)
    }

    fn spawn_ship(app: &mut App, empire: Entity, in_system: bool, is_core: bool) -> Entity {
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
                owner: Owner::Empire(empire),
                sublight_speed: 1.0,
                ftl_range: 5.0,
                ruler_aboard: false,
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
        let (mut app, empire) = test_app();
        let fid = to_ai_faction(empire);
        spawn_ship(&mut app, empire, true, false);
        spawn_ship(&mut app, empire, true, false);
        spawn_ship(&mut app, empire, false, false);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let total = bus.current(&metric::for_faction("my_total_ships", fid)).unwrap();
        assert!((total - 3.0).abs() < 1e-9);
    }

    #[test]
    fn emit_military_metrics_fleet_ready_fraction() {
        let (mut app, empire) = test_app();
        let fid = to_ai_faction(empire);
        spawn_ship(&mut app, empire, true, false);
        spawn_ship(&mut app, empire, false, false);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let ready = bus.current(&metric::for_faction("my_fleet_ready", fid)).unwrap();
        assert!((ready - 0.5).abs() < 1e-9);
    }

    #[test]
    fn emit_military_metrics_flagship_detection() {
        let (mut app, empire) = test_app();
        let fid = to_ai_faction(empire);
        spawn_ship(&mut app, empire, true, false);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let flag = bus.current(&metric::for_faction("my_has_flagship", fid)).unwrap();
        assert!((flag - 0.0).abs() < 1e-9);

        // Now add a core ship.
        spawn_ship(&mut app, empire, true, true);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let flag = bus.current(&metric::for_faction("my_has_flagship", fid)).unwrap();
        assert!((flag - 1.0).abs() < 1e-9);
    }

    #[test]
    fn emit_military_metrics_attack_defense() {
        let (mut app, empire) = test_app();
        let fid = to_ai_faction(empire);
        spawn_ship(&mut app, empire, true, false);
        spawn_ship(&mut app, empire, true, false);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let attack = bus.current(&metric::for_faction("my_total_attack", fid)).unwrap();
        let defense = bus.current(&metric::for_faction("my_total_defense", fid)).unwrap();
        // 2 ships x 10 attack = 20
        assert!((attack - 20.0).abs() < 1e-9);
        // 2 ships x 5 defense = 10
        assert!((defense - 10.0).abs() < 1e-9);
    }

    #[test]
    fn emit_military_metrics_vulnerability() {
        let (mut app, empire) = test_app();
        let fid = to_ai_faction(empire);
        spawn_ship(&mut app, empire, true, false);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let vuln = bus.current(&metric::for_faction("my_vulnerability_score", fid)).unwrap();
        // hull=40/50, armor=15/20, shield=8/10 => current=63, max=80
        // vuln = 1 - 63/80 = 0.2125
        assert!((vuln - 0.2125).abs() < 1e-4);
    }

    // -- Economic emitter tests ------------------------------------------------

    fn economic_test_app() -> (App, Entity) {
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
        let empire = spawn_empire(&mut app);
        (app, empire)
    }

    /// Spawn a minimal colony + star system for economic tests.
    /// Returns (colony_entity, system_entity, planet_entity).
    fn spawn_colony(
        app: &mut App,
        empire: Entity,
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
                Sovereignty {
                    owner: Some(Owner::Empire(empire)),
                    control_score: 1.0,
                },
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
                crate::species::ColonyPopulation {
                    species: vec![crate::species::ColonySpecies {
                        species_id: "human".to_string(),
                        population: population as u32,
                    }],
                    growth_accumulator: 0.0,
                },
                FactionOwner(empire),
            ))
            .id();

        (colony_entity, system_entity, planet_entity)
    }

    #[test]
    fn emit_economic_metrics_colony_count() {
        let (mut app, empire) = economic_test_app();
        let fid = to_ai_faction(empire);
        spawn_colony(&mut app, empire, 100.0, 10.0, 5.0, 500, 300, 4, 2);
        spawn_colony(&mut app, empire, 50.0, 5.0, 3.0, 200, 100, 3, 1);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let count = bus.current(&metric::for_faction("colony_count", fid)).unwrap();
        assert!((count - 2.0).abs() < 1e-9);
    }

    #[test]
    fn emit_economic_metrics_production_rates() {
        let (mut app, empire) = economic_test_app();
        let fid = to_ai_faction(empire);
        spawn_colony(&mut app, empire, 100.0, 10.0, 5.0, 500, 300, 4, 0);
        spawn_colony(&mut app, empire, 50.0, 5.0, 3.0, 200, 100, 3, 0);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let minerals = bus.current(&metric::for_faction("net_production_minerals", fid)).unwrap();
        let energy = bus.current(&metric::for_faction("net_production_energy", fid)).unwrap();
        // 10.0 + 5.0 = 15.0
        assert!((minerals - 15.0).abs() < 0.01);
        // 5.0 + 3.0 = 8.0
        assert!((energy - 8.0).abs() < 0.01);
    }

    #[test]
    fn emit_economic_metrics_stockpiles() {
        let (mut app, empire) = economic_test_app();
        let fid = to_ai_faction(empire);
        spawn_colony(&mut app, empire, 100.0, 10.0, 5.0, 500, 300, 4, 0);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let minerals = bus.current(&metric::for_faction("stockpile_minerals", fid)).unwrap();
        let energy = bus.current(&metric::for_faction("stockpile_energy", fid)).unwrap();
        assert!((minerals - 500.0).abs() < 0.01);
        assert!((energy - 300.0).abs() < 0.01);
    }

    #[test]
    fn emit_economic_metrics_stockpile_ratios() {
        let (mut app, empire) = economic_test_app();
        let fid = to_ai_faction(empire);
        // Default ResourceCapacity: minerals=1000, energy=1000, food=500
        spawn_colony(&mut app, empire, 100.0, 10.0, 5.0, 500, 300, 4, 0);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let ratio_m = bus.current(&metric::for_faction("stockpile_ratio_minerals", fid)).unwrap();
        let ratio_e = bus.current(&metric::for_faction("stockpile_ratio_energy", fid)).unwrap();
        // 500/1000 = 0.5
        assert!((ratio_m - 0.5).abs() < 0.01);
        // 300/1000 = 0.3
        assert!((ratio_e - 0.3).abs() < 0.01);
    }

    #[test]
    fn emit_economic_metrics_population() {
        let (mut app, empire) = economic_test_app();
        let fid = to_ai_faction(empire);
        spawn_colony(&mut app, empire, 100.0, 10.0, 5.0, 500, 300, 4, 0);
        spawn_colony(&mut app, empire, 50.0, 5.0, 3.0, 200, 100, 3, 0);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let pop = bus.current(&metric::for_faction("population_total", fid)).unwrap();
        assert!((pop - 150.0).abs() < 1e-9);

        let capacity = bus
            .current(&metric::for_faction("population_carrying_capacity", fid))
            .unwrap();
        // 200.0 * 0.8 = 160.0 per colony, 2 colonies = 320.0
        assert!((capacity - 320.0).abs() < 1e-9);
    }

    #[test]
    fn emit_economic_metrics_food_surplus() {
        let (mut app, empire) = economic_test_app();
        let fid = to_ai_faction(empire);
        // food production = 5.0, food consumption = 2.0 → surplus = 3.0
        spawn_colony(&mut app, empire, 100.0, 10.0, 5.0, 500, 300, 4, 0);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let surplus = bus.current(&metric::for_faction("food_surplus", fid)).unwrap();
        assert!((surplus - 3.0).abs() < 0.01);
    }

    #[test]
    fn emit_economic_metrics_building_slots() {
        let (mut app, empire) = economic_test_app();
        let fid = to_ai_faction(empire);
        // 4 slots, 2 occupied
        spawn_colony(&mut app, empire, 100.0, 10.0, 5.0, 500, 300, 4, 2);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let max = bus.current(&metric::for_faction("max_building_slots", fid)).unwrap();
        let used = bus.current(&metric::for_faction("used_building_slots", fid)).unwrap();
        let free = bus.current(&metric::for_faction("free_building_slots", fid)).unwrap();
        assert!((max - 4.0).abs() < 1e-9);
        assert!((used - 2.0).abs() < 1e-9);
        assert!((free - 2.0).abs() < 1e-9);
    }

    #[test]
    fn emit_economic_metrics_game_elapsed_time() {
        let (mut app, _empire) = economic_test_app();
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let elapsed = bus.current(&metric::game_elapsed_time()).unwrap();
        assert!((elapsed - 10.0).abs() < 1e-9);
    }

    #[test]
    fn emit_economic_metrics_no_colonies_emits_zeroes() {
        let (mut app, empire) = economic_test_app();
        let fid = to_ai_faction(empire);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let count = bus.current(&metric::for_faction("colony_count", fid)).unwrap();
        assert!((count - 0.0).abs() < 1e-9);
        let minerals = bus.current(&metric::for_faction("net_production_minerals", fid)).unwrap();
        assert!((minerals - 0.0).abs() < 1e-9);
        let pop = bus.current(&metric::for_faction("population_total", fid)).unwrap();
        assert!((pop - 0.0).abs() < 1e-9);
    }

    // -- Foreign emitter tests ------------------------------------------------

    fn foreign_test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(GameClock::new(10));
        app.insert_resource(GameSpeed::default());
        app.insert_resource(AiBusResource::with_warning_mode(WarningMode::Silent));
        {
            let mut bus = app.world_mut().resource_mut::<AiBusResource>();
            schema::declare_metrics_standalone(&mut bus.0);
        }
        app.add_systems(Update, emit_foreign_metrics);
        app
    }

    /// Spawn a second empire (foreign) and declare its foreign metric slots.
    fn spawn_foreign_empire(app: &mut App, name: &str) -> Entity {
        let entity = app.world_mut().spawn((
            Empire { name: name.into() },
            Faction::new(name, name),
        )).id();
        let fid = to_ai_faction(entity);
        // Declare per-faction self metric slots.
        let mut bus = app.world_mut().resource_mut::<AiBusResource>();
        for base in crate::ai::schema::ids::metric::PER_FACTION_METRIC_BASES {
            let id = metric::for_faction(base, fid);
            bus.0.declare_metric(id, macrocosmo_ai::MetricSpec::gauge(macrocosmo_ai::Retention::Medium, "per-faction self metric"));
        }
        // Declare foreign metric slots for this faction.
        for template in crate::ai::schema::foreign::foreign_metric_templates() {
            let id = crate::ai::schema::foreign::foreign_metric_id(&template.prefix, fid);
            bus.0.declare_metric(id, (template.spec_factory)());
        }
        entity
    }

    #[test]
    fn emit_foreign_metrics_fleet_count_and_strength() {
        let mut app = foreign_test_app();
        let empire_a = spawn_empire(&mut app);
        let empire_b = spawn_foreign_empire(&mut app, "Empire B");

        // Declare foreign slots for empire_a too (so empire_b can observe empire_a).
        {
            let fid_a = to_ai_faction(empire_a);
            let mut bus = app.world_mut().resource_mut::<AiBusResource>();
            for template in crate::ai::schema::foreign::foreign_metric_templates() {
                let id = crate::ai::schema::foreign::foreign_metric_id(&template.prefix, fid_a);
                bus.0.declare_metric(id, (template.spec_factory)());
            }
        }

        // Give empire_b 2 ships.
        spawn_ship(&mut app, empire_b, true, false);
        spawn_ship(&mut app, empire_b, true, false);

        app.update();

        let fid_b = to_ai_faction(empire_b);
        let bus = app.world().resource::<AiBusResource>();

        // Empire A should see empire B's 2 ships.
        let fleet = bus.current(&crate::ai::schema::foreign::foreign_metric_id("foreign.fleet_count", fid_b)).unwrap();
        assert!((fleet - 2.0).abs() < 1e-9);

        // Strength: each ship has attack=10, defense=5, hp=40+15+8=63 => per ship = 78.
        let strength = bus.current(&crate::ai::schema::foreign::foreign_metric_id("foreign.strength", fid_b)).unwrap();
        assert!((strength - 156.0).abs() < 1e-9);
    }

    #[test]
    fn emit_foreign_metrics_colony_count() {
        let mut app = foreign_test_app();
        let empire_a = spawn_empire(&mut app);
        let empire_b = spawn_foreign_empire(&mut app, "Empire B");

        // Declare foreign slots for empire_a.
        {
            let fid_a = to_ai_faction(empire_a);
            let mut bus = app.world_mut().resource_mut::<AiBusResource>();
            for template in crate::ai::schema::foreign::foreign_metric_templates() {
                let id = crate::ai::schema::foreign::foreign_metric_id(&template.prefix, fid_a);
                bus.0.declare_metric(id, (template.spec_factory)());
            }
        }

        // Give empire_b 2 colonies.
        spawn_colony(&mut app, empire_b, 100.0, 10.0, 5.0, 500, 300, 4, 2);
        spawn_colony(&mut app, empire_b, 50.0, 5.0, 3.0, 200, 100, 3, 1);

        app.update();

        let fid_b = to_ai_faction(empire_b);
        let bus = app.world().resource::<AiBusResource>();

        let col_count = bus.current(&crate::ai::schema::foreign::foreign_metric_id("foreign.colony_count", fid_b)).unwrap();
        assert!((col_count - 2.0).abs() < 1e-9);
    }

    #[test]
    fn emit_foreign_metrics_no_self_emission() {
        let mut app = foreign_test_app();
        let empire_a = spawn_empire(&mut app);

        // Only one empire — no foreign metrics should be emitted.
        spawn_ship(&mut app, empire_a, true, false);
        app.update();

        let fid_a = to_ai_faction(empire_a);
        let bus = app.world().resource::<AiBusResource>();

        // foreign.strength.faction_<A> should NOT have been emitted (no observer sees self).
        let strength = bus.current(&crate::ai::schema::foreign::foreign_metric_id("foreign.strength", fid_a));
        assert!(strength.is_none());
    }

    #[test]
    fn emit_foreign_metrics_zeroes_for_empty_empire() {
        let mut app = foreign_test_app();
        let _empire_a = spawn_empire(&mut app);
        let empire_b = spawn_foreign_empire(&mut app, "Empire B");

        // Declare foreign slots for empire_a.
        {
            let fid_a = to_ai_faction(_empire_a);
            let mut bus = app.world_mut().resource_mut::<AiBusResource>();
            for template in crate::ai::schema::foreign::foreign_metric_templates() {
                let id = crate::ai::schema::foreign::foreign_metric_id(&template.prefix, fid_a);
                bus.0.declare_metric(id, (template.spec_factory)());
            }
        }

        // Empire B has no ships, no colonies.
        app.update();

        let fid_b = to_ai_faction(empire_b);
        let bus = app.world().resource::<AiBusResource>();

        let fleet = bus.current(&crate::ai::schema::foreign::foreign_metric_id("foreign.fleet_count", fid_b)).unwrap();
        assert!((fleet - 0.0).abs() < 1e-9);
        let strength = bus.current(&crate::ai::schema::foreign::foreign_metric_id("foreign.strength", fid_b)).unwrap();
        assert!((strength - 0.0).abs() < 1e-9);
        let col = bus.current(&crate::ai::schema::foreign::foreign_metric_id("foreign.colony_count", fid_b)).unwrap();
        assert!((col - 0.0).abs() < 1e-9);
    }
}
