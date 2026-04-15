//! #334 Phase 4 Commit 2: end-to-end test for
//! `ctx.gamestate:request_command(kind, args)` + the Phase 4 Commit 1
//! `bridge_command_executed_to_gamestate` → `on("macrocosmo:command_completed")`
//! hook.
//!
//! The test exercises the full Lua-initiated command round-trip:
//! 1. A Lua event handler calls `evt.gamestate:request_command("move", {...})`
//!    which pushes a `MoveRequested` message via the scope-closure setter
//!    (`apply::request_command`).
//! 2. A minimal Bevy schedule runs the MoveRequested handler surrogate
//!    that writes `CommandExecuted { result: Ok }`.
//! 3. `bridge_command_executed_to_gamestate` forwards the terminal
//!    `CommandExecuted` to `EventSystem::fire_event_with_payload`.
//! 4. `dispatch_event_handlers` delivers the payload to a second Lua
//!    handler registered on `macrocosmo:command_completed`, which
//!    records the observed fields in a Lua-visible global.
//!
//! The surrogate handler (`run_move_requested_surrogate`) mimics the
//! Phase 1 handler's terminal emit without pulling in the heavyweight
//! routing-dependent `handle_move_requested` system — this test is about
//! the **bridge + Lua API + hook** path, not the FTL route planner.

use bevy::ecs::message::Messages;
use bevy::prelude::*;
use macrocosmo::event_system::{EventSystem, FiredEvent};
use macrocosmo::scripting::ScriptEngine;
use macrocosmo::scripting::gamestate_scope::{GamestateMode, dispatch_with_gamestate};
use macrocosmo::scripting::lifecycle::dispatch_event_handlers;
use macrocosmo::ship::bridges::bridge_command_executed_to_gamestate;
use macrocosmo::ship::command_events::{
    CommandEventsPlugin, CommandExecuted, CommandId, CommandKind, CommandResult, MoveRequested,
    NextCommandId,
};
use macrocosmo::time_system::GameClock;

fn make_world() -> World {
    let mut app = App::new();
    app.add_plugins(bevy::MinimalPlugins);
    app.add_plugins(CommandEventsPlugin);
    app.insert_resource(EventSystem::default());
    app.insert_resource(ScriptEngine::new().unwrap());
    app.insert_resource(GameClock::new(100));
    std::mem::take(app.world_mut())
}

/// Stand-in for the real move handler. Drains every `MoveRequested` and
/// writes a terminal `CommandExecuted { Ok }`. This keeps the test
/// focused on the bridge + hook, not on FTL routing logic.
fn run_move_requested_surrogate(world: &mut World, clock: i64) {
    let requests: Vec<MoveRequested> = {
        let mut msgs = world.resource_mut::<Messages<MoveRequested>>();
        let mut cursor = msgs.get_cursor();
        let collected: Vec<MoveRequested> = cursor.read(&msgs).cloned().collect();
        // Advance the cursor — the real handler would read via MessageReader.
        drop(cursor);
        // Clear the queue so it doesn't leak into the next iteration.
        msgs.update();
        collected
    };
    if requests.is_empty() {
        return;
    }
    let mut exec = world.resource_mut::<Messages<CommandExecuted>>();
    for req in requests {
        exec.write(CommandExecuted {
            command_id: req.command_id,
            kind: CommandKind::Move,
            ship: req.ship,
            result: CommandResult::Ok,
            completed_at: clock,
        });
    }
}

fn run_bridge(world: &mut World) {
    // `bridge_command_executed_to_gamestate` is a Bevy system — call it
    // by running a one-shot Schedule that contains only it, so we can
    // exercise the exact system logic.
    let mut schedule = Schedule::default();
    schedule.add_systems(bridge_command_executed_to_gamestate);
    schedule.run(world);
}

