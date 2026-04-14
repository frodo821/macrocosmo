use bevy::prelude::*;
use mlua::Lua;
use std::collections::HashMap;

use crate::condition::ScopedFlags;
use crate::event_system::EventSystem;
use crate::player::PlayerEmpire;
use crate::scripting::ScriptEngine;
use crate::technology::GameFlags;
use crate::time_system::GameClock;

/// Execute all registered on_game_start handlers.
pub fn run_on_game_start(lua: &Lua) -> Result<(), mlua::Error> {
    run_handlers(lua, "_on_game_start_handlers")
}

/// Execute all registered on_game_load handlers.
pub fn run_on_game_load(lua: &Lua) -> Result<(), mlua::Error> {
    run_handlers(lua, "_on_game_load_handlers")
}

/// Execute all registered on_scripts_loaded handlers.
pub fn run_on_scripts_loaded(lua: &Lua) -> Result<(), mlua::Error> {
    run_handlers(lua, "_on_scripts_loaded_handlers")
}

fn run_handlers(lua: &Lua, table_name: &str) -> Result<(), mlua::Error> {
    let handlers: mlua::Table = lua.globals().get(table_name)?;
    for i in 1..=handlers.len()? {
        let func: mlua::Function = handlers.get(i)?;
        func.call::<()>(())?;
    }
    Ok(())
}

/// Drain `_pending_flags` from Lua and return the flag names.
pub fn drain_pending_flags(lua: &Lua) -> Vec<String> {
    let Ok(flags) = lua.globals().get::<mlua::Table>("_pending_flags") else {
        return Vec::new();
    };
    let Ok(len) = flags.len() else {
        return Vec::new();
    };
    if len == 0 {
        return Vec::new();
    }

    let mut result = Vec::new();
    for i in 1..=len {
        if let Ok(flag) = flags.get::<String>(i) {
            result.push(flag);
        }
    }

    // Clear the table by replacing it with a fresh one
    if let Ok(new_table) = lua.create_table() {
        let _ = lua.globals().set("_pending_flags", new_table);
    }

    result
}

/// Startup system that runs lifecycle hooks after all scripts have been loaded.
/// Scripts are loaded by `load_all_scripts`; this system only executes callbacks.
/// Runs on_scripts_loaded and on_game_start hooks (on_game_load is reserved for
/// save/load which is not yet implemented).
/// After hooks execute, drains `_pending_flags` into `GameFlags` and `ScopedFlags`
/// on the empire entity.
pub fn run_lifecycle_hooks(
    engine: Res<ScriptEngine>,
    mut empire_query: Query<(&mut GameFlags, &mut ScopedFlags), With<PlayerEmpire>>,
) {
    let lua = engine.lua();

    // Run on_scripts_loaded hooks
    match run_on_scripts_loaded(lua) {
        Ok(()) => info!("on_scripts_loaded hooks executed"),
        Err(e) => warn!("on_scripts_loaded hook error: {e}"),
    }

    // Run on_game_start hooks (for now always — save/load not implemented)
    match run_on_game_start(lua) {
        Ok(()) => info!("on_game_start hooks executed"),
        Err(e) => warn!("on_game_start hook error: {e}"),
    }

    // Drain pending flags into GameFlags and ScopedFlags on the empire entity
    let pending = drain_pending_flags(lua);
    if !pending.is_empty() {
        if let Ok((mut game_flags, mut scoped_flags)) = empire_query.single_mut() {
            for flag in &pending {
                game_flags.set(flag);
                scoped_flags.set(flag);
            }
            info!("Drained {} pending flags into empire entity", pending.len());
        } else {
            warn!(
                "Could not find PlayerEmpire entity to drain {} pending flags",
                pending.len()
            );
        }
    }
}

/// Per-tick system that drains `_pending_script_events` from Lua and
/// forwards them to the Rust `EventSystem`.
pub fn drain_script_events(
    engine: Res<ScriptEngine>,
    mut event_system: ResMut<EventSystem>,
    clock: Res<GameClock>,
) {
    let lua = engine.lua();
    let Ok(events) = lua.globals().get::<mlua::Table>("_pending_script_events") else {
        return;
    };
    let Ok(len) = events.len() else {
        return;
    };
    if len == 0 {
        return;
    }

    for i in 1..=len {
        let Ok(entry) = events.get::<mlua::Table>(i) else {
            continue;
        };
        let Ok(event_id) = entry.get::<String>("event_id") else {
            continue;
        };
        let target: Option<u64> = entry.get("target").ok();
        // target is an Entity index — for now fire without target entity mapping
        let _ = target; // Reserved for future entity-target mapping
        event_system.fire_event(&event_id, None, clock.elapsed);
    }

    // Clear the table by replacing it with a fresh one
    if let Ok(new_table) = lua.create_table() {
        let _ = lua.globals().set("_pending_script_events", new_table);
    }
}

