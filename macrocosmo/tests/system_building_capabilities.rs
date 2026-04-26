//! Regression test for production Lua system buildings (`scripts/buildings/system/init.lua`).
//!
//! Bug: production Lua building definitions for `shipyard`, `port`, and
//! `orbital_research_lab` lacked the `capabilities = { ... }` field. The AI
//! emitter (`ai/emitters.rs:198-220`) reads `def.capabilities.contains_key("shipyard")`
//! / `"port"` to compute `systems_with_shipyard` / `can_build_ships`. With the
//! capability missing, `can_build_ships` was permanently 0.0, causing
//! NPC empires to be unable to build ships and Rule 5a to spam-construct
//! shipyards forever.
//!
//! Fix: production Lua now declares the canonical capability name on each
//! system building (matches the test fixture in `tests/fixtures/buildings_test.lua`).

use macrocosmo::scripting::ScriptEngine;
use macrocosmo::scripting::building_api::parse_building_definitions;

#[test]
fn production_system_buildings_declare_canonical_capabilities() {
    let engine = ScriptEngine::new().expect("ScriptEngine::new");
    let init = engine.scripts_dir().join("init.lua");
    engine.load_file(&init).expect("load init.lua");

    let defs = parse_building_definitions(engine.lua()).expect("parse buildings");

    let shipyard = defs
        .iter()
        .find(|d| d.id == "shipyard")
        .expect("shipyard building should be defined");
    assert!(
        shipyard.is_system_building,
        "shipyard must be a system building"
    );
    assert!(
        shipyard.capabilities.contains_key("shipyard"),
        "shipyard must declare the `shipyard` capability so the AI emitter can detect it (regression: previously missing → can_build_ships permanently 0.0)"
    );

    let port = defs
        .iter()
        .find(|d| d.id == "port")
        .expect("port building should be defined");
    assert!(port.is_system_building, "port must be a system building");
    assert!(
        port.capabilities.contains_key("port"),
        "port must declare the `port` capability (regression)"
    );

    let lab = defs
        .iter()
        .find(|d| d.id == "orbital_research_lab")
        .expect("orbital_research_lab building should be defined");
    assert!(
        lab.is_system_building,
        "orbital_research_lab must be a system building"
    );
    assert!(
        lab.capabilities.contains_key("research"),
        "orbital_research_lab must declare the `research` capability (consistency with test fixture)"
    );
}
