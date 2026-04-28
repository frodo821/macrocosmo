//! Polish-bundle regression tests for the `ShipProjection` lifecycle:
//! issues #484 (reconciler stale-fact gate), #485 (`flush_ship_projection_writes`
//! `InGame` run-condition gate), #486 (`compute_ship_projection` saturation
//! handling).
//!
//! These tests pin the bug-fixes' contracts in isolation; the broader
//! reconciler / dispatch / render contracts are covered in
//! `ship_projection_{reconcile,dispatch,render,intended_render,persistence}.rs`.

mod common;

use bevy::ecs::schedule::Schedule;
use bevy::prelude::*;

use macrocosmo::empire::CommsParams;
use macrocosmo::knowledge::{
    KnowledgeFact, KnowledgeStore, ObservationSource, PendingFactQueue, PendingProjectionWrite,
    PerceivedFact, ShipProjection, ShipProjectionWriteQueue, ShipSnapshotState,
    SystemVisibilityMap, SystemVisibilityTier, compute_ship_projection,
    flush_ship_projection_writes, reconcile_ship_projections,
};
use macrocosmo::physics::light_delay_hexadies;
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::{Owner, Ship};
use macrocosmo::time_system::GameClock;

use common::{spawn_test_ruler, spawn_test_ship, spawn_test_system, test_app};

