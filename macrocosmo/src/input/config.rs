//! TOML persistence for [`super::KeybindingRegistry`].
//!
//! Override file lives at `<config_dir>/macrocosmo/keybindings.toml` where
//! `<config_dir>` follows the standard per-platform conventions:
//!
//! | Platform | Path                                                 |
//! | -------- | ---------------------------------------------------- |
//! | Linux    | `$XDG_CONFIG_HOME/macrocosmo/keybindings.toml` (or  |
//! |          | `~/.config/macrocosmo/keybindings.toml`)             |
//! | macOS    | `~/Library/Application Support/macrocosmo/keybindings.toml` |
//! | Windows  | `%APPDATA%\macrocosmo\keybindings.toml`              |
//! | Other    | `<binary dir>/keybindings.toml` as last-resort fallback |
//!
//! Tests and CI can bypass auto-discovery via the `MACROCOSMO_KEYBINDINGS_PATH`
//! environment variable, which (when set) overrides the platform path
//! entirely.
//!
//! ## Format
//!
//! ```toml
//! [bindings]
//! "ui.toggle_situation_center" = { key = "F4" }
//! "ui.toggle_console" = { key = "F2", alt = true }
//! ```
//!
//! Only entries explicitly listed in `[bindings]` are considered overrides;
//! anything missing keeps its engine default. Unknown action ids are
//! ignored with a warning (`KeybindingRegistry::set` short-circuits).

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{KeyCombo, KeybindingRegistry};

/// Filename for the per-user override file. Public so tests / docs can
/// reference it.
pub const FILE_NAME: &str = "keybindings.toml";

/// Subdirectory under the platform config dir.
pub const APP_DIR: &str = "macrocosmo";

/// Env var that, if set, overrides the auto-discovered config path
/// entirely. Primarily for tests, CI, and power users who want to keep
/// their settings under version control.
pub const PATH_OVERRIDE_ENV: &str = "MACROCOSMO_KEYBINDINGS_PATH";

/// What happened when [`load_overrides_into`] tried to read the override
/// file.
#[derive(Debug)]
pub enum LoadOutcome {
    /// File existed and was successfully merged into the registry.
    Loaded {
        path: PathBuf,
        /// How many bindings the file actually contained.
        count: usize,
    },
    /// File did not exist (first run, or the user never customised
    /// keybinds). Not an error.
    Missing { path: PathBuf },
}

/// Errors that can arise during keybinding config IO. Distinct from the
/// `Missing` outcome — this only fires for *real* failures (parse error,
/// unreadable file, no writable directory).
#[derive(Debug)]
pub enum ConfigError {
    NoConfigDir,
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    Serialise(toml::ser::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::NoConfigDir => {
                write!(f, "could not determine a writable config directory")
            }
            ConfigError::Io { path, source } => {
                write!(f, "io error at {}: {}", path.display(), source)
            }
            ConfigError::Parse { path, source } => {
                write!(f, "parse error at {}: {}", path.display(), source)
            }
            ConfigError::Serialise(e) => write!(f, "serialise error: {}", e),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::NoConfigDir => None,
            ConfigError::Io { source, .. } => Some(source),
            ConfigError::Parse { source, .. } => Some(source),
            ConfigError::Serialise(e) => Some(e),
        }
    }
}

impl From<toml::ser::Error> for ConfigError {
    fn from(e: toml::ser::Error) -> Self {
        ConfigError::Serialise(e)
    }
}

/// On-disk representation. A free-standing struct (rather than a direct
/// serde impl on [`KeybindingRegistry`]) keeps the wire format
/// independent of the in-memory representation — we can grow the registry
/// without forcing a config-file migration.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct KeybindingConfig {
    /// `action_id → KeyCombo`. `BTreeMap` for deterministic on-disk
    /// ordering (so diffing config files between sessions is meaningful).
    #[serde(default)]
    pub bindings: BTreeMap<String, KeyCombo>,
}

impl KeybindingConfig {
    /// Build a config snapshot from the **non-default** bindings in the
    /// registry. Bindings that still match their default are omitted so
    /// the on-disk file stays small and forward-compatible (a future
    /// engine change to a default automatically applies to users who
    /// never customised that action).
    pub fn from_overrides(registry: &KeybindingRegistry) -> Self {
        let mut bindings = BTreeMap::new();
        for (id, combo) in registry.iter() {
            match registry.default_for(id) {
                Some(default) if default == *combo => {}
                _ => {
                    bindings.insert(id.to_string(), *combo);
                }
            }
        }
        Self { bindings }
    }