/// Per-tick **exclusive** system that dispatches recently fired events from
/// `EventSystem.fired_log` to Lua handlers — both:
/// * the `on(event_id, filter, fn)` bus handlers (stored in `_event_handlers`), and
/// * the `on_trigger` callback on the event definition itself (stored in the
///   Lua `_event_definitions` table).
///
/// The system runs with exclusive `&mut World` access so that `event.gamestate`
/// (a read-only world snapshot, #263) can be built inline and attached to the
/// payload table before any Lua callback is invoked. `ScriptEngine` is
/// re-acquired via `world.resource_scope` so we can hold both the Lua engine
/// and `&mut World` at the same time.
pub fn dispatch_event_handlers(world: &mut World) {
    // Fast path: nothing fired, skip world scope dance.
    let has_events = world
        .get_resource::<EventSystem>()
        .map(|es| !es.fired_log.is_empty())
        .unwrap_or(false);
    if !has_events {
        return;
    }

    // Drain fired log (mutable borrow scoped).
    let fired_events: Vec<crate::event_system::FiredEvent> = {
        let mut es = world.resource_mut::<EventSystem>();
        es.fired_log.drain(..).collect()
    };

    // Borrow ScriptEngine out of the world so that we can use &mut World to
    // build the gamestate snapshot for each dispatched event. `resource_scope`
    // temporarily removes the resource, giving us a &mut World that excludes
    // it; we restore it when the closure returns.
    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        let lua = engine.lua();
        for fired in &fired_events {
            let payload = if let Some(ref p) = fired.payload {
                p.clone()
            } else {
                let mut p = HashMap::new();
                p.insert("event_id".to_string(), fired.event_id.clone());
                p
            };

            // Build the payload table + attach live gamestate. Any error here
            // is non-fatal; we log and move on so a malformed gamestate
            // doesn't block other events from firing.
            let payload_table = match lua.create_table() {
                Ok(t) => t,
                Err(e) => {
                    warn!("dispatch_event_handlers: failed to create payload table: {e}");
                    continue;
                }
            };
            for (k, v) in &payload {
                let _ = payload_table.set(k.as_str(), v.as_str());
            }
            let _ = payload_table.set("event_id", fired.event_id.as_str());

            match crate::scripting::gamestate_view::attach_gamestate(lua, &payload_table, world) {
                Ok(()) => {}
                Err(e) => warn!(
                    "dispatch_event_handlers: failed to attach gamestate to '{}': {e}",
                    fired.event_id
                ),
            }

            // --- `on(id, filter, fn)` bus handlers ---
            dispatch_bus_handlers(lua, &fired.event_id, &payload, &payload_table);

            // --- `on_trigger` callback on the event definition ---
            dispatch_on_trigger(lua, &fired.event_id, &payload_table);
        }
    });
}

/// Re-implementation of `EventBus::fire` that reuses a caller-built
/// payload table (so `event.gamestate` is shared across all handlers for
/// a single fire).
fn dispatch_bus_handlers(
    lua: &mlua::Lua,
    event_id: &str,
    payload: &HashMap<String, String>,
    payload_table: &mlua::Table,
) {
    let Ok(handlers) = lua.globals().get::<mlua::Table>("_event_handlers") else {
        return;
    };
    let len = handlers.len().unwrap_or(0);
    if len == 0 {
        return;
    }
    for i in 1..=len {
        let Ok(entry) = handlers.get::<mlua::Table>(i) else {
            continue;
        };
        let Ok(eid) = entry.get::<String>("event_id") else {
            continue;
        };
        if eid != event_id {
            continue;
        }
        // Structural filter
        if let Ok(filter) = entry.get::<mlua::Table>("filter") {
            let mut matches = true;
            for pair in filter.pairs::<String, String>() {
                if let Ok((k, v)) = pair {
                    if payload.get(&k).map(|pv| pv.as_str()) != Some(v.as_str()) {
                        matches = false;
                        break;
                    }
                }
            }
            if !matches {
                continue;
            }
        }
        if let Ok(func) = entry.get::<mlua::Function>("func") {
            if let Err(e) = func.call::<()>(payload_table.clone()) {
                warn!("EventBus handler error for {}: {}", event_id, e);
            }
        }
    }
}

