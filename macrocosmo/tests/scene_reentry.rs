//! #439 Phase 4 — `OnExit(GameState::InGame)` scene teardown regression.
//!
//! Validates the two behaviours that make scene re-entry
//! (`InGame → NewGame → InGame`) safe:
//!
//! 1. `cleanup_ingame_entities` despawns every entity tagged
//!    [`macrocosmo::persistence::SaveableMarker`].
//! 2. `reset_ingame_resources` restores tick-accumulated resources
//!    (`GameClock`, event allocators, faction relations, etc.) to their
//!    `Default` values, and removes the one-shot `GalaxyConfig` /
//!    `HomeSystemAssignments` resources that the next `OnEnter(NewGame)`
//!    will re-insert.
//!
//! The test deliberately avoids the full production plugin chain: a
//! `ScriptingPlugin` bring-up per test would dominate the test wall-clock
//! and couple this regression to Lua loading. Instead we mount the
//! minimum wiring needed to exercise the exit handlers registered by
//! `GameSetupPlugin` — the scene-teardown contract itself.
//!
//! For the full-stack "does NewGame actually respawn a galaxy after
//! teardown" question we rely on the existing
//! `npc_empires_in_player_mode` / `scene_load_save` tests; the
//! teardown→spawn coupling is thin (it goes through Bevy's state
//! transition machinery with no additional glue) and is covered by
//! compile-time system-registration alone.

#![allow(dead_code)]

mod common;

use bevy::ecs::schedule::ApplyDeferred;
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use macrocosmo::ai::AiBusResource;
use macrocosmo::ai::plugin::DeclaredFactionSlots;
use macrocosmo::colony::LastProductionTick;
use macrocosmo::communication::{AppliedCommandIds, NextRemoteCommandId};
use macrocosmo::events::EventLog;
use macrocosmo::faction::{FactionRelations, HostileFactions};
use macrocosmo::galaxy::{GalaxyConfig, HomeSystemAssignments};
use macrocosmo::game_state::{
    GameState, GameStatePlugin, cleanup_ingame_entities, reset_ingame_resources,
};
use macrocosmo::knowledge::{
    DestroyedShipRegistry, NextEventId, NotifiedEventIds, PendingFactQueue, RelayNetwork,
};
use macrocosmo::notifications::NotificationQueue;
use macrocosmo::persistence::SaveableMarker;
use macrocosmo::technology::LastResearchTick;
use macrocosmo::time_system::{GameClock, GameSpeed};

/// Build a minimal App that wires the `OnExit(GameState::InGame)` teardown
/// chain exactly the way `GameSetupPlugin` does, without the rest of the
/// production plugin stack. Tests insert the resources we want to reset
/// and spawn dummy `SaveableMarker` entities, then flip the state machine
/// to observe the teardown.
fn build_reentry_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    if !app.is_plugin_added::<StatesPlugin>() {
        app.add_plugins(StatesPlugin);
    }
    app.add_plugins(GameStatePlugin);

    // Seed default state so `OnExit(InGame)` actually fires when we
    // transition away. `GameStatePlugin` sets the initial state to
    // `Bootstrapping`; we insert `InGame` directly so the next
    // `NextState::set` triggers an exit.
    app.insert_state(GameState::InGame);

    // Register the same exit chain `GameSetupPlugin` builds. Mirrored
    // literally so a future refactor of the plugin surface has to update
    // this test too — the teardown contract and its consumer are on the
    // same failure path.
    app.add_systems(
        OnExit(GameState::InGame),
        (
            cleanup_ingame_entities,
            ApplyDeferred,
            reset_ingame_resources,
        )
            .chain(),
    );
    // Note: `cleanup_star_visuals` is registered by `VisualizationPlugin`
    // in production and operates on a `pub(super)` marker type that the
    // test harness cannot spawn anyway. Coverage for that system lives
    // with its plugin; here we focus on the `GameSetupPlugin` teardown
    // contract.

    app
}

