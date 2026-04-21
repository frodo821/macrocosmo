//! #320 regression: LuaJIT aux-stack leak under sustained per-tick Lua
//! scheduling.
//!
//! Before the fix, `evaluate_fire_conditions` and `dispatch_event_handlers`
//! each built fresh gamestate tables every tick without ever triggering a
//! Lua GC. The `ValueRef` slots those tables held on LuaJIT's auxiliary
//! thread (`LUAI_MAXCSTACK`, ~8000) accumulated until the aux thread was
//! exhausted and further Lua callbacks panicked — typically after ~80
//! ticks with a gamestate snapshot that touches ~100 refs per build.
//!
//! This test stresses both paths simultaneously for 1000 ticks. Before the
//! fix it panics within the first couple of hundred iterations; with the
//! fix it finishes cleanly and Lua's reported memory footprint stays
//! bounded.

use bevy::prelude::*;
use macrocosmo::condition::ScopedFlags;
use macrocosmo::event_system::{
    EventDefinition, EventSystem, EventTrigger, FiredEvent, LuaFunctionRef,
};
use macrocosmo::player::{Empire, PlayerEmpire};
use macrocosmo::scripting::ScriptEngine;
use macrocosmo::scripting::lifecycle::{dispatch_event_handlers, evaluate_fire_conditions};
use macrocosmo::technology::{GameFlags, TechId, TechTree};
use macrocosmo::time_system::GameClock;

const STRESS_TICKS: i64 = 1000;

/// Upper bound on Lua memory growth across the whole stress run.
///
/// LuaJIT accounts its own heap separately from process RSS; 32 MiB is a
/// conservative ceiling well above the ~1 MiB baseline observed on a
/// healthy build but far below what an unbounded aux-stack leak would
/// reach after 1000 ticks with ~100 refs/tick worth of snapshot tables.
const LUA_MEMORY_CEILING_BYTES: usize = 32 * 1024 * 1024;

fn build_stress_world() -> World {
    let mut world = World::new();
    world.insert_resource(GameClock::new(0));
    world.insert_resource(EventSystem::default());
    world.insert_resource(ScriptEngine::new().expect("ScriptEngine init"));

    let mut tree = TechTree::default();
    tree.researched.insert(TechId("tech_a".to_string()));
    let mut flags = GameFlags::default();
    flags.set("fa");
    world.spawn((
        Empire {
            name: "StressEmpire".into(),
        },
        PlayerEmpire,
        tree,
        flags,
        ScopedFlags::default(),
    ));

    world
}

/// Seed a periodic event with a Lua fire_condition and a bus handler that
/// both observe `evt.gamestate`. Both paths therefore have to build a
/// snapshot table whenever the event is due/fires.
fn seed_events(world: &mut World) {
    let fref = {
        let engine = world.resource::<ScriptEngine>();
        let lua = engine.lua();
        // `on` handler for the fired event: reads gamestate to force a
        // snapshot build inside dispatch_event_handlers.
        lua.load(
            r#"
            _hits = 0
            on("stress_periodic", function(evt)
                _hits = _hits + 1
                -- Touch gamestate so the snapshot is actually used.
                local _ = evt.gamestate.clock.now
                local _ = evt.gamestate.player_empire.name
            end)
            "#,
        )
        .exec()
        .expect("install bus handler");

        // fire_condition: always returns true, but walks gamestate fields
        // so evaluate_fire_conditions has to materialise the snapshot.
        let f: mlua::Function = lua
            .load(
                r#"return function(evt)
                    local _ = evt.gamestate.player_empire.techs.tech_a
                    return true
                end"#,
            )
            .eval()
            .expect("compile fire_condition");
        LuaFunctionRef::from_function(lua, f).expect("wrap fire_condition")
    };

    let mut es = world.resource_mut::<EventSystem>();
    es.register(EventDefinition {
        id: "stress_periodic".to_string(),
        name: "Stress Periodic".to_string(),
        description: "Periodic event with Lua fire_condition, fires every tick.".to_string(),
        trigger: EventTrigger::Periodic {
            interval_hexadies: 1,
            last_fired: 0,
            fire_condition: Some(fref),
            max_times: None,
            times_triggered: 0,
        },
    });
}

/// Simulate a single tick: advance clock, inject a fired event (to drive
/// dispatch_event_handlers), then run the two Lua-heavy systems in the
/// same order as the main schedule.
fn run_tick(world: &mut World, now: i64) {
    world.resource_mut::<GameClock>().elapsed = now;

    // Drive the dispatcher every tick by pushing one FiredEvent per tick.
    // In production tick_events would do this; we skip the rest of the
    // event_system pipeline to keep the test focused on the Lua leak.
    world
        .resource_mut::<EventSystem>()
        .fired_log
        .push(FiredEvent {
            event_id: "stress_periodic".to_string(),
            target: None,
            fired_at: now,
            payload: None,
        });

    evaluate_fire_conditions(world);
    dispatch_event_handlers(world);
}

#[test]
fn stress_lua_scheduling_does_not_exhaust_aux_stack() {
    let mut world = build_stress_world();
    seed_events(&mut world);

    // Baseline Lua footprint *after* snapshot plumbing is installed.
    let baseline_memory = world.resource::<ScriptEngine>().lua().used_memory();

    for tick in 1..=STRESS_TICKS {
        run_tick(&mut world, tick);
    }

    // Hit counter confirms the bus handler actually ran every tick — a
    // silent early-exit would make the test useless as a regression.
    let engine = world.resource::<ScriptEngine>();
    let hits: i64 = engine
        .lua()
        .globals()
        .get("_hits")
        .expect("_hits global readable");
    assert_eq!(
        hits, STRESS_TICKS,
        "dispatcher must invoke the bus handler once per tick"
    );

    let final_memory = engine.lua().used_memory();
    eprintln!(
        "stress_lua_scheduling: baseline={} bytes, final={} bytes (ceiling {})",
        baseline_memory, final_memory, LUA_MEMORY_CEILING_BYTES
    );
    assert!(
        final_memory < LUA_MEMORY_CEILING_BYTES,
        "Lua heap grew beyond {} bytes (baseline {}, final {}); \
         #320 aux-stack leak likely regressed",
        LUA_MEMORY_CEILING_BYTES,
        baseline_memory,
        final_memory,
    );
}