/// End-to-end: Lua handler `fires → request_command` → surrogate handler
/// emits CommandExecuted(Ok) → bridge enqueues command_completed →
/// dispatch_event_handlers delivers to the on(...) callback.
#[test]
fn request_command_move_triggers_command_completed_hook() {
    let mut world = make_world();

    // Spawn two dummy entities — a ship and a target system. We never
    // actually move them; the surrogate handler short-circuits the
    // request. They just need valid Entity bits for the Lua payload.
    let ship = world.spawn_empty().id();
    let target = world.spawn_empty().id();

    // Register Lua-side state + handlers. Two handlers:
    //   1. `macrocosmo:lua_kickoff` — the caller of `request_command`.
    //   2. `macrocosmo:command_completed` — the hook-side observer.
    {
        let lua = world.resource::<ScriptEngine>().lua();
        lua.globals().set("_ship", ship.to_bits()).unwrap();
        lua.globals().set("_target", target.to_bits()).unwrap();
        lua.load(
            r#"
            -- initialise observer globals
            _observed_kind = nil
            _observed_result = nil
            _observed_cmd_id = nil
            _returned_cmd_id = nil

            -- kickoff handler: calls request_command and stores the returned id
            on("macrocosmo:lua_kickoff", function(evt)
                local id = evt.gamestate:request_command("move", {
                    ship = _ship,
                    target = _target,
                })
                _returned_cmd_id = id
            end)

            -- hook handler: records the fields it observed
            on("macrocosmo:command_completed", function(evt)
                _observed_kind = evt.kind
                _observed_result = evt.result
                _observed_cmd_id = evt.command_id
            end)
            "#,
        )
        .exec()
        .unwrap();
    }

    // --- Tick 1: fire the kickoff event; Lua handler calls request_command. ---
    {
        let mut es = world.resource_mut::<EventSystem>();
        es.fired_log.push(FiredEvent {
            event_id: "macrocosmo:lua_kickoff".into(),
            target: None,
            fired_at: 100,
            payload: None,
        });
    }
    dispatch_event_handlers(&mut world);

    // request_command should have emitted a MoveRequested message and
    // returned a non-zero id.
    let returned_id: u64 = world
        .resource::<ScriptEngine>()
        .lua()
        .globals()
        .get("_returned_cmd_id")
        .unwrap();
    assert!(returned_id > 0, "request_command must return a fresh id");
    // NextCommandId should have advanced.
    assert_eq!(
        world.resource::<NextCommandId>().0,
        returned_id,
        "returned id must match the counter"
    );
    // Peek the MoveRequested message.
    {
        let msgs = world.resource::<Messages<MoveRequested>>();
        let mut cursor = msgs.get_cursor();
        let all: Vec<&MoveRequested> = cursor.read(msgs).collect();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].ship, ship);
        assert_eq!(all[0].target, target);
        assert_eq!(all[0].command_id, CommandId(returned_id));
    }

    // --- Tick 2: run the surrogate handler (emits terminal CommandExecuted). ---
    run_move_requested_surrogate(&mut world, 101);

    // --- Tick 3: bridge forwards to EventSystem.fire_event_with_payload. ---
    run_bridge(&mut world);

    // EventSystem should now have a command_completed event queued.
    let has_completed = world
        .resource::<EventSystem>()
        .fired_log
        .iter()
        .any(|f| f.event_id == macrocosmo::event_system::COMMAND_COMPLETED_EVENT);
    assert!(
        has_completed,
        "bridge must push command_completed into fired_log"
    );

    // --- Tick 4: dispatch_event_handlers delivers to the Lua hook. ---
    dispatch_event_handlers(&mut world);

    // Verify the Lua observer recorded the fields.
    let engine = world.resource::<ScriptEngine>();
    let lua = engine.lua();
    let kind: String = lua.globals().get("_observed_kind").unwrap();
    let result: String = lua.globals().get("_observed_result").unwrap();
    let cmd_id: String = lua.globals().get("_observed_cmd_id").unwrap();
    assert_eq!(kind, "move");
    assert_eq!(result, "ok");
    assert_eq!(
        cmd_id,
        returned_id.to_string(),
        "hook command_id (decimal string) must match request_command return"
    );
}

