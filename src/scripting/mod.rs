pub mod event_api;
pub mod modifier_api;

use bevy::prelude::*;
use mlua::prelude::*;
use std::path::Path;

pub struct ScriptingPlugin;

impl Plugin for ScriptingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init_scripting);
    }
}

/// Startup system that initialises the Lua scripting engine and inserts it as a
/// Bevy resource. Other startup systems can depend on this via `.after(init_scripting)`.
pub fn init_scripting(mut commands: Commands) {
    let engine = ScriptEngine::new().expect("Failed to initialize Lua scripting engine");
    commands.insert_resource(engine);
}

#[derive(Resource)]
pub struct ScriptEngine {
    lua: Lua,
}

impl ScriptEngine {
    pub fn new() -> Result<Self, mlua::Error> {
        let lua = Lua::new();
        Self::setup_globals(&lua)?;
        Ok(Self { lua })
    }

    /// Configure global tables and functions available to all Lua scripts.
    pub fn setup_globals(lua: &Lua) -> Result<(), mlua::Error> {
        let globals = lua.globals();

        // Create the macrocosmo namespace table
        let mc = lua.create_table()?;
        globals.set("macrocosmo", mc)?;

        // Accumulator table for tech definitions
        let tech_defs = lua.create_table()?;
        globals.set("_tech_definitions", tech_defs)?;

        // define_tech(table) -- appends a tech definition table to _tech_definitions
        let define_tech = lua.create_function(|lua, table: mlua::Table| {
            let defs: mlua::Table = lua.globals().get("_tech_definitions")?;
            let len = defs.len()?;
            defs.set(len + 1, table)?;
            Ok(())
        })?;
        globals.set("define_tech", define_tech)?;

        // --- #45: Global param / flag Lua bindings ---

        // Pending modifications table: scripts call modify_global/set_flag/check_flag
        // and these are buffered for the Rust side to apply.
        let pending_mods = lua.create_table()?;
        globals.set("_pending_global_mods", pending_mods)?;

        let pending_flags = lua.create_table()?;
        globals.set("_pending_flags", pending_flags)?;

        let flag_store = lua.create_table()?;
        globals.set("_flag_store", flag_store)?;

        // modify_global(param_name, value) -- buffers a global param modification
        let modify_global = lua.create_function(|lua, (param_name, value): (String, f64)| {
            let mods: mlua::Table = lua.globals().get("_pending_global_mods")?;
            let len = mods.len()?;
            let entry = lua.create_table()?;
            entry.set("param", param_name)?;
            entry.set("value", value)?;
            mods.set(len + 1, entry)?;
            Ok(())
        })?;
        globals.set("modify_global", modify_global)?;

        // set_flag(name) -- sets a game flag
        let set_flag = lua.create_function(|lua, name: String| {
            let flags: mlua::Table = lua.globals().get("_pending_flags")?;
            let len = flags.len()?;
            flags.set(len + 1, name.clone())?;
            // Also store in _flag_store so check_flag works immediately
            let store: mlua::Table = lua.globals().get("_flag_store")?;
            store.set(name, true)?;
            Ok(())
        })?;
        globals.set("set_flag", set_flag)?;

        // check_flag(name) -- returns true if the flag is set
        let check_flag = lua.create_function(|lua, name: String| {
            let store: mlua::Table = lua.globals().get("_flag_store")?;
            let result: bool = store.get::<Option<bool>>(name)?.unwrap_or(false);
            Ok(result)
        })?;
        globals.set("check_flag", check_flag)?;

        // --- Event system Lua bindings ---

        // Accumulator table for event definitions
        let event_defs = lua.create_table()?;
        globals.set("_event_definitions", event_defs)?;

        // define_event(table) -- appends an event definition table to _event_definitions
        let define_event = lua.create_function(|lua, table: mlua::Table| {
            let defs: mlua::Table = lua.globals().get("_event_definitions")?;
            let len = defs.len()?;
            defs.set(len + 1, table)?;
            Ok(())
        })?;
        globals.set("define_event", define_event)?;

        // mtth_trigger(params) -- constructor that tags a table as type "mtth"
        let mtth_trigger = lua.create_function(|_, table: mlua::Table| {
            table.set("_type", "mtth")?;
            Ok(table)
        })?;
        globals.set("mtth_trigger", mtth_trigger)?;

        // periodic_trigger(params) -- constructor that tags a table as type "periodic"
        let periodic_trigger = lua.create_function(|_, table: mlua::Table| {
            table.set("_type", "periodic")?;
            Ok(table)
        })?;
        globals.set("periodic_trigger", periodic_trigger)?;

        // Pending script-fired events table
        let pending_script_events = lua.create_table()?;
        globals.set("_pending_script_events", pending_script_events)?;

        // fire_event(event_id, target?) -- queues an event to be fired from Lua
        let fire_event_fn = lua.create_function(|lua, args: (String, Option<u64>)| {
            let events: mlua::Table = lua.globals().get("_pending_script_events")?;
            let len = events.len()?;
            let entry = lua.create_table()?;
            entry.set("event_id", args.0)?;
            if let Some(target) = args.1 {
                entry.set("target", target)?;
            }
            events.set(len + 1, entry)?;
            Ok(())
        })?;
        globals.set("fire_event", fire_event_fn)?;

        Ok(())
    }

