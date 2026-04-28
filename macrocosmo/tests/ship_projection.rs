//! #479: End-to-end regression suite for the `ShipProjection` lifecycle
//! introduced by epic #473 (sub-issues #474 / #475 / #476 / #477 / #478).
//!
//! The individual sub-issue PRs each have their own focused unit/integration
//! tests (`tests/ship_projection_{persistence,dispatch,reconcile,render,intended_render}.rs`).
//! This file pins **the cross-cutting contracts** those individual tests
//! cannot — the full data path:
//!
//! ```text
//! [player issues survey command at T0]
//!   ↓
//! dispatch_writes_projection (#475)  → projection.dispatched_at=T0,
//!                                      intended_*=Some(...),
//!                                      intended_takes_effect_at=T0+light_delay
//!   ↓
//! [ticks T0..T0+light_delay-1]       → renderer (#477) shows ship at
//!                                      projected_system (= dispatch-time
//!                                      last_known); intended layer (#478)
//!                                      shows dashed line to target
//!   ↓
//! [fact arrives at empire's vantage]
//!   ↓
//! reconcile_ship_projections (#476)  → projected_*=fact's payload,
//!                                      intended_*=cleared
//!   ↓
//! [renderer (#477) now shows ship at the new projected position;
//!  intended layer (#478) hidden because projected==intended]
//! ```
//!
//! Test cases (per AC):
//!
//! 1. `ftl_leak_guard_galaxy_map_during_dispatch_window` — across the
//!    entire dispatch window, `compute_own_ship_render_inputs` must
//!    keep returning `home` as the projected system, regardless of how
//!    far realtime ECS state has drifted.
//! 2. `projected_intended_divergence_during_dispatch_window` — the
//!    intended overlay layer renders for the full dispatch window with
//!    the documented alpha curve (0.4..0.8) and disappears once the
//!    reconciler converges projected and intended.
//! 3. `reconcile_advances_each_fact_kind` — for each of the four
//!    reconciling fact kinds (`ShipArrived` / `SurveyComplete` /
//!    `ShipDestroyed` / `ShipMissing`), end-to-end fact-emit-then-
//!    reconcile produces the expected post-reconcile projection state.
//! 4. `save_load_mid_flight_preserves_projection` — a mid-flight save
//!    round-trips the projection's `dispatched_at`, `expected_*_at`,
//!    `projected_*`, `intended_*` fields via postcard, and the loaded
//!    world can continue to advance into reconciliation.

mod common;

use std::collections::HashMap;

use bevy::prelude::*;

use macrocosmo::components::Position;
use macrocosmo::knowledge::{
    KnowledgeFact, KnowledgeStore, ObservationSource, PendingFactQueue, PerceivedFact,
    ShipProjection, ShipSnapshotState, SystemVisibilityMap, SystemVisibilityTier,
    reconcile_ship_projections,
};
use macrocosmo::persistence::{load::load_game_from_reader, save::save_game_to_writer};
use macrocosmo::physics::light_delay_hexadies;
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::{Owner, Ship};
use macrocosmo::time_system::GameClock;
use macrocosmo::visualization::ships::{
    OwnShipMetadata, compute_intended_render_inputs, compute_own_ship_render_inputs,
    intended_layer_alpha,
};

use common::{spawn_test_ruler, spawn_test_ship, spawn_test_system, test_app};

// ---------------------------------------------------------------------------
// Shared scenario builder
// ---------------------------------------------------------------------------

/// Empire with its Ruler at `home` (origin), a frontier system 5 ly away
/// (= `light_delay_hexadies(5.0) = 300` hd of light delay), and an idle
/// survey ship docked at home. Mirrors the structure of
/// `tests/ship_projection_dispatch.rs::setup_scenario` but pinned at 5 ly
/// because the AC frames the dispatch-window arithmetic in those terms.
struct Scenario {
    empire: Entity,
    home: Entity,
    frontier: Entity,
    ship: Entity,
    /// Light delay between home and frontier in hexadies (= 300 for 5 ly).
    light_delay: i64,
}