/// Reentrancy: a `command_completed` hook callback can itself call
/// `request_command` for a **new** command without tripping the
/// gamestate `try_borrow_mut` guard. Because the bridge runs outside
/// the Lua scope and fire_event queues the next dispatch, there is no
/// synchronous callback chain from the handler into Lua.
#[test]
fn on_command_completed_may_call_request_command_again() {
    let mut world = make_world();
    let ship = world.spawn_empty().id();
    let target = world.spawn_empty().id();

    {
        let lua = world.resource::<ScriptEngine>().lua();
        lua.globals().set("_ship", ship.to_bits()).unwrap();
        lua.globals().set("_target", target.to_bits()).unwrap();
        lua.load(
            r#"
            _second_cmd_id = nil
            on("macrocosmo:lua_kickoff", function(evt)
                evt.gamestate:request_command("move", { ship = _ship, target = _target })
            end)
            -- Reentrancy exercise: issue another command from inside the hook.
            on("macrocosmo:command_completed", function(evt)
                if evt.kind == "move" and _second_cmd_id == nil then
                    _second_cmd_id = evt.gamestate:request_command("survey", {
                        ship = _ship,
                        target_system = _target,
                    })
                end
            end)
            "#,
        )
        .exec()
        .unwrap();
    }

    // Round 1: kickoff.
    {
        let mut es = world.resource_mut::<EventSystem>();
        es.fired_log.push(FiredEvent {
            event_id: "macrocosmo:lua_kickoff".into(),
            target: None,
            fired_at: 100,
            payload: None,
        });
    }
    dispatch_event_handlers(&mut world);
    run_move_requested_surrogate(&mut world, 101);
    run_bridge(&mut world);

    // Round 2: hook fires, reissues survey via gamestate.
    dispatch_event_handlers(&mut world);

    let second_id: Option<u64> = world
        .resource::<ScriptEngine>()
        .lua()
        .globals()
        .get("_second_cmd_id")
        .ok();
    assert!(
        second_id.is_some() && second_id.unwrap() > 0,
        "the hook must successfully call request_command a second time"
    );

    // A SurveyRequested message should have been emitted.
    let msgs = world.resource::<Messages<macrocosmo::ship::command_events::SurveyRequested>>();
    let mut cursor = msgs.get_cursor();
    let surveys: Vec<_> = cursor.read(msgs).collect();
    assert_eq!(surveys.len(), 1);
}

/// Malformed arguments surface as Lua errors (surfaced as pcall `false`).
#[test]
fn request_command_missing_arg_bubbles_runtime_error_to_lua() {
    let mut world = make_world();

    // Register a handler that intentionally omits `target`.
    {
        let lua = world.resource::<ScriptEngine>().lua();
        lua.load(
            r#"
            _ok = nil
            _err = nil
            on("macrocosmo:bad_req", function(evt)
                local ok, err = pcall(function()
                    evt.gamestate:request_command("move", { ship = 1 })
                end)
                _ok = ok
                _err = err and tostring(err) or nil
            end)
            "#,
        )
        .exec()
        .unwrap();
    }

    {
        let mut es = world.resource_mut::<EventSystem>();
        es.fired_log.push(FiredEvent {
            event_id: "macrocosmo:bad_req".into(),
            target: None,
            fired_at: 100,
            payload: None,
        });
    }
    dispatch_event_handlers(&mut world);

    let lua = world.resource::<ScriptEngine>().lua();
    let ok: bool = lua.globals().get("_ok").unwrap();
    let err: Option<String> = lua.globals().get("_err").unwrap();
    assert!(!ok, "malformed args must raise a Lua error");
    let msg = err.unwrap_or_default();
    assert!(
        msg.contains("missing") && msg.contains("target"),
        "error must name the missing field, got: {msg}"
    );

    // No MoveRequested should have been emitted.
    let msgs = world.resource::<Messages<MoveRequested>>();
    let mut cursor = msgs.get_cursor();
    assert_eq!(cursor.read(msgs).count(), 0);
}

/// Deferred results do not fire the hook (plan §10 R8).
#[test]
fn deferred_command_executed_does_not_fire_hook() {
    let mut world = make_world();

    {
        let lua = world.resource::<ScriptEngine>().lua();
        lua.load(
            r#"
            _hook_fired = false
            on("macrocosmo:command_completed", function(evt)
                _hook_fired = true
            end)
            "#,
        )
        .exec()
        .unwrap();
    }

    // Write a Deferred result directly — no dispatcher involvement.
    {
        let mut exec = world.resource_mut::<Messages<CommandExecuted>>();
        exec.write(CommandExecuted {
            command_id: CommandId(1),
            kind: CommandKind::Move,
            ship: Entity::from_raw_u32(1).unwrap(),
            result: CommandResult::Deferred,
            completed_at: 42,
        });
    }
    run_bridge(&mut world);

    assert!(
        world.resource::<EventSystem>().fired_log.is_empty(),
        "Deferred must not enqueue command_completed"
    );

    dispatch_event_handlers(&mut world);
    let fired: bool = world
        .resource::<ScriptEngine>()
        .lua()
        .globals()
        .get("_hook_fired")
        .unwrap();
    assert!(!fired, "hook must not fire for Deferred results");
}