    /// Load and execute a single Lua file.
    pub fn load_file(&self, path: &Path) -> Result<(), mlua::Error> {
        let code = std::fs::read_to_string(path).map_err(|e| {
            mlua::Error::RuntimeError(format!("Failed to read {}: {e}", path.display()))
        })?;
        self.lua
            .load(&code)
            .set_name(path.to_string_lossy())
            .exec()?;
        Ok(())
    }

    /// Load and execute all `.lua` files in a directory, sorted alphabetically.
    pub fn load_directory(&self, dir: &Path) -> Result<(), mlua::Error> {
        if !dir.exists() {
            return Ok(());
        }
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| mlua::Error::RuntimeError(e.to_string()))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "lua"))
            .collect();
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            self.load_file(&entry.path())?;
        }
        Ok(())
    }

    /// Access the underlying Lua state.
    pub fn lua(&self) -> &Lua {
        &self.lua
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_creates_globals() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        // macrocosmo table exists
        let mc: mlua::Table = lua.globals().get("macrocosmo").unwrap();
        assert!(mc.len().unwrap() == 0);

        // define_tech function exists
        let _func: mlua::Function = lua.globals().get("define_tech").unwrap();

        // _tech_definitions table exists and is empty
        let defs: mlua::Table = lua.globals().get("_tech_definitions").unwrap();
        assert_eq!(defs.len().unwrap(), 0);
    }

    #[test]
    fn test_define_tech_accumulates() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_tech { id = 1, name = "A" }
            define_tech { id = 2, name = "B" }
            "#,
        )
        .exec()
        .unwrap();

        let defs: mlua::Table = lua.globals().get("_tech_definitions").unwrap();
        assert_eq!(defs.len().unwrap(), 2);
    }

    #[test]
    fn test_load_directory_missing_dir() {
        let engine = ScriptEngine::new().unwrap();
        // Should not error when directory doesn't exist
        engine
            .load_directory(Path::new("/nonexistent/path"))
            .unwrap();
    }

    // --- #45: Lua binding tests ---

    #[test]
    fn test_modify_global_lua() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(r#"modify_global("sublight_speed_bonus", 0.5)"#)
            .exec()
            .unwrap();

        let mods: mlua::Table = lua.globals().get("_pending_global_mods").unwrap();
        assert_eq!(mods.len().unwrap(), 1);
        let entry: mlua::Table = mods.get(1).unwrap();
        let param: String = entry.get("param").unwrap();
        let value: f64 = entry.get("value").unwrap();
        assert_eq!(param, "sublight_speed_bonus");
        assert!((value - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_set_flag_lua() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(r#"set_flag("building_Starbase")"#)
            .exec()
            .unwrap();

        let flags: mlua::Table = lua.globals().get("_pending_flags").unwrap();
        assert_eq!(flags.len().unwrap(), 1);
        let flag: String = flags.get(1).unwrap();
        assert_eq!(flag, "building_Starbase");
    }

    #[test]
    fn test_check_flag_lua() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let result: bool = lua
            .load(r#"return check_flag("nonexistent")"#)
            .eval()
            .unwrap();
        assert!(!result);

        lua.load(r#"set_flag("my_flag")"#).exec().unwrap();

        let result: bool = lua
            .load(r#"return check_flag("my_flag")"#)
            .eval()
            .unwrap();
        assert!(result);
    }
}
