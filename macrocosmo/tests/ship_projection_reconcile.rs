//! #476: `ShipProjection` reconciler — integration tests for epic #473
//! sub-issue C.
//!
//! These tests pin the per-fact-kind reconciliation rules and the
//! per-empire isolation contract for the
//! [`reconcile_ship_projections`](macrocosmo::knowledge::reconcile_ship_projections)
//! system. Producer-side (#475) projection writes already have their own
//! coverage in `tests/ship_projection_dispatch.rs`; this file exercises
//! the consumer side: an arriving `KnowledgeFact` must update the
//! dispatcher empire's `ShipProjection` exactly as the AC describes.
//!
//! The tests mirror the helper style used by `ship_projection_dispatch.rs`:
//! a minimal `test_app()` plus a manual single-system schedule for the
//! reconciler so we exercise the unit-under-test in isolation. The
//! standard `KnowledgePlugin` wiring (which also runs `dispatch_knowledge_observed`,
//! `propagate_knowledge`, etc.) is verified end-to-end in
//! `reconcile_ordering_after_snapshot` by stepping a full `app.update()`.

mod common;

use bevy::ecs::schedule::Schedule;
use bevy::prelude::*;

use macrocosmo::components::Position;
use macrocosmo::empire::CommsParams;
use macrocosmo::knowledge::{
    KnowledgeFact, KnowledgeStore, ObservationSource, PendingFactQueue, PerceivedFact,
    RelayNetwork, ShipProjection, ShipSnapshot, ShipSnapshotState, SystemVisibilityMap,
    SystemVisibilityTier, reconcile_ship_projections,
};
use macrocosmo::physics::light_delay_hexadies;
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::{Owner, Ship};
use macrocosmo::time_system::GameClock;

use common::{spawn_test_ruler, spawn_test_ship, spawn_test_system, test_app};

// ---------------------------------------------------------------------------
// Shared scenario: empire with Ruler at "home" and a "frontier" system.
// ---------------------------------------------------------------------------

struct Scenario {
    empire: Entity,
    home: Entity,
    frontier: Entity,
    ship: Entity,
}

fn setup_scenario(app: &mut App, frontier_distance_ly: f64) -> Scenario {
    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "reconcile_test".into(),
                name: "Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            SystemVisibilityMap::default(),
            KnowledgeStore::default(),
            CommsParams::default(),
        ))
        .id();

    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [frontier_distance_ly, 0.0, 0.0],
        1.0,
        false,
        false,
    );
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut vis = em.get_mut::<SystemVisibilityMap>().unwrap();
        vis.set(home, SystemVisibilityTier::Local);
        vis.set(frontier, SystemVisibilityTier::Catalogued);
    }

    let ship = spawn_test_ship(
        app.world_mut(),
        "Scout-1",
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

    // The reconciler reads `RelayNetwork` (test_app already registers it)
    // and the global `PendingFactQueue` (also registered).

    Scenario {
        empire,
        home,
        frontier,
        ship,
    }
}

/// Seed an existing ShipProjection so we can verify the reconciler
/// updates it. Returns the dispatcher's reference projection state.
fn seed_projection(
    app: &mut App,
    empire: Entity,
    ship: Entity,
    intended_state: Option<ShipSnapshotState>,
    intended_system: Option<Entity>,
    home: Entity,
    dispatched_at: i64,
) {
    let projection = ShipProjection {
        entity: ship,
        dispatched_at,
        expected_arrival_at: Some(dispatched_at + 100),
        expected_return_at: Some(dispatched_at + 200),
        projected_state: ShipSnapshotState::InSystem,
        projected_system: Some(home),
        intended_state,
        intended_system,
        intended_takes_effect_at: Some(dispatched_at + 10),
    };
    let mut em = app.world_mut().entity_mut(empire);
    let mut store = em.get_mut::<KnowledgeStore>().unwrap();
    store.update_projection(projection);
}

/// Push a fact directly into the global queue at `arrives_at == observed_at`
/// (= the empire's own perception trigger). The reconciler recomputes
/// per-empire arrival from `origin_pos` vs viewer position so the queue's
/// stored value is not load-bearing here.
fn push_fact(app: &mut App, fact: KnowledgeFact, origin_pos: [f64; 3], observed_at: i64) {
    let pf = PerceivedFact {
        fact,
        observed_at,
        arrives_at: observed_at,
        source: ObservationSource::Direct,
        origin_pos,
        related_system: None,
    };
    app.world_mut()
        .resource_mut::<PendingFactQueue>()
        .record(pf);
}

