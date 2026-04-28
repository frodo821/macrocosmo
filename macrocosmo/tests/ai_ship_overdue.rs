//! #480: `ai::is_ship_overdue` Suspected seed-signal helper tests.
//!
//! These tests pin the contract for the AI-side consumer of
//! `ShipProjection.expected_return_at`. The helper is the sole bridge
//! the eventual ThreatState updater (#466 Phase 2) will use to decide
//! Suspected — so it MUST be light-speed-coherent (= reads only the
//! dispatcher's `KnowledgeStore`, never realtime ECS `ShipState`).
//!
//! Tests:
//! 1. `overdue_returns_false_for_unprojected_ship` — no projection.
//! 2. `overdue_returns_false_before_expected_return_at` — before deadline.
//! 3. `overdue_returns_false_within_tolerance` — inside grace.
//! 4. `overdue_returns_true_past_tolerance` — past grace.
//! 5. `overdue_clears_after_reconciliation` — reconciler clears
//!    `expected_return_at` on a matching `ShipArrived`, helper flips to
//!    `false`.
//! 6. `overdue_does_not_consult_realtime_ship_state` — FTL-leak guard:
//!    ship's runtime `ShipState` is set "back at home" but the
//!    reconciler hasn't seen the fact yet — helper still returns `true`
//!    because it trusts ONLY the projection.

mod common;

use bevy::ecs::schedule::Schedule;
use bevy::prelude::*;

use macrocosmo::ai::threat_query::{OVERDUE_TOLERANCE_HEXADIES, is_ship_overdue};
use macrocosmo::components::Position;
use macrocosmo::empire::CommsParams;
use macrocosmo::knowledge::{
    KnowledgeFact, KnowledgeStore, ObservationSource, PendingFactQueue, PerceivedFact,
    ShipProjection, ShipSnapshotState, SystemVisibilityMap, SystemVisibilityTier,
    reconcile_ship_projections,
};
use macrocosmo::physics::light_delay_hexadies;
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::{Owner, Ship, ShipState};
use macrocosmo::time_system::GameClock;

use common::{spawn_test_ruler, spawn_test_ship, spawn_test_system, test_app};

/// Build a minimal projection with an `expected_return_at` deadline.
/// Other fields are filled with sensible defaults appropriate to a
/// surveying / scouting return-leg mission.
fn make_projection(
    ship: Entity,
    home: Entity,
    target: Entity,
    dispatched_at: i64,
    expected_return_at: Option<i64>,
) -> ShipProjection {
    ShipProjection {
        entity: ship,
        dispatched_at,
        expected_arrival_at: Some(dispatched_at + 50),
        expected_return_at,
        projected_state: ShipSnapshotState::InTransit,
        projected_system: Some(home),
        intended_state: Some(ShipSnapshotState::Surveying),
        intended_system: Some(target),
        intended_takes_effect_at: Some(dispatched_at + 5),
    }
}

// ===========================================================================
// 1. No projection at all → not overdue.
// ===========================================================================

#[test]
fn overdue_returns_false_for_unprojected_ship() {
    let store = KnowledgeStore::default();
    // Stable arbitrary entity bits — the store has no projection for
    // anything, so the lookup must miss and return false regardless of
    // the supplied `now`.
    let mut world = World::new();
    let ship = world.spawn_empty().id();

    assert!(
        !is_ship_overdue(&store, ship, 1_000_000, OVERDUE_TOLERANCE_HEXADIES),
        "no projection → overdue must be false (dispatcher isn't tracking this ship)"
    );
}

// ===========================================================================
// 2. Now strictly before expected_return_at → not overdue.
// ===========================================================================

#[test]
fn overdue_returns_false_before_expected_return_at() {
    let mut world = World::new();
    let ship = world.spawn_empty().id();
    let home = world.spawn_empty().id();
    let target = world.spawn_empty().id();

    let mut store = KnowledgeStore::default();
    store.update_projection(make_projection(ship, home, target, 0, Some(100)));

    assert!(
        !is_ship_overdue(&store, ship, 50, 10),
        "now=50 < expected_return_at=100, must not be overdue"
    );
}

// ===========================================================================
// 3. Within tolerance grace → not overdue.
// ===========================================================================

#[test]
fn overdue_returns_false_within_tolerance() {
    let mut world = World::new();
    let ship = world.spawn_empty().id();
    let home = world.spawn_empty().id();
    let target = world.spawn_empty().id();

    let mut store = KnowledgeStore::default();
    store.update_projection(make_projection(ship, home, target, 0, Some(100)));

    // expected_return_at + tolerance = 110; now = 105 ⇒ inside grace.
    assert!(
        !is_ship_overdue(&store, ship, 105, 10),
        "now=105, deadline+tolerance=110, must still be inside grace"
    );
    // Boundary: now == deadline + tolerance is NOT yet overdue (strict >).
    assert!(
        !is_ship_overdue(&store, ship, 110, 10),
        "now == deadline+tolerance must not be overdue (strict >)"
    );
}

// ===========================================================================
// 4. Past tolerance → overdue.
// ===========================================================================

#[test]
fn overdue_returns_true_past_tolerance() {
    let mut world = World::new();
    let ship = world.spawn_empty().id();
    let home = world.spawn_empty().id();
    let target = world.spawn_empty().id();

    let mut store = KnowledgeStore::default();
    store.update_projection(make_projection(ship, home, target, 0, Some(100)));

    // expected_return_at + tolerance = 110; now = 115 ⇒ overdue.
    assert!(
        is_ship_overdue(&store, ship, 115, 10),
        "now=115 > deadline+tolerance=110, must be overdue"
    );
}

