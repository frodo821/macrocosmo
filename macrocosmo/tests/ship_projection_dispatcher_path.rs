//! #488 / #492: dispatcher-side `ShipProjection` defensive write.
//!
//! Background: pre-#488, projection writes happened at the **3 civilised
//! dispatch sites** (#475):
//!
//! * AI: `ai/command_outbox.rs::dispatch_ai_pending_commands`
//! * Lua: `scripting/gamestate_scope.rs::request_command`
//! * Player: `ui/mod.rs::draw_main_panels_system` + `ui/context_menu.rs`
//!   zero-delay branches (added by #482).
//!
//! Anything that appended to `CommandQueue` **outside those 3 paths** —
//! BRP `world.mutate_components`, future plugins, tests, etc. —
//! bypassed the projection write. Result: the renderer (#477) saw an
//! empty `KnowledgeStore.projections` map and the ship vanished from
//! the Galaxy Map.
//!
//! Detected by BRP exploratory: pushing a Survey command directly into
//! `Explorer-1.CommandQueue` left `KnowledgeStore.projections == {}`.
//!
//! Fix (#488): `dispatch_queued_commands` writes a defensive projection
//! at the queue-processing tick. The 3 caller-side writes stay intact
//! (they have access to the exact dispatch tick + Ruler position the
//! dispatcher cannot recover post-hoc); the dispatcher write only
//! covers the bypass paths.
//!
//! #492 follow-up: the original guard `is_some()` was dead-on-arrival
//! once #481 (`seed_own_ship_projections`) shipped — every own-empire
//! ship has a seed projection from spawn (`intended_state: None`), so
//! the defensive write never ran. Replaced with a head-command-aware
//! match that compares the existing projection's `intended_state` /
//! `intended_system` against what the queue head implies; mismatch
//! triggers an overwrite.
//!
//! Tests:
//!
//! 1. `direct_command_queue_push_writes_projection` — bypass the 3
//!    civilised paths by directly pushing a `Survey` command into the
//!    ship's `CommandQueue`; assert
//!    `KnowledgeStore.projections.get(ship)` has
//!    `intended_state = Surveying` after one Update.
//! 2. `dispatcher_writes_over_seed_projection` — #492 regression: with
//!    `seed_own_ship_projections` installed, a seed projection exists
//!    at spawn (`intended_state: None`); a direct Survey push must
//!    cause the dispatcher to overwrite the seed.
//! 3. `dispatcher_writes_after_reconcile_clears_intended` — projection
//!    in post-reconcile steady state (`intended_*: None`, but a real
//!    `projected_state`); a new mission via direct push must trigger
//!    a dispatcher write.
//! 4. `dispatcher_preserves_caller_projection_when_head_matches` —
//!    caller wrote a projection consistent with the queue head; the
//!    dispatcher's idempotency match fires; the caller's exact
//!    light-delay numbers survive.
//! 5. `dispatcher_overwrites_when_head_diverges_from_existing` — caller
//!    wrote a projection for mission X but the queue head is mission
//!    Y; the dispatcher overwrites the stale write.
//! 6. `dispatcher_handles_multiple_command_variants` — direct push of
//!    `MoveTo`, `Survey`, `Colonize` variants; assert each maps to the
//!    right `intended_state`.
//! 7. `dispatcher_skips_neutral_owner_ships` — `Owner::Neutral` ship's
//!    queue receives a direct push; assert no projection written
//!    (mirrors `seed_own_ship_projections`'s neutral skip).
//! 8. `dispatcher_skips_spatial_less_commands` — direct push of a
//!    `LoadDeliverable` (cargo op, no `ShipState` implication); assert
//!    no projection written.

mod common;

use bevy::prelude::*;

use macrocosmo::knowledge::{
    KnowledgeStore, ShipProjection, ShipSnapshotState, seed_own_ship_projections,
};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::{CommandQueue, Owner, QueuedCommand, Ship};

use common::{spawn_test_ruler, spawn_test_ship, spawn_test_system, test_app};

/// Like `test_app` but registers `seed_own_ship_projections` so newly
/// spawned own-empire ships get a seed projection installed before the
/// dispatcher runs — mirrors the production runtime where #481 + #488
/// interact. Used by the #492 regression tests below.
fn dispatcher_test_app_with_seed() -> App {
    let mut app = test_app();
    app.add_systems(
        Update,
        seed_own_ship_projections.before(macrocosmo::ship::dispatcher::dispatch_queued_commands),
    );
    app
}

