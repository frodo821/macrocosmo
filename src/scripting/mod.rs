pub mod building_api;
pub mod condition_parser;
pub mod event_api;
pub mod galaxy_api;
pub mod lifecycle;
pub mod modifier_api;
pub mod ship_design_api;
pub mod species_api;
pub mod structure_api;

use bevy::prelude::*;
use mlua::prelude::*;
use std::path::Path;

pub struct ScriptingPlugin;

impl Plugin for ScriptingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init_scripting)
            .add_systems(
                Startup,
                load_all_scripts.after(init_scripting),
            )
            .add_systems(
                Startup,
                lifecycle::run_lifecycle_hooks
                    .after(load_all_scripts)
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

/// Startup system that loads all Lua scripts via `scripts/init.lua` (if it exists),
/// falling back to loading individual directories for backward compatibility.
/// Other startup systems that parse definitions should use `.after(load_all_scripts)`.
pub fn load_all_scripts(engine: Res<ScriptEngine>) {
    let init_path = Path::new("scripts/init.lua");
    if init_path.exists() {
        match engine.load_file(init_path) {
            Ok(()) => {
                info!("All scripts loaded via scripts/init.lua");
                return;
            }
            Err(e) => {
                warn!("Failed to load scripts/init.lua: {e}; falling back to directory loading");
            }
        }
    }

    // Fallback: load directories individually (legacy path, used when init.lua is absent)
    let dirs = [
        "scripts/stars",
        "scripts/planets",
        "scripts/jobs",
        "scripts/species",
        "scripts/buildings",
        "scripts/tech",
        "scripts/ships",
        "scripts/structures",
        "scripts/events",
    ];
    for dir in &dirs {
        let path = Path::new(dir);
        if let Err(e) = engine.load_directory(path) {
            warn!("Failed to load scripts from {dir}: {e}");
        }
    }
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

        // --- Set up require() search path ---
        let package: mlua::Table = globals.get("package")?;
        package.set("path", "scripts/?.lua;scripts/?/init.lua")?;
        package.set("cpath", "")?; // disable C module loading

        // Create the macrocosmo namespace table
        let mc = lua.create_table()?;
        globals.set("macrocosmo", mc)?;

        // forward_ref(id) -- creates a placeholder reference for not-yet-defined items
        let forward_ref = lua.create_function(|lua, id: String| {
            let t = lua.create_table()?;
            t.set("_def_type", "forward_ref")?;
            t.set("id", id)?;
            Ok(t)
        })?;
        globals.set("forward_ref", forward_ref)?;

        // --- Define accumulator tables and define_xxx functions ---
        // Each define_xxx appends to its accumulator AND returns the table
        // with a _def_type tag, enabling return-value based references.

        Self::register_define_fn(lua, "tech", "_tech_definitions")?;
        Self::register_define_fn(lua, "building", "_building_definitions")?;
        Self::register_define_fn(lua, "star_type", "_star_type_definitions")?;
        Self::register_define_fn(lua, "planet_type", "_planet_type_definitions")?;

        // --- Species and job definition Lua bindings ---

        Self::register_define_fn(lua, "species", "_species_definitions")?;
        Self::register_define_fn(lua, "job", "_job_definitions")?;

        // --- Event definition ---

        Self::register_define_fn(lua, "event", "_event_definitions")?;

        // --- Ship design Lua bindings ---

        Self::register_define_fn(lua, "slot_type", "_slot_type_definitions")?;
        Self::register_define_fn(lua, "hull", "_hull_definitions")?;
        Self::register_define_fn(lua, "module", "_module_definitions")?;
        Self::register_define_fn(lua, "ship_design", "_ship_design_definitions")?;

        // --- Structure definition ---

        Self::register_define_fn(lua, "structure", "_structure_definitions")?;

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

        // --- Condition helper functions ---
        // These return Lua tables that represent condition nodes, parsed by condition_parser.

        // has_tech / has_modifier / has_building accept either a string ID
        // or a reference table (returned by define_xxx) from which the id is extracted.
        let has_tech = lua.create_function(|lua, value: mlua::Value| {
            let t = lua.create_table()?;
            t.set("type", "has_tech")?;
            t.set("id", extract_id_from_lua_value(&value)?)?;
            Ok(t)
        })?;
        globals.set("has_tech", has_tech)?;

        let has_modifier = lua.create_function(|lua, value: mlua::Value| {
            let t = lua.create_table()?;
            t.set("type", "has_modifier")?;
            t.set("id", extract_id_from_lua_value(&value)?)?;
            Ok(t)
        })?;
        globals.set("has_modifier", has_modifier)?;

        let has_building = lua.create_function(|lua, value: mlua::Value| {
            let t = lua.create_table()?;
            t.set("type", "has_building")?;
            t.set("id", extract_id_from_lua_value(&value)?)?;
            Ok(t)
        })?;
        globals.set("has_building", has_building)?;

        let all_fn = lua.create_function(|lua, args: mlua::MultiValue| {
            let t = lua.create_table()?;
            t.set("type", "all")?;
            let children = lua.create_table()?;
            for (i, arg) in args.into_iter().enumerate() {
                children.set(i + 1, arg)?;
            }
            t.set("children", children)?;
            Ok(t)
        })?;
        globals.set("all", all_fn)?;

        let any_fn = lua.create_function(|lua, args: mlua::MultiValue| {
            let t = lua.create_table()?;
            t.set("type", "any")?;
            let children = lua.create_table()?;
            for (i, arg) in args.into_iter().enumerate() {
                children.set(i + 1, arg)?;
            }
            t.set("children", children)?;
            Ok(t)
        })?;
        globals.set("any", any_fn)?;

        let one_of_fn = lua.create_function(|lua, args: mlua::MultiValue| {
            let t = lua.create_table()?;
            t.set("type", "one_of")?;
            let children = lua.create_table()?;
            for (i, arg) in args.into_iter().enumerate() {
                children.set(i + 1, arg)?;
            }
            t.set("children", children)?;
            Ok(t)
        })?;
        globals.set("one_of", one_of_fn)?;

        // "not" is a Lua keyword, so we use "not_cond" as the function name.
        let not_cond_fn = lua.create_function(|lua, child: mlua::Table| {
            let t = lua.create_table()?;
            t.set("type", "not")?;
            t.set("child", child)?;
            Ok(t)
        })?;
        globals.set("not_cond", not_cond_fn)?;

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

    /// Register a `define_xxx` global function that:
    /// 1. Creates an accumulator table `_xxx_definitions`
    /// 2. Registers `define_xxx(table)` which appends to the accumulator and
    ///    tags the table with `_def_type = def_type`, then returns it as a reference.
    fn register_define_fn(lua: &Lua, def_type: &str, accumulator_name: &str) -> Result<(), mlua::Error> {
        let globals = lua.globals();

        let acc = lua.create_table()?;
        globals.set(accumulator_name, acc)?;

        let acc_name = accumulator_name.to_string();
        let dtype = def_type.to_string();
        let func = lua.create_function(move |lua, table: mlua::Table| {
            table.set("_def_type", dtype.as_str())?;
            let defs: mlua::Table = lua.globals().get(acc_name.as_str())?;
            let len = defs.len()?;
            defs.set(len + 1, table.clone())?;
            Ok(table)
        })?;
        globals.set(format!("define_{def_type}"), func)?;

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

/// Extract an ID string from a Lua value that is either:
/// - A plain string → used as-is
/// - A reference table (from `define_xxx` or `forward_ref`) → reads the `id` field
fn extract_id_from_lua_value(value: &mlua::Value) -> Result<String, mlua::Error> {
    match value {
        mlua::Value::String(s) => Ok(s.to_str()?.to_string()),
        mlua::Value::Table(t) => t.get::<String>("id"),
        _ => Err(mlua::Error::RuntimeError(
            "Expected string ID or reference table".into(),
        )),
    }
}

/// Extract an ID from a Lua value, accepting both string IDs and reference tables.
/// This is the public API for use by Rust-side parsers.
pub fn extract_ref_id(value: &mlua::Value) -> Result<String, mlua::Error> {
    extract_id_from_lua_value(value)
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

    #[test]
    fn test_define_tech_returns_reference() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let result: mlua::Table = lua
            .load(r#"return define_tech { id = "test_tech", name = "Test" }"#)
            .eval()
            .unwrap();

        let def_type: String = result.get("_def_type").unwrap();
        assert_eq!(def_type, "tech");
        let id: String = result.get("id").unwrap();
        assert_eq!(id, "test_tech");
    }

    #[test]
    fn test_define_xxx_reference_in_prerequisites() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            local base = define_tech { id = "base_tech", name = "Base", branch = "physics", cost = 100, prerequisites = {} }
            define_tech { id = "advanced_tech", name = "Adv", branch = "physics", cost = 200, prerequisites = { base } }
            "#,
        )
        .exec()
        .unwrap();

        let defs: mlua::Table = lua.globals().get("_tech_definitions").unwrap();
        assert_eq!(defs.len().unwrap(), 2);
        // The second tech's prerequisites should contain a reference table
        let second: mlua::Table = defs.get(2).unwrap();
        let prereqs: mlua::Table = second.get("prerequisites").unwrap();
        let first_prereq: mlua::Table = prereqs.get(1).unwrap();
        let prereq_id: String = first_prereq.get("id").unwrap();
        assert_eq!(prereq_id, "base_tech");
    }

    #[test]
    fn test_forward_ref() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let result: mlua::Table = lua
            .load(r#"return forward_ref("future_tech")"#)
            .eval()
            .unwrap();

        let def_type: String = result.get("_def_type").unwrap();
        assert_eq!(def_type, "forward_ref");
        let id: String = result.get("id").unwrap();
        assert_eq!(id, "future_tech");
    }

    #[test]
    fn test_has_tech_accepts_reference() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        // String form (backward compatible)
        let cond_str: mlua::Table = lua
            .load(r#"return has_tech("my_tech")"#)
            .eval()
            .unwrap();
        assert_eq!(cond_str.get::<String>("id").unwrap(), "my_tech");

        // Reference form
        let cond_ref: mlua::Table = lua
            .load(r#"
                local t = define_tech { id = "ref_tech", name = "Ref" }
                return has_tech(t)
            "#)
            .eval()
            .unwrap();
        assert_eq!(cond_ref.get::<String>("id").unwrap(), "ref_tech");
    }

    #[test]
    fn test_require_support() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        // package.path should be set
        let package: mlua::Table = lua.globals().get("package").unwrap();
        let path: String = package.get("path").unwrap();
        assert!(path.contains("scripts/?.lua"));
        assert!(path.contains("scripts/?/init.lua"));

        // cpath should be empty
        let cpath: String = package.get("cpath").unwrap();
        assert!(cpath.is_empty());
    }

    #[test]
    fn test_extract_ref_id() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        // String value
        let s = mlua::Value::String(lua.create_string("hello").unwrap());
        assert_eq!(extract_ref_id(&s).unwrap(), "hello");

        // Table with id
        let t = lua.create_table().unwrap();
        t.set("id", "world").unwrap();
        let v = mlua::Value::Table(t);
        assert_eq!(extract_ref_id(&v).unwrap(), "world");

        // Number should fail
        let n = mlua::Value::Number(42.0);
        assert!(extract_ref_id(&n).is_err());
    }
}
