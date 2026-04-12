use bevy::prelude::*;

use crate::colony::{Buildings, SystemBuildings};
use crate::components::Position;
use crate::galaxy::{Planet, StarSystem};
use crate::player::{Faction, PlayerEmpire};
use crate::scripting::building_api::BuildingId;
use crate::scripting::faction_api::{lookup_on_game_start, FactionRegistry};
use crate::scripting::game_start_ctx::{GameStartActions, GameStartCtx};
use crate::scripting::ScriptEngine;
use crate::ship::{spawn_ship, Owner};
use crate::ship_design::ShipDesignRegistry;

pub struct GameSetupPlugin;

impl Plugin for GameSetupPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Startup,
            run_faction_on_game_start
                .after(crate::galaxy::generate_galaxy)
                .after(crate::player::spawn_player_empire)
                .after(crate::colony::spawn_capital_colony)
                .after(crate::scripting::load_all_scripts)
                .after(crate::scripting::load_faction_registry),
        );
    }
}

/// Find the faction id from the player empire's `Faction` component.
fn player_faction_id(world: &mut World) -> Option<String> {
    let mut q = world.query_filtered::<&Faction, With<PlayerEmpire>>();
    let f = q.iter(world).next()?;
    Some(f.id.clone())
}

/// Startup system that runs the player faction's `on_game_start` Lua callback,
/// then applies the recorded actions to the ECS (buildings, ships, capital marks).
pub fn run_faction_on_game_start(world: &mut World) {
    // Resolve player faction id
    let Some(faction_id) = player_faction_id(world) else {
        warn!("No PlayerEmpire/Faction found; skipping on_game_start");
        return;
    };

    // Verify faction is registered
    let has_callback = {
        let registry = world.resource::<FactionRegistry>();
        match registry.factions.get(&faction_id) {
            Some(def) => def.has_on_game_start,
            None => {
                warn!(
                    "Player faction '{}' not found in FactionRegistry; skipping on_game_start",
                    faction_id
                );
                return;
            }
        }
    };

    if !has_callback {
        info!(
            "Faction '{}' has no on_game_start callback; skipping",
            faction_id
        );
        return;
    }

    // Run the Lua callback to collect actions
    let ctx = GameStartCtx::new(faction_id.clone());
    let actions = {
        let engine = world.resource::<ScriptEngine>();
        let lua = engine.lua();
        let func = match lookup_on_game_start(lua, &faction_id) {
            Ok(Some(f)) => f,
            Ok(None) => {
                warn!(
                    "Faction '{}' on_game_start function not found despite registry flag",
                    faction_id
                );
                return;
            }
            Err(e) => {
                warn!("Failed to look up on_game_start for '{}': {e}", faction_id);
                return;
            }
        };

        if let Err(e) = func.call::<()>(ctx.clone()) {
            warn!("on_game_start for '{}' raised an error: {e}", faction_id);
            // Still apply any actions collected before the error.
        }
        ctx.take_actions()
    };

    apply_game_start_actions(world, &faction_id, actions);
}