/// Build a minimal scenario: one PlayerEmpire with a Ruler at `home`,
/// one frontier system, one ship docked at `home` and owned by the
/// empire. Returns `(empire, home, frontier, ship)`.
fn setup_scenario(app: &mut App) -> (Entity, Entity, Entity, Entity) {
    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "dispatcher_path_test".into(),
                name: "Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            macrocosmo::knowledge::SystemVisibilityMap::default(),
            KnowledgeStore::default(),
            macrocosmo::empire::CommsParams::default(),
        ))
        .id();
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [3.0, 0.0, 0.0],
        1.0,
        false,
        false,
    );
    let ship = spawn_test_ship(
        app.world_mut(),
        "Explorer-1",
        "explorer_mk1",
        home,
        [0.0, 0.0, 0.0],
    );
    app.world_mut()
        .entity_mut(ship)
        .get_mut::<Ship>()
        .unwrap()
        .owner = Owner::Empire(empire);
    spawn_test_ruler(app.world_mut(), empire, home);
    (empire, home, frontier, ship)
}

/// Helper: directly push a `QueuedCommand` into the ship's queue, the
/// way BRP `world.mutate_components` would. This is the "bypass" path
/// that pre-#488 left the projection unset.
fn direct_push(app: &mut App, ship: Entity, cmd: QueuedCommand) {
    let mut q = app
        .world_mut()
        .get_mut::<CommandQueue>(ship)
        .expect("ship must have a CommandQueue");
    q.commands.push(cmd);
}

#[test]
fn direct_command_queue_push_writes_projection() {
    let mut app = test_app();
    let (empire, _home, frontier, ship) = setup_scenario(&mut app);

    // Sanity: no projection before the dispatcher runs.
    {
        let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
        assert!(
            store.get_projection(ship).is_none(),
            "no projection should exist before the dispatcher runs"
        );
    }

    // Bypass the 3 civilised paths — directly push a Survey command.
    direct_push(&mut app, ship, QueuedCommand::Survey { system: frontier });

    // Run one Update — the dispatcher fires + emits SurveyRequested,
    // and (per #488) writes a defensive projection.
    app.update();

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let projection = store
        .get_projection(ship)
        .expect("dispatcher must write a defensive ShipProjection for direct CommandQueue pushes");
    assert_eq!(projection.entity, ship);
    assert_eq!(
        projection.intended_state,
        Some(ShipSnapshotState::Surveying),
        "Survey maps to Surveying intended state"
    );
    assert_eq!(projection.intended_system, Some(frontier));
    assert!(
        projection.intended_takes_effect_at.is_some(),
        "intended_takes_effect_at must be populated"
    );
    // Survey has a return leg.
    assert!(
        projection.expected_return_at.is_some(),
        "survey commands must populate expected_return_at"
    );
}

#[test]
fn dispatcher_preserves_caller_projection_when_head_matches() {
    // #492 head-command-aware idempotency: when the caller wrote a
    // projection consistent with the current queue head (= same
    // `intended_state` and `intended_system`), the dispatcher's match
    // guard fires and the caller's exact light-delay numbers survive.
    let mut app = test_app();
    let (empire, home, frontier, ship) = setup_scenario(&mut app);

    // Pre-populate a "caller-written" projection with distinguishing
    // sentinel values, mimicking what one of the 3 civilised dispatch
    // sites would have written for a Survey of `frontier`.
    let sentinel_arrival = 999_999_i64;
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 7,
            expected_arrival_at: Some(sentinel_arrival),
            expected_return_at: Some(123_456_i64),
            projected_state: ShipSnapshotState::InSystem,
            projected_system: Some(home),
            // intended_* match what `Survey { system: frontier }` implies.
            intended_state: Some(ShipSnapshotState::Surveying),
            intended_system: Some(frontier),
            intended_takes_effect_at: Some(8),
        });
    }

    // Push the matching Survey command — dispatcher must NOT overwrite
    // the existing projection (head-command-aware match fires).
    direct_push(&mut app, ship, QueuedCommand::Survey { system: frontier });
    app.update();

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let projection = store.get_projection(ship).unwrap();
    assert_eq!(
        projection.dispatched_at, 7,
        "caller-written dispatched_at must survive — head matches existing intended"
    );
    assert_eq!(
        projection.expected_arrival_at,
        Some(sentinel_arrival),
        "caller-written expected_arrival_at must survive"
    );
    assert_eq!(
        projection.expected_return_at,
        Some(123_456_i64),
        "caller-written expected_return_at must survive"
    );
    assert_eq!(
        projection.intended_state,
        Some(ShipSnapshotState::Surveying),
        "caller-written intended_state preserved (= matches head)"
    );
}

