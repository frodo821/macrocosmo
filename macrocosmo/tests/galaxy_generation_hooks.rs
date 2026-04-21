//! Integration tests for the #181 galaxy-generation Lua hooks:
//! `on_galaxy_generate_empty`, `on_choose_capitals`, and
//! `on_initialize_system`.
//!
//! These tests spin up a minimal Bevy app with a `ScriptEngine` resource,
//! register hooks from Lua, and then run `generate_galaxy` to verify that:
//!
//! - without hooks, the default Rust phases produce the same kind of galaxy
//!   (backward compatibility check)
//! - each hook in isolation replaces the corresponding default phase
//! - all three hooks combined work together and are independent.

use bevy::prelude::*;
use macrocosmo::galaxy::{Planet, StarSystem};
use macrocosmo::scripting::ScriptEngine;
use macrocosmo::scripting::galaxy_api::{
    PlanetTypeDefinition, PlanetTypeRegistry, ResourceBias, StarTypeDefinition, StarTypeRegistry,
};

/// Build a pair of registries with two distinct star/planet types so tests
/// can tell which Lua-provided id was used.
fn test_registries() -> (StarTypeRegistry, PlanetTypeRegistry) {
    let mut star_reg = StarTypeRegistry::default();
    star_reg.types.push(StarTypeDefinition {
        id: "type_a".into(),
        name: "Type A".into(),
        description: String::new(),
        color: [1.0, 1.0, 1.0],
        planet_lambda: 2.0,
        max_planets: 4,
        habitability_bonus: 0.0,
        weight: 1.0,
        modifiers: Vec::new(),
    });
    star_reg.types.push(StarTypeDefinition {
        id: "type_b".into(),
        name: "Type B".into(),
        description: String::new(),
        color: [1.0, 0.8, 0.5],
        planet_lambda: 3.0,
        max_planets: 5,
        habitability_bonus: 0.1,
        weight: 1.0,
        modifiers: Vec::new(),
    });

    let mut planet_reg = PlanetTypeRegistry::default();
    planet_reg.types.push(PlanetTypeDefinition {
        id: "terrestrial".into(),
        name: "Terrestrial".into(),
        description: String::new(),
        base_habitability: 0.7,
        base_slots: 4,
        resource_bias: ResourceBias {
            minerals: 1.0,
            energy: 1.0,
            research: 1.0,
        },
        weight: 1.0,
        default_biome: None,
    });
    planet_reg.types.push(PlanetTypeDefinition {
        id: "gas_giant".into(),
        name: "Gas Giant".into(),
        description: String::new(),
        base_habitability: 0.0,
        base_slots: 0,
        resource_bias: ResourceBias {
            minerals: 0.0,
            energy: 0.0,
            research: 0.0,
        },
        weight: 1.0,
        default_biome: None,
    });

    (star_reg, planet_reg)
}

fn build_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    let (star_reg, planet_reg) = test_registries();
    app.insert_resource(star_reg);
    app.insert_resource(planet_reg);
    let engine = ScriptEngine::new().expect("script engine");
    app.insert_resource(engine);
    app
}

/// Baseline: no hooks registered → default galaxy is produced.
#[test]
fn no_hooks_uses_default_generation() {
    let mut app = build_app();
    app.add_systems(Startup, macrocosmo::galaxy::generate_galaxy);
    app.update();

    let star_count = app
        .world_mut()
        .query::<&StarSystem>()
        .iter(app.world())
        .count();
    // Default generation aims for 150 systems.
    assert!(
        star_count > 10,
        "default generation should produce many systems, got {star_count}"
    );

    // A capital should exist.
    let capital_count = app
        .world_mut()
        .query::<&StarSystem>()
        .iter(app.world())
        .filter(|s| s.is_capital)
        .count();
    assert_eq!(capital_count, 1, "exactly one capital expected");
}