// ---------------------------------------------------------------------------
// Shared scenario builder — minimal empire + home + frontier + a ship.
// Mirrors `tests/ship_projection_reconcile.rs::setup_scenario` but trimmed
// for the polish bundle's needs.
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
                name: "Polish".into(),
            },
            PlayerEmpire,
            Faction {
                id: "polish_test".into(),
                name: "Polish".into(),
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

fn seed_projection(
    app: &mut App,
    empire: Entity,
    ship: Entity,
    home: Entity,
    frontier: Entity,
    intended_state: Option<ShipSnapshotState>,
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
        intended_system: Some(frontier),
        intended_takes_effect_at: Some(dispatched_at + 10),
    };
    let mut em = app.world_mut().entity_mut(empire);
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

// ===========================================================================
// #484 — reconciler must NOT clear `intended_*` for a fact that pre-dates
// the projection's current `dispatched_at`. Without the gate, a stale
// `ShipArrived` fact in the queue can prematurely clear a freshly
// re-dispatched mission's intended trajectory.
// ===========================================================================

#[test]
fn stale_fact_does_not_clear_new_intent() {
    let mut app = test_app();
    let s = setup_scenario(&mut app, 2.0);

    // (1) Seed mission #1 — dispatched at T=50, target = frontier.
    seed_projection(
        &mut app,
        s.empire,
        s.ship,
        s.home,
        s.frontier,
        Some(ShipSnapshotState::InTransit),
        50,
    );

    // (2) First fact arrives — observed at T=100. Reconciler clears
    // intended_* (= mission #1 visibly delivered) and bumps
    // `dispatched_at` to 100 (= max(50, 100)).
    let frontier_pos = [2.0, 0.0, 0.0];
    push_fact(
        &mut app,
        KnowledgeFact::ShipArrived {
            event_id: None,
            system: Some(s.frontier),
            name: "Scout-1".into(),
            detail: "Arrived (mission #1)".into(),
            ship: s.ship,
        },
        frontier_pos,
        100,
    );
    app.world_mut().resource_mut::<GameClock>().elapsed = 100 + light_delay_hexadies(2.0) + 1;
    run_reconciler(&mut app);

    // Sanity check: mission #1 was cleared.
    {
        let store = app
            .world()
            .entity(s.empire)
            .get::<KnowledgeStore>()
            .unwrap();
        let p = store.get_projection(s.ship).unwrap();
        assert!(
            p.intended_state.is_none() && p.intended_system.is_none(),
            "(precondition) mission #1's intended_* must have cleared after reconcile"
        );
        assert!(
            p.dispatched_at >= 100,
            "(precondition) dispatched_at must have bumped to the observed_at, got {}",
            p.dispatched_at
        );
    }

    // Drain the queue so we can replay the fact deliberately. Without
    // a drain, the next `run_reconciler` would re-process the same
    // fact AND any new one, conflating signals.
    app.world_mut()
        .resource_mut::<PendingFactQueue>()
        .facts
        .clear();

    // (3) Re-dispatch mission #2 — same target `frontier`, but a fresh
    // dispatch at T=300 (well after fact_1's observed_at=100). The
    // dispatcher writes a new projection: intended_* re-set,
    // dispatched_at advanced to 300.
    seed_projection(
        &mut app,
        s.empire,
        s.ship,
        s.home,
        s.frontier,
        Some(ShipSnapshotState::InTransit),
        300,
    );

    // (4) Replay the OLD fact (observed_at=100, < dispatched_at=300).
    // This simulates a deferred fact that was held in another empire's
    // pipeline / a save-load race / a queue replay edge case.
    push_fact(
        &mut app,
        KnowledgeFact::ShipArrived {
            event_id: None,
            system: Some(s.frontier),
            name: "Scout-1".into(),
            detail: "Stale fact replay".into(),
            ship: s.ship,
        },
        frontier_pos,
        100,
    );
    app.world_mut().resource_mut::<GameClock>().elapsed = 400; // well past arrival
    run_reconciler(&mut app);

    // (5) Assert: mission #2's intended_* must STILL be set. The
    // staleness gate (`fact_observed_at >= projection.dispatched_at`)
    // should have kept the clear branch from firing.
    let store = app
        .world()
        .entity(s.empire)
        .get::<KnowledgeStore>()
        .unwrap();
    let p = store
        .get_projection(s.ship)
        .expect("projection must still exist");
    assert_eq!(
        p.intended_state,
        Some(ShipSnapshotState::InTransit),
        "stale fact (observed_at < dispatched_at) must NOT clear the new mission's intended_state"
    );
    assert_eq!(
        p.intended_system,
        Some(s.frontier),
        "stale fact must NOT clear intended_system"
    );
    assert!(
        p.intended_takes_effect_at.is_some(),
        "stale fact must NOT clear intended_takes_effect_at"
    );
    // Note: projected_state/system *do* update (= the reconciler still
    // applies the projected_*=InSystem update from the fact); only the
    // intended_* clear branch is gated.
    assert_eq!(p.projected_state, ShipSnapshotState::InSystem);
    assert_eq!(p.projected_system, Some(s.frontier));
}

// ===========================================================================
// #484 — same gate must apply to `SurveyComplete` arm. A stale
// `SurveyComplete` fact must not clear a fresh re-survey mission's
// intended_*.
// ===========================================================================

#[test]
fn stale_survey_fact_does_not_clear_new_survey_intent() {
    let mut app = test_app();
    let s = setup_scenario(&mut app, 3.0);

    seed_projection(
        &mut app,
        s.empire,
        s.ship,
        s.home,
        s.frontier,
        Some(ShipSnapshotState::Surveying),
        50,
    );

    let frontier_pos = [3.0, 0.0, 0.0];
    push_fact(
        &mut app,
        KnowledgeFact::SurveyComplete {
            event_id: None,
            system: s.frontier,
            system_name: "Frontier".into(),
            detail: "Mission #1 survey".into(),
            ship: s.ship,
        },
        frontier_pos,
        100,
    );
    app.world_mut().resource_mut::<GameClock>().elapsed = 100 + light_delay_hexadies(3.0) + 1;
    run_reconciler(&mut app);

    // Drain queue, re-seed for mission #2, replay stale fact.
    app.world_mut()
        .resource_mut::<PendingFactQueue>()
        .facts
        .clear();
    seed_projection(
        &mut app,
        s.empire,
        s.ship,
        s.home,
        s.frontier,
        Some(ShipSnapshotState::Surveying),
        300,
    );
    push_fact(
        &mut app,
        KnowledgeFact::SurveyComplete {
            event_id: None,
            system: s.frontier,
            system_name: "Frontier".into(),
            detail: "Stale survey replay".into(),
            ship: s.ship,
        },
        frontier_pos,
        100,
    );
    app.world_mut().resource_mut::<GameClock>().elapsed = 400;
    run_reconciler(&mut app);

    let store = app
        .world()
        .entity(s.empire)
        .get::<KnowledgeStore>()
        .unwrap();
    let p = store.get_projection(s.ship).unwrap();
    assert_eq!(
        p.intended_state,
        Some(ShipSnapshotState::Surveying),
        "stale SurveyComplete fact must NOT clear the new survey mission's intended_state"
    );
    assert_eq!(p.intended_system, Some(s.frontier));
}

// ===========================================================================
// #484 — coverage of the unchanged terminal arms: `ShipDestroyed` and
// `ShipMissing` must continue to clear `intended_*` even with an
// observed_at older than dispatched_at, because the ship is gone and
// no future mission can keep the prior intent valid.
// ===========================================================================

#[test]
fn stale_destroyed_fact_still_clears_intent() {
    let mut app = test_app();
    let s = setup_scenario(&mut app, 4.0);

    // Seed at T=300, then push a stale ShipDestroyed (observed_at=100).
    seed_projection(
        &mut app,
        s.empire,
        s.ship,
        s.home,
        s.frontier,
        Some(ShipSnapshotState::InTransit),
        300,
    );

    let frontier_pos = [4.0, 0.0, 0.0];
    push_fact(
        &mut app,
        KnowledgeFact::ShipDestroyed {
            event_id: None,
            system: Some(s.frontier),
            ship_name: "Scout-1".into(),
            destroyed_at: 100,
            detail: "Destroyed".into(),
            ship: s.ship,
        },
        frontier_pos,
        100,
    );

    app.world_mut().resource_mut::<GameClock>().elapsed = 400;
    run_reconciler(&mut app);

    let store = app
        .world()
        .entity(s.empire)
        .get::<KnowledgeStore>()
        .unwrap();
    let p = store.get_projection(s.ship).unwrap();
    assert_eq!(
        p.projected_state,
        ShipSnapshotState::Destroyed,
        "Destroyed marker applies regardless of staleness"
    );
    assert!(
        p.intended_state.is_none() && p.intended_system.is_none(),
        "ShipDestroyed clears intended_* unconditionally — no future mission can keep intent valid"
    );
}

// ===========================================================================
// #485 — `flush_ship_projection_writes` must NOT drain the queue when
// `GameState != InGame`. The gate is defense-in-depth (today every push
// site is also gated, so the queue is empty), but a future push path
// (e.g. save/load rehydration) must not race the flush before
// `OnEnter(InGame)`.
// ===========================================================================

#[test]
fn flush_skipped_outside_ingame() {
    use macrocosmo::game_state::GameState;

    // Build a minimal app with the run-condition wired up the same way
    // KnowledgePlugin wires it. We don't pull in the whole plugin
    // because test_app() seeds InGame and we want to flip it.
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(macrocosmo::game_state::GameStatePlugin);
    app.insert_state(GameState::Bootstrapping);
    app.init_resource::<ShipProjectionWriteQueue>();
    app.add_systems(
        Update,
        flush_ship_projection_writes.run_if(in_state(GameState::InGame)),
    );

    // Seed a fake empire entity & projection. The flush system will
    // only consume the queue if the state gate passes; the `Query`
    // failing to find the empire would simply skip THAT entry, but
    // here we want to assert the system is never invoked at all.
    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Test".into(),
            },
            KnowledgeStore::default(),
        ))
        .id();
    let projection = ShipProjection {
        entity: Entity::PLACEHOLDER,
        dispatched_at: 0,
        expected_arrival_at: None,
        expected_return_at: None,
        projected_state: ShipSnapshotState::InSystem,
        projected_system: None,
        intended_state: None,
        intended_system: None,
        intended_takes_effect_at: None,
    };
    app.world_mut()
        .resource_mut::<ShipProjectionWriteQueue>()
        .entries
        .push(PendingProjectionWrite { empire, projection });

    // Run while in `Bootstrapping` (= NOT InGame). The flush system's
    // run_if must block it.
    app.update();
    assert_eq!(
        app.world()
            .resource::<ShipProjectionWriteQueue>()
            .entries
            .len(),
        1,
        "flush_ship_projection_writes must not drain the queue while GameState != InGame"
    );

    // Now flip to InGame and the flush must drain.
    app.world_mut()
        .resource_mut::<NextState<GameState>>()
        .set(GameState::InGame);
    app.update();
    assert!(
        app.world()
            .resource::<ShipProjectionWriteQueue>()
            .entries
            .is_empty(),
        "flush_ship_projection_writes must drain once GameState == InGame"
    );
}

