//! #478: Galaxy Map intended-trajectory overlay (dashed/translucent
//! layer) atop the projected layer #477 landed.
//!
//! These tests pin the **input data** the renderer consumes — the actual
//! gizmo emission is excluded from `test_app()` (visualization systems
//! need an EguiContext) so we exercise the pure helpers
//! [`compute_intended_render_inputs`] and [`intended_layer_alpha`].
//!
//! The visual contract: the dashed line + ring are drawn *only* when
//! the intended target diverges from the projected position (= command
//! in flight or ship en route). Once the reconciler converges them, the
//! overlay disappears.

mod common;

use std::collections::HashMap;

use bevy::prelude::*;

use macrocosmo::knowledge::{KnowledgeStore, ShipProjection, ShipSnapshotState};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::visualization::ships::{
    OwnShipMetadata, compute_intended_render_inputs, intended_layer_alpha,
    intended_layer_dash_pattern,
};

use common::{spawn_test_system, test_app};

// ---------------------------------------------------------------------------
// Shared scenario helpers
// ---------------------------------------------------------------------------

fn spawn_minimal_empire(app: &mut App) -> Entity {
    app.world_mut()
        .spawn((
            Empire {
                name: "Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "intended_render_test".into(),
                name: "Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            KnowledgeStore::default(),
            macrocosmo::knowledge::SystemVisibilityMap::default(),
        ))
        .id()
}

fn meta(design_id: &str, owned_by_viewing_empire: bool) -> OwnShipMetadata {
    OwnShipMetadata {
        design_id: design_id.into(),
        is_station: false,
        is_harbour: false,
        owned_by_viewing_empire,
    }
}

fn projection_with_intent(
    ship: Entity,
    projected_system: Option<Entity>,
    projected_state: ShipSnapshotState,
    intended_system: Option<Entity>,
    intended_state: Option<ShipSnapshotState>,
    dispatched_at: i64,
    takes_effect_at: Option<i64>,
) -> ShipProjection {
    ShipProjection {
        entity: ship,
        dispatched_at,
        expected_arrival_at: takes_effect_at.map(|t| t + 5),
        expected_return_at: None,
        projected_state,
        projected_system,
        intended_state,
        intended_system,
        intended_takes_effect_at: takes_effect_at,
    }
}

// ---------------------------------------------------------------------------
// 1. intended_layer_renders_when_diverged
// ---------------------------------------------------------------------------

/// Projection at A with intent toward B → intended item is emitted.
#[test]
fn intended_layer_renders_when_diverged() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);

    let a = spawn_test_system(app.world_mut(), "A", [0.0, 0.0, 0.0], 1.0, true, true);
    let b = spawn_test_system(app.world_mut(), "B", [10.0, 0.0, 0.0], 1.0, true, true);

    let ship = app.world_mut().spawn_empty().id();
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(projection_with_intent(
            ship,
            Some(a),
            ShipSnapshotState::InSystem,
            Some(b),
            Some(ShipSnapshotState::Surveying),
            0,
            Some(5),
        ));
    }

    let mut metadata = HashMap::new();
    metadata.insert(ship, meta("explorer_mk1", true));

    let store = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire has KnowledgeStore");

    // At t=0 (dispatch tick): full divergence — should appear with high alpha.
    let items = compute_intended_render_inputs(store, &metadata, 0);
    assert_eq!(items.len(), 1, "diverged projection must produce one item");
    let item = &items[0];
    assert_eq!(item.entity, ship);
    assert_eq!(item.projected_system, Some(a));
    assert_eq!(item.intended_system, Some(b));
    // #489: widened curve — at dispatch, alpha rides the ceiling (1.0).
    assert!(
        item.alpha > 0.3,
        "alpha at dispatch tick must be elevated above the floor (got {})",
        item.alpha
    );
}

// ---------------------------------------------------------------------------
// 2. intended_layer_hidden_when_converged
// ---------------------------------------------------------------------------

