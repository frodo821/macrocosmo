//! #488: dispatcher-side `ShipProjection` defensive write.
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
//! Fix: `dispatch_queued_commands` writes a defensive projection at
//! the queue-processing tick whenever no projection already exists for
//! the ship's owning empire. The 3 caller-side writes are kept intact
//! (they have access to the exact dispatch tick + Ruler position the
//! dispatcher cannot recover post-hoc); the dispatcher write only
//! covers the bypass paths.
//!
//! Tests:
//!
//! 1. `direct_command_queue_push_writes_projection` — bypass the 3
//!    civilised paths by directly pushing a `Survey` command into the
//!    ship's `CommandQueue`; assert
//!    `KnowledgeStore.projections.get(ship)` is `Some(_)` with
//!    `intended_state = Surveying` after one Update.
//! 2. `dispatcher_does_not_overwrite_fresh_caller_projection` —
//!    pre-populate a projection (mimicking a caller-side write); push a
//!    queued command; run dispatcher; assert the projection was NOT
//!    overwritten — the dispatcher's idempotency guard works.
//! 3. `dispatcher_handles_multiple_command_variants` — direct push of
//!    `MoveTo`, `Survey`, `Colonize` variants; assert each maps to the
//!    right `intended_state`.
//! 4. `dispatcher_skips_neutral_owner_ships` — `Owner::Neutral` ship's
//!    queue receives a direct push; assert no projection written
//!    (mirrors `seed_own_ship_projections`'s neutral skip).
//! 5. `dispatcher_skips_spatial_less_commands` — direct push of a
//!    `LoadDeliverable` (cargo op, no `ShipState` implication); assert
//!    no projection written.

mod common;

use bevy::prelude::*;

use macrocosmo::knowledge::{KnowledgeStore, ShipProjection, ShipSnapshotState};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::{CommandQueue, Owner, QueuedCommand, Ship};

use common::{spawn_test_ruler, spawn_test_ship, spawn_test_system, test_app};

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
fn dispatcher_does_not_overwrite_fresh_caller_projection() {
    let mut app = test_app();
    let (empire, home, frontier, ship) = setup_scenario(&mut app);

    // Pre-populate a "caller-written" projection with distinguishing
    // sentinel values, mimicking what one of the 3 civilised dispatch
    // sites would have written.
    let sentinel_arrival = 999_999_i64;
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 7,
            expected_arrival_at: Some(sentinel_arrival),
            expected_return_at: None,
            projected_state: ShipSnapshotState::InSystem,
            projected_system: Some(home),
            intended_state: Some(ShipSnapshotState::InTransit),
            intended_system: Some(frontier),
            intended_takes_effect_at: Some(8),
        });
    }

    // Now push a direct Survey command — dispatcher must NOT overwrite
    // the existing projection (its idempotency guard fires).
    direct_push(&mut app, ship, QueuedCommand::Survey { system: frontier });
    app.update();

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let projection = store.get_projection(ship).unwrap();
    // The caller-written sentinel survives — dispatcher saw the
    // existing entry and skipped the defensive write.
    assert_eq!(
        projection.dispatched_at, 7,
        "caller-written dispatched_at must survive (idempotency guard)"
    );
    assert_eq!(
        projection.expected_arrival_at,
        Some(sentinel_arrival),
        "caller-written expected_arrival_at must survive"
    );
    assert_eq!(
        projection.intended_state,
        Some(ShipSnapshotState::InTransit),
        "caller-written intended_state must survive — not overwritten by Surveying"
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