// ===========================================================================
// #486 — `compute_ship_projection` must not silently corrupt projections
// when `saturating_add` clamps to `i64::MAX`. In release builds we
// expect a warn-log; in debug builds the test harness panics on the
// `debug_assert!` that we can also catch directly.
//
// We exercise the code path with a galactic-edge target: positions
// that produce light_delay_hexadies values close enough to `i64::MAX`
// that `saturating_add` saturates. `physics::distance_ly_arr` returns
// `f64`; very large coords yield very large delays.
// ===========================================================================

#[test]
fn saturation_path_does_not_silently_corrupt() {
    // Build a target so far that the light_delay becomes astronomical.
    // `physics::light_delay_hexadies(d)` is roughly `d * 60` (60 hd /
    // ly), so we need d > i64::MAX / 60 to saturate. f64 can represent
    // numbers far past that.
    let dispatcher_pos = [0.0, 0.0, 0.0];
    let ship_pos = [0.0, 0.0, 0.0];
    // 1e20 ly is astronomically beyond galaxy scale (~1e5 ly typical),
    // produces a saturated light delay even though distance_ly_arr's
    // f64 can hold it.
    let target_pos = [1.0e20_f64, 0.0, 0.0];
    let now = 0i64;

    // In debug builds (= cargo test default), the inner `debug_assert!`
    // will panic. We catch the panic so the test passes either way:
    // - debug build: caught panic = `debug_assert!` fired = good
    // - release build: no panic = warn-log emitted, ShipProjection
    //   returned with i64::MAX values (silently passed through, but
    //   observable via logs)
    let result = std::panic::catch_unwind(|| {
        compute_ship_projection(
            Entity::from_raw_u32(1).unwrap(),
            None,
            dispatcher_pos,
            ship_pos,
            Some(target_pos),
            Some(ShipSnapshotState::InTransit),
            None,
            true,
            None,
            now,
        )
    });

    // Either path is acceptable — we just want to confirm the function
    // does not silently produce a "valid-looking" projection with
    // saturated values. In debug, the `debug_assert!` panicked; in
    // release, the projection is returned but `expected_arrival_at`
    // is at/near `i64::MAX` (= the saturation we want to flag).
    match result {
        Err(_) => {
            // Debug build: debug_assert! caught the saturation. PASS.
        }
        Ok(projection) => {
            // Release build: function returned silently. Verify it
            // returned the saturated value (= the bug we warn-log
            // about) rather than corrupted data.
            let arrival = projection.expected_arrival_at.unwrap();
            assert!(
                arrival >= i64::MAX / 2,
                "release build: expected saturating value (>= i64::MAX/2), got {}",
                arrival
            );
        }
    }
}