/// Once projected_system == intended_system (= reconciler observed
/// arrival), the dashed overlay must disappear.
#[test]
fn intended_layer_hidden_when_converged() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);

    let target = spawn_test_system(app.world_mut(), "T", [0.0, 0.0, 0.0], 1.0, true, true);

    let ship = app.world_mut().spawn_empty().id();
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(projection_with_intent(
            ship,
            Some(target),
            ShipSnapshotState::Surveying,
            Some(target),
            Some(ShipSnapshotState::Surveying),
            0,
            Some(5),
        ));
    }

    let mut metadata = HashMap::new();
    metadata.insert(ship, meta("explorer_mk1", true));

    let store = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire has KnowledgeStore");

    let items = compute_intended_render_inputs(store, &metadata, 10);
    assert!(
        items.is_empty(),
        "converged projection must not produce any intended overlay"
    );
}

// ---------------------------------------------------------------------------
// 3. intended_layer_hidden_when_no_intent
// ---------------------------------------------------------------------------

/// Steady-state projection (no in-flight command) → no intended overlay.
#[test]
fn intended_layer_hidden_when_no_intent() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);

    let home = spawn_test_system(app.world_mut(), "H", [0.0, 0.0, 0.0], 1.0, true, true);

    let ship = app.world_mut().spawn_empty().id();
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(projection_with_intent(
            ship,
            Some(home),
            ShipSnapshotState::InSystem,
            None, // no intent
            None,
            0,
            None,
        ));
    }

    let mut metadata = HashMap::new();
    metadata.insert(ship, meta("explorer_mk1", true));

    let store = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire has KnowledgeStore");

    let items = compute_intended_render_inputs(store, &metadata, 0);
    assert!(
        items.is_empty(),
        "projection with intended_system=None must not produce overlay"
    );
}

// ---------------------------------------------------------------------------
// 4. intended_alpha_fades_with_clock
// ---------------------------------------------------------------------------

/// As the clock advances from `dispatched_at` toward
/// `intended_takes_effect_at`, the alpha decays monotonically.
#[test]
fn intended_alpha_fades_with_clock() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);

    let a = spawn_test_system(app.world_mut(), "A", [0.0, 0.0, 0.0], 1.0, true, true);
    let b = spawn_test_system(app.world_mut(), "B", [10.0, 0.0, 0.0], 1.0, true, true);

    let ship = app.world_mut().spawn_empty().id();
    let proj = projection_with_intent(
        ship,
        Some(a),
        ShipSnapshotState::InSystem,
        Some(b),
        Some(ShipSnapshotState::Surveying),
        0,
        Some(10),
    );
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(proj.clone());
    }

    let mut metadata = HashMap::new();
    metadata.insert(ship, meta("explorer_mk1", true));

    let store = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire has KnowledgeStore");

    let alpha_t0 = compute_intended_render_inputs(store, &metadata, 0)[0].alpha;
    let alpha_t5 = compute_intended_render_inputs(store, &metadata, 5)[0].alpha;
    let alpha_t10 = compute_intended_render_inputs(store, &metadata, 10)[0].alpha;
    let alpha_t20 = compute_intended_render_inputs(store, &metadata, 20)[0].alpha;

    assert!(
        alpha_t0 > alpha_t5,
        "alpha must decrease as clock approaches takes_effect_at \
         (t0={}, t5={})",
        alpha_t0,
        alpha_t5
    );
    assert!(
        alpha_t5 > alpha_t10,
        "alpha must continue decreasing pre-arrival \
         (t5={}, t10={})",
        alpha_t5,
        alpha_t10
    );
    // #489: floor widened from 0.4 to 0.3 to expand the perceptible
    // delta against the dark Galaxy Map background.
    assert!(
        (alpha_t10 - 0.3).abs() < 1e-4,
        "alpha at takes_effect_at must reach the floor 0.3 (got {})",
        alpha_t10
    );
    assert!(
        (alpha_t20 - 0.3).abs() < 1e-4,
        "alpha after takes_effect_at must hold at the floor 0.3 (got {})",
        alpha_t20
    );

    // Pure helper agreement.
    assert!((intended_layer_alpha(&proj, 0) - alpha_t0).abs() < 1e-6);
    assert!((intended_layer_alpha(&proj, 5) - alpha_t5).abs() < 1e-6);
}

