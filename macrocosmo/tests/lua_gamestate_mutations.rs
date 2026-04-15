//! #332: integration tests for Option B live-within-tick mutation
//! through `evt.gamestate:push_*_modifier(...)` and related setters.
//!
//! These complement the per-module unit tests in
//! `src/scripting/gamestate_scope.rs` by exercising the full dispatch
//! path (scope setup -> Lua callback -> setter -> World mutation) with
//! a realistic event handler installed on `_event_handlers`.

use bevy::prelude::*;
use macrocosmo::amount::Amt;
use macrocosmo::colony::{Colony, Production, ResourceStockpile};
use macrocosmo::components::Position;
use macrocosmo::condition::ScopedFlags;
use macrocosmo::event_system::{EventSystem, FiredEvent};
use macrocosmo::galaxy::{StarSystem, SystemModifiers};
use macrocosmo::modifier::ModifiedValue;
use macrocosmo::player::{Empire, PlayerEmpire};
use macrocosmo::scripting::gamestate_scope::{dispatch_with_gamestate, GamestateMode};
use macrocosmo::scripting::lifecycle::dispatch_event_handlers;
use macrocosmo::scripting::ScriptEngine;
use macrocosmo::technology::{EmpireModifiers, GameFlags, TechTree};
use macrocosmo::time_system::GameClock;

fn make_mutation_world() -> World {
    let mut world = World::new();
    world.insert_resource(GameClock::new(10));
    world.insert_resource(EventSystem::default());
    world.insert_resource(ScriptEngine::new().unwrap());
    // Player empire with tech + flag + EmpireModifiers.
    let mut tree = TechTree::default();
    tree.researched
        .insert(macrocosmo::technology::TechId("tech_a".into()));
    let flags = GameFlags::default();
    world.spawn((
        Empire {
            name: "Terran".into(),
        },
        PlayerEmpire,
        tree,
        flags,
        ScopedFlags::default(),
        EmpireModifiers::default(),
    ));

    // One star system with SystemModifiers + ResourceStockpile.
    let system = world
        .spawn((
            StarSystem {
                name: "Sol".into(),
                surveyed: true,
                is_capital: true,
                star_type: "yellow".into(),
            },
            Position {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            SystemModifiers::default(),
            ResourceStockpile {
                minerals: Amt::units(100),
                energy: Amt::units(50),
                research: Amt::ZERO,
                food: Amt::units(25),
                authority: Amt::ZERO,
            },
        ))
        .id();

    // One planet + colony with Production.
    let planet = world
        .spawn(macrocosmo::galaxy::Planet {
            name: "Earth".into(),
            system,
            planet_type: "terrestrial".into(),
        })
        .id();
    world.spawn((
        Colony {
            planet,
            population: 50.0,
            growth_rate: 0.01,
        },
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::units(10)),
            energy_per_hexadies: ModifiedValue::new(Amt::units(5)),
            research_per_hexadies: ModifiedValue::new(Amt::units(2)),
            food_per_hexadies: ModifiedValue::new(Amt::units(8)),
        },
    ));
    world
}

fn register_handler(world: &World, event_id: &str, body: &str) {
    let engine = world.resource::<ScriptEngine>();
    let script = format!(
        r#"
        on("{event_id}", function(evt)
{body}
        end)
        "#
    );
    engine.lua().load(&script).exec().unwrap();
}

fn fire_event(world: &mut World, event_id: &str) {
    let mut es = world.resource_mut::<EventSystem>();
    es.fired_log.push(FiredEvent {
        event_id: event_id.to_string(),
        target: None,
        fired_at: 10,
        payload: None,
    });
}

#[test]
fn test_push_empire_modifier_applies_live() {
    let mut world = make_mutation_world();
    let pe_id = {
        let mut q = world.query_filtered::<Entity, With<PlayerEmpire>>();
        q.iter(&world).next().unwrap().to_bits()
    };
    world
        .resource::<ScriptEngine>()
        .lua()
        .globals()
        .set("_pe_id", pe_id)
        .unwrap();
    register_handler(
        &world,
        "macrocosmo:empire_mod_test",
        r#"
            evt.gamestate:push_empire_modifier(
                _pe_id,
                "empire.population_growth",
                { id = "integration_live", add = 2.0 }
            )
        "#,
    );
    fire_event(&mut world, "macrocosmo:empire_mod_test");
    dispatch_event_handlers(&mut world);

    let empire = Entity::from_bits(pe_id);
    let em = world.get::<EmpireModifiers>(empire).unwrap();
    assert!(
        em.population_growth.final_value().to_f64() >= 2.0,
        "modifier should have been applied live"
    );
}