/// Seed the tick-accumulated resources that the reset system touches, so
/// we can assert they actually get reset rather than just happening to be
/// at their default values because nobody populated them.
fn seed_tick_resources(world: &mut World) {
    world.insert_resource(GameClock::new(42));
    world.insert_resource(GameSpeed {
        hexadies_per_second: 5.0,
        previous_speed: 2.0,
    });
    world.insert_resource(LastProductionTick(42));
    world.insert_resource(LastResearchTick(42));

    // Knowledge allocators — `NextEventId` is a counter resource that
    // exposes `allocate()`; bump it a few times to prove the reset.
    let mut next_id = NextEventId::default();
    let _ = next_id.allocate();
    let _ = next_id.allocate();
    let _ = next_id.allocate();
    world.insert_resource(next_id);

    world.insert_resource(NotifiedEventIds::default());
    world.insert_resource(DestroyedShipRegistry::default());
    world.insert_resource(PendingFactQueue::default());
    world.insert_resource(RelayNetwork::default());

    world.insert_resource(NextRemoteCommandId(99));
    world.insert_resource(AppliedCommandIds::default());

    world.insert_resource(FactionRelations::default());
    world.insert_resource(HostileFactions::default());
    // #464: KnownFactions is now a per-empire Component, not a Resource —
    // scene_reentry.rs does not spawn empires so nothing to insert.

    // EventLog: push a fake entry so we can assert the reset wipes it.
    let mut log = EventLog::default();
    let mut nid = NextEventId::default();
    log.entries.push(macrocosmo::events::GameEvent::new(
        &mut nid,
        1,
        macrocosmo::events::GameEventKind::ResourceAlert,
        "seeded".into(),
        None,
    ));
    world.insert_resource(log);

    world.insert_resource(NotificationQueue::new());

    // One-shot world-spawn resources — removed on exit.
    world.insert_resource(GalaxyConfig {
        radius: 100.0,
        num_systems: 7,
    });
    world.insert_resource(HomeSystemAssignments::default());
}

/// Pump frames so `StateTransition` has a chance to fire queued
/// transitions. Bevy runs `StateTransition` once per frame, so multiple
/// updates are needed to settle on state change.
fn pump(app: &mut App, n: usize) {
    for _ in 0..n {
        app.update();
    }
}

/// Count every entity carrying `SaveableMarker`. Used to check that the
/// cleanup system despawns them all.
fn count_saveable(app: &mut App) -> usize {
    let mut q = app
        .world_mut()
        .query_filtered::<Entity, With<SaveableMarker>>();
    q.iter(app.world()).count()
}

#[test]
fn cleanup_ingame_entities_despawns_saveable_marker_entities() {
    let mut app = build_reentry_app();

    // Spawn a mix of saveable and non-saveable entities.
    let saveable_count = 5usize;
    for _ in 0..saveable_count {
        app.world_mut().spawn(SaveableMarker);
    }
    // Non-saveable (e.g. camera / UI) must survive the teardown.
    let survivor = app.world_mut().spawn_empty().id();

    assert_eq!(
        count_saveable(&mut app),
        saveable_count,
        "precondition: saveable entities were spawned"
    );

    // Trigger scene exit.
    app.world_mut()
        .resource_mut::<NextState<GameState>>()
        .set(GameState::NewGame);
    pump(&mut app, 3);

    assert_eq!(
        count_saveable(&mut app),
        0,
        "cleanup_ingame_entities must despawn every SaveableMarker entity on OnExit(InGame)"
    );
    assert!(
        app.world().get_entity(survivor).is_ok(),
        "non-saveable entities (camera / UI) must survive the teardown"
    );
}

#[test]
fn reset_ingame_resources_restores_defaults_and_removes_one_shot_resources() {
    let mut app = build_reentry_app();
    seed_tick_resources(app.world_mut());

    // Sanity: seeding landed.
    assert_eq!(app.world().resource::<GameClock>().elapsed, 42);
    assert!(app.world().get_resource::<GalaxyConfig>().is_some());

    // Trigger scene exit.
    app.world_mut()
        .resource_mut::<NextState<GameState>>()
        .set(GameState::NewGame);
    pump(&mut app, 3);

    // Tick-accumulated resources: back to Default.
    assert_eq!(
        app.world().resource::<GameClock>().elapsed,
        0,
        "GameClock must be reset to 0 on scene exit"
    );
    assert_eq!(
        app.world().resource::<GameSpeed>().hexadies_per_second,
        0.0,
        "GameSpeed must be reset to default (paused)"
    );
    assert_eq!(app.world().resource::<LastProductionTick>().0, 0);
    assert_eq!(app.world().resource::<LastResearchTick>().0, 0);
    assert_eq!(app.world().resource::<NextRemoteCommandId>().0, 0);
    assert_eq!(
        app.world().resource::<EventLog>().entries.len(),
        0,
        "EventLog entries must be cleared on scene exit"
    );

    // One-shot resources: removed entirely so the next NewGame re-inserts
    // them from generate_galaxy.
    assert!(
        app.world().get_resource::<GalaxyConfig>().is_none(),
        "GalaxyConfig must be removed on scene exit (OnEnter(NewGame) re-inserts it)"
    );
    assert!(
        app.world()
            .get_resource::<HomeSystemAssignments>()
            .is_none(),
        "HomeSystemAssignments must be removed on scene exit"
    );
}