// ---------------------------------------------------------------------------
// 5. destroyed_projection_no_intended_layer
// ---------------------------------------------------------------------------

/// A projection whose projected_state is Destroyed must not surface an
/// intended overlay even if `intended_*` happens to still be populated.
/// (The reconciler clears intent on Destroyed/Missing, but the renderer
/// is defense-in-depth here.)
#[test]
fn destroyed_projection_no_intended_layer() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);

    let a = spawn_test_system(app.world_mut(), "A", [0.0, 0.0, 0.0], 1.0, true, true);
    let b = spawn_test_system(app.world_mut(), "B", [10.0, 0.0, 0.0], 1.0, true, true);

    let ship = app.world_mut().spawn_empty().id();
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(projection_with_intent(
            ship,
            Some(a),
            ShipSnapshotState::Destroyed,
            Some(b),
            Some(ShipSnapshotState::Surveying),
            0,
            Some(5),
        ));
    }

    let mut metadata = HashMap::new();
    metadata.insert(ship, meta("explorer_mk1", true));

    let store = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire has KnowledgeStore");

    let items = compute_intended_render_inputs(store, &metadata, 0);
    assert!(
        items.is_empty(),
        "Destroyed projection must NOT produce an intended overlay item"
    );
}

// ---------------------------------------------------------------------------
// 6. foreign_or_despawned_filtered (bonus)
// ---------------------------------------------------------------------------

/// Foreign-empire and despawned ships must be filtered, mirroring the
/// projected-layer filter (#477).
#[test]
fn foreign_or_despawned_filtered() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);

    let a = spawn_test_system(app.world_mut(), "A", [0.0, 0.0, 0.0], 1.0, true, true);
    let b = spawn_test_system(app.world_mut(), "B", [10.0, 0.0, 0.0], 1.0, true, true);

    let foreign_ship = app.world_mut().spawn_empty().id();
    let despawned_ship = app.world_mut().spawn_empty().id();
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(projection_with_intent(
            foreign_ship,
            Some(a),
            ShipSnapshotState::InSystem,
            Some(b),
            Some(ShipSnapshotState::Surveying),
            0,
            Some(5),
        ));
        store.update_projection(projection_with_intent(
            despawned_ship,
            Some(a),
            ShipSnapshotState::InSystem,
            Some(b),
            Some(ShipSnapshotState::Surveying),
            0,
            Some(5),
        ));
    }

    let mut metadata = HashMap::new();
    metadata.insert(foreign_ship, meta("explorer_mk1", false));
    // despawned_ship has no metadata entry.

    let store = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire has KnowledgeStore");

    let items = compute_intended_render_inputs(store, &metadata, 0);
    assert!(
        items.is_empty(),
        "foreign + despawned ships must be filtered from the intended overlay"
    );
}

// ---------------------------------------------------------------------------
// 7. alpha_curve_widened_range (#489)
// ---------------------------------------------------------------------------

/// #489: The dispatch-window alpha range was widened from `0.4 → 0.8` to
/// `0.3 → 1.0` so the three phases (dispatch / mid / takes-effect) are
/// visually distinguishable against the dark Galaxy Map background.
///
/// Pin both endpoints so a regression that re-narrows the curve fails CI.
#[test]
fn alpha_curve_widened_range() {
    let mut app = test_app();
    let ship = app.world_mut().spawn_empty().id();
    let proj = projection_with_intent(
        ship,
        None,
        ShipSnapshotState::InSystem,
        None,
        Some(ShipSnapshotState::Surveying),
        0,
        Some(10),
    );

    // Top of the new range (dispatch tick): alpha must be ≥ 0.9 — the
    // pre-#489 ceiling of 0.8 is no longer enough.
    let alpha_dispatch = intended_layer_alpha(&proj, 0);
    assert!(
        alpha_dispatch >= 0.9,
        "alpha at dispatch must hit the widened ceiling (got {})",
        alpha_dispatch
    );

    // Bottom of the new range (post-takes-effect): alpha must be ≤ 0.4
    // — the pre-#489 floor of 0.4 is now 0.3.
    let alpha_after = intended_layer_alpha(&proj, 30);
    assert!(
        alpha_after <= 0.4,
        "alpha after takes_effect_at must sit at the widened floor (got {})",
        alpha_after
    );

    // Cross-check: total span (ceiling - floor) is now ≥ 0.6 (was 0.4
    // pre-#489).
    let span = alpha_dispatch - alpha_after;
    assert!(
        span >= 0.6,
        "alpha curve span must be widened (got {} = {} - {})",
        span,
        alpha_dispatch,
        alpha_after
    );
}