#[test]
fn test_push_system_modifier_live() {
    let mut world = make_mutation_world();
    let sys_id = {
        let mut q = world.query_filtered::<Entity, With<StarSystem>>();
        q.iter(&world).next().unwrap().to_bits()
    };
    world
        .resource::<ScriptEngine>()
        .lua()
        .globals()
        .set("_sys_id", sys_id)
        .unwrap();
    register_handler(
        &world,
        "macrocosmo:system_mod_test",
        r#"
            evt.gamestate:push_system_modifier(
                _sys_id,
                "ship.speed",
                { id = "solar_storm", multiplier = -0.5 }
            )
        "#,
    );
    fire_event(&mut world, "macrocosmo:system_mod_test");
    dispatch_event_handlers(&mut world);

    let sys_entity = Entity::from_bits(sys_id);
    let sm = world.get::<SystemModifiers>(sys_entity).unwrap();
    // Base 0.0 with -0.5 multiplier yields 0 still, but the modifier
    // should be in the modifier list. Check count via ScopedModifiers
    // value's modifier count isn't directly exposed; we assert the
    // generation counter changed.
    assert!(
        sm.ship_speed.generation() > 0,
        "system modifier generation should have advanced"
    );
}

#[test]
fn test_push_colony_modifier_live() {
    let mut world = make_mutation_world();
    let colony_id = {
        let mut q = world.query_filtered::<Entity, With<Colony>>();
        q.iter(&world).next().unwrap().to_bits()
    };
    world
        .resource::<ScriptEngine>()
        .lua()
        .globals()
        .set("_colony_id", colony_id)
        .unwrap();
    register_handler(
        &world,
        "macrocosmo:colony_mod_test",
        r#"
            evt.gamestate:push_colony_modifier(
                _colony_id,
                "production.minerals",
                { id = "bounty", add = 5.0 }
            )
        "#,
    );
    fire_event(&mut world, "macrocosmo:colony_mod_test");
    dispatch_event_handlers(&mut world);

    let colony = Entity::from_bits(colony_id);
    let prod = world.get::<Production>(colony).unwrap();
    // Base 10 + add 5 = final 15.
    assert!(
        prod.minerals_per_hexadies.final_value().to_f64() >= 15.0,
        "colony minerals production should include the new modifier"
    );
}

#[test]
fn test_set_flag_live_within_handler() {
    let mut world = make_mutation_world();
    let pe_id = {
        let mut q = world.query_filtered::<Entity, With<PlayerEmpire>>();
        q.iter(&world).next().unwrap().to_bits()
    };
    world
        .resource::<ScriptEngine>()
        .lua()
        .globals()
        .set("_pe_id", pe_id)
        .unwrap();
    register_handler(
        &world,
        "macrocosmo:flag_test",
        r#"
            evt.gamestate:set_flag("empire", _pe_id, "contact_established", true)
        "#,
    );
    fire_event(&mut world, "macrocosmo:flag_test");
    dispatch_event_handlers(&mut world);

    let empire = Entity::from_bits(pe_id);
    let gf = world.get::<GameFlags>(empire).unwrap();
    assert!(gf.check("contact_established"));
}

#[test]
fn test_mutation_observable_via_subsequent_read() {
    // The canonical live-within-tick promise: a setter inside one
    // callback + a read of the same state later in the same callback
    // reflects the mutation.
    let mut world = make_mutation_world();
    let pe_id = {
        let mut q = world.query_filtered::<Entity, With<PlayerEmpire>>();
        q.iter(&world).next().unwrap().to_bits()
    };
    world
        .resource::<ScriptEngine>()
        .lua()
        .globals()
        .set("_pe_id", pe_id)
        .unwrap();
    // Handler: set a flag, then read flags to confirm presence.
    register_handler(
        &world,
        "macrocosmo:live_test",
        r#"
            _before = evt.gamestate:empire(_pe_id).flags.live_now == true
            evt.gamestate:set_flag("empire", _pe_id, "live_now", true)
            _after = evt.gamestate:empire(_pe_id).flags.live_now == true
        "#,
    );
    fire_event(&mut world, "macrocosmo:live_test");
    dispatch_event_handlers(&mut world);

    let engine = world.resource::<ScriptEngine>();
    let before: bool = engine.lua().globals().get("_before").unwrap();
    let after: bool = engine.lua().globals().get("_after").unwrap();
    assert!(!before, "flag must be absent before setter");
    assert!(after, "flag must be observable via live read after setter");
}