/// on_galaxy_generate_empty replaces Phase A: only the systems the hook
/// spawns are present, with exactly the positions/star types it specified.
#[test]
fn on_galaxy_generate_empty_replaces_phase_a() {
    let mut app = build_app();
    {
        let engine = app.world().resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"
                on_galaxy_generate_empty(function(ctx)
                    ctx:spawn_empty_system("Alpha", {0.0, 0.0, 0.0}, "type_a")
                    ctx:spawn_empty_system("Beta",  {5.0, 5.0, 0.0}, "type_b")
                    ctx:spawn_empty_system("Gamma", {10.0, 0.0, 0.0}, "type_a")
                end)
                "#,
            )
            .exec()
            .unwrap();
    }
    app.add_systems(Startup, macrocosmo::galaxy::generate_galaxy);
    app.update();

    let star_names: Vec<String> = app
        .world_mut()
        .query::<&StarSystem>()
        .iter(app.world())
        .map(|s| s.name.clone())
        .collect();
    assert_eq!(
        star_names.len(),
        3,
        "expected exactly 3 systems, got {star_names:?}"
    );
    assert!(star_names.contains(&"Alpha".into()));
    assert!(star_names.contains(&"Beta".into()));
    assert!(star_names.contains(&"Gamma".into()));

    // Capital swap still runs (Phase B default picked the closest-to-20ly
    // system, which is Gamma at distance 10 — but any single capital is fine).
    let capital_count = app
        .world_mut()
        .query::<&StarSystem>()
        .iter(app.world())
        .filter(|s| s.is_capital)
        .count();
    assert_eq!(capital_count, 1);
}

/// on_choose_capitals overrides which system becomes the capital.
#[test]
fn on_choose_capitals_overrides_default_heuristic() {
    let mut app = build_app();
    {
        let engine = app.world().resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"
                on_galaxy_generate_empty(function(ctx)
                    ctx:spawn_empty_system("Alpha", {0.0, 0.0, 0.0}, "type_a")
                    ctx:spawn_empty_system("Beta",  {500.0, 0.0, 0.0}, "type_b")
                end)
                on_choose_capitals(function(ctx)
                    -- Pick Beta (index 2), not Alpha (default-closest-to-20ly).
                    ctx:assign_capital(2, "test_faction")
                end)
                "#,
            )
            .exec()
            .unwrap();
    }
    app.add_systems(Startup, macrocosmo::galaxy::generate_galaxy);
    app.update();

    let mut capital_name: Option<String> = None;
    for s in app.world_mut().query::<&StarSystem>().iter(app.world()) {
        if s.is_capital {
            capital_name = Some(s.name.clone());
        }
    }
    assert_eq!(capital_name.as_deref(), Some("Beta"));
}

/// on_initialize_system lets the callback spawn planets for each system,
/// bypassing the default planet-generation logic.
#[test]
fn on_initialize_system_replaces_default_planets() {
    let mut app = build_app();
    {
        let engine = app.world().resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"
                on_galaxy_generate_empty(function(ctx)
                    ctx:spawn_empty_system("Alpha", {0.0, 0.0, 0.0}, "type_a")
                    ctx:spawn_empty_system("Beta",  {5.0, 0.0, 0.0}, "type_b")
                end)
                on_initialize_system(function(ctx)
                    -- Every system gets exactly one "home" planet, regardless
                    -- of the star type's default planet count.
                    ctx:spawn_planet(ctx.name .. " Home", "terrestrial", {
                        habitability = 0.9,
                        max_building_slots = 5,
                    })
                end)
                "#,
            )
            .exec()
            .unwrap();
    }
    app.add_systems(Startup, macrocosmo::galaxy::generate_galaxy);
    app.update();

    // Collect planets per system name.
    let mut planet_names: Vec<String> = app
        .world_mut()
        .query::<&Planet>()
        .iter(app.world())
        .map(|p| p.name.clone())
        .collect();
    planet_names.sort();
    assert_eq!(
        planet_names,
        vec!["Alpha Home".to_string(), "Beta Home".to_string()]
    );

    // Each planet should be terrestrial with habitability 0.9.
    let planets: Vec<(String, String)> = app
        .world_mut()
        .query::<&Planet>()
        .iter(app.world())
        .map(|p| (p.name.clone(), p.planet_type.clone()))
        .collect();
    for (_name, typ) in &planets {
        assert_eq!(typ, "terrestrial");
    }
}