const FRONTIER_DISTANCE_LY: f64 = 5.0;

fn setup_scenario(app: &mut App) -> Scenario {
    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "ship_projection_e2e".into(),
                name: "Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            SystemVisibilityMap::default(),
            KnowledgeStore::default(),
            macrocosmo::empire::CommsParams::default(),
        ))
        .id();

    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [FRONTIER_DISTANCE_LY, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut vis = em.get_mut::<SystemVisibilityMap>().unwrap();
        vis.set(home, SystemVisibilityTier::Local);
        vis.set(frontier, SystemVisibilityTier::Surveyed);
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

    let light_delay = light_delay_hexadies(FRONTIER_DISTANCE_LY);
    assert_eq!(
        light_delay, 300,
        "sanity: 5 ly is 300 hexadies of light-speed delay (used in the \
         dispatch-window arithmetic below)",
    );

    Scenario {
        empire,
        home,
        frontier,
        ship,
        light_delay,
    }
}

/// Construct an `OwnShipMetadata` matching the ship spawned by
/// `spawn_test_ship` for the viewing empire — used by the renderer
/// helpers `compute_*_render_inputs`. The renderer doesn't consult
/// realtime `ShipState`; metadata only carries Components that are
/// not light-delayed (design id, role flags, ownership).
fn ship_metadata(ship: Entity) -> HashMap<Entity, OwnShipMetadata> {
    let mut metadata = HashMap::new();
    metadata.insert(
        ship,
        OwnShipMetadata {
            design_id: "explorer_mk1".into(),
            is_station: false,
            is_harbour: false,
            owned_by_viewing_empire: true,
        },
    );
    metadata
}

/// Issue a `Survey` command via the Lua dispatch site. This is the most
/// stable producer of `ShipProjection` writes from #475 (the AI bus path
/// requires `AiPlugin`'s warmup tick + manual schema declaration; the
/// Lua path goes through `apply::request_command` synchronously).
///
/// Returns the dispatch tick (= the value of `GameClock` at the moment
/// of dispatch).
fn dispatch_survey_command(app: &mut App, ship: Entity, frontier: Entity) -> i64 {
    use macrocosmo::scripting::gamestate_scope::apply::{ParsedRequest, request_command};

    // Ensure the resource the dispatcher allocates command ids from is
    // present (test_app() doesn't always seed it).
    if app
        .world()
        .get_resource::<macrocosmo::ship::command_events::NextCommandId>()
        .is_none()
    {
        app.world_mut()
            .insert_resource(macrocosmo::ship::command_events::NextCommandId::default());
    }

    let dispatch_tick = app.world().resource::<GameClock>().elapsed;
    request_command(
        app.world_mut(),
        ParsedRequest::Survey {
            ship,
            target_system: frontier,
        },
    )
    .expect("Lua request_command should succeed");
    dispatch_tick
}

/// Push a `KnowledgeFact` directly into the global queue with the given
/// origin position. The reconciler recomputes per-empire arrival from
/// `origin_pos` vs the empire viewer's position so the queue's
/// `arrives_at` is not load-bearing — we still set it to `observed_at`
/// for shape-conformance with the rest of the perception pipeline.
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

/// Run only the reconciler in isolation — same approach as
/// `tests/ship_projection_reconcile.rs`, lets us pin the consumer side
/// without a full `app.update()` (which would also run other knowledge
/// systems irrelevant to the AC).
fn run_reconciler(app: &mut App) {
    let mut schedule = bevy::ecs::schedule::Schedule::default();
    schedule.add_systems(reconcile_ship_projections);
    schedule.run(app.world_mut());
}

/// Read the empire's projection for `ship`, returning a clone so the
/// caller is free of borrow lifetime constraints.
fn get_projection(app: &App, empire: Entity, ship: Entity) -> ShipProjection {
    app.world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire has KnowledgeStore")
        .get_projection(ship)
        .expect("projection must exist")
        .clone()
}

// ===========================================================================
// Test 1: FTL leak guard — Galaxy Map renderer must show the ship at the
// dispatch-time `last_known` system across the entire dispatch window.
// ===========================================================================