/// Apply the actions recorded by a faction's `on_game_start` callback to the ECS.
/// Operates on the capital StarSystem and its first planet.
pub fn apply_game_start_actions(world: &mut World, faction_id: &str, actions: GameStartActions) {
    // Find the capital system entity, its position, and the list of (idx, planet_entity) for its planets.
    let (capital_entity, capital_pos, capital_name, planet_entities) = {
        let mut sys_q = world.query::<(Entity, &StarSystem, &Position)>();
        let capital = sys_q
            .iter(world)
            .find(|(_, s, _)| s.is_capital)
            .map(|(e, s, p)| (e, *p, s.name.clone()));
        let Some((entity, pos, name)) = capital else {
            warn!(
                "No capital StarSystem found while applying on_game_start for '{}'",
                faction_id
            );
            return;
        };

        // Collect planets belonging to this capital, sorted by name (which encodes the
        // Roman numeral ordering used in galaxy generation).
        let mut planet_q = world.query::<(Entity, &Planet)>();
        let mut planets: Vec<(Entity, String)> = planet_q
            .iter(world)
            .filter(|(_, p)| p.system == entity)
            .map(|(e, p)| (e, p.name.clone()))
            .collect();
        planets.sort_by(|a, b| a.1.cmp(&b.1));
        let entities: Vec<Entity> = planets.into_iter().map(|(e, _)| e).collect();
        (entity, pos, name, entities)
    };

    // Apply mark_capital / mark_surveyed (mark_capital is generally redundant since
    // the capital is selected during galaxy generation, but support the API).
    if actions.mark_capital || actions.mark_surveyed {
        if let Some(mut star) = world.get_mut::<StarSystem>(capital_entity) {
            if actions.mark_capital {
                star.is_capital = true;
            }
            if actions.mark_surveyed {
                star.surveyed = true;
            }
        }
    }

    // Apply colonize_planet — currently a no-op because spawn_capital_colony has
    // already created the Colony scaffold on the first planet. We log the intent
    // for cross-checking with the Lua-side declaration.
    if let Some(idx) = actions.colonize_planet {
        if idx == 0 || idx > planet_entities.len() {
            warn!(
                "on_game_start for '{}' called colonize on out-of-range planet index {} (have {})",
                faction_id,
                idx,
                planet_entities.len()
            );
        } else {
            info!(
                "on_game_start for '{}' colonize_planet({}) acknowledged",
                faction_id, idx
            );
        }
    }

    // Apply planet_buildings — each entry is (planet_idx, building_id). Find the
    // colony attached to that planet and add the building to its first empty slot.
    for (planet_idx, building_id) in &actions.planet_buildings {
        if *planet_idx == 0 || *planet_idx > planet_entities.len() {
            warn!(
                "on_game_start for '{}' add_building skipped: planet index {} out of range",
                faction_id, planet_idx
            );
            continue;
        }
        let planet_entity = planet_entities[*planet_idx - 1];
        // Find the Colony entity whose planet matches.
        let colony_entity = {
            let mut q = world.query::<(Entity, &crate::colony::Colony)>();
            q.iter(world)
                .find(|(_, c)| c.planet == planet_entity)
                .map(|(e, _)| e)
        };
        let Some(colony_entity) = colony_entity else {
            warn!(
                "on_game_start for '{}' add_building '{}' skipped: no colony on planet idx {}",
                faction_id, building_id, planet_idx
            );
            continue;
        };
        let Some(mut buildings) = world.get_mut::<Buildings>(colony_entity) else {
            warn!(
                "on_game_start for '{}' add_building '{}' skipped: colony has no Buildings component",
                faction_id, building_id
            );
            continue;
        };
        if let Some(slot) = buildings.slots.iter_mut().find(|s| s.is_none()) {
            *slot = Some(BuildingId::new(building_id));
        } else {
            warn!(
                "on_game_start for '{}' add_building '{}': no free planet slot on planet idx {}",
                faction_id, building_id, planet_idx
            );
        }
    }

    // Apply system_buildings — add to first empty slot of SystemBuildings on the capital.
    if !actions.system_buildings.is_empty() {
        let Some(mut sys_b) = world.get_mut::<SystemBuildings>(capital_entity) else {
            warn!(
                "on_game_start for '{}': capital has no SystemBuildings component",
                faction_id
            );
            return;
        };
        for building_id in &actions.system_buildings {
            if let Some(slot) = sys_b.slots.iter_mut().find(|s| s.is_none()) {
                *slot = Some(BuildingId::new(building_id));
            } else {
                warn!(
                    "on_game_start for '{}' add system building '{}': no free system slot",
                    faction_id, building_id
                );
            }
        }
    }

    // Spawn ships at the capital. Use the player empire as owner.
    if !actions.ships.is_empty() {
        let owner = {
            let mut q = world.query_filtered::<Entity, With<PlayerEmpire>>();
            match q.iter(world).next() {
                Some(e) => Owner::Empire(e),
                None => {
                    warn!(
                        "No PlayerEmpire found; ships from on_game_start of '{}' will be neutral",
                        faction_id
                    );
                    Owner::Neutral
                }
            }
        };
        // Use SystemState to obtain a Commands queue from the world.
        let mut state: bevy::ecs::system::SystemState<(
            Commands,
            Res<ShipDesignRegistry>,
        )> = bevy::ecs::system::SystemState::new(world);
        {
            let (mut commands, registry) = state.get_mut(world);
            for (design_id, name) in &actions.ships {
                spawn_ship(
                    &mut commands,
                    design_id,
                    name.clone(),
                    capital_entity,
                    capital_pos,
                    owner,
                    &registry,
                );
            }
        }
        state.apply(world);
        info!(
            "Spawned {} ships from on_game_start of '{}' at {}",
            actions.ships.len(),
            faction_id,
            capital_name
        );
    }

    info!(
        "Applied on_game_start actions for faction '{}' at capital {}",
        faction_id, capital_name
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amount::Amt;
    use crate::colony::{
        BuildQueue, BuildingQueue, Colony, FoodConsumption, MaintenanceCost, Production,
        ProductionFocus, ResourceCapacity, ResourceStockpile, SystemBuildingQueue,
        DEFAULT_SYSTEM_BUILDING_SLOTS,
    };
    use crate::components::Position;
    use crate::condition::ScopedFlags;
    use crate::galaxy::{Anomalies, Sovereignty, SystemAttributes, SystemModifiers};
    use crate::knowledge::KnowledgeStore;
    use crate::modifier::ModifiedValue;
    use crate::player::Empire;
    use crate::ship::{Ship, ShipState};
    use crate::ship_design::{ShipDesignDefinition, ShipDesignRegistry};
    use crate::technology::{
        EmpireModifiers, GameFlags, GlobalParams, RecentlyResearched, ResearchPool, ResearchQueue,
        TechKnowledge, TechTree,
    };

    fn setup_world() -> (World, Entity, Entity) {
        let mut world = World::new();

        // Spawn capital StarSystem with stockpile, system buildings, etc.
        let capital = world
            .spawn((
                StarSystem {
                    name: "Sol".into(),
                    surveyed: true,
                    is_capital: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
                Sovereignty::default(),
                TechKnowledge::default(),
                SystemModifiers::default(),
                Anomalies::default(),
                ResourceStockpile {
                    minerals: Amt::units(500),
                    energy: Amt::units(500),
                    research: Amt::ZERO,
                    food: Amt::units(200),
                    authority: Amt::ZERO,
                },
                ResourceCapacity::default(),
                SystemBuildings {
                    slots: vec![None; DEFAULT_SYSTEM_BUILDING_SLOTS],
                },
                SystemBuildingQueue::default(),
            ))
            .id();

        // Planet 1 (Sol I) — sorted alphabetically as "Sol I" < "Sol II"
        let planet = world
            .spawn((
                Planet {
                    name: "Sol I".into(),
                    system: capital,
                    planet_type: "terrestrial".into(),
                },
                SystemAttributes {
                    habitability: 1.0,
                    mineral_richness: 0.5,
                    energy_potential: 0.5,
                    research_potential: 0.5,
                    max_building_slots: 5,
                },
                Position::from([0.0, 0.0, 0.0]),
            ))
            .id();

        // Colony with empty buildings
        world.spawn((
            Colony {
                planet,
                population: 100.0,
                growth_rate: 0.01,
            },
            Production {
                minerals_per_hexadies: ModifiedValue::new(Amt::units(5)),
                energy_per_hexadies: ModifiedValue::new(Amt::units(5)),
                research_per_hexadies: ModifiedValue::new(Amt::units(1)),
                food_per_hexadies: ModifiedValue::new(Amt::units(5)),
            },
            BuildQueue { queue: Vec::new() },
            crate::colony::Buildings {
                slots: vec![None; 5],
            },
            BuildingQueue::default(),
            ProductionFocus::default(),
            MaintenanceCost::default(),
            FoodConsumption::default(),
        ));

        // Player empire entity (needed for ship owner resolution)
        world.spawn((
            Empire {
                name: "Test Empire".into(),
            },
            PlayerEmpire,
            Faction {
                id: "test_faction".into(),
                name: "Test".into(),
            },
            TechTree::default(),
            ResearchQueue::default(),
            ResearchPool::default(),
            RecentlyResearched::default(),
            crate::colony::AuthorityParams::default(),
            crate::colony::ConstructionParams::default(),
            EmpireModifiers::default(),
            GameFlags::default(),
            GlobalParams::default(),
            KnowledgeStore::default(),
            crate::communication::CommandLog::default(),
            ScopedFlags::default(),
        ));

        // Ship design registry with a single explorer design.
        let mut registry = ShipDesignRegistry::default();
        registry.insert(ShipDesignDefinition {
            id: "explorer_mk1".into(),
            name: "Explorer Mk.I".into(),
            description: String::new(),
            hull_id: "corvette".into(),
            modules: Vec::new(),
            can_survey: true,
            can_colonize: false,
            maintenance: Amt::new(0, 500),
            build_cost_minerals: Amt::units(200),
            build_cost_energy: Amt::units(100),
            build_time: 60,
            hp: 50.0,
            sublight_speed: 0.75,
            ftl_range: 10.0,
            revision: 0,
        });
        world.insert_resource(registry);

        (world, capital, planet)
    }

    #[test]
    fn apply_actions_adds_planet_buildings() {
        let (mut world, _capital, planet) = setup_world();

        let mut actions = GameStartActions::default();
        actions.planet_buildings.push((1, "mine".into()));
        actions.planet_buildings.push((1, "power_plant".into()));

        apply_game_start_actions(&mut world, "test_faction", actions);

        // Find the colony and verify its buildings
        let mut q = world.query::<(&Colony, &crate::colony::Buildings)>();
        let mut found = false;
        for (colony, buildings) in q.iter(&world) {
            if colony.planet == planet {
                let names: Vec<String> = buildings
                    .slots
                    .iter()
                    .filter_map(|s| s.as_ref().map(|b| b.0.clone()))
                    .collect();
                assert_eq!(names, vec!["mine".to_string(), "power_plant".to_string()]);
                found = true;
            }
        }
        assert!(found, "colony not found");
    }

    #[test]
    fn apply_actions_adds_system_buildings() {
        let (mut world, capital, _planet) = setup_world();

        let mut actions = GameStartActions::default();
        actions.system_buildings.push("shipyard".into());

        apply_game_start_actions(&mut world, "test_faction", actions);

        let sys_b = world
            .get::<SystemBuildings>(capital)
            .expect("capital has SystemBuildings");
        let names: Vec<String> = sys_b
            .slots
            .iter()
            .filter_map(|s| s.as_ref().map(|b| b.0.clone()))
            .collect();
        assert_eq!(names, vec!["shipyard".to_string()]);
    }

    #[test]
    fn apply_actions_spawns_ships_at_capital() {
        let (mut world, capital, _planet) = setup_world();

        let mut actions = GameStartActions::default();
        actions.ships.push(("explorer_mk1".into(), "Explorer-1".into()));
        actions.ships.push(("explorer_mk1".into(), "Explorer-2".into()));

        apply_game_start_actions(&mut world, "test_faction", actions);

        let mut q = world.query::<(&Ship, &ShipState)>();
        let ships: Vec<_> = q.iter(&world).collect();
        assert_eq!(ships.len(), 2);
        for (ship, state) in &ships {
            assert!(matches!(ship.owner, Owner::Empire(_)));
            match state {
                ShipState::Docked { system } => assert_eq!(*system, capital),
                _ => panic!("Expected Docked state at capital"),
            }
        }
        let names: Vec<String> = ships.iter().map(|(s, _)| s.name.clone()).collect();
        assert!(names.contains(&"Explorer-1".to_string()));
        assert!(names.contains(&"Explorer-2".to_string()));
    }

    #[test]
    fn apply_actions_set_capital_and_surveyed() {
        let (mut world, capital, _planet) = setup_world();

        // Set surveyed=false so we can verify mark_surveyed flips it back on.
        // is_capital must remain true so apply_game_start_actions can locate it.
        if let Some(mut star) = world.get_mut::<StarSystem>(capital) {
            star.surveyed = false;
        }

        let actions = GameStartActions {
            mark_capital: true,
            mark_surveyed: true,
            ..Default::default()
        };
        apply_game_start_actions(&mut world, "test_faction", actions);

        let star = world.get::<StarSystem>(capital).unwrap();
        assert!(star.is_capital);
        assert!(star.surveyed);
    }

    #[test]
    fn apply_actions_oob_planet_index_logs_warning_no_panic() {
        let (mut world, _capital, _planet) = setup_world();
        let mut actions = GameStartActions::default();
        actions.planet_buildings.push((99, "mine".into()));
        // Should not panic — just warn.
        apply_game_start_actions(&mut world, "test_faction", actions);
    }
}