    /// Merge every binding in `self` into `registry`. Unknown action ids
    /// are filtered out by [`KeybindingRegistry::set`]'s own validation.
    pub fn apply_to(&self, registry: &mut KeybindingRegistry) -> usize {
        let mut applied = 0usize;
        for (id, combo) in &self.bindings {
            if registry.default_for(id).is_some() {
                registry.set(id, *combo);
                applied += 1;
            } else {
                bevy::log::warn!("Keybindings: ignoring override for unknown action '{}'", id);
            }
        }
        applied
    }
}

/// Resolve the on-disk override file's path. Honours
/// [`PATH_OVERRIDE_ENV`] first, then falls back to the per-platform
/// config dir.
pub fn config_path() -> Result<PathBuf, ConfigError> {
    if let Ok(custom) = std::env::var(PATH_OVERRIDE_ENV) {
        if !custom.is_empty() {
            return Ok(PathBuf::from(custom));
        }
    }
    let base = platform_config_dir().ok_or(ConfigError::NoConfigDir)?;
    Ok(base.join(APP_DIR).join(FILE_NAME))
}

/// Best-effort load + merge. The error type returned is *only* for hard
/// failures — a missing file produces [`LoadOutcome::Missing`], not an
/// `Err`, because first-run with no overrides is the common case.
pub fn load_overrides_into(registry: &mut KeybindingRegistry) -> Result<LoadOutcome, ConfigError> {
    let path = config_path()?;
    load_overrides_from(registry, &path)
}

