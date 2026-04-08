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
}
