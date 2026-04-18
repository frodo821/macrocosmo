use bevy::prelude::*;
use mlua::prelude::*;
use rand_xoshiro::Xoshiro256PlusPlus;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::game_rng::{GameRng, register_game_rand};
use super::globals;

/// Environment variable that, when set, forces [`resolve_scripts_dir`] to use
/// the supplied path. Intended primarily for CI and distributed test runners
/// where the scripts bundle lives in a predictable absolute location that is
/// not discoverable from the executable or CWD.
pub const SCRIPTS_DIR_ENV_VAR: &str = "MACROCOSMO_SCRIPTS_DIR";

/// Consider a directory a valid scripts bundle only if it contains `init.lua`.
/// This prevents accidental matches against stray `scripts/` directories that
/// happen to exist higher up in the filesystem (e.g. a parent workspace).
fn is_valid_scripts_dir(candidate: &Path) -> bool {
    candidate.is_dir() && candidate.join("init.lua").is_file()
}

/// Walk `start` and each of its ancestors looking for a `scripts/` sub-directory
/// that contains `init.lua`. Returns the first match. Useful when the test
/// binary lives several directories below the repo root (e.g.
/// `target/debug/deps/<test>`), which is the common `cargo test` layout.
pub fn find_scripts_dir_upwards(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join("scripts");
        if is_valid_scripts_dir(&candidate) {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Resolve the scripts directory by searching multiple locations.
///
/// Priority:
/// 1. `MACROCOSMO_SCRIPTS_DIR` environment variable (for CI / test overrides).
/// 2. `scripts/` next to the currently running executable (bundled installs).
/// 3. `scripts/` in any ancestor of the current executable (common in
///    `cargo test`, where the binary lives under `target/debug/deps/`).
/// 4. `scripts/` in the current working directory or any of its ancestors.
/// 5. `scripts/` under `CARGO_MANIFEST_DIR` — **last resort**; this path is
///    baked in at compile time and can point at a stale worktree when a
///    binary is moved, so we only consult it if every other lookup fails.
///
/// If no candidate is valid the function still returns a `PathBuf`
/// (literal `"scripts"`) so that legacy callers keep compiling; prefer
/// [`try_resolve_scripts_dir`] when you want a hard error instead.
pub fn resolve_scripts_dir() -> PathBuf {
    try_resolve_scripts_dir().unwrap_or_else(|_| PathBuf::from("scripts"))
}

/// Inputs consumed by [`resolve_scripts_dir_from`]. Extracted so unit tests
/// can exercise the full resolution order without having to mutate
/// process-global state (env vars, CWD) which would race with other tests.
pub struct ScriptsDirInputs<'a> {
    pub env_override: Option<&'a Path>,
    pub exe_dir: Option<&'a Path>,
    pub cwd: Option<&'a Path>,
    pub manifest_dir: Option<&'a Path>,
}

/// Like [`resolve_scripts_dir`] but surfaces a descriptive error when no
/// candidate directory can be located. Use this from new code that is willing
/// to handle missing-scripts as a recoverable condition.
pub fn try_resolve_scripts_dir() -> Result<PathBuf, ScriptsDirError> {
    let env = std::env::var(SCRIPTS_DIR_ENV_VAR).ok();
    let env_path = env.as_deref().map(Path::new);
    let exe = std::env::current_exe().ok();
    let exe_dir = exe.as_deref().and_then(|e| e.parent());
    let cwd = std::env::current_dir().ok();
    let manifest = std::env::var("CARGO_MANIFEST_DIR").ok();
    let manifest_path = manifest.as_deref().map(Path::new);

    resolve_scripts_dir_from(&ScriptsDirInputs {
        env_override: env_path,
        exe_dir,
        cwd: cwd.as_deref(),
        manifest_dir: manifest_path,
    })
}

/// Pure resolution kernel. Given the four inputs (env override, executable
/// directory, CWD, `CARGO_MANIFEST_DIR`), return the first matching scripts
/// directory in the documented priority order. This function performs
/// filesystem existence checks but otherwise reads no process state, so it
/// is safe to call from parallel tests.
pub fn resolve_scripts_dir_from(inputs: &ScriptsDirInputs<'_>) -> Result<PathBuf, ScriptsDirError> {
    let mut tried: Vec<PathBuf> = Vec::new();

    // 1. Explicit env-var override.
    if let Some(value) = inputs.env_override {
        let candidate = PathBuf::from(value);
        if is_valid_scripts_dir(&candidate) {
            return Ok(candidate);
        }
        tried.push(candidate);
    }

    // 2. Next to the executable.
    if let Some(exe_dir) = inputs.exe_dir {
        let candidate = exe_dir.join("scripts");
        if is_valid_scripts_dir(&candidate) {
            return Ok(candidate);
        }
        tried.push(candidate);

        // 3. Ancestors of the executable.
        if let Some(found) = find_scripts_dir_upwards(exe_dir) {
            return Ok(found);
        }
    }

    // 4. CWD and its ancestors.
    if let Some(cwd) = inputs.cwd {
        if let Some(found) = find_scripts_dir_upwards(cwd) {
            return Ok(found);
        }
        tried.push(cwd.join("scripts"));
    }

    // 5. CARGO_MANIFEST_DIR (baked-in, last resort — warn so operators notice).
    if let Some(manifest_dir) = inputs.manifest_dir {
        let candidate = manifest_dir.join("scripts");
        if is_valid_scripts_dir(&candidate) {
            warn!(
                "Resolved scripts dir via CARGO_MANIFEST_DIR ({}). This path is \
                 baked in at compile time and may point at a stale location; \
                 set {SCRIPTS_DIR_ENV_VAR} to override.",
                candidate.display(),
            );
            return Ok(candidate);
        }
        tried.push(candidate);
    }

    Err(ScriptsDirError { tried })
}

/// Error returned by [`try_resolve_scripts_dir`] when every candidate path has
/// been exhausted. The `tried` list is purely informational — callers generally
/// only need the `Display` impl for logging.
#[derive(Debug, Clone)]
pub struct ScriptsDirError {
    pub tried: Vec<PathBuf>,
}

impl std::fmt::Display for ScriptsDirError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Could not locate a valid scripts/ directory (checked: ")?;
        for (i, p) in self.tried.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}", p.display())?;
        }
        write!(f, ")")
    }
}

