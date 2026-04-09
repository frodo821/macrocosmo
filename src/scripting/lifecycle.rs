use bevy::prelude::*;
use mlua::Lua;

use crate::event_system::EventSystem;
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
/// Runs on_scripts_loaded and on_game_start hooks (on_game_load is reserved for
/// save/load which is not yet implemented).
pub fn run_lifecycle_hooks(engine: Res<ScriptEngine>) {
    let lua = engine.lua();

    // Load lifecycle scripts
    let lifecycle_dir = std::path::Path::new("scripts/lifecycle");
    if lifecycle_dir.exists() {
        match engine.load_directory(lifecycle_dir) {
            Ok(()) => info!("Lifecycle scripts loaded"),
            Err(e) => warn!("Failed to load lifecycle scripts: {e}"),
        }
    }

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
}
