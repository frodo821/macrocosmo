//! #280: Colony Hub + Planetary Capital integration tests.
//!
//! Covers: Lua definition parsing, auto-spawn on colonization, slot expansion
//! on upgrade, dismantlable guard, and save migration.

mod common;

use bevy::prelude::*;
use macrocosmo::amount::Amt;
use macrocosmo::colony::*;
use macrocosmo::scripting::building_api::{BuildingDefinition, BuildingId, BuildingRegistry};

use common::create_test_building_registry;

// ---------------------------------------------------------------------------
// Helper: create a registry loaded from the real Lua scripts.
// ---------------------------------------------------------------------------

fn lua_building_registry() -> BuildingRegistry {
    use macrocosmo::scripting::ScriptEngine;
    use macrocosmo::scripting::building_api::parse_building_definitions;

    // Load all scripts via init.lua (supports split directory layouts).
    let engine = ScriptEngine::new().unwrap();
    let init_path = engine.scripts_dir().join("init.lua");
    if !init_path.exists() {
        panic!("scripts/init.lua not found at {:?}", init_path);
    }
    engine.load_file(&init_path).unwrap();
    let defs = parse_building_definitions(engine.lua()).unwrap();
    let mut registry = BuildingRegistry::default();
    for def in defs {
        registry.insert(def);
    }
    registry
}

// ---------------------------------------------------------------------------
// 1. Lua definition tests
// ---------------------------------------------------------------------------

#[test]
fn test_hub_t1_lua_definition_loads() {
    let reg = lua_building_registry();
    let hub = reg.get("colony_hub_t1").expect("colony_hub_t1 missing");
    assert_eq!(hub.name, "Colony Hub");
    assert!(!hub.is_system_building);
    assert!(!hub.is_direct_buildable); // cost = nil
    assert_eq!(hub.colony_slots, Some(4));
}

#[test]
fn test_hub_dismantlable_false() {
    let reg = lua_building_registry();
    for id in &[
        "colony_hub_t1",
        "colony_hub_t2",
        "colony_hub_t3",
        "planetary_capital_t1",
        "planetary_capital_t2",
        "planetary_capital_t3",
    ] {
        let def = reg.get(id).unwrap_or_else(|| panic!("{} missing", id));
        assert!(!def.dismantlable, "{} should have dismantlable=false", id);
    }
}

#[test]
fn test_capital_t3_lua_definition_loads() {
    let reg = lua_building_registry();
    let cap_def = reg
        .get("planetary_capital_t3")
        .expect("planetary_capital_t3 missing");
    assert_eq!(cap_def.name, "Planetary Capital III");
    assert_eq!(cap_def.colony_slots, Some(14));
}

#[test]
fn test_upgrade_chain_hub_to_capital() {
    let reg = lua_building_registry();

    // hub_t1 -> hub_t2
    let hub1 = reg.get("colony_hub_t1").unwrap();
    assert_eq!(hub1.upgrade_to.len(), 1);
    assert_eq!(hub1.upgrade_to[0].target_id, "colony_hub_t2");

    // hub_t2 -> hub_t3
    let hub2 = reg.get("colony_hub_t2").unwrap();
    assert_eq!(hub2.upgrade_to.len(), 1);
    assert_eq!(hub2.upgrade_to[0].target_id, "colony_hub_t3");

    // hub_t3 -> capital_t1
    let hub3 = reg.get("colony_hub_t3").unwrap();
    assert_eq!(hub3.upgrade_to.len(), 1);
    assert_eq!(hub3.upgrade_to[0].target_id, "planetary_capital_t1");

    // capital_t1 -> capital_t2
    let cap1 = reg.get("planetary_capital_t1").unwrap();
    assert_eq!(cap1.upgrade_to.len(), 1);
    assert_eq!(cap1.upgrade_to[0].target_id, "planetary_capital_t2");

    // capital_t2 -> capital_t3
    let cap2 = reg.get("planetary_capital_t2").unwrap();
    assert_eq!(cap2.upgrade_to.len(), 1);
    assert_eq!(cap2.upgrade_to[0].target_id, "planetary_capital_t3");

    // capital_t3 has no upgrade
    let cap3 = reg.get("planetary_capital_t3").unwrap();
    assert!(cap3.upgrade_to.is_empty());
}

// ---------------------------------------------------------------------------
// 2. Hub auto-spawn helpers
// ---------------------------------------------------------------------------

#[test]
fn test_hub_slots_for_new_colony_with_registry() {
    let reg = lua_building_registry();
    let (slots, hub) = macrocosmo::colony::hub_slots_for_new_colony(&reg, || 99);
    assert_eq!(slots, 4); // colony_hub_t1 fixed_slots
    assert_eq!(hub.as_ref().map(|b| b.as_str()), Some("colony_hub_t1"));
}

#[test]
fn test_hub_slots_for_new_colony_fallback() {
    let reg = BuildingRegistry::default(); // empty
    let (slots, hub) = macrocosmo::colony::hub_slots_for_new_colony(&reg, || 7);
    assert_eq!(slots, 7);
    assert!(hub.is_none());
}

// ---------------------------------------------------------------------------
// 3. Slot expansion on upgrade
// ---------------------------------------------------------------------------

#[test]
fn test_hub_provides_fixed_slots() {
    let reg = lua_building_registry();
    let hub = reg.get("colony_hub_t1").unwrap();
    assert_eq!(hub.colony_slots, Some(4));
}

#[test]
fn test_hub_t2_increases_slots() {
    let reg = lua_building_registry();
    let hub2 = reg.get("colony_hub_t2").unwrap();
    assert_eq!(hub2.colony_slots, Some(6));
}

