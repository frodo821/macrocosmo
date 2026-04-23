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
//!
//! Phase 4 adds `OnExit(GameState::InGame)` cleanup so a running scene can
//! be torn down and re-entered (InGame → NewGame → InGame) without stale
//! entities or resource state leaking across the boundary. Two system
//! families run on exit:
//!
//! - [`cleanup_ingame_entities`] — despawns every entity tagged
//!   `SaveableMarker`. This marker is carried by every world-spawned
//!   entity (StarSystem / Planet / Colony / Ship / Empire / Ruler / …),
//!   so despawning them clears the game world wholesale.
//! - [`reset_ingame_resources`] — resets tick-accumulated resources
//!   (`GameClock`, research/production tick counters, event id
//!   allocators, pending-fact queues, faction relations, AI bus state,
//!   notification queue, event log) to their `Default` values, and
//!   removes one-shot resources (`GalaxyConfig`, `HomeSystemAssignments`)
//!   that `OnEnter(NewGame)` will re-insert from scratch. Registries and
//!   the Lua [`ScriptEngine`] are **not** reset — they hold content
//!   loaded at bootstrap that stays valid across scene re-entry.

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

/// Phase 4 `OnExit(GameState::InGame)` system — despawn every entity that
/// carries a [`crate::persistence::SaveableMarker`]. The marker is attached
/// to every world-spawned game entity (via `SaveableMarkerPlugin` and
/// world-spawn hooks), so a single query clears the entire game world
/// without per-module teardown hooks.
///
/// Entities created outside the game world (camera, egui contexts, UI
/// state holders) do not carry the marker and are preserved. Visual
/// cleanup for non-saveable sprites lives alongside the visualization
/// plugin — see [`crate::visualization::cleanup_star_visuals`].
pub fn cleanup_ingame_entities(
    mut commands: Commands,
    entities: Query<Entity, With<crate::persistence::SaveableMarker>>,
) {
    let mut count = 0usize;
    for e in &entities {
        commands.entity(e).despawn();
        count += 1;
    }
    if count > 0 {
        info!(
            "cleanup_ingame_entities: despawned {} saveable entit{}",
            count,
            if count == 1 { "y" } else { "ies" }
        );
    }
}

/// Phase 4 `OnExit(GameState::InGame)` system — reset tick-accumulated
/// resources to their `Default` values and remove one-shot resources
/// that `OnEnter(GameState::NewGame)` will re-insert.
///
/// The partition between "reset to default" and "remove entirely" is
/// deliberate:
/// - **Reset**: resources that `init_resource`d at plugin build time
///   (via `app.init_resource` / `app.insert_resource` in plugin `build`).
///   Removing them would leave systems that read them to panic; resetting
///   to `Default` mirrors a fresh-bootstrap world.
/// - **Remove**: resources that are inserted by a world-spawn system
///   ([`crate::galaxy::GalaxyConfig`],
///   [`crate::galaxy::HomeSystemAssignments`]). `OnEnter(NewGame)` will
///   insert them again; leaving stale copies would tie the next scene to
///   the previous galaxy's parameters.
///
/// Registries (`BuildingRegistry`, `ShipDesignRegistry`, `ScriptEngine`,
/// etc.) are **not** touched — they hold script-loaded content that is
/// valid across any number of scene re-entries. Likewise CLI-derived
/// context (`NewGameParams`, `LoadSaveRequest`, `ObserverMode`, `RngSeed`,
/// `AiPlayerMode`) is preserved.
pub fn reset_ingame_resources(world: &mut World) {
    // --- Time / tick bookkeeping ---
    world.insert_resource(crate::time_system::GameClock::new(0));
    world.insert_resource(crate::time_system::GameSpeed::default());
    world.insert_resource(crate::colony::LastProductionTick(0));
    world.insert_resource(crate::technology::LastResearchTick(0));

    // --- Knowledge / event allocators ---
    world.insert_resource(crate::knowledge::NextEventId::default());
    world.insert_resource(crate::knowledge::NotifiedEventIds::default());
    world.insert_resource(crate::knowledge::DestroyedShipRegistry::default());
    world.insert_resource(crate::knowledge::PendingFactQueue::default());
    world.insert_resource(crate::knowledge::RelayNetwork::default());

    // --- Communication (remote-command bridge) ---
    world.insert_resource(crate::communication::NextRemoteCommandId::default());
    world.insert_resource(crate::communication::AppliedCommandIds::default());

    // --- Ship routing (async task bookkeeping) ---
    world.insert_resource(crate::ship::routing::RouteCalculationsPending::default());

    // --- Faction diplomacy state ---
    world.insert_resource(crate::faction::FactionRelations::default());
    world.insert_resource(crate::faction::HostileFactions::default());
    world.insert_resource(crate::faction::KnownFactions::default());

    // --- AI bus state ---
    // Reset only when present so tests that build without AiPlugin still
    // exercise the cleanup without needing to register the resources.
    if world.contains_resource::<crate::ai::AiBusResource>() {
        world.insert_resource(crate::ai::AiBusResource::default());
    }
    if world.contains_resource::<crate::ai::plugin::DeclaredFactionSlots>() {
        world.insert_resource(crate::ai::plugin::DeclaredFactionSlots::default());
    }

    // --- Event log / notifications ---
    world.insert_resource(crate::events::EventLog::default());
    world.insert_resource(crate::notifications::NotificationQueue::new());

    // --- One-shot world-spawn resources (re-inserted by OnEnter(NewGame)) ---
    world.remove_resource::<crate::galaxy::GalaxyConfig>();
    world.remove_resource::<crate::galaxy::HomeSystemAssignments>();

    info!("reset_ingame_resources: tick-accumulated resources cleared for scene re-entry");
}