// ---------------------------------------------------------------------------
// 8. dash_pattern_varies_with_clock (#489)
// ---------------------------------------------------------------------------

/// #489: The dashed-line dash/gap pattern is the second perceptible
/// channel — short urgent dashes at dispatch, long settled dashes once
/// the command has reached the ship.
///
/// Pin: dash_length at dispatch < dash_length post-takes-effect, and
/// the helper agrees with `compute_intended_render_inputs` outputs.
#[test]
fn dash_pattern_varies_with_clock() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);

    let a = spawn_test_system(app.world_mut(), "A", [0.0, 0.0, 0.0], 1.0, true, true);
    let b = spawn_test_system(app.world_mut(), "B", [10.0, 0.0, 0.0], 1.0, true, true);

    let ship = app.world_mut().spawn_empty().id();
    let proj = projection_with_intent(
        ship,
        Some(a),
        ShipSnapshotState::InSystem,
        Some(b),
        Some(ShipSnapshotState::Surveying),
        0,
        Some(10),
    );
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(proj.clone());
    }

    let mut metadata = HashMap::new();
    metadata.insert(ship, meta("explorer_mk1", true));

    let store = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire has KnowledgeStore");

    let item_dispatch = &compute_intended_render_inputs(store, &metadata, 0)[0];
    let item_settled = &compute_intended_render_inputs(store, &metadata, 20)[0];

    let (dash_dispatch, gap_dispatch) = item_dispatch.dash_pattern;
    let (dash_settled, gap_settled) = item_settled.dash_pattern;

    assert!(
        dash_dispatch < dash_settled,
        "dispatch dashes must be shorter than settled dashes \
         (dispatch={}, settled={})",
        dash_dispatch,
        dash_settled
    );
    assert!(
        gap_dispatch < gap_settled,
        "dispatch gaps must be shorter than settled gaps \
         (dispatch={}, settled={})",
        gap_dispatch,
        gap_settled
    );

    // Pure helper agreement (mirrors the alpha-helper agreement check
    // in `intended_alpha_fades_with_clock`).
    let helper_dispatch = intended_layer_dash_pattern(&proj, 0);
    let helper_settled = intended_layer_dash_pattern(&proj, 20);
    assert!((helper_dispatch.0 - dash_dispatch).abs() < 1e-6);
    assert!((helper_dispatch.1 - gap_dispatch).abs() < 1e-6);
    assert!((helper_settled.0 - dash_settled).abs() < 1e-6);
    assert!((helper_settled.1 - gap_settled).abs() < 1e-6);

    // Mid-window must lie strictly between the two endpoints — proves
    // the linear interpolation, not just two discrete states.
    let helper_mid = intended_layer_dash_pattern(&proj, 5);
    assert!(
        helper_mid.0 > dash_dispatch && helper_mid.0 < dash_settled,
        "mid-window dash length must interpolate between endpoints \
         (mid={}, dispatch={}, settled={})",
        helper_mid.0,
        dash_dispatch,
        dash_settled
    );
}

// ---------------------------------------------------------------------------
// #496: saturation guard — `intended_takes_effect_at` close to `i64::MAX`
// (= release-build slip-through past the producer's `debug_assert!` in
// `compute_ship_projection`) must NOT pin the alpha curve at the
// dispatch-fresh ceiling forever. The renderer-side guard short-circuits
// to the steady-state floor / settled dash pattern.
// ---------------------------------------------------------------------------

