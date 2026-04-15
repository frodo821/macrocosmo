//! #332 Phase B: integration tests for lifecycle hook live gamestate
//! mutation.
//!
//! Exercises the new `run_on_game_start_with_gamestate` entry point
//! (promoted to an exclusive `&mut World` system in Phase B2) and
//! verifies that `on_game_start` callbacks can mutate the world
//! directly via `gs:set_flag` / `gs:push_empire_modifier` — the same
//! `ReadWrite` setter surface that event callbacks already use.

use bevy::prelude::*;
use macrocosmo::condition::ScopedFlags;
use macrocosmo::player::{Empire, PlayerEmpire};
use macrocosmo::scripting::ScriptEngine;
use macrocosmo::scripting::lifecycle::{
    run_on_game_load_with_gamestate, run_on_game_start_with_gamestate,
};
use macrocosmo::technology::{EmpireModifiers, GameFlags};
use macrocosmo::time_system::GameClock;

fn make_lifecycle_world() -> World {
    let mut world = World::new();
    world.insert_resource(GameClock::new(5));
    world.insert_resource(ScriptEngine::new().unwrap());
    world.spawn((
        Empire {
            name: "Terran".into(),
        },
        PlayerEmpire,
        GameFlags::default(),
        ScopedFlags::default(),
        EmpireModifiers::default(),
    ));
    world
}

/// `on_game_start` handler can call `gs:set_flag("empire", id, name)`
/// and the mutation lands on `GameFlags` / `ScopedFlags` of the
/// `PlayerEmpire` entity by the time the hook returns.
#[test]
fn test_on_game_start_handler_sets_flag_live() {
    let mut world = make_lifecycle_world();

    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        let lua = engine.lua();
        lua.load(
            r#"
            _on_game_start_handlers = _on_game_start_handlers or {}
            table.insert(_on_game_start_handlers, function(evt)
                local emp = evt.gamestate:player_empire()
                evt.gamestate:set_flag("empire", emp.id, "lifecycle_applied", true)
            end)
            "#,
        )
        .exec()
        .unwrap();
        run_on_game_start_with_gamestate(lua, world).unwrap();
    });

    let mut q = world.query_filtered::<&GameFlags, With<PlayerEmpire>>();
    let game_flags = q.single(&world).unwrap();
    assert!(
        game_flags.check("lifecycle_applied"),
        "flag set via gs:set_flag should be live on the empire entity"
    );
    let mut q2 = world.query_filtered::<&ScopedFlags, With<PlayerEmpire>>();
    let scoped = q2.single(&world).unwrap();
    assert!(
        scoped.check("lifecycle_applied"),
        "scoped flag should also be live"
    );
}

/// `on_game_start` handler can push an empire modifier; the value is
/// reflected on the `EmpireModifiers` component immediately.
#[test]
fn test_on_game_start_handler_pushes_empire_modifier_live() {
    let mut world = make_lifecycle_world();

    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        let lua = engine.lua();
        lua.load(
            r#"
            _on_game_start_handlers = _on_game_start_handlers or {}
            table.insert(_on_game_start_handlers, function(evt)
                local emp = evt.gamestate:player_empire()
                evt.gamestate:push_empire_modifier(
                    emp.id,
                    "empire.population_growth",
                    { add = 0.25, description = "Lifecycle-seeded" }
                )
            end)
            "#,
        )
        .exec()
        .unwrap();
        run_on_game_start_with_gamestate(lua, world).unwrap();
    });

    let mut q = world.query_filtered::<&EmpireModifiers, With<PlayerEmpire>>();
    let em = q.single(&world).unwrap();
    // The value slot accumulated the add modifier.
    let final_value = em.population_growth.final_value();
    assert!(
        final_value > macrocosmo::amount::Amt::ZERO,
        "population_growth modifier should be live post-hook (got {final_value:?})"
    );
}

/// Handlers run in registration order; mutations from handler N are
/// visible to handler N+1 because each invocation re-enters
/// `dispatch_with_gamestate` with a fresh borrow and a fresh view.
#[test]
fn test_on_game_start_handlers_share_mutations_across_handlers() {
    let mut world = make_lifecycle_world();

    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        let lua = engine.lua();
        lua.load(
            r#"
            _on_game_start_handlers = _on_game_start_handlers or {}
            table.insert(_on_game_start_handlers, function(evt)
                local emp = evt.gamestate:player_empire()
                evt.gamestate:set_flag("empire", emp.id, "first_handler", true)
            end)
            table.insert(_on_game_start_handlers, function(evt)
                local emp = evt.gamestate:player_empire()
                -- Read reflects the first handler's mutation.
                assert(emp.flags["first_handler"] == true, "first handler's flag should be visible")
                evt.gamestate:set_flag("empire", emp.id, "second_handler", true)
            end)
            "#,
        )
        .exec()
        .unwrap();
        run_on_game_start_with_gamestate(lua, world).unwrap();
    });

    let mut q = world.query_filtered::<&GameFlags, With<PlayerEmpire>>();
    let game_flags = q.single(&world).unwrap();
    assert!(game_flags.check("first_handler"));
    assert!(game_flags.check("second_handler"));
}

/// `on_game_load` path shares the same plumbing; smoke-test that the
/// same setter surface is available.
#[test]
fn test_on_game_load_handler_mutates_live() {
    let mut world = make_lifecycle_world();

    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        let lua = engine.lua();
        lua.load(
            r#"
            _on_game_load_handlers = _on_game_load_handlers or {}
            table.insert(_on_game_load_handlers, function(evt)
                local emp = evt.gamestate:player_empire()
                evt.gamestate:set_flag("empire", emp.id, "loaded", true)
            end)
            "#,
        )
        .exec()
        .unwrap();
        run_on_game_load_with_gamestate(lua, world).unwrap();
    });

    let mut q = world.query_filtered::<&GameFlags, With<PlayerEmpire>>();
    let game_flags = q.single(&world).unwrap();
    assert!(game_flags.check("loaded"));
}

/// Errors from a handler are propagated up — downstream handlers
/// registered after it never run.
#[test]
fn test_on_game_start_handler_error_propagates() {
    let mut world = make_lifecycle_world();

    let result = world.resource_scope::<ScriptEngine, _>(|world, engine| {
        let lua = engine.lua();
        lua.load(
            r#"
            _on_game_start_handlers = _on_game_start_handlers or {}
            table.insert(_on_game_start_handlers, function(evt)
                error("boom from handler")
            end)
            table.insert(_on_game_start_handlers, function(evt)
                local emp = evt.gamestate:player_empire()
                evt.gamestate:set_flag("empire", emp.id, "should_not_run", true)
            end)
            "#,
        )
        .exec()
        .unwrap();
        run_on_game_start_with_gamestate(lua, world)
    });
    assert!(result.is_err(), "handler error should surface to caller");

    let mut q = world.query_filtered::<&GameFlags, With<PlayerEmpire>>();
    let game_flags = q.single(&world).unwrap();
    assert!(
        !game_flags.check("should_not_run"),
        "handler registered after the failing one must not run"
    );
}