/// Across ticks `T0+1 .. T0+light_delay-1`, the renderer's input feed
/// must keep `projected_system == home` regardless of how the realtime
/// ECS state has drifted. This is the central FTL-leak guard the epic
/// exists to fix. Even though no fact has reconciled the projection,
/// the renderer must not "see" the ship at the survey target.
#[test]
fn ftl_leak_guard_galaxy_map_during_dispatch_window() {
    let mut app = test_app();
    let s = setup_scenario(&mut app);

    // Dispatch at T0 = 0.
    let t0 = dispatch_survey_command(&mut app, s.ship, s.frontier);
    assert_eq!(t0, 0, "dispatch must occur at the test's chosen T0");

    // Sanity: the projection landed at dispatch tick.
    let p = get_projection(&app, s.empire, s.ship);
    assert_eq!(p.dispatched_at, 0);
    assert_eq!(
        p.projected_state,
        ShipSnapshotState::InSystem,
        "pre-dispatch state inferred from snapshot/home_port = InSystem at home",
    );
    assert_eq!(
        p.projected_system,
        Some(s.home),
        "projected layer must track the dispatch-time home system",
    );
    assert_eq!(p.intended_state, Some(ShipSnapshotState::Surveying));
    assert_eq!(p.intended_system, Some(s.frontier));

    let metadata = ship_metadata(s.ship);

    // Adversarial drift: move the ship's realtime ECS Position across
    // the galaxy. If anything in the renderer leaked through to realtime
    // state, this would make the assertion below fire.
    {
        let mut em = app.world_mut().entity_mut(s.ship);
        if let Some(mut p) = em.get_mut::<Position>() {
            *p = Position::from([100.0, 0.0, 0.0]);
        }
    }

    // Sample several ticks across the dispatch window. Choosing a few
    // representative points (start, quarter, half, just-before-arrival)
    // keeps the test fast while still guarding the contract across the
    // whole window.
    let samples = [
        1,
        s.light_delay / 4,
        s.light_delay / 2,
        (s.light_delay * 3) / 4,
        s.light_delay - 1,
    ];
    for tick in samples {
        app.world_mut().resource_mut::<GameClock>().elapsed = tick;
        let store = app
            .world()
            .entity(s.empire)
            .get::<KnowledgeStore>()
            .expect("empire has KnowledgeStore");
        let items = compute_own_ship_render_inputs(store, &metadata);
        assert_eq!(
            items.len(),
            1,
            "exactly one render item at tick {tick}; FTL-leak filter must \
             not drop the projection during the dispatch window",
        );
        assert_eq!(
            items[0].projected_system,
            Some(s.home),
            "FTL leak regression at tick {tick}: renderer must keep the ship \
             at home (= dispatch-time last_known) until reconciliation arrives, \
             but it returned {:?}",
            items[0].projected_system,
        );
        assert!(
            matches!(items[0].projected_state, ShipSnapshotState::InSystem),
            "renderer must surface the projected (pre-dispatch) state at tick {tick}",
        );
        assert_ne!(
            items[0].projected_system,
            Some(s.frontier),
            "FTL leak regression at tick {tick}: command target must NOT \
             appear in render input until the projection is reconciled",
        );
    }
}

// ===========================================================================
// Test 2: Projected vs intended divergence with alpha curve.
// ===========================================================================