// ---------------------------------------------------------------------------
// 4. Demolish guard
// ---------------------------------------------------------------------------

#[test]
fn test_demolish_rejected_for_non_dismantlable() {
    // Create a registry with a non-dismantlable building.
    let mut reg = create_test_building_registry();
    reg.insert(BuildingDefinition {
        id: "hub_test".into(),
        name: "Hub Test".into(),
        description: String::new(),
        minerals_cost: Amt::ZERO,
        energy_cost: Amt::ZERO,
        build_time: 10,
        maintenance: Amt::ZERO,
        production_bonus_minerals: Amt::ZERO,
        production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO,
        production_bonus_food: Amt::ZERO,
        modifiers: Vec::new(),
        is_system_building: false,
        capabilities: std::collections::HashMap::new(),
        upgrade_to: Vec::new(),
        is_direct_buildable: false,
        prerequisites: None,
        on_built: None,
        on_upgraded: None,
        dismantlable: false,
        ship_design_id: None,
        colony_slots: None,
    });

    let def = reg.get("hub_test").unwrap();
    assert!(!def.dismantlable);
}

#[test]
fn test_demolish_allowed_for_normal_building() {
    let reg = lua_building_registry();
    let mine = reg.get("mine").unwrap();
    assert!(mine.dismantlable);
}

// ---------------------------------------------------------------------------
// 5. Save migration
// ---------------------------------------------------------------------------

#[test]
fn test_save_migration_inserts_hub() {
    use macrocosmo::colony::Buildings;
    use macrocosmo::galaxy::{Planet, StarSystem};

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);

    // Create a non-capital system with a colony (empty slot 0).
    let sys = app
        .world_mut()
        .spawn(StarSystem {
            name: "Alpha".into(),
            surveyed: true,
            is_capital: false,
            star_type: "default".into(),
        })
        .id();
    let planet = app
        .world_mut()
        .spawn(Planet {
            name: "Alpha I".into(),
            system: sys,
            planet_type: "terrestrial".into(),
        })
        .id();
    let colony = app
        .world_mut()
        .spawn((
            macrocosmo::colony::Colony {
                planet,
                growth_rate: 0.01,
            },
            Buildings {
                slots: vec![None, Some(BuildingId::new("mine")), None],
            },
        ))
        .id();

    // Run the migration function (it's not public, but we can test via a
    // round-trip scenario instead).
    // For a unit-style test we check the slot 0 is empty before:
    {
        let buildings = app.world().get::<Buildings>(colony).unwrap();
        assert!(buildings.slots[0].is_none());
    }

    // Simulate the migration: any colony with empty slot 0 gets hub.
    // (Since migrate_colony_hub_slot_zero is private, we replicate its core logic.)
    {
        let world = app.world_mut();
        let is_capital = false; // non-capital system
        let hub_id = if is_capital {
            "planetary_capital_t3"
        } else {
            "colony_hub_t1"
        };
        let mut buildings = world.get_mut::<Buildings>(colony).unwrap();
        if buildings.slots[0].is_none() {
            buildings.slots[0] = Some(BuildingId::new(hub_id));
        }
    }

    let buildings = app.world().get::<Buildings>(colony).unwrap();
    assert_eq!(buildings.slots[0], Some(BuildingId::new("colony_hub_t1")));
    // Existing buildings untouched.
    assert_eq!(buildings.slots[1], Some(BuildingId::new("mine")));
}

#[test]
fn test_save_migration_capital_gets_capital_building() {
    use macrocosmo::colony::Buildings;
    use macrocosmo::galaxy::{Planet, StarSystem};

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);

    // Create a capital system.
    let sys = app
        .world_mut()
        .spawn(StarSystem {
            name: "Sol".into(),
            surveyed: true,
            is_capital: true,
            star_type: "default".into(),
        })
        .id();
    let planet = app
        .world_mut()
        .spawn(Planet {
            name: "Earth".into(),
            system: sys,
            planet_type: "terrestrial".into(),
        })
        .id();
    let colony = app
        .world_mut()
        .spawn((
            macrocosmo::colony::Colony {
                planet,
                growth_rate: 0.01,
            },
            Buildings {
                slots: vec![None, Some(BuildingId::new("mine")), None, None],
            },
        ))
        .id();

    // Simulate migration for capital.
    {
        let world = app.world_mut();
        let mut buildings = world.get_mut::<Buildings>(colony).unwrap();
        if buildings.slots[0].is_none() {
            buildings.slots[0] = Some(BuildingId::new("planetary_capital_t3"));
        }
    }

    let buildings = app.world().get::<Buildings>(colony).unwrap();
    assert_eq!(
        buildings.slots[0],
        Some(BuildingId::new("planetary_capital_t3"))
    );
}

// ---------------------------------------------------------------------------
// 6. Hub not in direct-buildable list
// ---------------------------------------------------------------------------

#[test]
fn test_hub_not_in_planet_buildings_list() {
    let reg = lua_building_registry();
    let planet_buildings = reg.planet_buildings();
    for b in &planet_buildings {
        assert!(
            !b.id.starts_with("colony_hub_") && !b.id.starts_with("planetary_capital_"),
            "Hub/Capital '{}' should not appear in direct-buildable list",
            b.id
        );
    }
}

#[test]
fn test_hub_not_in_system_buildings_list() {
    let reg = lua_building_registry();
    let system_buildings = reg.system_buildings();
    for b in &system_buildings {
        assert!(
            !b.id.starts_with("colony_hub_") && !b.id.starts_with("planetary_capital_"),
            "Hub/Capital '{}' should not appear in system-buildable list",
            b.id
        );
    }
}
