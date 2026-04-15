//! #289: β Lua View types — integration tests.
//!
//! These cover end-to-end navigation across the `event.gamestate`
//! snapshot as it appears from Lua (not just field-by-field from Rust),
//! exercising the hierarchical `system -> planets/colonies ->
//! buildings/production` chain, the Ship.state tag-union, and the
//! Fleet-through-flagship proxy. They complement the per-commit unit
//! tests in `src/scripting/gamestate_view.rs`.

use bevy::prelude::*;
use macrocosmo::amount::Amt;
use macrocosmo::colony::{Buildings, Colony, Production, ResourceStockpile};
use macrocosmo::components::Position;
use macrocosmo::condition::ScopedFlags;
use macrocosmo::galaxy::{Planet, Sovereignty, StarSystem, SystemModifiers};
use macrocosmo::modifier::ModifiedValue;
use macrocosmo::player::{Empire, PlayerEmpire};
use macrocosmo::scripting::building_api::BuildingId;
use macrocosmo::scripting::gamestate_view::build_gamestate_table;
use macrocosmo::scripting::ScriptEngine;
use macrocosmo::ship::fleet::{Fleet, FleetMembers};
use macrocosmo::ship::{EquippedModule, Owner, Ship, ShipHitpoints, ShipState};
use macrocosmo::technology::{GameFlags, TechId, TechTree};
use macrocosmo::time_system::GameClock;

/// Build a small but representative world touching every view type:
/// - one player empire with a tech + flag
/// - one capital system with position, sovereignty, modifiers
/// - two planets under that system (one colonized)
/// - one colony with buildings + production
/// - one ship with hp + modules + Docked state
/// - one fleet with that ship as flagship
fn scenario_world() -> World {
    let mut world = World::new();
    world.insert_resource(GameClock::new(1));

    let mut tree = TechTree::default();
    tree.researched.insert(TechId("industrial_mining".into()));
    let mut flags = GameFlags::default();
    flags.set("first_contact");

    let empire = world
        .spawn((
            Empire {
                name: "Terran Republic".into(),
            },
            PlayerEmpire,
            tree,
            flags,
            ScopedFlags::default(),
        ))
        .id();

    let system = world
        .spawn((
            StarSystem {
                name: "Sol".into(),
                surveyed: true,
                is_capital: true,
                star_type: "yellow_dwarf".into(),
            },
            Position {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            ResourceStockpile {
                minerals: Amt::units(1000),
                energy: Amt::units(500),
                research: Amt::ZERO,
                food: Amt::units(250),
                authority: Amt::units(10000),
            },
            Sovereignty {
                owner: Some(Owner::Empire(empire)),
                control_score: 1.0,
            },
            SystemModifiers::default(),
        ))
        .id();

    let planet_earth = world
        .spawn(Planet {
            name: "Earth".into(),
            system,
            planet_type: "terrestrial".into(),
        })
        .id();
    let _planet_mars = world
        .spawn(Planet {
            name: "Mars".into(),
            system,
            planet_type: "barren".into(),
        })
        .id();
    world.spawn((
        Colony {
            planet: planet_earth,
            population: 100.0,
            growth_rate: 0.02,
        },
        Buildings {
            slots: vec![
                Some(BuildingId("mine".into())),
                Some(BuildingId("farm".into())),
            ],
        },
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::units(10)),
            energy_per_hexadies: ModifiedValue::new(Amt::units(5)),
            research_per_hexadies: ModifiedValue::new(Amt::units(2)),
            food_per_hexadies: ModifiedValue::new(Amt::units(8)),
        },
    ));

    let ship = world
        .spawn((
            Ship {
                name: "Pioneer".into(),
                design_id: "explorer_mk1".into(),
                hull_id: "corvette".into(),
                modules: vec![EquippedModule {
                    slot_type: "aux".into(),
                    module_id: "scanner".into(),
                }],
                owner: Owner::Empire(empire),
                sublight_speed: 1.0,
                ftl_range: 5.0,
                player_aboard: false,
                home_port: system,
                design_revision: 0,
                fleet: None,
            },
            ShipHitpoints {
                hull: 50.0,
                hull_max: 50.0,
                armor: 0.0,
                armor_max: 0.0,
                shield: 10.0,
                shield_max: 10.0,
                shield_regen: 0.5,
            },
            ShipState::Docked { system },
        ))
        .id();
    world.spawn((
        Fleet {
            name: "Alpha".into(),
            flagship: Some(ship),
        },
        FleetMembers(vec![ship]),
    ));

    world
}

