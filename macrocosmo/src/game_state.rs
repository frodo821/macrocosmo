//! #439 — Bevy `States` driving scene-level game lifecycle.
//!
//! `GameState` is the scene-granularity state machine: it distinguishes
//! "engine is still booting up" from "game is being constructed" from
//! "game is running". Pause is intentionally **not** modelled here — it
//! continues to live on [`crate::time_system::GameSpeed`] (the speed-0
//! convention is preserved).
//!
//! ### Transition flow
//! ```text
//! Bootstrapping ──(scripts loaded)──┬── NewGame    ──(world built)──┐
//!                                   └── LoadingSave ──(save applied)─┴── InGame
//! ```
//!
//! The middle states (`NewGame` / `LoadingSave`) read the context
//! resources [`NewGameParams`] / [`LoadSaveRequest`] respectively — this
//! is how the "how did we get here" information is expressed without
//! putting data on the enum variants (Bevy `States` requires
//! `Clone + Eq + Hash`, and keeping context out of the enum also makes
//! it trivial to read via ordinary `SystemParam`s).
//!
//! Phase 3 migrated world-spawn systems from `Startup` to
//! `OnEnter(GameState::NewGame)`; see `galaxy/player/colony/faction/
//! knowledge/ai/observer/setup` plugins. `OnEnter(GameState::LoadingSave)`
//! remains a flow-through stub until the save-apply pipeline lands.

use std::path::PathBuf;

use bevy::prelude::*;

/// Scene-level state machine for the game lifecycle. See module docs.
#[derive(States, Default, Debug, Clone, Eq, PartialEq, Hash)]
pub enum GameState {
    /// Boot phase: Lua scripts / registries loading, no game instance yet.
    #[default]
    Bootstrapping,
    /// Constructing a fresh game world from [`NewGameParams`].
    NewGame,
    /// Restoring a game world from [`LoadSaveRequest`].
    LoadingSave,
    /// Active gameplay — game-tick systems run here.
    InGame,
}

/// Parameters for a new-game construction pass. Populated from the CLI
/// (or, in the future, from a main-menu screen) **before** transitioning
/// into [`GameState::NewGame`]. Consumed by world-spawn systems.
#[derive(Resource, Clone, Debug, Default)]
pub struct NewGameParams {
    /// Deterministic RNG seed for galaxy generation. `None` = random.
    pub seed: Option<u64>,
    /// Scenario identifier selected from a future scenario picker.
    pub scenario_id: Option<String>,
    /// Run without a `PlayerEmpire` — one full NPC empire per faction.
    pub observer_mode: bool,
    /// Override the player-faction id (future main-menu selector).
    pub faction_override: Option<String>,
}

/// Request to restore a game from a save file on disk. Inserted by
/// `--load <path>` (or a future load-game screen) **before** transitioning
/// into [`GameState::LoadingSave`].
#[derive(Resource, Clone, Debug)]
pub struct LoadSaveRequest {
    pub path: PathBuf,
}

/// Run-condition shorthand: game tick is active. Prefer this over
/// `in_state(GameState::InGame)` at call sites so Phase 2's mass
/// `run_if` additions read consistently.
pub fn in_active_game() -> impl FnMut(Option<Res<State<GameState>>>) -> bool + Clone {
    in_state(GameState::InGame)
}

/// Registers [`GameState`] (via `init_state`) together with the ECS
/// machinery Bevy requires (`StatesPlugin` → `StateTransition`
/// schedule). Idempotent: if `StatesPlugin` is already present (e.g. it
/// came in via `DefaultPlugins`), we skip re-adding it so registration
/// order in `main.rs` / tests doesn't matter.
///
/// `GameSetupPlugin` adds this plugin automatically. `main.rs` adds it
/// alongside `DefaultPlugins`; tests that want game-tick systems to run
/// should seed `GameState::InGame` via `insert_state` after this plugin
/// is added.
pub struct GameStatePlugin;

impl Plugin for GameStatePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<bevy::state::app::StatesPlugin>() {
            app.add_plugins(bevy::state::app::StatesPlugin);
        }
        app.init_state::<GameState>();
    }
}