/// Throughout the dispatch window, `compute_intended_render_inputs`
/// must surface a divergent overlay (projected = home, intended =
/// frontier) at the documented #478 alpha curve. Once a `ShipArrived`
/// fact reconciles the projection (projected == intended at frontier),
/// the overlay must vanish.
///
/// Note on the alpha curve: in this scenario the ship is docked at the
/// dispatcher's home system, so the dispatch-time `light_delay_to_ship`
/// is 0 and `intended_takes_effect_at == dispatched_at`. That puts every
/// observable tick on the post-takes-effect branch of
/// `intended_layer_alpha`, which holds the floor 0.4. The (0.4, 0.8]
/// curve only meaningfully decays when there is non-zero light delay
/// between the Ruler and the ship — that's covered as a sub-case below
/// using a synthetic projection (`intended_layer_alpha` is a pure helper).
#[test]
fn projected_intended_divergence_during_dispatch_window() {
    let mut app = test_app();
    let s = setup_scenario(&mut app);
    let t0 = dispatch_survey_command(&mut app, s.ship, s.frontier);
    let metadata = ship_metadata(s.ship);

    // Read the dispatch-time projection so we can inspect the alpha curve
    // independently of the renderer.
    let p_at_dispatch = get_projection(&app, s.empire, s.ship);
    let takes_effect_at = p_at_dispatch
        .intended_takes_effect_at
        .expect("dispatch must populate intended_takes_effect_at");
    assert!(
        takes_effect_at >= t0,
        "intended_takes_effect_at ({takes_effect_at}) must be >= dispatched_at ({t0})",
    );
    // For an at-home ship, dispatcher → ship light_delay is 0, so
    // takes_effect_at == dispatched_at. Pin that explicitly so a future
    // refactor of `compute_ship_projection` doesn't silently change the
    // semantic this test depends on.
    assert_eq!(
        takes_effect_at, t0,
        "Ruler and ship co-located at home → command takes effect at dispatch tick",
    );

    // (a) At dispatch tick — overlay renders (projected != intended) at
    // the floor alpha 0.4 (= command has locally reached the ship; the
    // projected state hasn't yet caught up because no fact has come back).
    {
        app.world_mut().resource_mut::<GameClock>().elapsed = t0;
        let store = app
            .world()
            .entity(s.empire)
            .get::<KnowledgeStore>()
            .unwrap();
        let items = compute_intended_render_inputs(store, &metadata, t0);
        assert_eq!(items.len(), 1, "diverged projection must produce overlay");
        let item = &items[0];
        assert_eq!(item.projected_system, Some(s.home));
        assert_eq!(item.intended_system, Some(s.frontier));
        assert!(
            (item.alpha - 0.4).abs() < 1e-4,
            "at-home dispatch alpha ({}) must equal the 0.4 floor — \
             takes_effect_at == dispatched_at",
            item.alpha,
        );
    }

    // (b) Across the entire dispatch window — overlay continues to
    // render at the 0.4 floor. The projection is *not* yet reconciled
    // (the survey-complete fact's light hasn't reached us), so the
    // dashed line stays visible.
    let samples = [
        1,
        s.light_delay / 4,
        s.light_delay / 2,
        s.light_delay - 1,
        s.light_delay,
        s.light_delay + 50,
    ];
    for tick in samples {
        app.world_mut().resource_mut::<GameClock>().elapsed = tick;
        let store = app
            .world()
            .entity(s.empire)
            .get::<KnowledgeStore>()
            .unwrap();
        let items = compute_intended_render_inputs(store, &metadata, tick);
        assert_eq!(
            items.len(),
            1,
            "overlay must continue rendering at tick {tick} until reconcile",
        );
        assert!(
            (items[0].alpha - 0.4).abs() < 1e-4,
            "alpha must hold at 0.4 floor at tick {tick} (got {})",
            items[0].alpha,
        );
        assert_eq!(items[0].projected_system, Some(s.home));
        assert_eq!(items[0].intended_system, Some(s.frontier));
    }

    // (c) Pure-helper agreement: `intended_layer_alpha` produces the
    // (0.4, 0.8] decay curve when there IS non-zero light delay between
    // dispatcher and ship. Synthesize a projection with span = 10 to
    // exercise the curve directly. This pins the #478 contract end-to-
    // end without depending on a remote-dispatch scenario the at-home
    // setup_scenario can't produce.
    let spanning = ShipProjection {
        entity: s.ship,
        dispatched_at: 0,
        expected_arrival_at: Some(15),
        expected_return_at: None,
        projected_state: ShipSnapshotState::InSystem,
        projected_system: Some(s.home),
        intended_state: Some(ShipSnapshotState::Surveying),
        intended_system: Some(s.frontier),
        intended_takes_effect_at: Some(10),
    };
    let alpha_t0 = intended_layer_alpha(&spanning, 0);
    let alpha_t5 = intended_layer_alpha(&spanning, 5);
    let alpha_t10 = intended_layer_alpha(&spanning, 10);
    let alpha_t20 = intended_layer_alpha(&spanning, 20);
    assert!(
        alpha_t0 > 0.4 && alpha_t0 <= 0.8 + 1e-4,
        "synthetic-span dispatch alpha ({alpha_t0}) must be in (0.4, 0.8]",
    );
    assert!(
        alpha_t0 > alpha_t5 && alpha_t5 > alpha_t10,
        "alpha must decay monotonically across the dispatch window \
         (t0={alpha_t0}, t5={alpha_t5}, t10={alpha_t10})",
    );
    assert!(
        (alpha_t10 - 0.4).abs() < 1e-4,
        "alpha at takes_effect_at must equal 0.4 floor (got {alpha_t10})",
    );
    assert!(
        (alpha_t20 - 0.4).abs() < 1e-4,
        "alpha must hold at floor past takes_effect_at (got {alpha_t20})",
    );

    // (d) After reconciliation: emit a ShipArrived at frontier so the
    // reconciler converges projected and intended. The overlay must
    // disappear.
    let frontier_pos = [FRONTIER_DISTANCE_LY, 0.0, 0.0];
    let observed_at = takes_effect_at + 10;
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
        observed_at,
    );
    app.world_mut().resource_mut::<GameClock>().elapsed = observed_at + s.light_delay;
    run_reconciler(&mut app);

    let p_after = get_projection(&app, s.empire, s.ship);
    assert_eq!(
        p_after.projected_system,
        Some(s.frontier),
        "reconciler must move projected_system to the arrival target",
    );
    assert!(
        p_after.intended_system.is_none(),
        "reconciler must clear intended_* on matching arrival",
    );

    let store = app
        .world()
        .entity(s.empire)
        .get::<KnowledgeStore>()
        .unwrap();
    let items = compute_intended_render_inputs(store, &metadata, observed_at + s.light_delay);
    assert!(
        items.is_empty(),
        "after reconciliation (projected == intended), the intended overlay \
         must not render — got {} items",
        items.len(),
    );
}

