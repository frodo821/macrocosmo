//! #289 / #332: Lua View types — integration tests.
//!
//! These cover end-to-end navigation across the `event.gamestate`
//! accessor as it appears from Lua, exercising the hierarchical
//! `system -> planets/colonies -> buildings/production` chain, the
//! Ship.state tag-union, and the Fleet-through-flagship proxy.
//!
//! #332 migration: these tests previously exercised the snapshot-based
//! `build_gamestate_table` directly. They now go through
//! `dispatch_with_gamestate` to validate the same navigation under the
//! Option B scope-closure path (method form: `gs:system(id)` instead
//! of `gs.systems[id]`; `gs:list_systems()` instead of `gs.system_ids`).

use bevy::prelude::*;
use macrocosmo::amount::Amt;
use macrocosmo::colony::{Buildings, Colony, Production, ResourceStockpile};
use macrocosmo::components::Position;
use macrocosmo::condition::ScopedFlags;
use macrocosmo::galaxy::{Biome, Planet, Sovereignty, StarSystem, SystemModifiers};
use macrocosmo::modifier::ModifiedValue;
use macrocosmo::player::{Empire, PlayerEmpire};
use macrocosmo::scripting::ScriptEngine;
use macrocosmo::scripting::building_api::BuildingId;
use macrocosmo::scripting::gamestate_scope::{GamestateMode, dispatch_with_gamestate};
use macrocosmo::ship::fleet::{Fleet, FleetMembers};
use macrocosmo::ship::{EquippedModule, Owner, Ship, ShipHitpoints, ShipState};
use macrocosmo::technology::{EmpireModifiers, GameFlags, TechId, TechTree};
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
    world.insert_resource(ScriptEngine::new().unwrap());

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
            EmpireModifiers::default(),
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

    // #335: Planet entities now carry a separate `Biome` component. The
    // PlanetView.biome surface reads this component (no longer aliased to
    // planet_type). Tests spawn both components explicitly here.
    let planet_earth = world
        .spawn((
            Planet {
                name: "Earth".into(),
                system,
                planet_type: "terrestrial".into(),
            },
            Biome::new("temperate"),
        ))
        .id();
    let _planet_mars = world
        .spawn((
            Planet {
                name: "Mars".into(),
                system,
                planet_type: "barren".into(),
            },
            Biome::new("arid"),
        ))
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

/// Helper: run a Lua chunk with `_evt` populated by `dispatch_with_gamestate`.
/// The chunk can reference `evt = _evt` or use `_evt` directly.
fn with_gamestate<R, F>(world: &mut World, mode: GamestateMode, f: F) -> R
where
    F: FnOnce(&mlua::Lua) -> R,
    R: Default,
{
    let out = std::cell::RefCell::new(R::default());
    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        let lua = engine.lua();
        let payload = lua.create_table().unwrap();
        dispatch_with_gamestate(lua, world, &payload, mode, |lua_inner, p| {
            lua_inner.globals().set("_evt", p.clone())?;
            *out.borrow_mut() = f(lua_inner);
            Ok(())
        })
        .unwrap();
    });
    out.into_inner()
}

#[test]
fn test_gamestate_view_hierarchical_navigation() {
    // Navigation chain: list_systems -> system -> planets/colonies
    // -> buildings + production -> ship -> state.kind. Under #332
    // Option B, all nested lookups are method calls on the gamestate
    // table rather than indexing into nested maps.
    let mut world = scenario_world();
    let summary: String = with_gamestate(&mut world, GamestateMode::ReadOnly, |lua| {
        lua.load(
            r#"
            local gs = _evt.gamestate
            local parts = {}
            for _, sid in ipairs(gs:list_systems()) do
                local sys = gs:system(sid)
                table.insert(parts, sys.name)
                for _, pid in ipairs(gs:list_planets(sid)) do
                    local p = gs:planet(pid)
                    table.insert(parts, p.name .. ":" .. p.biome)
                end
                for _, cid in ipairs(gs:list_colonies(sid)) do
                    local c = gs:colony(cid)
                    table.insert(parts, "pop=" .. tostring(c.population))
                    table.insert(parts, "bld1=" .. c.building_ids[1])
                    table.insert(parts, "m/hx=" .. tostring(c.production.minerals_per_hexadies))
                end
            end
            for _, sid in ipairs(gs:list_ships()) do
                local s = gs:ship(sid)
                table.insert(parts, "state=" .. s.state.kind)
                table.insert(parts, "hp=" .. tostring(s.hp.hull))
                table.insert(parts, "mod1=" .. s.modules[1].module_id)
            end
            for _, fid in ipairs(gs:list_fleets()) do
                local f = gs:fleet(fid)
                table.insert(parts, "fleet_state=" .. f.state.kind)
                table.insert(parts, "fleet_owner_kind=" .. f.owner_kind)
            end
            return table.concat(parts, "|")
            "#,
        )
        .eval::<String>()
        .unwrap()
    });
    assert!(summary.contains("Sol"), "Sol missing: {summary}");
    // #335: PlanetView.biome is now the real Biome component id, not the
    // planet_type placeholder. Earth's Biome is "temperate", Mars's is "arid".
    assert!(
        summary.contains("Earth:temperate"),
        "Earth biome: {summary}"
    );
    assert!(summary.contains("Mars:arid"), "Mars biome: {summary}");
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
fn test_ship_state_tag_union_docked() {
    // Ship state is a `{kind = ..., ...}` tag-union table exposed as
    // `ship.state` via `gs:ship(id)`. Docked ship should have
    // `kind="docked"` and `system=<u64>`.
    let mut world = scenario_world();
    let (kind, has_system): (String, bool) =
        with_gamestate(&mut world, GamestateMode::ReadOnly, |lua| {
            lua.load(
                r#"
            local gs = _evt.gamestate
            local ship_ids = gs:list_ships()
            assert(#ship_ids > 0)
            local s = gs:ship(ship_ids[1])
            return s.state.kind, (s.state.system ~= nil)
            "#,
            )
            .eval::<(String, bool)>()
            .unwrap()
        });
    assert_eq!(kind, "docked");
    assert!(has_system);
}

#[test]
fn test_fleet_proxy_through_flagship() {
    // Fleet has no Owner or State of its own pre γ-2; owner_kind /
    // state proxy through the flagship ship.
    let mut world = scenario_world();
    let (owner_kind, state_kind): (String, String) =
        with_gamestate(&mut world, GamestateMode::ReadOnly, |lua| {
            lua.load(
                r#"
            local gs = _evt.gamestate
            local fids = gs:list_fleets()
            assert(#fids > 0)
            local f = gs:fleet(fids[1])
            return f.owner_kind, f.state.kind
            "#,
            )
            .eval::<(String, String)>()
            .unwrap()
        });
    assert_eq!(owner_kind, "empire");
    assert_eq!(state_kind, "docked");
}

#[test]
fn test_empire_tech_alias_matches_techs() {
    // `empire.tech[id]` is an alias for `empire.techs[id]`.
    let mut world = scenario_world();
    let both_true: bool = with_gamestate(&mut world, GamestateMode::ReadOnly, |lua| {
        lua.load(
            r#"
            local e = _evt.gamestate:player_empire()
            return e.tech.industrial_mining == true
               and e.techs.industrial_mining == true
            "#,
        )
        .eval::<bool>()
        .unwrap()
    });
    assert!(both_true);
}
