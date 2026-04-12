//! #182 — integration tests for `define_predefined_system`,
//! `define_map_type`, `set_active_map_type`, `ctx:spawn_predefined_system`,
//! and `ctx:assign_predefined_capitals`.

use bevy::prelude::*;
use macrocosmo::galaxy::{Planet, StarSystem};
use macrocosmo::scripting::galaxy_api::{
    PlanetTypeDefinition, PlanetTypeRegistry, ResourceBias, StarTypeDefinition, StarTypeRegistry,
};
use macrocosmo::scripting::map_api::{
    parse_map_types, parse_predefined_systems, MapTypeRegistry, PredefinedSystemRegistry,
};
use macrocosmo::scripting::ScriptEngine;

fn test_registries() -> (StarTypeRegistry, PlanetTypeRegistry) {
    let mut star_reg = StarTypeRegistry::default();
    star_reg.types.push(StarTypeDefinition {
        id: "yellow_dwarf".into(),
        name: "Yellow Dwarf".into(),
        description: String::new(),
        color: [1.0, 1.0, 0.7],
        planet_lambda: 2.0,
        max_planets: 5,
        habitability_bonus: 0.0,
        weight: 1.0,
        modifiers: Vec::new(),
    });

    let mut planet_reg = PlanetTypeRegistry::default();
    for (id, hab) in [("terrestrial", 0.8), ("barren", 0.1)] {
        planet_reg.types.push(PlanetTypeDefinition {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            base_habitability: hab,
            base_slots: 4,
            resource_bias: ResourceBias {
                minerals: 1.0,
                energy: 1.0,
                research: 1.0,
            },
            weight: 1.0,
        });
    }

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
    // Default empty registries — tests populate via Lua + parse.
    app.insert_resource(PredefinedSystemRegistry::default());
    app.insert_resource(MapTypeRegistry::default());
    app
}

/// Load Lua then parse-and-insert the #182 registries so `generate_galaxy`
/// can pick them up.
fn finalize_registries(app: &mut App) {
    let lua = app.world().resource::<ScriptEngine>().lua();
    let predefined = parse_predefined_systems(lua).unwrap();
    let map_types = parse_map_types(lua).unwrap();
    let active = macrocosmo::scripting::map_api::read_active_map_type(lua);

    let mut p_reg = PredefinedSystemRegistry::default();
    for d in predefined {
        p_reg.systems.insert(d.id.clone(), d);
    }
    let mut m_reg = MapTypeRegistry::default();
    for d in map_types {
        m_reg.types.insert(d.id.clone(), d);
    }
    m_reg.current = active;

    app.insert_resource(p_reg);
    app.insert_resource(m_reg);
}

/// Baseline: map type defined WITHOUT a generator stays compatible with the
/// default pipeline. This is the "default" map_type shipped in scripts/.
#[test]
fn map_type_without_generator_uses_default_pipeline() {
    let mut app = build_app();
    {
        let engine = app.world().resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"
                define_map_type { id = "default", name = "Default" }
                set_active_map_type("default")
                "#,
            )
            .exec()
            .unwrap();
    }
    finalize_registries(&mut app);
    app.add_systems(Startup, macrocosmo::galaxy::generate_galaxy);
    app.update();

    // Default spiral generator should produce many systems.
    let count = app
        .world_mut()
        .query::<&StarSystem>()
        .iter(app.world())
        .count();
    assert!(count > 10, "expected default spiral galaxy, got {count}");
}

/// Active map_type with a generator fully replaces Phase A.
#[test]
fn active_map_type_generator_runs_phase_a() {
    let mut app = build_app();
    {
        let engine = app.world().resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"
                define_map_type {
                    id = "two_stars",
                    generator = function(ctx)
                        ctx:spawn_empty_system("Alpha", {0.0, 0.0, 0.0}, "yellow_dwarf")
                        ctx:spawn_empty_system("Beta",  {5.0, 0.0, 0.0}, "yellow_dwarf")
                    end,
                }
                set_active_map_type("two_stars")
                "#,
            )
            .exec()
            .unwrap();
    }
    finalize_registries(&mut app);
    app.add_systems(Startup, macrocosmo::galaxy::generate_galaxy);
    app.update();

    let names: Vec<String> = app
        .world_mut()
        .query::<&StarSystem>()
        .iter(app.world())
        .map(|s| s.name.clone())
        .collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"Alpha".to_string()));
    assert!(names.contains(&"Beta".to_string()));
}

/// `ctx:spawn_predefined_system(id)` expands to the full spawn (position,
/// star type, planets) from the definition.
#[test]
fn spawn_predefined_system_spawns_planets() {
    let mut app = build_app();
    {
        let engine = app.world().resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"
                define_predefined_system {
                    id = "sol",
                    name = "Sol",
                    position = { 0.0, 0.0, 0.0 },
                    star_type = "yellow_dwarf",
                    planets = {
                        { name = "Earth", type = "terrestrial", habitability = 1.0 },
                        { name = "Mars",  type = "barren" },
                    },
                    capital_for_faction = "humanity_empire",
                }
                define_map_type {
                    id = "sol_only",
                    generator = function(ctx)
                        ctx:spawn_predefined_system("sol")
                    end,
                }
                set_active_map_type("sol_only")
                "#,
            )
            .exec()
            .unwrap();
    }
    finalize_registries(&mut app);
    app.add_systems(Startup, macrocosmo::galaxy::generate_galaxy);
    app.update();

    // The system "Sol" should exist with exactly the two predefined planets.
    let mut systems: Vec<_> = app
        .world_mut()
        .query::<&StarSystem>()
        .iter(app.world())
        .map(|s| s.name.clone())
        .collect();
    systems.sort();
    assert_eq!(systems, vec!["Sol".to_string()]);

    let mut planets: Vec<_> = app
        .world_mut()
        .query::<&Planet>()
        .iter(app.world())
        .map(|p| p.name.clone())
        .collect();
    planets.sort();
    assert_eq!(planets, vec!["Earth".to_string(), "Mars".to_string()]);
}