// ===========================================================================
// Test 3: Reconciler advances projection for each fact kind.
// ===========================================================================

/// For each of the four reconciling fact kinds — `ShipArrived`,
/// `SurveyComplete`, `ShipDestroyed`, `ShipMissing` — run the
/// dispatch → fact-emit → reconcile pipeline end-to-end with a
/// realistic light delay and verify the post-reconcile projection
/// matches the contract documented in
/// `knowledge::apply_reconciliation`.
///
/// The unit-level rules for each kind are also covered in
/// `tests/ship_projection_reconcile.rs`; this test pins the **end-to-end**
/// path including dispatch-time projection writes and the per-empire
/// vantage gate.
#[test]
fn reconcile_advances_each_fact_kind() {
    // ----- ShipArrived -----
    {
        let mut app = test_app();
        let s = setup_scenario(&mut app);
        let _t0 = dispatch_survey_command(&mut app, s.ship, s.frontier);

        let observed_at = 50;
        push_fact(
            &mut app,
            KnowledgeFact::ShipArrived {
                event_id: None,
                system: Some(s.frontier),
                name: "Scout-1".into(),
                detail: "Arrived".into(),
                ship: s.ship,
            },
            [FRONTIER_DISTANCE_LY, 0.0, 0.0],
            observed_at,
        );
        app.world_mut().resource_mut::<GameClock>().elapsed = observed_at + s.light_delay;
        run_reconciler(&mut app);

        let p = get_projection(&app, s.empire, s.ship);
        assert_eq!(p.projected_state, ShipSnapshotState::InSystem);
        assert_eq!(p.projected_system, Some(s.frontier));
        assert!(
            p.intended_state.is_none() && p.intended_system.is_none(),
            "ShipArrived at intended target must clear intended_*",
        );
        assert!(
            p.expected_arrival_at.is_none() && p.expected_return_at.is_none(),
            "ShipArrived at intended target must clear expected_*_at",
        );
    }

    // ----- SurveyComplete -----
    {
        let mut app = test_app();
        let s = setup_scenario(&mut app);
        let _t0 = dispatch_survey_command(&mut app, s.ship, s.frontier);

        // Sanity: dispatch wrote `expected_return_at` (Survey carries a
        // return leg). The SurveyComplete reconcile must NOT clear it —
        // the home-leg ShipArrived hasn't landed yet.
        let p_pre = get_projection(&app, s.empire, s.ship);
        let pre_return_at = p_pre.expected_return_at;
        assert!(
            pre_return_at.is_some(),
            "Survey dispatch should populate expected_return_at",
        );

        let observed_at = 60;
        push_fact(
            &mut app,
            KnowledgeFact::SurveyComplete {
                event_id: None,
                system: s.frontier,
                system_name: "Frontier".into(),
                detail: "Surveyed".into(),
                ship: s.ship,
            },
            [FRONTIER_DISTANCE_LY, 0.0, 0.0],
            observed_at,
        );
        app.world_mut().resource_mut::<GameClock>().elapsed = observed_at + s.light_delay;
        run_reconciler(&mut app);

        let p = get_projection(&app, s.empire, s.ship);
        assert_eq!(p.projected_state, ShipSnapshotState::InSystem);
        assert_eq!(p.projected_system, Some(s.frontier));
        assert!(
            p.intended_state.is_none() && p.intended_system.is_none(),
            "SurveyComplete at the surveying target must clear intended_*",
        );
        assert!(
            p.expected_arrival_at.is_none(),
            "SurveyComplete must clear expected_arrival_at (outbound leg observed)",
        );
        // Per `apply_reconciliation`: SurveyComplete keeps
        // expected_return_at intact for the home-leg "carrier returning"
        // hint. Pin that contract here.
        assert_eq!(
            p.expected_return_at, pre_return_at,
            "SurveyComplete must retain expected_return_at — the home-leg \
             ShipArrived has not landed yet, so the carrier-back hint is \
             still load-bearing UI state",
        );
    }

    // ----- ShipDestroyed -----
    {
        let mut app = test_app();
        let s = setup_scenario(&mut app);
        let _t0 = dispatch_survey_command(&mut app, s.ship, s.frontier);

        let observed_at = 70;
        push_fact(
            &mut app,
            KnowledgeFact::ShipDestroyed {
                event_id: None,
                system: Some(s.frontier),
                ship_name: "Scout-1".into(),
                destroyed_at: observed_at,
                detail: "Destroyed".into(),
                ship: s.ship,
            },
            [FRONTIER_DISTANCE_LY, 0.0, 0.0],
            observed_at,
        );
        app.world_mut().resource_mut::<GameClock>().elapsed = observed_at + s.light_delay;
        run_reconciler(&mut app);

        let p = get_projection(&app, s.empire, s.ship);
        assert_eq!(
            p.projected_state,
            ShipSnapshotState::Destroyed,
            "ShipDestroyed reconcile must mark the projection terminal",
        );
        assert_eq!(p.projected_system, Some(s.frontier));
        assert!(
            p.intended_state.is_none()
                && p.intended_system.is_none()
                && p.intended_takes_effect_at.is_none(),
            "ShipDestroyed must clear all intended_*",
        );
        assert!(
            p.expected_arrival_at.is_none() && p.expected_return_at.is_none(),
            "ShipDestroyed must clear all expected_*_at",
        );
        // Projection retained for situational memory (the "graveyard"
        // marker in the UI).
        assert!(
            app.world()
                .entity(s.empire)
                .get::<KnowledgeStore>()
                .unwrap()
                .get_projection(s.ship)
                .is_some(),
            "Destroyed reconcile must NOT call clear_projection",
        );
    }

    // ----- ShipMissing -----
    {
        let mut app = test_app();
        let s = setup_scenario(&mut app);
        let _t0 = dispatch_survey_command(&mut app, s.ship, s.frontier);

        let observed_at = 80;
        push_fact(
            &mut app,
            KnowledgeFact::ShipMissing {
                event_id: None,
                system: Some(s.frontier),
                ship_name: "Scout-1".into(),
                detail: "Missing".into(),
                ship: s.ship,
            },
            [FRONTIER_DISTANCE_LY, 0.0, 0.0],
            observed_at,
        );
        app.world_mut().resource_mut::<GameClock>().elapsed = observed_at + s.light_delay;
        run_reconciler(&mut app);

        let p = get_projection(&app, s.empire, s.ship);
        assert_eq!(
            p.projected_state,
            ShipSnapshotState::Missing,
            "ShipMissing reconcile must mark the projection amber/missing",
        );
        assert_eq!(p.projected_system, Some(s.frontier));
        assert!(
            p.intended_state.is_none() && p.intended_system.is_none(),
            "ShipMissing must clear intended_*",
        );
        // Projection retained — same retain-rule as Destroyed.
        assert!(
            app.world()
                .entity(s.empire)
                .get::<KnowledgeStore>()
                .unwrap()
                .get_projection(s.ship)
                .is_some(),
            "Missing reconcile must NOT call clear_projection",
        );
    }
}

