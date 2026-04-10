pub mod building_api;
pub mod event_api;
pub mod galaxy_api;
pub mod lifecycle;
pub mod modifier_api;
pub mod ship_design_api;
pub mod species_api;

use bevy::prelude::*;
use mlua::prelude::*;
use std::path::Path;

pub struct ScriptingPlugin;

impl Plugin for ScriptingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init_scripting)
            .add_systems(
                Startup,
                lifecycle::run_lifecycle_hooks
                    .after(init_scripting)
                    .after(crate::colony::load_building_registry)
                    .after(crate::technology::load_technologies),
            )
            .add_systems(
                Update,
                lifecycle::drain_script_events.after(crate::time_system::advance_game_time),
            )
            .add_systems(
                Update,
                lifecycle::dispatch_event_handlers
                    .after(crate::event_system::tick_events)
                    .after(crate::time_system::advance_game_time),
            );
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

        // Accumulator table for building definitions
        let building_defs = lua.create_table()?;
        globals.set("_building_definitions", building_defs)?;

        // define_building(table) -- appends a building definition table to _building_definitions
        let define_building = lua.create_function(|lua, table: mlua::Table| {
            let defs: mlua::Table = lua.globals().get("_building_definitions")?;
            let len = defs.len()?;
            defs.set(len + 1, table)?;
            Ok(())
        })?;
        globals.set("define_building", define_building)?;

        // Accumulator table for star type definitions
        let star_type_defs = lua.create_table()?;
        globals.set("_star_type_definitions", star_type_defs)?;

        // define_star_type(table) -- appends a star type definition table
        let define_star_type = lua.create_function(|lua, table: mlua::Table| {
            let defs: mlua::Table = lua.globals().get("_star_type_definitions")?;
            let len = defs.len()?;
            defs.set(len + 1, table)?;
            Ok(())
        })?;
        globals.set("define_star_type", define_star_type)?;

        // Accumulator table for planet type definitions
        let planet_type_defs = lua.create_table()?;
        globals.set("_planet_type_definitions", planet_type_defs)?;

        // define_planet_type(table) -- appends a planet type definition table
        let define_planet_type = lua.create_function(|lua, table: mlua::Table| {
            let defs: mlua::Table = lua.globals().get("_planet_type_definitions")?;
            let len = defs.len()?;
            defs.set(len + 1, table)?;
            Ok(())
        })?;
        globals.set("define_planet_type", define_planet_type)?;

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

        // --- Species and job definition Lua bindings ---

        // Accumulator table for species definitions
        let species_defs = lua.create_table()?;
        globals.set("_species_definitions", species_defs)?;

        // define_species(table) -- appends a species definition table to _species_definitions
        let define_species = lua.create_function(|lua, table: mlua::Table| {
            let defs: mlua::Table = lua.globals().get("_species_definitions")?;
            let len = defs.len()?;
            defs.set(len + 1, table)?;
            Ok(())
        })?;
        globals.set("define_species", define_species)?;

        // Accumulator table for job definitions
        let job_defs = lua.create_table()?;
        globals.set("_job_definitions", job_defs)?;

        // define_job(table) -- appends a job definition table to _job_definitions
        let define_job = lua.create_function(|lua, table: mlua::Table| {
            let defs: mlua::Table = lua.globals().get("_job_definitions")?;
            let len = defs.len()?;
            defs.set(len + 1, table)?;
            Ok(())
        })?;
        globals.set("define_job", define_job)?;

        // --- EventBus handler registration ---

        // Handler table for on() registrations
        let event_handlers = lua.create_table()?;
        globals.set("_event_handlers", event_handlers)?;

        // on(event_id, [filter,] handler) -- registers an event handler with optional structural filter
        let on_fn = lua.create_function(|lua, args: mlua::MultiValue| {
            let handlers: mlua::Table = lua.globals().get("_event_handlers")?;
            let len = handlers.len()?;

            let entry = lua.create_table()?;

            let mut args_iter = args.into_iter();
            // First arg: event_id string
            let event_id: String = match args_iter.next() {
                Some(mlua::Value::String(s)) => s.to_str()?.to_string(),
                _ => {
                    return Err(mlua::Error::RuntimeError(
                        "on() requires event_id string as first argument".into(),
                    ));
                }
            };
            entry.set("event_id", event_id)?;

            // Second arg: either a filter table or a handler function
            let second = args_iter.next().ok_or_else(|| {
                mlua::Error::RuntimeError(
                    "on() requires handler function (or filter table + handler function)".into(),
                )
            })?;

            match second {
                mlua::Value::Function(func) => {
                    // on(event_id, handler) -- no filter
                    entry.set("func", func)?;
                }
                mlua::Value::Table(filter) => {
                    // on(event_id, filter, handler)
                    entry.set("filter", filter)?;
                    let func = match args_iter.next() {
                        Some(mlua::Value::Function(f)) => f,
                        _ => {
                            return Err(mlua::Error::RuntimeError(
                                "on() with filter requires handler function as 3rd argument"
                                    .into(),
                            ));
                        }
                    };
                    entry.set("func", func)?;
                }
                _ => {
                    return Err(mlua::Error::RuntimeError(
                        "on() 2nd argument must be a filter table or handler function".into(),
                    ));
                }
            }

            handlers.set(len + 1, entry)?;
            Ok(())
        })?;
        globals.set("on", on_fn)?;

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

        // --- Ship design Lua bindings ---

        // Accumulator table for slot type definitions
        let slot_type_defs = lua.create_table()?;
        globals.set("_slot_type_definitions", slot_type_defs)?;

        let define_slot_type = lua.create_function(|lua, table: mlua::Table| {
            let defs: mlua::Table = lua.globals().get("_slot_type_definitions")?;
            let len = defs.len()?;
            defs.set(len + 1, table)?;
            Ok(())
        })?;
        globals.set("define_slot_type", define_slot_type)?;

        // Accumulator table for hull definitions
        let hull_defs = lua.create_table()?;
        globals.set("_hull_definitions", hull_defs)?;

        let define_hull = lua.create_function(|lua, table: mlua::Table| {
            let defs: mlua::Table = lua.globals().get("_hull_definitions")?;
            let len = defs.len()?;
            defs.set(len + 1, table)?;
            Ok(())
        })?;
        globals.set("define_hull", define_hull)?;

        // Accumulator table for module definitions
        let module_defs = lua.create_table()?;
        globals.set("_module_definitions", module_defs)?;

        let define_module = lua.create_function(|lua, table: mlua::Table| {
            let defs: mlua::Table = lua.globals().get("_module_definitions")?;
            let len = defs.len()?;
            defs.set(len + 1, table)?;
            Ok(())
        })?;
        globals.set("define_module", define_module)?;

        // Accumulator table for ship design definitions
        let ship_design_defs = lua.create_table()?;
        globals.set("_ship_design_definitions", ship_design_defs)?;

        let define_ship_design = lua.create_function(|lua, table: mlua::Table| {
            let defs: mlua::Table = lua.globals().get("_ship_design_definitions")?;
            let len = defs.len()?;
            defs.set(len + 1, table)?;
            Ok(())
        })?;
        globals.set("define_ship_design", define_ship_design)?;

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

        // --- Lifecycle hook registration ---

        // Handler tables for lifecycle hooks
        globals.set("_on_game_start_handlers", lua.create_table()?)?;
        globals.set("_on_game_load_handlers", lua.create_table()?)?;
        globals.set("_on_scripts_loaded_handlers", lua.create_table()?)?;

        // on_game_start(fn) -- registers a callback to run when a new game starts
        let on_game_start = lua.create_function(|lua, func: mlua::Function| {
            let handlers: mlua::Table = lua.globals().get("_on_game_start_handlers")?;
            let len = handlers.len()?;
            handlers.set(len + 1, func)?;
            Ok(())
        })?;
        globals.set("on_game_start", on_game_start)?;

        // on_game_load(fn) -- registers a callback to run when a saved game is loaded
        let on_game_load = lua.create_function(|lua, func: mlua::Function| {
            let handlers: mlua::Table = lua.globals().get("_on_game_load_handlers")?;
            let len = handlers.len()?;
            handlers.set(len + 1, func)?;
            Ok(())
        })?;
        globals.set("on_game_load", on_game_load)?;

        // on_scripts_loaded(fn) -- registers a callback to run after all scripts have been loaded
        let on_scripts_loaded = lua.create_function(|lua, func: mlua::Function| {
            let handlers: mlua::Table = lua.globals().get("_on_scripts_loaded_handlers")?;
            let len = handlers.len()?;
            handlers.set(len + 1, func)?;
            Ok(())
        })?;
        globals.set("on_scripts_loaded", on_scripts_loaded)?;

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

    #[test]
    fn test_on_function_registers_handler() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            on("macrocosmo:test_event", function(evt)
                -- handler body
            end)
            "#,
        )
        .exec()
        .unwrap();

        let handlers: mlua::Table = lua.globals().get("_event_handlers").unwrap();
        assert_eq!(handlers.len().unwrap(), 1);

        let entry: mlua::Table = handlers.get(1).unwrap();
        let eid: String = entry.get("event_id").unwrap();
        assert_eq!(eid, "macrocosmo:test_event");

        // No filter should be set
        let filter: mlua::Value = entry.get("filter").unwrap();
        assert!(matches!(filter, mlua::Value::Nil));

        // Handler function should be present
        let _func: mlua::Function = entry.get("func").unwrap();
    }

    #[test]
    fn test_on_with_filter() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            on("macrocosmo:building_lost", { cause = "combat" }, function(evt)
                -- handler body
            end)
            "#,
        )
        .exec()
        .unwrap();

        let handlers: mlua::Table = lua.globals().get("_event_handlers").unwrap();
        assert_eq!(handlers.len().unwrap(), 1);

        let entry: mlua::Table = handlers.get(1).unwrap();
        let eid: String = entry.get("event_id").unwrap();
        assert_eq!(eid, "macrocosmo:building_lost");

        // Filter should be present with the correct key/value
        let filter: mlua::Table = entry.get("filter").unwrap();
        let cause: String = filter.get("cause").unwrap();
        assert_eq!(cause, "combat");

        // Handler function should be present
        let _func: mlua::Function = entry.get("func").unwrap();
    }

    #[test]
    fn test_on_multiple_handlers() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            on("macrocosmo:event_a", function(evt) end)
            on("macrocosmo:event_b", { key = "val" }, function(evt) end)
            on("macrocosmo:event_a", function(evt) end)
            "#,
        )
        .exec()
        .unwrap();

        let handlers: mlua::Table = lua.globals().get("_event_handlers").unwrap();
        assert_eq!(handlers.len().unwrap(), 3);
    }
}