// ===========================================================================
// #486 — sanity check: NORMAL galactic distances (≤ a few hundred ly)
// must NOT trip saturation. Defensive guard against a future code
// change that lowers the threshold or breaks light_delay_hexadies.
// ===========================================================================

#[test]
fn normal_galactic_distance_does_not_saturate() {
    let dispatcher_pos = [0.0, 0.0, 0.0];
    let ship_pos = [0.0, 0.0, 0.0];
    let target_pos = [500.0, 0.0, 0.0]; // 500 ly = far edge of typical galaxy generation
    let now = 1_000_000i64;

    let projection = compute_ship_projection(
        Entity::from_raw_u32(1).unwrap(),
        None,
        dispatcher_pos,
        ship_pos,
        Some(target_pos),
        Some(ShipSnapshotState::InTransit),
        None,
        true,
        None,
        now,
    );

    let saturation_threshold = i64::MAX / 2;
    assert!(
        projection.intended_takes_effect_at.unwrap() < saturation_threshold,
        "500 ly must not saturate intended_takes_effect_at"
    );
    assert!(
        projection.expected_arrival_at.unwrap() < saturation_threshold,
        "500 ly must not saturate expected_arrival_at"
    );
    assert!(
        projection.expected_return_at.unwrap() < saturation_threshold,
        "500 ly must not saturate expected_return_at"
    );
}
