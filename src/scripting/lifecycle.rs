use bevy::prelude::*;
use mlua::Lua;
use std::collections::HashMap;

use crate::event_system::{EventBus, EventSystem};
use crate::scripting::ScriptEngine;
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

/// Startup system that runs lifecycle hooks after all scripts have been loaded.
/// Scripts are loaded by `load_all_scripts`; this system only executes callbacks.
/// Runs on_scripts_loaded and on_game_start hooks (on_game_load is reserved for
/// save/load which is not yet implemented).
pub fn run_lifecycle_hooks(engine: Res<ScriptEngine>) {
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

/// Per-tick system that dispatches recently fired events from `EventSystem.fired_log`
/// to Lua handlers registered via the `on()` API (stored in `_event_handlers`).
///
/// This runs after `tick_events` so that events fired during this tick are
/// available in `fired_log`. The fired_log is drained after processing to
/// avoid re-dispatching the same events on subsequent ticks.
pub fn dispatch_event_handlers(
    engine: Res<ScriptEngine>,
    mut event_system: ResMut<EventSystem>,
    _bus: Res<EventBus>,
) {
    if event_system.fired_log.is_empty() {
        return;
    }

    let lua = engine.lua();

    // Collect events to dispatch, then clear the log
    let fired_events: Vec<_> = event_system.fired_log.drain(..).collect();

    for fired in &fired_events {
        let payload = if let Some(ref p) = fired.payload {
            p.clone()
        } else {
            let mut p = HashMap::new();
            p.insert("event_id".to_string(), fired.event_id.clone());
            p
        };
        EventBus::fire(lua, &fired.event_id, &payload);
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
}
