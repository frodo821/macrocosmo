//! Integration regression test for #148 — scripts-directory resolution.
//!
//! Most of the resolution logic is covered by unit tests against the pure
//! [`resolve_scripts_dir_from`] kernel. This binary exercises the real
//! env-var-driven path (`try_resolve_scripts_dir` + `ScriptEngine::new`) to
//! make sure the kernel is wired up correctly to `std::env`.
//!
//! The test mutates a process-global env var, so the file contains a single
//! `#[test]` function that serializes its scenarios under a `Mutex`. Cargo
//! runs integration-test files in separate binaries, so this test cannot
//! race with unit tests inside the main crate.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use macrocosmo::scripting::{try_resolve_scripts_dir, ScriptEngine, SCRIPTS_DIR_ENV_VAR};

static LOCK: Mutex<()> = Mutex::new(());

fn scratch_scripts_dir(name: &str) -> (PathBuf, PathBuf) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let base = std::env::temp_dir().join(format!(
        "macrocosmo-scripts-it-{}-{}-{}",
        name,
        std::process::id(),
        nanos,
    ));
    let scripts = base.join("scripts");
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&scripts).unwrap();
    fs::write(scripts.join("init.lua"), "-- test\n").unwrap();
    (base, scripts)
}

#[test]
fn env_var_drives_real_resolution() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var(SCRIPTS_DIR_ENV_VAR).ok();

    let (_root, scripts) = scratch_scripts_dir("envvar");
    // SAFETY: guarded by LOCK; we restore before returning.
    unsafe {
        std::env::set_var(SCRIPTS_DIR_ENV_VAR, &scripts);
    }
    let resolved = try_resolve_scripts_dir().expect("env var override should succeed");
    let engine_scripts = ScriptEngine::new()
        .expect("engine boots with env override")
        .scripts_dir()
        .to_path_buf();

    // Restore.
    unsafe {
        match prev {
            Some(v) => std::env::set_var(SCRIPTS_DIR_ENV_VAR, v),
            None => std::env::remove_var(SCRIPTS_DIR_ENV_VAR),
        }
    }

    assert_eq!(resolved, scripts);
    assert_eq!(engine_scripts, scripts);
}

#[test]
fn explicit_scripts_dir_constructor_ignores_env() {
    // `new_with_scripts_dir` must take the caller's path verbatim even when
    // the env var points elsewhere — callers that use it are explicitly
    // opting out of auto-resolution.
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var(SCRIPTS_DIR_ENV_VAR).ok();

    let (_root_env, env_scripts) = scratch_scripts_dir("explicit-env");
    let (_root_arg, arg_scripts) = scratch_scripts_dir("explicit-arg");

    unsafe {
        std::env::set_var(SCRIPTS_DIR_ENV_VAR, &env_scripts);
    }
    let engine = ScriptEngine::new_with_scripts_dir(arg_scripts.clone()).unwrap();
    let got = engine.scripts_dir().to_path_buf();

    unsafe {
        match prev {
            Some(v) => std::env::set_var(SCRIPTS_DIR_ENV_VAR, v),
            None => std::env::remove_var(SCRIPTS_DIR_ENV_VAR),
        }
    }

    assert_eq!(got, arg_scripts);
}