/// Dispatch the `on_trigger` callback declared on an event definition
/// (`_event_definitions[i].on_trigger`). Unlike bus handlers (`on(id, fn)`),
/// `on_trigger` is defined at `define_event { ... on_trigger = fn }` time and
/// is keyed by the event definition's `id`.
fn dispatch_on_trigger(lua: &mlua::Lua, event_id: &str, payload_table: &mlua::Table) {
    let Ok(defs) = lua.globals().get::<mlua::Table>("_event_definitions") else {
        return;
    };
    let Ok(len) = defs.len() else { return };
    for i in 1..=len {
        let Ok(def) = defs.get::<mlua::Table>(i) else {
            continue;
        };
        let Ok(id) = def.get::<String>("id") else {
            continue;
        };
        if id != event_id {
            continue;
        }
        match def.get::<mlua::Value>("on_trigger") {
            Ok(mlua::Value::Function(f)) => {
                if let Err(e) = f.call::<()>(payload_table.clone()) {
                    warn!("on_trigger error for event '{}': {}", event_id, e);
                }
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ScriptEngine;

    #[test]
    fn test_on_game_start_handler_called() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            on_game_start(function()
                _test_game_start_called = true
            end)
            "#,
        )
        .exec()
        .unwrap();

        run_on_game_start(lua).unwrap();

        let called: bool = lua.globals().get("_test_game_start_called").unwrap();
        assert!(called);
    }

    #[test]
    fn test_multiple_handlers() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            _test_order = {}
            on_game_start(function()
                table.insert(_test_order, "first")
            end)
            on_game_start(function()
                table.insert(_test_order, "second")
            end)
            "#,
        )
        .exec()
        .unwrap();

        run_on_game_start(lua).unwrap();

        let order: mlua::Table = lua.globals().get("_test_order").unwrap();
        let first: String = order.get(1).unwrap();
        let second: String = order.get(2).unwrap();
        assert_eq!(first, "first");
        assert_eq!(second, "second");
        assert_eq!(order.len().unwrap(), 2);
    }

    #[test]
    fn test_on_scripts_loaded() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            on_scripts_loaded(function()
                _test_scripts_loaded_called = true
            end)
            "#,
        )
        .exec()
        .unwrap();

        run_on_scripts_loaded(lua).unwrap();

        let called: bool = lua.globals().get("_test_scripts_loaded_called").unwrap();
        assert!(called);
    }

    #[test]
    fn test_on_game_load() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            on_game_load(function()
                _test_game_load_called = true
            end)
            "#,
        )
        .exec()
        .unwrap();

        run_on_game_load(lua).unwrap();

        let called: bool = lua.globals().get("_test_game_load_called").unwrap();
        assert!(called);
    }

    #[test]
    fn test_lifecycle_hooks_fire_events() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            on_game_start(function()
                fire_event("test_event")
            end)
            "#,
        )
        .exec()
        .unwrap();

        run_on_game_start(lua).unwrap();

        // Verify _pending_script_events has the event
        let events: mlua::Table = lua.globals().get("_pending_script_events").unwrap();
        assert_eq!(events.len().unwrap(), 1);
        let entry: mlua::Table = events.get(1).unwrap();
        let event_id: String = entry.get("event_id").unwrap();
        assert_eq!(event_id, "test_event");
    }

    #[test]
    fn test_no_handlers_is_ok() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        // Should not error when no handlers are registered
        run_on_game_start(lua).unwrap();
        run_on_game_load(lua).unwrap();
        run_on_scripts_loaded(lua).unwrap();
    }

    #[test]
    fn test_drain_pending_flags() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            set_flag("flag_a")
            set_flag("flag_b")
            "#,
        )
        .exec()
        .unwrap();

        let flags = drain_pending_flags(lua);
        assert_eq!(flags.len(), 2);
        assert!(flags.contains(&"flag_a".to_string()));
        assert!(flags.contains(&"flag_b".to_string()));

        // After draining, the table should be empty
        let flags_after = drain_pending_flags(lua);
        assert!(flags_after.is_empty());
    }

    #[test]
    fn test_drain_pending_flags_empty() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let flags = drain_pending_flags(lua);
        assert!(flags.is_empty());
    }

    /// CRITICAL #2: Verify fire_event from Lua queues into _pending_script_events.
    #[test]
    fn test_drain_script_events_integration() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        // Simulate fire_event from Lua side
        lua.load(r#"fire_event("test_event")"#).exec().unwrap();

        // Read _pending_script_events and verify the event was queued
        let events: mlua::Table = lua.globals().get("_pending_script_events").unwrap();
        assert_eq!(events.len().unwrap(), 1);
        let entry: mlua::Table = events.get(1).unwrap();
        let event_id: String = entry.get("event_id").unwrap();
        assert_eq!(event_id, "test_event");
    }

    /// CRITICAL #2: Verify fire_event with target parameter.
    #[test]
    fn test_drain_script_events_with_target() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(r#"fire_event("targeted_event", 42)"#).exec().unwrap();

        let events: mlua::Table = lua.globals().get("_pending_script_events").unwrap();
        assert_eq!(events.len().unwrap(), 1);
        let entry: mlua::Table = events.get(1).unwrap();
        let event_id: String = entry.get("event_id").unwrap();
        assert_eq!(event_id, "targeted_event");
        let target: u64 = entry.get("target").unwrap();
        assert_eq!(target, 42);
    }

    /// CRITICAL #2: Verify multiple fire_event calls accumulate.
    #[test]
    fn test_drain_script_events_multiple() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(r#"
            fire_event("event_a")
            fire_event("event_b")
            fire_event("event_c")
        "#).exec().unwrap();

        let events: mlua::Table = lua.globals().get("_pending_script_events").unwrap();
        assert_eq!(events.len().unwrap(), 3);

        let e1: mlua::Table = events.get(1).unwrap();
        assert_eq!(e1.get::<String>("event_id").unwrap(), "event_a");
        let e2: mlua::Table = events.get(2).unwrap();
        assert_eq!(e2.get::<String>("event_id").unwrap(), "event_b");
        let e3: mlua::Table = events.get(3).unwrap();
        assert_eq!(e3.get::<String>("event_id").unwrap(), "event_c");
    }

    // ------------- #263 dispatch + gamestate integration tests -------------

    use crate::event_system::FiredEvent;

    /// Build a minimal world with ScriptEngine + EventSystem + a player empire
    /// and clock, suitable for exercising `dispatch_event_handlers`.
    fn make_world() -> World {
        let mut world = World::new();
        world.insert_resource(crate::time_system::GameClock::new(42));
        world.insert_resource(EventSystem::default());
        world.insert_resource(ScriptEngine::new().unwrap());
        // Spawn a player empire so gamestate snapshot has something to show.
        let mut tree = crate::technology::TechTree::default();
        tree.researched
            .insert(crate::technology::TechId("tech_a".to_string()));
        let mut flags = GameFlags::default();
        flags.set("fa");
        world.spawn((
            crate::player::Empire { name: "E".into() },
            PlayerEmpire,
            tree,
            flags,
            ScopedFlags::default(),
        ));
        world
    }

    #[test]
    fn test_dispatch_attaches_gamestate_to_bus_handler() {
        let mut world = make_world();

        // Register a Lua `on` handler that records gamestate.clock.now.
        {
            let engine = world.resource::<ScriptEngine>();
            engine
                .lua()
                .load(
                    r#"
                    _captured_now = -1
                    _captured_empire_name = nil
                    on("macrocosmo:test", function(evt)
                        _captured_now = evt.gamestate.clock.now
                        _captured_empire_name = evt.gamestate.player_empire.name
                    end)
                    "#,
                )
                .exec()
                .unwrap();
        }

        // Fire via fired_log directly
        {
            let mut es = world.resource_mut::<EventSystem>();
            es.fired_log.push(FiredEvent {
                event_id: "macrocosmo:test".to_string(),
                target: None,
                fired_at: 42,
                payload: None,
            });
        }

        dispatch_event_handlers(&mut world);

        let engine = world.resource::<ScriptEngine>();
        let now: i64 = engine.lua().globals().get("_captured_now").unwrap();
        assert_eq!(now, 42, "event.gamestate.clock.now must match GameClock");
        let name: String = engine
            .lua()
            .globals()
            .get("_captured_empire_name")
            .unwrap();
        assert_eq!(name, "E");
    }

    #[test]
    fn test_dispatch_invokes_on_trigger_with_gamestate() {
        let mut world = make_world();
        {
            let engine = world.resource::<ScriptEngine>();
            engine
                .lua()
                .load(
                    r#"
                    _trigger_called = false
                    _trigger_has_tech = false
                    define_event {
                        id = "harvest_ended",
                        name = "Harvest Ended",
                        on_trigger = function(evt)
                            _trigger_called = true
                            _trigger_has_tech = evt.gamestate.player_empire.techs.tech_a
                        end,
                    }
                    "#,
                )
                .exec()
                .unwrap();
        }

        {
            let mut es = world.resource_mut::<EventSystem>();
            es.fired_log.push(FiredEvent {
                event_id: "harvest_ended".to_string(),
                target: None,
                fired_at: 42,
                payload: None,
            });
        }

        dispatch_event_handlers(&mut world);

        let engine = world.resource::<ScriptEngine>();
        let called: bool = engine.lua().globals().get("_trigger_called").unwrap();
        assert!(called, "on_trigger must fire when event_id matches");
        let has_tech: bool = engine.lua().globals().get("_trigger_has_tech").unwrap();
        assert!(has_tech, "gamestate techs lookup must work inside on_trigger");
    }

    #[test]
    fn test_dispatch_gamestate_mutation_inside_handler_fails_gracefully() {
        let mut world = make_world();
        {
            let engine = world.resource::<ScriptEngine>();
            engine
                .lua()
                .load(
                    r#"
                    _mutation_error = nil
                    on("macrocosmo:bad", function(evt)
                        local ok, err = pcall(function()
                            evt.gamestate.clock.now = 999
                        end)
                        if not ok then
                            _mutation_error = tostring(err)
                        end
                    end)
                    "#,
                )
                .exec()
                .unwrap();
        }
        {
            let mut es = world.resource_mut::<EventSystem>();
            es.fired_log.push(FiredEvent {
                event_id: "macrocosmo:bad".to_string(),
                target: None,
                fired_at: 42,
                payload: None,
            });
        }

        dispatch_event_handlers(&mut world);

        let engine = world.resource::<ScriptEngine>();
        let err: Option<String> = engine.lua().globals().get("_mutation_error").ok();
        let err = err.unwrap_or_default();
        assert!(
            err.contains("read-only"),
            "mutation must fail with a read-only error, got: {err}"
        );
    }

    #[test]
    fn test_dispatch_empty_fired_log_is_noop() {
        let mut world = make_world();
        // No events fired. Should not panic, should not touch Lua state.
        dispatch_event_handlers(&mut world);
        // Sanity: resource still there.
        assert!(world.get_resource::<EventSystem>().is_some());
        assert!(world.get_resource::<ScriptEngine>().is_some());
    }

    #[test]
    fn test_existing_event_scripts_still_work() {
        // Regression: a pre-#263 event script that doesn't touch gamestate
        // must still receive payload fields as before.
        let mut world = make_world();
        {
            let engine = world.resource::<ScriptEngine>();
            engine
                .lua()
                .load(
                    r#"
                    _old_style_cause = nil
                    on("macrocosmo:building_lost", { cause = "combat" }, function(evt)
                        _old_style_cause = evt.cause
                    end)
                    "#,
                )
                .exec()
                .unwrap();
        }
        {
            let mut es = world.resource_mut::<EventSystem>();
            let mut payload = HashMap::new();
            payload.insert("cause".to_string(), "combat".to_string());
            payload.insert("building_id".to_string(), "mine".to_string());
            es.fired_log.push(FiredEvent {
                event_id: "macrocosmo:building_lost".to_string(),
                target: None,
                fired_at: 1,
                payload: Some(payload),
            });
        }

        dispatch_event_handlers(&mut world);

        let engine = world.resource::<ScriptEngine>();
        let cause: String = engine.lua().globals().get("_old_style_cause").unwrap();
        assert_eq!(cause, "combat");
    }
}