// ===========================================================================
// 5. Reconciler clears expected_return_at on matching fact → no longer
//    overdue, even past the original deadline.
// ===========================================================================
//
// This re-uses the projection-reconcile test scaffold from
// `tests/ship_projection_reconcile.rs` with a minimal scenario builder
// duplicated locally — the helpers there are file-private.

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
                id: "overdue_test".into(),
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

    Scenario {
        empire,
        home,
        frontier,
        ship,
    }
}

fn seed_return_leg_projection(app: &mut App, s: &Scenario, dispatched_at: i64, return_at: i64) {
    let projection = ShipProjection {
        entity: s.ship,
        dispatched_at,
        expected_arrival_at: Some(dispatched_at + 50),
        expected_return_at: Some(return_at),
        projected_state: ShipSnapshotState::InTransit,
        projected_system: Some(s.home),
        intended_state: Some(ShipSnapshotState::InTransit),
        intended_system: Some(s.frontier),
        intended_takes_effect_at: Some(dispatched_at + 5),
    };
    let mut em = app.world_mut().entity_mut(s.empire);
    let mut store = em.get_mut::<KnowledgeStore>().unwrap();
    store.update_projection(projection);
}

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

fn run_reconciler(app: &mut App) {
    let mut schedule = Schedule::default();
    schedule.add_systems(reconcile_ship_projections);
    schedule.run(app.world_mut());
}

#[test]
fn overdue_clears_after_reconciliation() {
    let mut app = test_app();
    let s = setup_scenario(&mut app, 2.0);
    let dispatched_at: i64 = 50;
    let return_at: i64 = 150;

    seed_return_leg_projection(&mut app, &s, dispatched_at, return_at);

    // Sanity: pre-reconciliation, past deadline+tolerance ⇒ overdue.
    {
        let store = app
            .world()
            .entity(s.empire)
            .get::<KnowledgeStore>()
            .unwrap();
        assert!(
            is_ship_overdue(store, s.ship, return_at + 100, OVERDUE_TOLERANCE_HEXADIES),
            "pre-reconciliation: ship is past deadline+tolerance, must be overdue"
        );
    }

    // Push a `ShipArrived` matching the intended target. The reconciler
    // clears expected_*_at and intended_*.
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
    app.world_mut().resource_mut::<GameClock>().elapsed = 100 + light_delay_hexadies(2.0);
    run_reconciler(&mut app);

    // Now even though the original `expected_return_at` has long since
    // passed, the projection's `expected_return_at` was cleared by the
    // reconciler ⇒ helper returns false.
    let store = app
        .world()
        .entity(s.empire)
        .get::<KnowledgeStore>()
        .unwrap();
    let proj = store
        .get_projection(s.ship)
        .expect("projection must still exist after reconciliation");
    assert!(
        proj.expected_return_at.is_none(),
        "reconciler must clear expected_return_at on matching ShipArrived"
    );
    assert!(
        !is_ship_overdue(store, s.ship, return_at + 100, OVERDUE_TOLERANCE_HEXADIES),
        "post-reconciliation: expected_return_at cleared ⇒ not overdue"
    );
}

// ===========================================================================
// 6. FTL-leak guard: helper trusts projection, NOT realtime ECS state.
//    Even when the ship's `ShipState` says "back at home", the projection
//    still says "expected back by tick T, no observation yet" ⇒ overdue.
// ===========================================================================

#[test]
fn overdue_does_not_consult_realtime_ship_state() {
    let mut app = test_app();
    let s = setup_scenario(&mut app, 2.0);
    let dispatched_at: i64 = 50;
    let return_at: i64 = 150;

    seed_return_leg_projection(&mut app, &s, dispatched_at, return_at);

    // Mutate the realtime ECS ship state so the ship is "back home" in
    // ECS terms — but DO NOT push a KnowledgeFact, so the reconciler
    // sees nothing. A correct helper must NOT consult `ShipState`.
    *app.world_mut()
        .entity_mut(s.ship)
        .get_mut::<ShipState>()
        .unwrap() = ShipState::InSystem { system: s.home };
    *app.world_mut()
        .entity_mut(s.ship)
        .get_mut::<Position>()
        .unwrap() = Position::from([0.0, 0.0, 0.0]);

    // Bump clock past the deadline + tolerance.
    let now = return_at + OVERDUE_TOLERANCE_HEXADIES + 5;
    app.world_mut().resource_mut::<GameClock>().elapsed = now;

    let store = app
        .world()
        .entity(s.empire)
        .get::<KnowledgeStore>()
        .unwrap();
    assert!(
        is_ship_overdue(store, s.ship, now, OVERDUE_TOLERANCE_HEXADIES),
        "helper must trust projection (still says overdue) over realtime ECS \
         state (says ship is back home) — this is the FTL-leak guard"
    );
}

// ===========================================================================
// Bonus: `expected_return_at = None` → never overdue, regardless of `now`.
// ===========================================================================

#[test]
fn overdue_returns_false_when_no_return_leg() {
    let mut world = World::new();
    let ship = world.spawn_empty().id();
    let home = world.spawn_empty().id();
    let target = world.spawn_empty().id();

    let mut store = KnowledgeStore::default();
    // One-way command (e.g. colonize_system) ⇒ no return leg.
    store.update_projection(make_projection(ship, home, target, 0, None));

    assert!(
        !is_ship_overdue(&store, ship, 1_000_000, OVERDUE_TOLERANCE_HEXADIES),
        "no return leg ⇒ never overdue"
    );
}