/// Run only the reconciler in an isolated schedule so test failures
/// surface on the unit-under-test, not on incidental plugin behaviour.
fn run_reconciler(app: &mut App) {
    let mut schedule = Schedule::default();
    schedule.add_systems(reconcile_ship_projections);
    schedule.run(app.world_mut());
}

// ===========================================================================
// 1. ShipArrived → projected_state becomes InSystem at the arrival system.
//    intended_* clears when the arrival matches the intended target.
// ===========================================================================

#[test]
fn reconcile_on_ship_arrived() {
    let mut app = test_app();
    let s = setup_scenario(&mut app, 2.0);

    seed_projection(
        &mut app,
        s.empire,
        s.ship,
        Some(ShipSnapshotState::InTransitSubLight),
        Some(s.frontier),
        s.home,
        50,
    );

    // Push the arrival fact at the frontier (= 2 ly from home → 120 hd
    // light delay, but the reconciler recomputes against the empire's
    // viewer position so we set the clock past that).
    let frontier_pos = [2.0, 0.0, 0.0];
    push_fact(
        &mut app,
        KnowledgeFact::ShipArrived {
            event_id: None,
            system: Some(s.frontier),
            name: "Scout-1".into(),
            detail: "Arrived".into(),
            ship: s.ship,
        },
        frontier_pos,
        100,
    );

    // Tick past light arrival.
    app.world_mut().resource_mut::<GameClock>().elapsed = 100 + light_delay_hexadies(2.0);
    run_reconciler(&mut app);

    let store = app
        .world()
        .entity(s.empire)
        .get::<KnowledgeStore>()
        .unwrap();
    let p = store
        .get_projection(s.ship)
        .expect("projection must still exist after reconciliation");
    assert_eq!(p.projected_state, ShipSnapshotState::InSystem);
    assert_eq!(p.projected_system, Some(s.frontier));
    assert!(
        p.intended_state.is_none() && p.intended_system.is_none(),
        "intended_* must clear when the arrival matches the intended target"
    );
    assert!(
        p.expected_arrival_at.is_none() && p.expected_return_at.is_none(),
        "expected_*_at must clear once the command has visibly delivered"
    );
}

// ===========================================================================
// 2. SurveyComplete with Surveying intent → InSystem at surveyed system,
//    intended_state cleared, expected_arrival_at cleared.
// ===========================================================================

#[test]
fn reconcile_on_survey_complete() {
    let mut app = test_app();
    let s = setup_scenario(&mut app, 3.0);

    seed_projection(
        &mut app,
        s.empire,
        s.ship,
        Some(ShipSnapshotState::Surveying),
        Some(s.frontier),
        s.home,
        50,
    );

    let frontier_pos = [3.0, 0.0, 0.0];
    push_fact(
        &mut app,
        KnowledgeFact::SurveyComplete {
            event_id: None,
            system: s.frontier,
            system_name: "Frontier".into(),
            detail: "Surveyed".into(),
            ship: s.ship,
        },
        frontier_pos,
        120,
    );

    app.world_mut().resource_mut::<GameClock>().elapsed = 120 + light_delay_hexadies(3.0);
    run_reconciler(&mut app);

    let store = app
        .world()
        .entity(s.empire)
        .get::<KnowledgeStore>()
        .unwrap();
    let p = store.get_projection(s.ship).expect("projection retained");
    assert_eq!(p.projected_state, ShipSnapshotState::InSystem);
    assert_eq!(p.projected_system, Some(s.frontier));
    assert!(
        p.intended_state.is_none() && p.intended_system.is_none(),
        "Surveying intent at frontier must clear once survey completes there"
    );
    assert!(
        p.expected_arrival_at.is_none(),
        "expected_arrival_at must clear once the survey leg has been observed"
    );
}

// ===========================================================================
// 3. ShipDestroyed → projection retained with terminal marker. Confirms
//    the reconciler does NOT call `clear_projection`.
// ===========================================================================

