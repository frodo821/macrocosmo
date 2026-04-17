//! Integration tests for the `scripts/lib/capital.lua` helper
//! (`initialize_default_capital`).
//!
//! These tests load the helper through `require("lib.capital")` against a
//! freshly constructed `ScriptEngine`, drive it with a mock `GameStartCtx`,
//! and assert that the recorded `GameStartActions` reflect the expected
//! defaults / overrides. We don't spin up a full Bevy app because the helper
//! purely records intent into the ctx — applying those actions to the ECS is
//! already covered by the existing GameStartCtx integration tests.
//!
//! Issue #180.

use macrocosmo::scripting::ScriptEngine;
use macrocosmo::scripting::game_start_ctx::{GameStartCtx, PlanetRef};

/// Build an engine with `initialize_default_capital` loaded and a fresh ctx
/// installed at the global name `ctx`. Tests then `lua.load("...")` to call
/// the helper with whatever opts they want.
fn setup(faction_id: &str) -> (ScriptEngine, GameStartCtx) {
    let engine = ScriptEngine::new().expect("script engine");
    // Load the library. This sets `_G.initialize_default_capital`.
    engine
        .lua()
        .load(r#"require("lib.capital")"#)
        .exec()
        .expect("require lib.capital");
    let ctx = GameStartCtx::new(faction_id.to_string());
    engine.lua().globals().set("ctx", ctx.clone()).unwrap();
    (engine, ctx)
}

#[test]
fn defaults_record_capital_layout() {
    let (engine, ctx) = setup("test_faction");

    engine
        .lua()
        .load("initialize_default_capital(ctx)")
        .exec()
        .expect("call helper");

    let actions = ctx.take_actions();

    // System-level: marked capital, surveyed, and existing planets cleared.
    assert!(actions.mark_capital, "system should be marked capital");
    assert!(actions.mark_surveyed, "system should be marked surveyed");
    assert!(actions.clear_planets, "existing planets should be cleared");

    // Home planet: spawned first, with default name "<faction> Prime".
    assert!(
        !actions.spawned_planets.is_empty(),
        "expected at least one spawned planet (the home)"
    );
    let home = &actions.spawned_planets[0];
    assert_eq!(home.name, "test_faction Prime");
    assert_eq!(home.planet_type, "terrestrial");

    // Home planet attribute roll: high habitability and 5-6 building slots.
    let attrs = &home.attributes;
    let hab = attrs.habitability.expect("habitability set");
    assert!(
        (0.85..=1.0).contains(&hab),
        "home habitability {hab} not in [0.85, 1.0]"
    );
    let slots = attrs.max_building_slots.expect("max_building_slots set");
    assert!(
        (5..=6).contains(&slots),
        "home max_building_slots {slots} not in [5, 6]"
    );

    // Additional planets: 2-4 of them (so total spawned = 3..=5).
    assert!(
        (3..=5).contains(&actions.spawned_planets.len()),
        "expected 3-5 total spawned planets, got {}",
        actions.spawned_planets.len()
    );

    // Colonize the home planet (spawned index 1).
    assert_eq!(actions.colonize_planet, Some(PlanetRef::Spawned(1)));

    // Default starter planet buildings: mine, power_plant, farm — all on
    // the home planet (Spawned(1)).
    let planet_buildings: Vec<&str> = actions
        .planet_buildings
        .iter()
        .map(|(p, b)| {
            assert_eq!(*p, PlanetRef::Spawned(1));
            b.as_str()
        })
        .collect();
    // #280: planetary_capital_t3 is prepended by lib/capital.lua.
    assert_eq!(
        planet_buildings,
        vec!["planetary_capital_t3", "mine", "power_plant", "farm"]
    );

    // Default system buildings: shipyard.
    assert_eq!(actions.system_buildings, vec!["shipyard".to_string()]);

    // Default starter ships: one explorer.
    assert_eq!(actions.ships.len(), 1);
    assert_eq!(
        actions.ships[0],
        ("explorer_mk1".to_string(), "Explorer I".to_string())
    );
}

#[test]
fn overrides_home_planet_name_and_type() {
    let (engine, ctx) = setup("frodos");

    engine
        .lua()
        .load(
            r#"
            initialize_default_capital(ctx, {
                home_planet_name = "Shire",
                home_planet_type = "ocean",
                home_planet_attrs = {
                    habitability = 0.95,
                    mineral_richness = 0.6,
                    energy_potential = 0.6,
                    research_potential = 0.7,
                    max_building_slots = 6,
                },
            })
            "#,
        )
        .exec()
        .unwrap();

    let actions = ctx.take_actions();
    let home = &actions.spawned_planets[0];
    assert_eq!(home.name, "Shire");
    assert_eq!(home.planet_type, "ocean");
    assert_eq!(home.attributes.habitability, Some(0.95));
    assert_eq!(home.attributes.max_building_slots, Some(6));
}

#[test]
fn additional_planets_count_override_via_number() {
    let (engine, ctx) = setup("numerics");

    engine
        .lua()
        .load(r#"initialize_default_capital(ctx, { additional_planets = 7 })"#)
        .exec()
        .unwrap();

    let actions = ctx.take_actions();
    // 1 home + 7 additional = 8 spawned planets total.
    assert_eq!(actions.spawned_planets.len(), 8);
}

#[test]
fn additional_planets_explicit_specs_are_respected() {
    let (engine, ctx) = setup("manualists");

    engine
        .lua()
        .load(
            r#"
            initialize_default_capital(ctx, {
                additional_planets = {
                    { name = "Forge",  type = "barren",   attrs = { habitability = 0.0, max_building_slots = 4 } },
                    { name = "Garden", type = "ocean",    attrs = { habitability = 0.5 } },
                    { name = "Crown",  type = "gas_giant" },
                },
            })
            "#,
        )
        .exec()
        .unwrap();

    let actions = ctx.take_actions();
    // 1 home + 3 explicit = 4 spawned planets.
    assert_eq!(actions.spawned_planets.len(), 4);
    assert_eq!(actions.spawned_planets[1].name, "Forge");
    assert_eq!(actions.spawned_planets[1].planet_type, "barren");
    assert_eq!(
        actions.spawned_planets[1].attributes.habitability,
        Some(0.0)
    );
    assert_eq!(
        actions.spawned_planets[1].attributes.max_building_slots,
        Some(4)
    );
    assert_eq!(actions.spawned_planets[2].name, "Garden");
    assert_eq!(actions.spawned_planets[2].planet_type, "ocean");
    assert_eq!(actions.spawned_planets[3].name, "Crown");
    assert_eq!(actions.spawned_planets[3].planet_type, "gas_giant");
}

#[test]
fn starter_buildings_and_ships_are_overridable() {
    let (engine, ctx) = setup("custom");

    engine
        .lua()
        .load(
            r#"
            initialize_default_capital(ctx, {
                starter_buildings = { "mine" },
                starter_system_buildings = { "shipyard", "port" },
                starter_ships = {
                    { "explorer_mk1", "Pathfinder" },
                    { "courier_mk1",  "Mailman"    },
                },
            })
            "#,
        )
        .exec()
        .unwrap();

    let actions = ctx.take_actions();

    let planet_building_ids: Vec<&str> = actions
        .planet_buildings
        .iter()
        .map(|(_, b)| b.as_str())
        .collect();
    // #280: planetary_capital_t3 prepended by lib/capital.lua.
    assert_eq!(planet_building_ids, vec!["planetary_capital_t3", "mine"]);

    assert_eq!(
        actions.system_buildings,
        vec!["shipyard".to_string(), "port".to_string()]
    );

    assert_eq!(actions.ships.len(), 2);
    assert_eq!(
        actions.ships[0],
        ("explorer_mk1".to_string(), "Pathfinder".to_string())
    );
    assert_eq!(
        actions.ships[1],
        ("courier_mk1".to_string(), "Mailman".to_string())
    );
}

#[test]
fn helper_is_globally_available_after_require() {
    // Ensures the module exports the function as a global so that downstream
    // faction scripts can call it without re-requiring.
    let engine = ScriptEngine::new().unwrap();
    engine
        .lua()
        .load(r#"require("lib.capital")"#)
        .exec()
        .unwrap();

    let kind: String = engine
        .lua()
        .load(r#"return type(initialize_default_capital)"#)
        .eval()
        .unwrap();
    assert_eq!(kind, "function");
}

#[test]
fn additional_planets_random_count_within_bounds() {
    // Run multiple times; with the default opts the additional-planet count
    // should always be in [2, 4].
    for trial in 0..20 {
        let (engine, ctx) = setup(&format!("trial_{trial}"));
        engine
            .lua()
            .load("initialize_default_capital(ctx)")
            .exec()
            .unwrap();
        let actions = ctx.take_actions();
        let additional = actions.spawned_planets.len() - 1; // minus the home
        assert!(
            (2..=4).contains(&additional),
            "trial {trial}: additional planet count {additional} out of [2, 4]"
        );
    }
}