#[test]
fn test_unknown_target_returns_runtime_error() {
    let mut world = make_mutation_world();
    let pe_id = {
        let mut q = world.query_filtered::<Entity, With<PlayerEmpire>>();
        q.iter(&world).next().unwrap().to_bits()
    };
    world
        .resource::<ScriptEngine>()
        .lua()
        .globals()
        .set("_pe_id", pe_id)
        .unwrap();
    register_handler(
        &world,
        "macrocosmo:bad_target",
        r#"
            local ok, err = pcall(function()
                evt.gamestate:push_empire_modifier(
                    _pe_id,
                    "empire.not_a_real_target",
                    { id = "x", add = 1.0 }
                )
            end)
            _mut_ok = ok
            _mut_err = err and tostring(err) or nil
        "#,
    );
    fire_event(&mut world, "macrocosmo:bad_target");
    dispatch_event_handlers(&mut world);

    let engine = world.resource::<ScriptEngine>();
    let ok: bool = engine.lua().globals().get("_mut_ok").unwrap();
    let err: Option<String> = engine.lua().globals().get("_mut_err").ok();
    assert!(!ok, "unknown target must raise a Lua error");
    let err_msg = err.unwrap_or_default();
    assert!(
        err_msg.contains("unknown target"),
        "error must mention 'unknown target', got: {err_msg}"
    );
}

#[test]
fn test_fire_event_stays_queued() {
    // Event callbacks can re-fire events — they must go through the
    // `_pending_script_events` queue, not sync dispatch.
    let mut world = make_mutation_world();
    register_handler(
        &world,
        "macrocosmo:chain_outer",
        r#"
            fire_event("macrocosmo:chain_inner")
        "#,
    );
    register_handler(
        &world,
        "macrocosmo:chain_inner",
        r#"
            _inner_fired = true
        "#,
    );
    fire_event(&mut world, "macrocosmo:chain_outer");
    dispatch_event_handlers(&mut world);

    // Immediately after dispatch, the inner event should NOT have
    // fired yet — it should be sitting in `_pending_script_events`.
    let engine = world.resource::<ScriptEngine>();
    let inner_fired: bool = engine
        .lua()
        .globals()
        .get::<Option<bool>>("_inner_fired")
        .unwrap()
        .unwrap_or(false);
    assert!(
        !inner_fired,
        "fire_event must queue, not sync-dispatch"
    );
    // Confirm it IS in the queue.
    let pending: mlua::Table = engine
        .lua()
        .globals()
        .get("_pending_script_events")
        .unwrap();
    assert_eq!(pending.len().unwrap(), 1);
}

#[test]
fn test_readonly_mode_rejects_setters() {
    // ReadOnly mode (used by fire_condition) must not expose setters;
    // calling one surfaces a Lua 'attempt to call a nil value' error.
    let mut world = make_mutation_world();
    let pe_id = {
        let mut q = world.query_filtered::<Entity, With<PlayerEmpire>>();
        q.iter(&world).next().unwrap().to_bits()
    };
    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        let lua = engine.lua();
        lua.globals().set("_pe_id", pe_id).unwrap();
        let payload = lua.create_table().unwrap();
        let result = dispatch_with_gamestate(
            lua,
            world,
            &payload,
            GamestateMode::ReadOnly,
            |lua_inner, p| {
                lua_inner.globals().set("_evt", p.clone())?;
                let r: mlua::Result<()> = lua_inner
                    .load(
                        r#"
                        evt = _evt
                        evt.gamestate:set_flag("empire", _pe_id, "x", true)
                        "#,
                    )
                    .exec();
                assert!(
                    r.is_err(),
                    "ReadOnly mode must not expose set_flag"
                );
                Ok(())
            },
        );
        assert!(result.is_ok());
    });
}