#[test]
fn reconcile_on_ship_destroyed_retains_projection() {
    let mut app = test_app();
    let s = setup_scenario(&mut app, 4.0);

    seed_projection(
        &mut app,
        s.empire,
        s.ship,
        Some(ShipSnapshotState::InTransitSubLight),
        Some(s.frontier),
        s.home,
        80,
    );

    let frontier_pos = [4.0, 0.0, 0.0];
    push_fact(
        &mut app,
        KnowledgeFact::ShipDestroyed {
            event_id: None,
            system: Some(s.frontier),
            ship_name: "Scout-1".into(),
            destroyed_at: 150,
            detail: "Destroyed".into(),
            ship: s.ship,
        },
        frontier_pos,
        150,
    );

    app.world_mut().resource_mut::<GameClock>().elapsed = 150 + light_delay_hexadies(4.0);
    run_reconciler(&mut app);

    let store = app
        .world()
        .entity(s.empire)
        .get::<KnowledgeStore>()
        .unwrap();
    let p = store
        .get_projection(s.ship)
        .expect("ShipDestroyed must NOT call clear_projection — situational memory matters");
    assert_eq!(p.projected_state, ShipSnapshotState::Destroyed);
    assert_eq!(p.projected_system, Some(s.frontier));
    assert!(
        p.intended_state.is_none()
            && p.intended_system.is_none()
            && p.intended_takes_effect_at.is_none(),
        "intended_* must clear on terminal Destroyed marker"
    );
    assert!(
        p.expected_arrival_at.is_none() && p.expected_return_at.is_none(),
        "expected_*_at must clear on terminal Destroyed marker"
    );
}

// ===========================================================================
// 4. ShipMissing → projected_state = Missing, projection retained.
// ===========================================================================

#[test]
fn reconcile_on_ship_missing_marks_projection() {
    let mut app = test_app();
    let s = setup_scenario(&mut app, 5.0);

    seed_projection(
        &mut app,
        s.empire,
        s.ship,
        Some(ShipSnapshotState::InTransitSubLight),
        Some(s.frontier),
        s.home,
        90,
    );

    let frontier_pos = [5.0, 0.0, 0.0];
    push_fact(
        &mut app,
        KnowledgeFact::ShipMissing {
            event_id: None,
            system: Some(s.frontier),
            ship_name: "Scout-1".into(),
            detail: "Missing".into(),
            ship: s.ship,
        },
        frontier_pos,
        200,
    );

    app.world_mut().resource_mut::<GameClock>().elapsed = 200 + light_delay_hexadies(5.0);
    run_reconciler(&mut app);

    let store = app
        .world()
        .entity(s.empire)
        .get::<KnowledgeStore>()
        .unwrap();
    let p = store
        .get_projection(s.ship)
        .expect("Missing facts must retain the projection (UI's amber state)");
    assert_eq!(p.projected_state, ShipSnapshotState::Missing);
    assert_eq!(p.projected_system, Some(s.frontier));
}

// ===========================================================================
// 5. Per-empire isolation — empire A close to the event, empire B far. A
//    fact from a system A can already see must NOT yet apply at B.
// ===========================================================================