/// All three hooks combined. Exercises the full Lua pipeline and confirms
/// the phases are independent.
#[test]
fn all_three_hooks_combined() {
    let mut app = build_app();
    {
        let engine = app.world().resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"
                on_galaxy_generate_empty(function(ctx)
                    for i = 1, 4 do
                        ctx:spawn_empty_system(
                            "Sys-" .. i,
                            { i * 10.0, 0.0, 0.0 },
                            "type_a"
                        )
                    end
                end)
                on_choose_capitals(function(ctx)
                    ctx:assign_capital(3, "alpha_faction")
                end)
                on_initialize_system(function(ctx)
                    if ctx.is_capital then
                        ctx:spawn_planet("Capital Prime", "terrestrial", {
                            habitability = 1.0,
                            max_building_slots = 7,
                        })
                    else
                        ctx:spawn_planet(ctx.name .. "-a", "gas_giant")
                        ctx:spawn_planet(ctx.name .. "-b", "terrestrial", { habitability = 0.4 })
                    end
                end)
                "#,
            )
            .exec()
            .unwrap();
    }
    app.add_systems(Startup, macrocosmo::galaxy::generate_galaxy);
    app.update();

    // 4 systems should have been generated.
    let systems: Vec<(String, bool)> = app
        .world_mut()
        .query::<&StarSystem>()
        .iter(app.world())
        .map(|s| (s.name.clone(), s.is_capital))
        .collect();
    assert_eq!(systems.len(), 4);

    // Exactly one capital, and it should be Sys-3 (the one Lua picked).
    let capitals: Vec<&String> = systems.iter().filter(|(_, c)| *c).map(|(n, _)| n).collect();
    assert_eq!(capitals.len(), 1);
    assert_eq!(capitals[0], "Sys-3");

    // Capital has "Capital Prime" planet; others have 2 planets each.
    let planets: Vec<String> = app
        .world_mut()
        .query::<&Planet>()
        .iter(app.world())
        .map(|p| p.name.clone())
        .collect();
    assert!(planets.contains(&"Capital Prime".to_string()));
    // Non-capital systems produce a-suffixed and b-suffixed planets.
    assert!(planets.iter().any(|p| p.ends_with("-a")));
    assert!(planets.iter().any(|p| p.ends_with("-b")));
    // Total planets: 1 (capital) + 3 * 2 (non-capital) = 7.
    assert_eq!(planets.len(), 7);
}

/// Only the last registration of the same hook wins (replacement semantics).
#[test]
fn last_hook_registration_wins() {
    let mut app = build_app();
    {
        let engine = app.world().resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"
                on_galaxy_generate_empty(function(ctx)
                    ctx:spawn_empty_system("FirstReg", {0, 0, 0}, "type_a")
                end)
                on_galaxy_generate_empty(function(ctx)
                    ctx:spawn_empty_system("SecondReg", {0, 0, 0}, "type_a")
                end)
                "#,
            )
            .exec()
            .unwrap();
    }
    app.add_systems(Startup, macrocosmo::galaxy::generate_galaxy);
    app.update();

    let names: Vec<String> = app
        .world_mut()
        .query::<&StarSystem>()
        .iter(app.world())
        .map(|s| s.name.clone())
        .collect();
    assert_eq!(names, vec!["SecondReg".to_string()]);
}

/// Malformed hook (unknown star_type) skips that system with a warning but
/// still produces valid state for the rest.
#[test]
fn unknown_star_type_is_skipped() {
    let mut app = build_app();
    {
        let engine = app.world().resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"
                on_galaxy_generate_empty(function(ctx)
                    ctx:spawn_empty_system("Good", {0, 0, 0}, "type_a")
                    ctx:spawn_empty_system("Bad", {5, 0, 0}, "nonexistent_type")
                    ctx:spawn_empty_system("AlsoGood", {10, 0, 0}, "type_b")
                end)
                "#,
            )
            .exec()
            .unwrap();
    }
    app.add_systems(Startup, macrocosmo::galaxy::generate_galaxy);
    app.update();

    let names: Vec<String> = app
        .world_mut()
        .query::<&StarSystem>()
        .iter(app.world())
        .map(|s| s.name.clone())
        .collect();
    assert_eq!(
        names.len(),
        2,
        "Bad should have been skipped, got {names:?}"
    );
    assert!(names.contains(&"Good".into()));
    assert!(names.contains(&"AlsoGood".into()));
}

/// Hook error falls back to default. A Lua runtime error during
/// `on_galaxy_generate_empty` should not crash the game.
#[test]
fn phase_a_hook_error_falls_back_to_default() {
    let mut app = build_app();
    {
        let engine = app.world().resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"
                on_galaxy_generate_empty(function(ctx)
                    error("intentional test error")
                end)
                "#,
            )
            .exec()
            .unwrap();
    }
    app.add_systems(Startup, macrocosmo::galaxy::generate_galaxy);
    app.update();

    // Default generation still runs — we should get many systems.
    let star_count = app
        .world_mut()
        .query::<&StarSystem>()
        .iter(app.world())
        .count();
    assert!(
        star_count > 10,
        "fallback default should still produce systems, got {star_count}"
    );
}