fn saturated_projection(now: i64) -> ShipProjection {
    ShipProjection {
        entity: Entity::PLACEHOLDER,
        dispatched_at: now,
        expected_arrival_at: Some(i64::MAX),
        expected_return_at: None,
        projected_state: ShipSnapshotState::InTransitSubLight,
        projected_system: None,
        intended_state: Some(ShipSnapshotState::InTransitSubLight),
        intended_system: None,
        intended_takes_effect_at: Some(i64::MAX),
    }
}

#[test]
fn intended_layer_alpha_floors_at_saturation() {
    let proj = saturated_projection(0);
    // Without the #496 guard, `(i64::MAX - 0) as f32` would yield
    // ~9.22e18, the clamp would pin `fraction = 1.0`, and alpha would
    // come back at the dispatch ceiling forever.
    let alpha = intended_layer_alpha(&proj, 0);
    // INTENDED_ALPHA_FLOOR is 0.3 (private to ships.rs, so reproduce
    // the contract here): the steady-state value the helper falls
    // back to when no in-flight curve can be computed.
    assert!(
        (alpha - 0.3).abs() < 1e-6,
        "saturated takes_effect_at must collapse to floor 0.3, got {}",
        alpha
    );

    // Same after the clock has advanced — must still be the floor, not
    // a slowly diverging curve.
    let alpha_later = intended_layer_alpha(&proj, 10_000);
    assert!(
        (alpha_later - 0.3).abs() < 1e-6,
        "saturated takes_effect_at must remain at floor 0.3 even after clock advance, got {}",
        alpha_later
    );
}

#[test]
fn intended_layer_dash_pattern_settles_at_saturation() {
    let proj = saturated_projection(0);
    // Without the #496 guard, the helper would produce the urgent
    // dispatch pattern (4.0, 2.0) forever.
    let pattern = intended_layer_dash_pattern(&proj, 0);
    // INTENDED_DASH_AFTER_TAKES_EFFECT is (8.0, 4.0).
    assert!(
        (pattern.0 - 8.0).abs() < 1e-6 && (pattern.1 - 4.0).abs() < 1e-6,
        "saturated takes_effect_at must collapse to settled (8.0, 4.0), got ({}, {})",
        pattern.0,
        pattern.1
    );

    let pattern_later = intended_layer_dash_pattern(&proj, 10_000);
    assert!(
        (pattern_later.0 - 8.0).abs() < 1e-6 && (pattern_later.1 - 4.0).abs() < 1e-6,
        "saturated takes_effect_at must remain at settled (8.0, 4.0) after clock advance, got ({}, {})",
        pattern_later.0,
        pattern_later.1
    );
}

#[test]
fn intended_layer_helpers_unaffected_by_normal_far_future_takes_effect() {
    // Sanity guard: a takes_effect_at well below `i64::MAX / 2` (= the
    // saturation threshold) must NOT trigger the short-circuit — the
    // normal interpolation curve still applies. Pick a value 100x the
    // typical galactic light-delay (~6000 hexadies for 100 ly) but
    // still tiny vs `i64::MAX / 2 ≈ 4.6e18`.
    let proj = ShipProjection {
        entity: Entity::PLACEHOLDER,
        dispatched_at: 0,
        expected_arrival_at: Some(1_000_000),
        expected_return_at: None,
        projected_state: ShipSnapshotState::InTransitSubLight,
        projected_system: None,
        intended_state: Some(ShipSnapshotState::InTransitSubLight),
        intended_system: None,
        intended_takes_effect_at: Some(600_000),
    };
    let alpha = intended_layer_alpha(&proj, 0);
    // Right at dispatch should be at/near the ceiling 1.0, not floor.
    assert!(
        alpha > 0.9,
        "non-saturated dispatch-tick alpha must be near ceiling 1.0, got {}",
        alpha
    );
}