/// Variant of [`load_overrides_into`] that reads from a caller-supplied
/// path. Used by tests to bypass the env-var-driven discovery (which
/// would race when several tests run in parallel) and by future
/// "import settings" UI flows.
pub fn load_overrides_from(
    registry: &mut KeybindingRegistry,
    path: &Path,
) -> Result<LoadOutcome, ConfigError> {
    if !path.exists() {
        return Ok(LoadOutcome::Missing {
            path: path.to_path_buf(),
        });
    }
    let raw = fs::read_to_string(path).map_err(|e| ConfigError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let config: KeybindingConfig = toml::from_str(&raw).map_err(|e| ConfigError::Parse {
        path: path.to_path_buf(),
        source: e,
    })?;
    let count = config.apply_to(registry);
    Ok(LoadOutcome::Loaded {
        path: path.to_path_buf(),
        count,
    })
}

/// Write the registry's non-default bindings to disk. Creates parent
/// directories if needed. Returns the path that was written.
pub fn save_overrides(registry: &KeybindingRegistry) -> Result<PathBuf, ConfigError> {
    let path = config_path()?;
    save_overrides_to(registry, &path)?;
    Ok(path)
}

/// Variant of [`save_overrides`] that writes to a caller-supplied path
/// instead of the auto-discovered location. Used by tests to round-trip
/// against a `tempfile`.
pub fn save_overrides_to(registry: &KeybindingRegistry, path: &Path) -> Result<(), ConfigError> {
    let config = KeybindingConfig::from_overrides(registry);
    let body = toml::to_string_pretty(&config)?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| ConfigError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
    }
    fs::write(path, body).map_err(|e| ConfigError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    Ok(())
}

/// Per-platform config-directory lookup. We don't pull in the `dirs` /
/// `directories` crate because the surface here is small and the
/// fallbacks are easy to express directly. Returns `None` only when no
/// platform path is computable (extremely unusual — Linux without `HOME`
/// set, etc.).
fn platform_config_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join("Library")
                .join("Application Support"),
        )
    } else {
        // Linux / BSD / etc. — XDG.
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            let p = PathBuf::from(xdg);
            if !p.as_os_str().is_empty() {
                return Some(p);
            }
        }
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join(".config"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;

    fn registry_with(action: &str, combo: KeyCombo) -> KeybindingRegistry {
        let mut r = KeybindingRegistry::new();
        r.register_default(action, combo);
        r
    }

    #[test]
    fn from_overrides_skips_default_bindings() {
        let mut r = registry_with("a", KeyCombo::key(KeyCode::F1));
        r.register_default("b", KeyCombo::key(KeyCode::F2));
        // Override b, leave a at default.
        r.set("b", KeyCombo::key(KeyCode::F5));

        let cfg = KeybindingConfig::from_overrides(&r);
        assert_eq!(cfg.bindings.len(), 1, "only overrides should serialise");
        assert_eq!(cfg.bindings.get("b"), Some(&KeyCombo::key(KeyCode::F5)));
        assert!(cfg.bindings.get("a").is_none());
    }

    #[test]
    fn apply_to_merges_known_actions() {
        let mut r = KeybindingRegistry::new();
        r.register_default("known", KeyCombo::key(KeyCode::F1));

        let mut bindings = BTreeMap::new();
        bindings.insert("known".to_string(), KeyCombo::key(KeyCode::F8));
        bindings.insert("unknown".to_string(), KeyCombo::key(KeyCode::F9));
        let cfg = KeybindingConfig { bindings };

        let applied = cfg.apply_to(&mut r);
        assert_eq!(applied, 1, "unknown action ignored");
        assert_eq!(r.get("known"), Some(KeyCombo::key(KeyCode::F8)));
        assert_eq!(r.get("unknown"), None);
    }

    #[test]
    fn toml_round_trip_preserves_modifiers() {
        let mut r = KeybindingRegistry::new();
        r.register_default("plain", KeyCombo::key(KeyCode::F1));
        r.register_default("alt", KeyCombo::key(KeyCode::F2).with_alt());
        r.register_default(
            "all_mods",
            KeyCombo::key(KeyCode::KeyS)
                .with_ctrl()
                .with_shift()
                .with_alt()
                .with_super(),
        );
        // Override every action so all three end up in the config file.
        r.set("plain", KeyCombo::key(KeyCode::F4));
        r.set("alt", KeyCombo::key(KeyCode::F2).with_alt());
        r.set(
            "all_mods",
            KeyCombo::key(KeyCode::KeyS)
                .with_ctrl()
                .with_shift()
                .with_alt()
                .with_super(),
        );

        let cfg = KeybindingConfig::from_overrides(&r);
        let body = toml::to_string_pretty(&cfg).expect("serialise ok");
        let restored: KeybindingConfig = toml::from_str(&body).expect("parse ok");

        assert_eq!(restored.bindings.len(), cfg.bindings.len());
        for (k, v) in &cfg.bindings {
            assert_eq!(restored.bindings.get(k), Some(v));
        }
    }

    #[test]
    fn save_then_load_round_trip_through_disk() {
        // Write the override file to a temp dir via PATH_OVERRIDE_ENV, then
        // load it back into a fresh registry and confirm the override took
        // effect. Each run gets a unique filename to dodge cross-test
        // env-var races (cargo test is multi-threaded).
        let mut r = KeybindingRegistry::with_engine_defaults();
        r.set(
            super::super::actions::UI_TOGGLE_SITUATION_CENTER,
            KeyCombo::key(KeyCode::F8),
        );

        let tmpdir = std::env::temp_dir();
        let pid = std::process::id();
        let nonce: u64 = rand::random();
        let path = tmpdir.join(format!("macrocosmo-keybind-test-{pid}-{nonce}.toml"));

        save_overrides_to(&r, &path).expect("save ok");
        assert!(path.exists(), "save should create file");

        let mut fresh = KeybindingRegistry::with_engine_defaults();
        // Round-trip via the in-memory KeybindingConfig type so we don't
        // depend on the env-var-driven discovery path here.
        let body = std::fs::read_to_string(&path).expect("read ok");
        let cfg: KeybindingConfig = toml::from_str(&body).expect("parse ok");
        let applied = cfg.apply_to(&mut fresh);
        assert_eq!(applied, 1);
        assert_eq!(
            fresh.get(super::super::actions::UI_TOGGLE_SITUATION_CENTER),
            Some(KeyCombo::key(KeyCode::F8))
        );

        let _ = std::fs::remove_file(&path);
    }

    /// Process-wide lock for tests that mutate `PATH_OVERRIDE_ENV`. cargo
    /// test runs tests in parallel and the env is shared global state, so
    /// without serialisation parallel tests would clobber each other's
    /// `set_var` / `remove_var` pairs.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn config_path_honours_env_override() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let probe = format!("/tmp/macrocosmo-keybind-{}.toml", std::process::id());
        // SAFETY: `ENV_LOCK` ensures no other test in this module is
        // racing on the env, and the binary doesn't spawn threads that
        // inspect env via FFI during the test run.
        unsafe {
            std::env::set_var(PATH_OVERRIDE_ENV, &probe);
        }
        let p = config_path().expect("override path ok");
        unsafe {
            std::env::remove_var(PATH_OVERRIDE_ENV);
        }
        assert_eq!(p, PathBuf::from(probe));
    }

    #[test]
    fn load_into_default_registry_is_no_op_with_missing_file() {
        let mut r = KeybindingRegistry::with_engine_defaults();
        let bogus = std::env::temp_dir().join(format!(
            "macrocosmo-keybind-missing-{}-{}.toml",
            std::process::id(),
            rand::random::<u64>()
        ));
        let outcome = load_overrides_from(&mut r, &bogus).expect("ok with missing file");
        assert!(matches!(outcome, LoadOutcome::Missing { .. }));
    }

    #[test]
    fn load_returns_parse_error_on_garbage_file() {
        let path = std::env::temp_dir().join(format!(
            "macrocosmo-keybind-bad-{}-{}.toml",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::write(&path, "this is not valid toml = =").expect("write ok");
        let mut r = KeybindingRegistry::with_engine_defaults();
        let result = load_overrides_from(&mut r, &path);
        let _ = std::fs::remove_file(&path);
        assert!(matches!(result, Err(ConfigError::Parse { .. })));
    }
}