#[test]
fn reconcile_per_empire_isolation() {
    let mut app = test_app();
    // Empire A's home at origin; the second empire's home far away.
    let s = setup_scenario(&mut app, 3.0);

    // Spawn empire B with a Ruler far away so its light delay greatly
    // exceeds A's. Reuse the existing ship (= same Entity) — both
    // empires hold an independent projection of it (= the
    // multi-dispatcher edge case the AC calls out).
    let far_pos = [60.0, 0.0, 0.0];
    let far_home = spawn_test_system(app.world_mut(), "BHome", far_pos, 1.0, true, true);
    let empire_b = app
        .world_mut()
        .spawn((
            Empire { name: "B".into() },
            Faction {
                id: "reconcile_test_b".into(),
                name: "B".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            SystemVisibilityMap::default(),
            KnowledgeStore::default(),
            CommsParams::default(),
        ))
        .id();
    spawn_test_ruler(app.world_mut(), empire_b, far_home);

    seed_projection(
        &mut app,
        s.empire,
        s.ship,
        Some(ShipSnapshotState::InTransitSubLight),
        Some(s.frontier),
        s.home,
        50,
    );
    seed_projection(
        &mut app,
        empire_b,
        s.ship,
        Some(ShipSnapshotState::InTransitSubLight),
        Some(s.frontier),
        far_home,
        50,
    );

    // Fact at the frontier system, 3 ly from A and ~57 ly from B.
    let frontier_pos = [3.0, 0.0, 0.0];
    push_fact(
        &mut app,
        KnowledgeFact::ShipArrived {
            event_id: None,
            system: Some(s.frontier),
            name: "Scout-1".into(),
            detail: "Arrived".into(),
            ship: s.ship,
        },
        frontier_pos,
        100,
    );

    // Tick past A's light delay only. Empire B's vantage is much
    // further so its recomputed `arrives_at` is well in the future.
    let a_delay = light_delay_hexadies(3.0);
    let b_delay = light_delay_hexadies(57.0);
    assert!(
        a_delay < b_delay,
        "A must observe before B for the test to mean anything"
    );
    app.world_mut().resource_mut::<GameClock>().elapsed = 100 + a_delay + 1;
    run_reconciler(&mut app);

    let store_a = app
        .world()
        .entity(s.empire)
        .get::<KnowledgeStore>()
        .unwrap();
    let pa = store_a.get_projection(s.ship).unwrap();
    assert_eq!(
        pa.projected_state,
        ShipSnapshotState::InSystem,
        "empire A must reconcile (it has observed the arrival)"
    );

    let store_b = app
        .world()
        .entity(empire_b)
        .get::<KnowledgeStore>()
        .unwrap();
    let pb = store_b.get_projection(s.ship).unwrap();
    assert_eq!(
        pb.projected_state,
        ShipSnapshotState::InSystem,
        "(precondition) projection_state was seeded as InSystem; the reconciler must NOT have advanced B"
    );
    assert_eq!(
        pb.intended_state,
        Some(ShipSnapshotState::InTransitSubLight),
        "empire B must NOT have its projection touched (light delay not yet elapsed)"
    );
    assert!(
        pb.expected_arrival_at.is_some(),
        "empire B's expected_arrival_at must remain (no reconciliation occurred)"
    );
}

// ===========================================================================
// 6. Reconciler runs after snapshot updates within the same Update tick.
//    Verified by registering the full KnowledgePlugin and stepping
//    `app.update()` with a clock past the fact's per-empire arrival.
// ===========================================================================

#[test]
fn reconcile_ordering_after_snapshot() {
    let mut app = test_app();
    // `test_app()` already wires `propagate_knowledge` and
    // `update_destroyed_ship_knowledge` (see `tests/common/mod.rs`).
    // Hand-register the reconciler so this test exercises the
    // post-snapshot `.after(propagate_knowledge)` ordering — same
    // contract as `KnowledgePlugin::build` but without the duplicate
    // wiring `test_app` would otherwise produce.
    app.add_systems(
        Update,
        reconcile_ship_projections.after(macrocosmo::knowledge::propagate_knowledge),
    );
    let s = setup_scenario(&mut app, 2.0);

    // Stamp a baseline snapshot so propagate_knowledge has a non-empty
    // KnowledgeStore for this ship (the reconciler does not require
    // the snapshot to mutate, but the AC says reconciler must observe
    // a coherent post-snapshot world).
    {
        let mut em = app.world_mut().entity_mut(s.empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_ship(ShipSnapshot {
            entity: s.ship,
            name: "Scout-1".into(),
            design_id: "explorer_mk1".into(),
            last_known_state: ShipSnapshotState::InTransitSubLight,
            last_known_system: Some(s.home),
            observed_at: 0,
            hp: 100.0,
            hp_max: 100.0,
            source: ObservationSource::Direct,
        });
    }

    seed_projection(
        &mut app,
        s.empire,
        s.ship,
        Some(ShipSnapshotState::InTransitSubLight),
        Some(s.frontier),
        s.home,
        50,
    );

    let frontier_pos = [2.0, 0.0, 0.0];
    push_fact(
        &mut app,
        KnowledgeFact::ShipArrived {
            event_id: None,
            system: Some(s.frontier),
            name: "Scout-1".into(),
            detail: "Arrived".into(),
            ship: s.ship,
        },
        frontier_pos,
        100,
    );

    app.world_mut().resource_mut::<GameClock>().elapsed = 100 + light_delay_hexadies(2.0) + 1;
    app.update();

    let store = app
        .world()
        .entity(s.empire)
        .get::<KnowledgeStore>()
        .unwrap();
    let p = store
        .get_projection(s.ship)
        .expect("projection must be present after a full app.update() pass");
    assert_eq!(p.projected_state, ShipSnapshotState::InSystem);
    assert_eq!(
        p.projected_system,
        Some(s.frontier),
        "projection's system must match the snapshot/fact arrival system after the chained Update"
    );
}