impl std::error::Error for ScriptsDirError {}

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
    ///
    /// The scripts directory is auto-resolved via [`resolve_scripts_dir`].
    /// Call [`Self::new_with_rng_and_dir`] to pin it explicitly (tests, CI).
    pub fn new_with_rng(rng: Arc<Mutex<Xoshiro256PlusPlus>>) -> Result<Self, mlua::Error> {
        Self::new_with_rng_and_dir(rng, resolve_scripts_dir())
    }

    /// Create a new ScriptEngine with an explicitly supplied scripts
    /// directory. Intended for tests and CI — production code should use
    /// [`Self::new_with_rng`] so the auto-resolution logic takes effect.
    pub fn new_with_rng_and_dir(
        rng: Arc<Mutex<Xoshiro256PlusPlus>>,
        scripts_dir: PathBuf,
    ) -> Result<Self, mlua::Error> {
        // Sandbox: only load safe libraries (no io, os, debug, ffi)
        let lua = Lua::new_with(
            LuaStdLib::TABLE
                | LuaStdLib::STRING
                | LuaStdLib::MATH
                | LuaStdLib::PACKAGE
                | LuaStdLib::BIT,
            mlua::LuaOptions::default(),
        )?;
        globals::setup_globals(&lua, &scripts_dir)?;
        register_game_rand(&lua, rng)?;
        info!("Lua scripts directory: {}", scripts_dir.display());
        Ok(Self { lua, scripts_dir })
    }

    /// Create a new ScriptEngine with an explicit scripts directory and a
    /// freshly seeded RNG. Preferred over [`Self::new`] in tests that need a
    /// deterministic scripts path (e.g. to pin the sandbox root regardless of
    /// which `cargo` layout the test binary happens to live in).
    pub fn new_with_scripts_dir(scripts_dir: PathBuf) -> Result<Self, mlua::Error> {
        let rng = GameRng::default();
        Self::new_with_rng_and_dir(rng.handle(), scripts_dir)
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

#[cfg(test)]
mod path_resolution_tests {
    //! Regression tests for #148.
    //!
    //! All scenarios drive the pure [`resolve_scripts_dir_from`] kernel so
    //! they don't mutate process-global env vars or CWD — that keeps them
    //! parallel-safe with other tests that call `ScriptEngine::new()`.
    use super::*;
    use std::fs;

    /// Build a fake `scripts/init.lua` tree inside a temp directory. Returns
    /// `(temp_root, scripts_path)`. Each call uses a unique suffix so parallel
    /// tests cannot collide.
    fn scratch_scripts_dir(name: &str) -> (PathBuf, PathBuf) {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let base = std::env::temp_dir().join(format!(
            "macrocosmo-scripts-{}-{}-{}",
            name,
            std::process::id(),
            suffix,
        ));
        let scripts = base.join("scripts");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&scripts).unwrap();
        fs::write(scripts.join("init.lua"), "-- test\n").unwrap();
        (base, scripts)
    }

    fn empty_dir(name: &str) -> PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "macrocosmo-scripts-empty-{}-{}-{}",
            name,
            std::process::id(),
            suffix,
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn explicit_path_construction_succeeds() {
        let (_root, scripts) = scratch_scripts_dir("explicit");
        let engine = ScriptEngine::new_with_scripts_dir(scripts.clone()).unwrap();
        assert_eq!(engine.scripts_dir(), scripts.as_path());
    }

    #[test]
    fn find_scripts_dir_upwards_walks_ancestors() {
        let (root, scripts) = scratch_scripts_dir("upwards");
        let nested = root.join("a").join("b").join("c");
        fs::create_dir_all(&nested).unwrap();
        let found = find_scripts_dir_upwards(&nested).expect("should find scripts");
        assert_eq!(found, scripts);
    }

    #[test]
    fn find_scripts_dir_upwards_ignores_dirs_without_init_lua() {
        // A `scripts/` directory that lacks init.lua must be skipped.
        let base = empty_dir("noinit-base");
        let decoy = base.join("scripts");
        fs::create_dir_all(&decoy).unwrap();
        assert!(find_scripts_dir_upwards(&base).is_none());
    }

    #[test]
    fn env_override_wins_over_other_candidates() {
        let (_root_env, env_scripts) = scratch_scripts_dir("kernel-env");
        let (_root_exe, exe_scripts_root) = scratch_scripts_dir("kernel-exe");
        // If both env and exe dir point at valid bundles, env wins.
        let resolved = resolve_scripts_dir_from(&ScriptsDirInputs {
            env_override: Some(&env_scripts),
            exe_dir: Some(&exe_scripts_root), // `scripts/init.lua` *inside* this
            cwd: None,
            manifest_dir: None,
        })
        .expect("resolution should succeed");
        assert_eq!(resolved, env_scripts);
    }

    #[test]
    fn exe_neighbor_preferred_over_manifest_dir() {
        // exe_dir points at a directory whose `scripts/` is valid; manifest
        // also valid but should be ignored because exe is earlier.
        let (_root_exe, exe_scripts) = scratch_scripts_dir("kernel-exe-neighbor");
        let exe_dir = exe_scripts.parent().unwrap();
        let (_root_manifest, _manifest_scripts) = scratch_scripts_dir("kernel-manifest");
        let manifest_dir = _root_manifest.as_path();
        let resolved = resolve_scripts_dir_from(&ScriptsDirInputs {
            env_override: None,
            exe_dir: Some(exe_dir),
            cwd: None,
            manifest_dir: Some(manifest_dir),
        })
        .unwrap();
        assert_eq!(resolved, exe_scripts);
    }

    #[test]
    fn exe_ancestor_search_finds_scripts() {
        // Put scripts at `<root>/scripts` and pretend exe lives at
        // `<root>/a/b/c/exe-dir` (no `scripts/` directly adjacent).
        let (root, scripts) = scratch_scripts_dir("kernel-ancestor");
        let exe_dir = root.join("a").join("b").join("c").join("exe-dir");
        fs::create_dir_all(&exe_dir).unwrap();
        let resolved = resolve_scripts_dir_from(&ScriptsDirInputs {
            env_override: None,
            exe_dir: Some(&exe_dir),
            cwd: None,
            manifest_dir: None,
        })
        .unwrap();
        assert_eq!(resolved, scripts);
    }

    #[test]
    fn cwd_ancestor_search_finds_scripts() {
        let (root, scripts) = scratch_scripts_dir("kernel-cwd");
        let cwd = root.join("nested").join("deep");
        fs::create_dir_all(&cwd).unwrap();
        let resolved = resolve_scripts_dir_from(&ScriptsDirInputs {
            env_override: None,
            exe_dir: None,
            cwd: Some(&cwd),
            manifest_dir: None,
        })
        .unwrap();
        assert_eq!(resolved, scripts);
    }

    #[test]
    fn manifest_dir_used_as_last_resort() {
        let (root_manifest, manifest_scripts) = scratch_scripts_dir("kernel-manifest-only");
        let resolved = resolve_scripts_dir_from(&ScriptsDirInputs {
            env_override: None,
            exe_dir: None,
            cwd: None,
            manifest_dir: Some(&root_manifest),
        })
        .unwrap();
        assert_eq!(resolved, manifest_scripts);
    }

    #[test]
    fn errors_when_every_candidate_missing() {
        let empty_env = empty_dir("kernel-empty-env");
        let empty_exe = empty_dir("kernel-empty-exe");
        let empty_cwd = empty_dir("kernel-empty-cwd");
        let empty_manifest = empty_dir("kernel-empty-manifest");
        let res = resolve_scripts_dir_from(&ScriptsDirInputs {
            env_override: Some(&empty_env.join("missing")),
            exe_dir: Some(&empty_exe),
            cwd: Some(&empty_cwd),
            manifest_dir: Some(&empty_manifest),
        });
        match res {
            Err(e) => {
                assert!(!e.tried.is_empty(), "tried list should be populated");
                let msg = format!("{e}");
                assert!(msg.contains("Could not locate"), "unexpected msg: {msg}");
            }
            Ok(p) => panic!("expected error, got {}", p.display()),
        }
    }

    #[test]
    fn invalid_env_override_falls_through_to_exe() {
        // Env var set but pointing at a missing dir → exe neighbor must still
        // win.
        let empty_env = empty_dir("kernel-bad-env");
        let (_root_exe, exe_scripts) = scratch_scripts_dir("kernel-fallthrough-exe");
        let exe_dir = exe_scripts.parent().unwrap();
        let resolved = resolve_scripts_dir_from(&ScriptsDirInputs {
            env_override: Some(&empty_env.join("missing")),
            exe_dir: Some(exe_dir),
            cwd: None,
            manifest_dir: None,
        })
        .unwrap();
        assert_eq!(resolved, exe_scripts);
    }

    #[test]
    fn resolve_scripts_dir_fallback_is_stable() {
        // The infallible wrapper must always return *some* PathBuf even if
        // nothing is discoverable — downstream callers detect a missing
        // init.lua via their own `exists()` checks.
        let p = resolve_scripts_dir();
        assert!(!p.as_os_str().is_empty());
    }
}
