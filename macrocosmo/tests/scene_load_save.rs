//! Integration tests for the `OnEnter(GameState::LoadingSave)` save-apply
//! pipeline (#439 Phase 3 Agent 2).
//!
//! Exercises the thin state-machine glue around
//! [`macrocosmo::setup::perform_load`]:
//!
//! - A valid `LoadSaveRequest` causes the state to transition
//!   `LoadingSave → InGame` after the save is applied to the world.
//! - A missing file causes the state to fall back to `NewGame` (no
//!   partially-applied world, no panic).
//! - A missing `LoadSaveRequest` resource also falls back to `NewGame`
//!   (defensive branch).
//!
//! The full Phase 3 `GameSetupPlugin` requires Lua/script plugins we do
//! not want to bring up in this scene test; here we register only
//! `GameStatePlugin` + the `OnEnter(LoadingSave)` system under test so the
//! contract is exercised in isolation.

#![allow(dead_code)]

mod common;

use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use common::fixture::fixtures_dir;
use macrocosmo::game_state::{GameState, GameStatePlugin, LoadSaveRequest};
use macrocosmo::setup::perform_load;
use macrocosmo::time_system::GameClock;

/// Build a minimal App that has the `GameState` state machine and the
/// `OnEnter(GameState::LoadingSave)` handler wired up, but none of the
/// other world-spawn plugins. Tests drive the state machine by inserting
/// `NextState<GameState>::LoadingSave` and calling `app.update()`.
fn build_loadingsave_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    if !app.is_plugin_added::<StatesPlugin>() {
        app.add_plugins(StatesPlugin);
    }
    app.add_plugins(GameStatePlugin);
    app.add_systems(OnEnter(GameState::LoadingSave), perform_load);
    app
}

/// Pump a few frames so `StateTransition` has a chance to fire any
/// queued transitions. Bevy runs `StateTransition` once per frame, so
/// multiple updates may be needed to settle after `NextState::set`.
fn pump(app: &mut App, n: usize) {
    for _ in 0..n {
        app.update();
    }
}

/// A valid `LoadSaveRequest` pointing at the committed `minimal_game.bin`
/// fixture drives the state machine through `LoadingSave → InGame` and
/// restores the saved `GameClock`.
#[test]
fn load_save_request_drives_loading_save_to_ingame() {
    let mut app = build_loadingsave_app();
    let fixture = fixtures_dir().join("minimal_game.bin");
    app.world_mut()
        .insert_resource(LoadSaveRequest { path: fixture });

    // Kick the state machine from the default `Bootstrapping` into
    // `LoadingSave`. `perform_load` is an `OnEnter(LoadingSave)` system,
    // so it runs once the StateTransition schedule processes the queued
    // NextState. The system itself sets NextState to InGame on success.
    app.world_mut()
        .resource_mut::<NextState<GameState>>()
        .set(GameState::LoadingSave);
    pump(&mut app, 3);

    let state = app.world().resource::<State<GameState>>();
    assert_eq!(
        state.get(),
        &GameState::InGame,
        "successful load must transition LoadingSave → InGame"
    );

    // `minimal_game.bin` was seeded with GameClock::new(123); verify the
    // save actually landed in the world rather than just the transition
    // firing.
    let clock = app.world().resource::<GameClock>();
    assert_eq!(
        clock.elapsed, 123,
        "GameClock must be restored from the minimal_game fixture"
    );
}

/// A `LoadSaveRequest` with a path that does not exist must surface as an
/// error and fall back to `GameState::NewGame` rather than leaving the
/// world in `LoadingSave` or advancing into a half-loaded `InGame`.
#[test]
fn missing_save_file_falls_back_to_new_game() {
    let mut app = build_loadingsave_app();
    let missing = std::path::PathBuf::from("/nonexistent/macrocosmo/scene_load_save_missing.bin");
    app.world_mut()
        .insert_resource(LoadSaveRequest { path: missing });

    app.world_mut()
        .resource_mut::<NextState<GameState>>()
        .set(GameState::LoadingSave);
    pump(&mut app, 3);

    let state = app.world().resource::<State<GameState>>();
    assert_eq!(
        state.get(),
        &GameState::NewGame,
        "missing save must fall back to NewGame, not advance to InGame"
    );
}

/// Defensive branch: if `OnEnter(LoadingSave)` fires without a
/// `LoadSaveRequest` resource inserted, `perform_load` logs a warning and
/// falls back to `NewGame` instead of panicking.
#[test]
fn missing_load_request_falls_back_to_new_game() {
    let mut app = build_loadingsave_app();
    // Note: no LoadSaveRequest inserted.

    app.world_mut()
        .resource_mut::<NextState<GameState>>()
        .set(GameState::LoadingSave);
    pump(&mut app, 3);

    let state = app.world().resource::<State<GameState>>();
    assert_eq!(
        state.get(),
        &GameState::NewGame,
        "missing LoadSaveRequest must fall back to NewGame without panic"
    );
}