#[test]
fn dispatcher_overwrites_when_head_diverges_from_existing() {
    // #492: if the caller-written projection's `intended_*` no longer
    // matches what the queue head implies (= a different mission was
    // pushed since the caller wrote), the dispatcher's match guard
    // does NOT fire and the stale caller write is overwritten.
    let mut app = test_app();
    let (empire, home, frontier, ship) = setup_scenario(&mut app);

    // Caller wrote a projection for a MoveTo (InTransit) to `home`.
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 7,
            expected_arrival_at: Some(999_999_i64),
            expected_return_at: None,
            projected_state: ShipSnapshotState::InSystem,
            projected_system: Some(home),
            intended_state: Some(ShipSnapshotState::InTransit),
            intended_system: Some(home),
            intended_takes_effect_at: Some(8),
        });
    }

    // Queue head is Survey of `frontier` — divergent mission.
    direct_push(&mut app, ship, QueuedCommand::Survey { system: frontier });
    app.update();

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let projection = store.get_projection(ship).unwrap();
    // Dispatcher overwrote — projection now reflects the head Survey.
    assert_eq!(
        projection.intended_state,
        Some(ShipSnapshotState::Surveying),
        "stale caller intended_state must be overwritten by head's Surveying"
    );
    assert_eq!(
        projection.intended_system,
        Some(frontier),
        "stale caller intended_system must be overwritten by head's target"
    );
    // Survey has a return leg — dispatcher write populates it.
    assert!(
        projection.expected_return_at.is_some(),
        "dispatcher write for Survey must populate expected_return_at"
    );
}

#[test]
fn dispatcher_writes_over_seed_projection() {
    // #492 regression: with `seed_own_ship_projections` installed,
    // every newly-spawned own-empire ship has a seed projection
    // (`intended_state: None`, `projected_state: InSystem`). A direct
    // CommandQueue push (BRP / plugin / test bypass) must cause the
    // dispatcher to overwrite the seed with the head command's
    // intended trajectory — otherwise the renderer (#477) sees a
    // ship "parked at home" forever.
    let mut app = dispatcher_test_app_with_seed();
    let (empire, home, frontier, ship) = setup_scenario(&mut app);

    // Direct push BEFORE the first Update — both the seed system and
    // the dispatcher run in this Update; seed runs first via
    // `.before(dispatch_queued_commands)`.
    direct_push(&mut app, ship, QueuedCommand::Survey { system: frontier });
    app.update();

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let projection = store
        .get_projection(ship)
        .expect("seed system installed a projection; dispatcher must overwrite, not skip");
    // Seed had `intended_state: None`; dispatcher overwrote with
    // Surveying — this is the literal #492 fix.
    assert_eq!(
        projection.intended_state,
        Some(ShipSnapshotState::Surveying),
        "dispatcher must overwrite seed projection's None intended_state with head's Surveying"
    );
    assert_eq!(projection.intended_system, Some(frontier));
    assert!(
        projection.intended_takes_effect_at.is_some(),
        "intended_takes_effect_at must be populated after overwrite"
    );
    assert!(
        projection.expected_return_at.is_some(),
        "Survey return leg must be populated after overwrite"
    );
    // home is unused in this assertion block, silence the binding.
    let _ = home;
}

#[test]
fn dispatcher_writes_after_reconcile_clears_intended() {
    // #492 follow-on: post-reconcile (= mission completed, fact
    // arrived, intended_* cleared) projection state. A new mission
    // pushed via direct CommandQueue must trigger a dispatcher write.
    // This is the same code path as the seed case — both have
    // `intended_state: None` — but we exercise it explicitly to lock
    // in the post-reconcile path.
    let mut app = test_app();
    let (empire, home, frontier, ship) = setup_scenario(&mut app);

    // Mimic `reconcile_ship_projections` clearing intended_* after the
    // ship completed a previous mission and arrived home.
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 5,
            expected_arrival_at: None,
            expected_return_at: None,
            projected_state: ShipSnapshotState::InSystem,
            projected_system: Some(home),
            intended_state: None,
            intended_system: None,
            intended_takes_effect_at: None,
        });
    }

    // New mission pushed directly (BRP / plugin / test bypass).
    direct_push(&mut app, ship, QueuedCommand::MoveTo { system: frontier });
    app.update();

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let projection = store
        .get_projection(ship)
        .expect("projection must persist through update");
    assert_eq!(
        projection.intended_state,
        Some(ShipSnapshotState::InTransit),
        "post-reconcile None intended_state must be overwritten by head's InTransit"
    );
    assert_eq!(
        projection.intended_system,
        Some(frontier),
        "post-reconcile None intended_system must be overwritten by head's target"
    );
}

