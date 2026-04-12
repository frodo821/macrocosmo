use bevy::prelude::*;
use mlua::prelude::*;
use rand::rngs::SmallRng;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::game_rng::{register_game_rand, GameRng};
use super::globals;

/// Resolve the scripts directory by searching multiple locations.
/// Priority: 1) next to executable, 2) CWD, 3) CARGO_MANIFEST_DIR (dev)
pub fn resolve_scripts_dir() -> PathBuf {
    // 1. Next to the executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let candidate = exe_dir.join("scripts");
            if candidate.is_dir() {
                return candidate;
            }
        }
    }

    // 2. CWD
    let cwd = PathBuf::from("scripts");
    if cwd.is_dir() {
        return cwd;
    }

    // 3. CARGO_MANIFEST_DIR (development)
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let candidate = PathBuf::from(manifest_dir).join("scripts");
        if candidate.is_dir() {
            return candidate;
        }
    }

    // Fallback to CWD-relative (will fail gracefully later)
    PathBuf::from("scripts")
}

#[derive(Resource)]
pub struct ScriptEngine {
    lua: Lua,
    scripts_dir: PathBuf,
}

impl ScriptEngine {
    /// Create a new ScriptEngine with a freshly-seeded RNG handle.
    /// Convenient for tests; production code should prefer
    /// [`Self::new_with_rng`] so the engine shares the Bevy [`GameRng`]
    /// resource.
    pub fn new() -> Result<Self, mlua::Error> {
        let rng = GameRng::default();
        Self::new_with_rng(rng.handle())
    }

    /// Create a new ScriptEngine wired to the given RNG handle. The handle
    /// is used to back the `game_rand` Lua global.
    pub fn new_with_rng(rng: Arc<Mutex<SmallRng>>) -> Result<Self, mlua::Error> {
        let scripts_dir = resolve_scripts_dir();
        // Sandbox: only load safe libraries (no io, os, debug, ffi)
        let lua = Lua::new_with(
            LuaStdLib::TABLE | LuaStdLib::STRING | LuaStdLib::MATH
                | LuaStdLib::PACKAGE | LuaStdLib::BIT,
            mlua::LuaOptions::default(),
        )?;
        globals::setup_globals(&lua, &scripts_dir)?;
        register_game_rand(&lua, rng)?;
        info!("Lua scripts directory: {}", scripts_dir.display());
        Ok(Self { lua, scripts_dir })
    }

    /// The resolved scripts directory path.
    pub fn scripts_dir(&self) -> &Path {
        &self.scripts_dir
    }

    /// Backward-compatible static method that delegates to `globals::setup_globals`.
    pub fn setup_globals(lua: &Lua, scripts_dir: &Path) -> Result<(), mlua::Error> {
        globals::setup_globals(lua, scripts_dir)
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
