//! Test-only helper for loading committed save fixtures into a fresh App.
//!
//! The #247 spec calls for test-grade fixture support: `load_fixture(path)`
//! takes a relative path under `tests/fixtures/`, reads the postcard binary,
//! decodes it into a fresh `bevy::App`, and returns the App for assertions.
//!
//! Committed fixtures (e.g. `tests/fixtures/minimal_game.bin`) let us pin
//! the save wire format — if `SAVE_VERSION` bumps or `SavedComponentBag`
//! gains a non-backwards-compatible field, the fixture decodes will fail
//! CI. To regenerate, run the `#[ignore]` test
//! `regenerate_minimal_game_fixture` in `tests/fixtures_smoke.rs`:
//!
//! ```bash
//! cargo test -p macrocosmo --test fixtures_smoke \
//!     regenerate_minimal_game_fixture -- --ignored
//! ```
//!
//! Intentionally lightweight — just enough `App` surface to read back the
//! loaded state. Tests that need the full scheduling stack should build a
//! `test_app()` and call `load_game_from_reader` directly.

use bevy::prelude::*;
use macrocosmo::persistence::load::load_game_from_reader;
use std::path::{Path, PathBuf};

/// Absolute fs path of the repo-committed `tests/fixtures/` directory.
///
/// Resolves relative to `CARGO_MANIFEST_DIR` so `cargo test` from any
/// working directory finds the fixtures.
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Load a committed fixture at `<fixtures_dir>/<rel_path>` into a fresh
/// `bevy::App` and return it. Panics on I/O or decode error — fixtures are
/// test-critical; a missing or corrupt fixture should fail loudly rather
/// than silently degrade.
pub fn load_fixture(rel_path: impl AsRef<Path>) -> App {
    let full = fixtures_dir().join(rel_path);
    let bytes = std::fs::read(&full)
        .unwrap_or_else(|e| panic!("fixture {} unreadable: {e}", full.display()));
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    load_game_from_reader(app.world_mut(), &bytes[..])
        .unwrap_or_else(|e| panic!("fixture {} decode failed: {e}", full.display()));
    app
}