#[test]
fn dispatcher_handles_multiple_command_variants() {
    // MoveTo ⇒ InTransit, Survey ⇒ Surveying, Colonize ⇒ Settling.
    // Each gets its own ship in the same scenario so we can assert
    // independently after one Update.
    let mut app = test_app();
    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Multi".into(),
            },
            PlayerEmpire,
            Faction {
                id: "dispatcher_multi".into(),
                name: "Multi".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            macrocosmo::knowledge::SystemVisibilityMap::default(),
            KnowledgeStore::default(),
            macrocosmo::empire::CommsParams::default(),
        ))
        .id();
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let move_target = spawn_test_system(
        app.world_mut(),
        "Move-Target",
        [3.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    let survey_target = spawn_test_system(
        app.world_mut(),
        "Survey-Target",
        [4.0, 0.0, 0.0],
        1.0,
        false,
        false,
    );
    let colonize_target = spawn_test_system(
        app.world_mut(),
        "Colonize-Target",
        [5.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    spawn_test_ruler(app.world_mut(), empire, home);

    let make_ship = |app: &mut App, name: &str| {
        let s = spawn_test_ship(app.world_mut(), name, "explorer_mk1", home, [0.0, 0.0, 0.0]);
        app.world_mut()
            .entity_mut(s)
            .get_mut::<Ship>()
            .unwrap()
            .owner = Owner::Empire(empire);
        s
    };
    let move_ship = make_ship(&mut app, "Mover");
    let survey_ship = make_ship(&mut app, "Surveyor");
    let colonize_ship = make_ship(&mut app, "Colonizer");

    direct_push(
        &mut app,
        move_ship,
        QueuedCommand::MoveTo {
            system: move_target,
        },
    );
    direct_push(
        &mut app,
        survey_ship,
        QueuedCommand::Survey {
            system: survey_target,
        },
    );
    direct_push(
        &mut app,
        colonize_ship,
        QueuedCommand::Colonize {
            system: colonize_target,
            planet: None,
        },
    );

    app.update();

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();

    let move_proj = store
        .get_projection(move_ship)
        .expect("MoveTo projection missing");
    assert_eq!(
        move_proj.intended_state,
        Some(ShipSnapshotState::InTransit),
        "MoveTo ⇒ InTransit"
    );
    assert_eq!(move_proj.intended_system, Some(move_target));

    let survey_proj = store
        .get_projection(survey_ship)
        .expect("Survey projection missing");
    assert_eq!(
        survey_proj.intended_state,
        Some(ShipSnapshotState::Surveying),
        "Survey ⇒ Surveying"
    );
    assert_eq!(survey_proj.intended_system, Some(survey_target));

    let colonize_proj = store
        .get_projection(colonize_ship)
        .expect("Colonize projection missing");
    assert_eq!(
        colonize_proj.intended_state,
        Some(ShipSnapshotState::Settling),
        "Colonize ⇒ Settling"
    );
    assert_eq!(colonize_proj.intended_system, Some(colonize_target));
}

#[test]
fn dispatcher_skips_neutral_owner_ships() {
    // Neutral / hostile ships must not seed any empire's projection
    // store — mirrors `seed_own_ship_projections`'s neutral skip.
    let mut app = test_app();
    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "dispatcher_neutral".into(),
                name: "Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            macrocosmo::knowledge::SystemVisibilityMap::default(),
            KnowledgeStore::default(),
            macrocosmo::empire::CommsParams::default(),
        ))
        .id();
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [3.0, 0.0, 0.0],
        1.0,
        false,
        false,
    );
    spawn_test_ruler(app.world_mut(), empire, home);

    // spawn_test_ship leaves owner = Owner::Neutral by default.
    let neutral_ship = spawn_test_ship(
        app.world_mut(),
        "Pirate-1",
        "explorer_mk1",
        home,
        [0.0, 0.0, 0.0],
    );
    assert_eq!(
        app.world()
            .entity(neutral_ship)
            .get::<Ship>()
            .unwrap()
            .owner,
        Owner::Neutral
    );

    direct_push(
        &mut app,
        neutral_ship,
        QueuedCommand::Survey { system: frontier },
    );
    app.update();

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    assert!(
        store.get_projection(neutral_ship).is_none(),
        "Owner::Neutral ships must not seed any empire's projection"
    );
}

#[test]
fn dispatcher_skips_spatial_less_commands() {
    // Cargo / structure ops (`LoadDeliverable`, `DeployDeliverable`,
    // `TransferToStructure`, `LoadFromScrapyard`) imply no `ShipState`
    // change — the projection write must skip them. Otherwise we'd
    // create a bogus `intended_state = None` projection that the
    // renderer would try to draw with no useful info.
    let mut app = test_app();
    let (empire, home, _frontier, ship) = setup_scenario(&mut app);

    direct_push(
        &mut app,
        ship,
        QueuedCommand::LoadDeliverable {
            system: home,
            stockpile_index: 0,
        },
    );
    app.update();

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    assert!(
        store.get_projection(ship).is_none(),
        "spatial-less LoadDeliverable must not write a projection"
    );
}