#[test]
fn test_gamestate_view_hierarchical_navigation() {
    // Navigation chain: empire -> colonies (through system planet_ids)
    // -> buildings + production -> ship -> state.kind
    let engine = ScriptEngine::new().unwrap();
    let mut world = scenario_world();
    let gs = build_gamestate_table(engine.lua(), &mut world).unwrap();
    engine.lua().globals().set("_test_gs", gs).unwrap();

    let summary: String = engine
        .lua()
        .load(
            r#"
            local gs = _test_gs
            -- Sol + capital check
            local parts = {}
            for _, sid in ipairs(gs.system_ids) do
                local sys = gs.systems[sid]
                table.insert(parts, sys.name)
                -- planets under this system
                for _, pid in ipairs(sys.planet_ids) do
                    local p = gs.planets[pid]
                    table.insert(parts, p.name .. ":" .. p.biome)
                end
                -- colonies under this system
                for _, cid in ipairs(sys.colony_ids) do
                    local c = gs.colonies[cid]
                    table.insert(parts, "pop=" .. tostring(c.population))
                    table.insert(parts, "bld1=" .. c.building_ids[1])
                    table.insert(parts, "m/hx=" .. tostring(c.production.minerals_per_hexadies))
                end
            end
            -- ship.state via first ship_id
            for _, sid in ipairs(gs.ship_ids) do
                local s = gs.ships[sid]
                table.insert(parts, "state=" .. s.state.kind)
                table.insert(parts, "hp=" .. tostring(s.hp.hull))
                table.insert(parts, "mod1=" .. s.modules[1].module_id)
            end
            -- fleet proxy
            for _, fid in ipairs(gs.fleet_ids) do
                local f = gs.fleets[fid]
                table.insert(parts, "fleet_state=" .. f.state.kind)
                table.insert(parts, "fleet_owner_kind=" .. f.owner_kind)
            end
            return table.concat(parts, "|")
            "#,
        )
        .eval()
        .unwrap();
    assert!(summary.contains("Sol"), "Sol missing: {summary}");
    assert!(
        summary.contains("Earth:terrestrial"),
        "Earth biome: {summary}"
    );
    assert!(summary.contains("Mars:barren"), "Mars biome: {summary}");
    assert!(summary.contains("pop=100"), "colony pop: {summary}");
    assert!(summary.contains("bld1=mine"), "building id: {summary}");
    assert!(summary.contains("m/hx=10"), "production: {summary}");
    assert!(summary.contains("state=docked"), "ship state: {summary}");
    assert!(summary.contains("hp=50"), "ship hp: {summary}");
    assert!(summary.contains("mod1=scanner"), "ship module: {summary}");
    assert!(
        summary.contains("fleet_state=docked"),
        "fleet state: {summary}"
    );
    assert!(
        summary.contains("fleet_owner_kind=empire"),
        "fleet owner: {summary}"
    );
}

#[test]
fn test_view_mutation_blocked_all_nested() {
    // #289 β: every nested sealed table must refuse writes.
    let engine = ScriptEngine::new().unwrap();
    let mut world = scenario_world();
    let gs = build_gamestate_table(engine.lua(), &mut world).unwrap();
    engine.lua().globals().set("_test_gs", gs).unwrap();

    // A handful of representative writes across the new surface.
    let scripts: &[&str] = &[
        r#"
        for _, sid in ipairs(_test_gs.system_ids) do
            _test_gs.systems[sid].position.x = 99.0
        end
        "#,
        r#"
        for _, sid in ipairs(_test_gs.system_ids) do
            _test_gs.systems[sid].modifiers.ship_speed = 2.0
        end
        "#,
        r#"
        for _, cid in ipairs(_test_gs.colony_ids) do
            _test_gs.colonies[cid].production.minerals_per_hexadies = 0
        end
        "#,
        r#"
        for _, sid in ipairs(_test_gs.ship_ids) do
            _test_gs.ships[sid].hp.hull = 0
        end
        "#,
        r#"
        for _, sid in ipairs(_test_gs.ship_ids) do
            _test_gs.ships[sid].state.kind = "destroyed"
        end
        "#,
        r#"
        for _, pid in ipairs(_test_gs.planet_ids) do
            _test_gs.planets[pid].biome = "molten"
        end
        "#,
    ];
    for script in scripts {
        let r: mlua::Result<()> = engine.lua().load(*script).exec();
        assert!(r.is_err(), "expected read-only error running: {}", script);
        let msg = r.err().unwrap().to_string();
        assert!(
            msg.contains("read-only"),
            "expected 'read-only' in error: {msg}"
        );
    }
}

#[test]
fn test_empire_tech_alias_matches_techs() {
    // #289 β: `empire.tech[id]` is an alias for `empire.techs[id]`.
    let engine = ScriptEngine::new().unwrap();
    let mut world = scenario_world();
    let gs = build_gamestate_table(engine.lua(), &mut world).unwrap();
    engine.lua().globals().set("_test_gs", gs).unwrap();
    let both_true: bool = engine
        .lua()
        .load(
            r#"
            local e = _test_gs.player_empire
            return e.tech.industrial_mining == true
               and e.techs.industrial_mining == true
            "#,
        )
        .eval()
        .unwrap();
    assert!(both_true);
}