#[test]
fn ai_resources_reset_only_when_present() {
    // `reset_ingame_resources` is coded defensively: it only touches
    // `AiBusResource` / `DeclaredFactionSlots` if they exist, so a test
    // / scenario that never mounts `AiPlugin` still exits cleanly. This
    // test exercises both sides of that branch.

    // Case A — AI resources not present. Reset must not panic.
    {
        let mut app = build_reentry_app();
        app.world_mut()
            .resource_mut::<NextState<GameState>>()
            .set(GameState::NewGame);
        pump(&mut app, 3);
        assert!(app.world().get_resource::<AiBusResource>().is_none());
        assert!(app.world().get_resource::<DeclaredFactionSlots>().is_none());
    }

    // Case B — AI resources present. Reset swaps them for fresh defaults.
    {
        let mut app = build_reentry_app();
        let mut slots = DeclaredFactionSlots::default();
        slots.0.insert(app.world_mut().spawn_empty().id());
        app.world_mut().insert_resource(slots);
        app.world_mut().insert_resource(AiBusResource::default());

        assert_eq!(
            app.world().resource::<DeclaredFactionSlots>().0.len(),
            1,
            "precondition: seeded DeclaredFactionSlots has one entry"
        );

        app.world_mut()
            .resource_mut::<NextState<GameState>>()
            .set(GameState::NewGame);
        pump(&mut app, 3);

        assert_eq!(
            app.world().resource::<DeclaredFactionSlots>().0.len(),
            0,
            "DeclaredFactionSlots must be reset to empty default"
        );
    }
}

#[test]
fn scene_reenters_with_fresh_state() {
    // Full scene reentry loop: InGame → NewGame → InGame → NewGame → ...
    //
    // This is the headline regression for the Phase 4 contract: after a
    // scene exit, the world must look identical to a freshly-booted
    // game. We simulate the "game ran for a while" state by seeding
    // tick-accumulated resources and saveable entities, then assert the
    // teardown brings us back to the baseline. A second round-trip
    // proves the cleanup itself doesn't accumulate state.
    let mut app = build_reentry_app();
    seed_tick_resources(app.world_mut());
    for _ in 0..3 {
        app.world_mut().spawn(SaveableMarker);
    }

    // Round 1: InGame -> NewGame (exit fires).
    app.world_mut()
        .resource_mut::<NextState<GameState>>()
        .set(GameState::NewGame);
    pump(&mut app, 3);
    assert_eq!(count_saveable(&mut app), 0);
    assert_eq!(app.world().resource::<GameClock>().elapsed, 0);
    assert!(app.world().get_resource::<GalaxyConfig>().is_none());

    // Simulate a second play-through: manually re-enter InGame, seed
    // accumulated state again, then exit once more.
    app.world_mut()
        .resource_mut::<NextState<GameState>>()
        .set(GameState::InGame);
    pump(&mut app, 2);
    seed_tick_resources(app.world_mut());
    for _ in 0..2 {
        app.world_mut().spawn(SaveableMarker);
    }
    assert_eq!(app.world().resource::<GameClock>().elapsed, 42);
    assert_eq!(count_saveable(&mut app), 2);

    // Round 2: InGame -> NewGame (exit fires again).
    app.world_mut()
        .resource_mut::<NextState<GameState>>()
        .set(GameState::NewGame);
    pump(&mut app, 3);

    assert_eq!(
        count_saveable(&mut app),
        0,
        "scene re-entry: second exit must also despawn SaveableMarker entities"
    );
    assert_eq!(
        app.world().resource::<GameClock>().elapsed,
        0,
        "scene re-entry: second exit must reset GameClock"
    );
    assert!(
        app.world().get_resource::<GalaxyConfig>().is_none(),
        "scene re-entry: second exit must remove GalaxyConfig"
    );
}