// ===========================================================================
// Test 4: Save/load mid-flight preserves projection.
// ===========================================================================

/// At a mid-flight tick (dispatch tick + 50, well before light_delay
/// elapses), save the world via the postcard pipeline and load it into
/// a fresh world. Verify the projection round-trips field-for-field
/// and that the loaded world can continue advancing into reconciliation
/// — i.e. emit a `SurveyComplete` against the loaded world and observe
/// the reconciler land it.
///
/// This pins the #474 persistence shim's mid-flight contract:
/// `dispatched_at`, `expected_*_at`, `projected_*`, `intended_*` all
/// survive the round-trip with stable `Entity` remapping.
#[test]
fn save_load_mid_flight_preserves_projection() {
    let mut src = test_app();
    let s = setup_scenario(&mut src);
    let _t0 = dispatch_survey_command(&mut src, s.ship, s.frontier);

    // Advance to mid-flight. The light_delay is 300; pick 50 so we are
    // unambiguously mid-window with reconciliation not yet triggered.
    let mid_flight_tick = 50;
    src.world_mut().resource_mut::<GameClock>().elapsed = mid_flight_tick;

    // Snapshot the source projection so we can compare field-for-field
    // post-load.
    let p_src = get_projection(&src, s.empire, s.ship);
    assert_eq!(p_src.dispatched_at, 0);
    assert_eq!(p_src.intended_system, Some(s.frontier));
    assert_eq!(p_src.intended_state, Some(ShipSnapshotState::Surveying));
    assert!(
        p_src.expected_arrival_at.is_some(),
        "Survey dispatch must populate expected_arrival_at pre-save",
    );
    assert!(
        p_src.expected_return_at.is_some(),
        "Survey dispatch must populate expected_return_at pre-save",
    );
    let pre_save_intended_takes_effect = p_src
        .intended_takes_effect_at
        .expect("dispatch must populate intended_takes_effect_at");

    // Save → load.
    let mut bytes: Vec<u8> = Vec::new();
    save_game_to_writer(src.world_mut(), &mut bytes).expect("save");
    assert!(!bytes.is_empty(), "postcard produced an empty blob");

    let mut dst = World::new();
    load_game_from_reader(&mut dst, &bytes[..]).expect("load");

    // Locate the empire's KnowledgeStore in the loaded world (Entity
    // ids are remapped, so we have to re-query).
    let dst_empire = dst
        .query_filtered::<Entity, With<PlayerEmpire>>()
        .iter(&dst)
        .next()
        .expect("PlayerEmpire must round-trip");

    let store = dst
        .entity(dst_empire)
        .get::<KnowledgeStore>()
        .expect("loaded empire has KnowledgeStore");

    // The projection key (= ship entity) is also remapped; iterate to
    // find the survey projection by its intended_state.
    let (loaded_ship, p_loaded) = store
        .iter_projections()
        .next()
        .map(|(e, p)| (*e, p.clone()))
        .expect("at least one ShipProjection must round-trip");

    assert_eq!(
        p_loaded.dispatched_at, p_src.dispatched_at,
        "dispatched_at must round-trip exactly",
    );
    assert_eq!(
        p_loaded.expected_arrival_at, p_src.expected_arrival_at,
        "expected_arrival_at must round-trip exactly",
    );
    assert_eq!(
        p_loaded.expected_return_at, p_src.expected_return_at,
        "expected_return_at must round-trip exactly",
    );
    assert_eq!(
        p_loaded.projected_state, p_src.projected_state,
        "projected_state must round-trip exactly",
    );
    assert_eq!(
        p_loaded.intended_state, p_src.intended_state,
        "intended_state must round-trip exactly",
    );
    assert_eq!(
        p_loaded.intended_takes_effect_at,
        Some(pre_save_intended_takes_effect),
        "intended_takes_effect_at must round-trip exactly",
    );

    // Resolve the remapped frontier system entity to verify the
    // intended_system pointer survived `EntityMap`.
    let dst_frontier = dst
        .query::<(Entity, &macrocosmo::galaxy::StarSystem)>()
        .iter(&dst)
        .find(|(_, sys)| sys.name == "Frontier")
        .map(|(e, _)| e)
        .expect("Frontier system must round-trip");
    assert_eq!(
        p_loaded.intended_system,
        Some(dst_frontier),
        "intended_system must remap through EntityMap to the loaded Frontier",
    );

    // Continuation: make sure the loaded world can reconcile a
    // SurveyComplete fact against the round-tripped projection. The
    // load pipeline is responsible for installing PendingFactQueue +
    // RelayNetwork, but to be defensive we ensure they exist before
    // running the reconciler in isolation.
    if dst.get_resource::<PendingFactQueue>().is_none() {
        dst.insert_resource(PendingFactQueue::default());
    }
    if dst
        .get_resource::<macrocosmo::knowledge::RelayNetwork>()
        .is_none()
    {
        dst.insert_resource(macrocosmo::knowledge::RelayNetwork::default());
    }

    // Build a tiny scaffold App around the loaded world so we can run
    // the reconciler as a system. Bevy's World has no ergonomic
    // run_system_once for queries-with-filters in 0.18, so we re-use a
    // Schedule.
    let observed_at = 200;
    let pf = PerceivedFact {
        fact: KnowledgeFact::SurveyComplete {
            event_id: None,
            system: dst_frontier,
            system_name: "Frontier".into(),
            detail: "Surveyed".into(),
            ship: loaded_ship,
        },
        observed_at,
        arrives_at: observed_at,
        source: ObservationSource::Direct,
        origin_pos: [FRONTIER_DISTANCE_LY, 0.0, 0.0],
        related_system: Some(dst_frontier),
    };
    dst.resource_mut::<PendingFactQueue>().record(pf);

    // Move the loaded clock past per-empire light arrival.
    if dst.get_resource::<GameClock>().is_none() {
        dst.insert_resource(GameClock::new(observed_at + s.light_delay + 1));
    } else {
        dst.resource_mut::<GameClock>().elapsed = observed_at + s.light_delay + 1;
    }

    let mut schedule = bevy::ecs::schedule::Schedule::default();
    schedule.add_systems(reconcile_ship_projections);
    schedule.run(&mut dst);

    let store = dst
        .entity(dst_empire)
        .get::<KnowledgeStore>()
        .expect("loaded empire has KnowledgeStore");
    let p_after = store
        .get_projection(loaded_ship)
        .expect("projection retained after reconcile");
    assert_eq!(
        p_after.projected_state,
        ShipSnapshotState::InSystem,
        "loaded-world reconcile must advance projected_state to InSystem",
    );
    assert_eq!(
        p_after.projected_system,
        Some(dst_frontier),
        "loaded-world reconcile must advance projected_system to the surveyed frontier",
    );
    assert!(
        p_after.intended_system.is_none() && p_after.intended_state.is_none(),
        "loaded-world reconcile must clear intended_* on matching SurveyComplete",
    );
}