/// `capital_for_faction` on a predefined system is picked up by the default
/// Phase B, making that system the capital even if it's not closest to 20ly.
#[test]
fn predefined_capital_hint_selects_capital() {
    let mut app = build_app();
    {
        let engine = app.world().resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"
                define_predefined_system {
                    id = "homeworld",
                    name = "Homeworld",
                    -- Intentionally far away (500 ly) — default heuristic would
                    -- pick the closest-to-20ly system instead.
                    position = { 500.0, 0.0, 0.0 },
                    star_type = "yellow_dwarf",
                    planets = { { name = "H1", type = "terrestrial" } },
                    capital_for_faction = "humanity_empire",
                }
                define_map_type {
                    id = "two",
                    generator = function(ctx)
                        ctx:spawn_empty_system("Nearby", {20.0, 0.0, 0.0}, "yellow_dwarf")
                        ctx:spawn_predefined_system("homeworld")
                    end,
                }
                set_active_map_type("two")
                "#,
            )
            .exec()
            .unwrap();
    }
    finalize_registries(&mut app);
    app.add_systems(Startup, macrocosmo::galaxy::generate_galaxy);
    app.update();

    let mut capital_name: Option<String> = None;
    for s in app.world_mut().query::<&StarSystem>().iter(app.world()) {
        if s.is_capital {
            capital_name = Some(s.name.clone());
        }
    }
    assert_eq!(capital_name.as_deref(), Some("Homeworld"));
}

/// `ctx:assign_predefined_capitals()` records assignments for every system
/// with a `capital_for_faction` hint.
#[test]
fn assign_predefined_capitals_records_assignments() {
    let mut app = build_app();
    {
        let engine = app.world().resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"
                define_predefined_system {
                    id = "sol",
                    name = "Sol",
                    position = { 0, 0, 0 },
                    star_type = "yellow_dwarf",
                    planets = { { name = "Earth", type = "terrestrial" } },
                    capital_for_faction = "humanity_empire",
                }
                _capitals_counted = -1
                define_map_type {
                    id = "solonly",
                    generator = function(ctx)
                        ctx:spawn_predefined_system("sol")
                        ctx:spawn_empty_system("Filler", {1.0, 0.0, 0.0}, "yellow_dwarf")
                    end,
                }
                set_active_map_type("solonly")
                on_choose_capitals(function(ctx)
                    _capitals_counted = ctx:assign_predefined_capitals()
                end)
                "#,
            )
            .exec()
            .unwrap();
    }
    finalize_registries(&mut app);
    app.add_systems(Startup, macrocosmo::galaxy::generate_galaxy);
    app.update();

    // Capital should be "Sol".
    let mut capital_name: Option<String> = None;
    for s in app.world_mut().query::<&StarSystem>().iter(app.world()) {
        if s.is_capital {
            capital_name = Some(s.name.clone());
        }
    }
    assert_eq!(capital_name.as_deref(), Some("Sol"));

    // The Lua hook should have been called and reported 1 assignment.
    let engine = app.world().resource::<ScriptEngine>();
    let count: i64 = engine.lua().globals().get("_capitals_counted").unwrap();
    assert_eq!(count, 1);
}

/// Unknown predefined id raises a Lua error inside the generator; the engine
/// falls back to the default generation rather than hanging / producing zero
/// systems.
#[test]
fn spawn_predefined_system_unknown_id_falls_back() {
    let mut app = build_app();
    {
        let engine = app.world().resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"
                define_map_type {
                    id = "bad",
                    generator = function(ctx)
                        ctx:spawn_predefined_system("does_not_exist")
                    end,
                }
                set_active_map_type("bad")
                "#,
            )
            .exec()
            .unwrap();
    }
    finalize_registries(&mut app);
    app.add_systems(Startup, macrocosmo::galaxy::generate_galaxy);
    app.update();

    // Should fall back to default, producing many systems.
    let count = app
        .world_mut()
        .query::<&StarSystem>()
        .iter(app.world())
        .count();
    assert!(count > 0, "expected fallback to default spiral");
}

/// Backward-compatibility: when neither a map_type nor a generate_empty hook
/// is registered, `generate_galaxy` still runs the default pipeline.
#[test]
fn no_map_type_no_hooks_still_default() {
    let mut app = build_app();
    finalize_registries(&mut app); // no Lua definitions at all
    app.add_systems(Startup, macrocosmo::galaxy::generate_galaxy);
    app.update();

    let count = app
        .world_mut()
        .query::<&StarSystem>()
        .iter(app.world())
        .count();
    assert!(count > 10);
    let capital_count = app
        .world_mut()
        .query::<&StarSystem>()
        .iter(app.world())
        .filter(|s| s.is_capital)
        .count();
    assert_eq!(capital_count, 1);
}
