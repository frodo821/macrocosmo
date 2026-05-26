//! #532: Galaxy Map own-ship marker interpolation between
//! `intended_takes_effect_at` and `expected_arrival_at`.
//!
//! These tests pin the four-region contract of the
//! [`own_ship_marker_screen_pos`] / [`intended_lerp_fraction`] pair:
//!
//! 1. `now < intended_takes_effect_at` (= PR #530's dispatch window) →
//!    marker stays at the projected (origin) position. This is the
//!    no-FTL-leak invariant.
//! 2. `intended_takes_effect_at <= now <= expected_arrival_at` →
//!    marker is linearly interpolated from projected → intended.
//! 3. `now == expected_arrival_at` → marker pinned at the intended
//!    (destination) position.
//! 4. `now > expected_arrival_at` (post-arrival, simulation hasn't
//!    reconciled yet) → marker stays clamped at the intended position.
//!
//! The interpolation behaviour is exercised via the pure-math helper so
//! the test doesn't need a Bevy `Query` — the real renderer
//! [`projection_screen_pos`] resolves star positions from the world and
//! then delegates to the same helper.

use bevy::prelude::*;

use macrocosmo::components::Position;
use macrocosmo::knowledge::{ShipProjection, ShipSnapshotState};
use macrocosmo::visualization::ships::{
    intended_lerp_fraction, own_ship_marker_screen_pos,
    should_draw_projected_marker_with_interpolation,
};

const VIEW_SCALE: f32 = 1.0;

fn make_projection(intended_state: Option<ShipSnapshotState>) -> ShipProjection {
    ShipProjection {
        // `Entity::PLACEHOLDER` is fine — the helpers under test never
        // dereference the entity id.
        entity: Entity::PLACEHOLDER,
        dispatched_at: 0,
        expected_arrival_at: Some(50),
        expected_return_at: None,
        projected_state: ShipSnapshotState::InSystem,
        projected_system: Some(Entity::PLACEHOLDER),
        intended_state,
        intended_system: Some(Entity::PLACEHOLDER),
        intended_takes_effect_at: Some(10),
    }
}

fn origin() -> Position {
    Position {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    }
}

fn dest() -> Position {
    Position {
        x: 100.0,
        y: 0.0,
        z: 0.0,
    }
}

// ---------------------------------------------------------------------------
// 1. Pre-effect (dispatch window) — marker stays at the projected origin.
// ---------------------------------------------------------------------------

#[test]
fn pre_effect_marker_pinned_at_projected_system() {
    let p = make_projection(Some(ShipSnapshotState::InTransitFTL));
    // now = 5; intended_takes_effect_at = 10.
    let pos = own_ship_marker_screen_pos(&p, &origin(), Some(&dest()), VIEW_SCALE, 5);
    assert!(
        (pos - Vec2::new(0.0, 0.0)).length() < 1e-4,
        "pre-effect marker must stay at origin (PR #530 no-FTL-leak \
         invariant); got {pos:?}",
    );
    assert!(
        intended_lerp_fraction(&p, 5).is_none(),
        "pre-effect lerp fraction must be None so callers short-circuit \
         to the projected position",
    );
}

// ---------------------------------------------------------------------------
// 2. Mid-flight — marker at 50% of the leg at the midpoint of
//    [takes_effect_at, expected_arrival_at].
// ---------------------------------------------------------------------------

#[test]
fn mid_flight_marker_interpolated() {
    let p = make_projection(Some(ShipSnapshotState::InTransitFTL));
    // now = 30. takes_effect=10, arrival=50 → fraction = (30-10)/(50-10) = 0.5.
    let pos = own_ship_marker_screen_pos(&p, &origin(), Some(&dest()), VIEW_SCALE, 30);
    assert!(
        (pos - Vec2::new(50.0, 0.0)).length() < 1e-3,
        "mid-flight (50%) marker must lerp to halfway between origin \
         (0,0) and destination (100,0); got {pos:?}",
    );

    let frac = intended_lerp_fraction(&p, 30).expect("mid-flight has a lerp fraction");
    assert!(
        (frac - 0.5).abs() < 1e-4,
        "mid-flight fraction must be 0.5; got {frac}",
    );
}

// ---------------------------------------------------------------------------
// 3. Arrival tick — marker at destination.
// ---------------------------------------------------------------------------

#[test]
fn arrival_tick_marker_at_destination() {
    let p = make_projection(Some(ShipSnapshotState::InTransitSubLight));
    // now = 50. fraction = (50-10)/(50-10) = 1.0.
    let pos = own_ship_marker_screen_pos(&p, &origin(), Some(&dest()), VIEW_SCALE, 50);
    assert!(
        (pos - Vec2::new(100.0, 0.0)).length() < 1e-3,
        "arrival-tick marker must sit at the destination; got {pos:?}",
    );
}

// ---------------------------------------------------------------------------
// 4. Past arrival — marker clamps at destination until reconcile.
// ---------------------------------------------------------------------------

#[test]
fn past_arrival_marker_clamped_at_destination() {
    let p = make_projection(Some(ShipSnapshotState::InTransitFTL));
    // now = 60 > expected_arrival_at = 50 — clamp to 1.0.
    let pos = own_ship_marker_screen_pos(&p, &origin(), Some(&dest()), VIEW_SCALE, 60);
    assert!(
        (pos - Vec2::new(100.0, 0.0)).length() < 1e-3,
        "past-arrival marker must clamp at the destination (no \
         extrapolation past the intended system); got {pos:?}",
    );

    let frac =
        intended_lerp_fraction(&p, 60).expect("post-arrival still produces a clamped fraction");
    assert!(
        (frac - 1.0).abs() < 1e-4,
        "past-arrival fraction must be clamped to 1.0; got {frac}",
    );
}

// ---------------------------------------------------------------------------
// 5. Non-transit intended state — no interpolation even after takes_effect.
// ---------------------------------------------------------------------------

/// When the command would put the ship into a *non*-transit state
/// (Surveying, Settling, etc.) the marker must NOT slide between
/// systems — those activities anchor the ship at the intended system.
/// The marker should stay at the projected position until the
/// projection reconciles and `projected_system` updates.
#[test]
fn non_transit_intended_state_does_not_interpolate() {
    let p = make_projection(Some(ShipSnapshotState::Surveying));
    let pos = own_ship_marker_screen_pos(&p, &origin(), Some(&dest()), VIEW_SCALE, 30);
    assert!(
        (pos - Vec2::new(0.0, 0.0)).length() < 1e-4,
        "non-transit intended state must not trigger lerp; got {pos:?}",
    );
    assert!(
        intended_lerp_fraction(&p, 30).is_none(),
        "non-transit intended state must produce no lerp fraction",
    );
}

// ---------------------------------------------------------------------------
// 6. Missing intended timing — graceful fall-through to projected.
// ---------------------------------------------------------------------------

/// If `expected_arrival_at` is missing (= a legacy / fallback projection
/// that didn't fill the arrival ETA) we cannot interpolate — the lerp
/// endpoint is undefined. The marker must stay at the projected
/// position rather than divide-by-zero or guess.
#[test]
fn missing_arrival_eta_falls_back_to_projected() {
    let mut p = make_projection(Some(ShipSnapshotState::InTransitFTL));
    p.expected_arrival_at = None;
    let pos = own_ship_marker_screen_pos(&p, &origin(), Some(&dest()), VIEW_SCALE, 30);
    assert!(
        (pos - Vec2::new(0.0, 0.0)).length() < 1e-4,
        "missing arrival ETA must fall back to projected position; \
         got {pos:?}",
    );
    assert!(
        intended_lerp_fraction(&p, 30).is_none(),
        "missing arrival ETA must produce no lerp fraction",
    );
}

// ---------------------------------------------------------------------------
// 7. No intended target — pure projection fallback.
// ---------------------------------------------------------------------------

/// Steady-state projections (no in-flight command) have
/// `intended_state == None`. The marker must stay at the projected
/// position — no interpolation possible without an intended target.
#[test]
fn steady_state_marker_at_projected() {
    let p = make_projection(None);
    let pos = own_ship_marker_screen_pos(&p, &origin(), None, VIEW_SCALE, 30);
    assert!(
        (pos - Vec2::new(0.0, 0.0)).length() < 1e-4,
        "steady-state projection must place marker at projected \
         position; got {pos:?}",
    );
}

// ===========================================================================
// #532 follow-up: routing decision for the `InSystem | Refitting` branch
// of `draw_ships`.
//
// PR #534 wired interpolation into the `InTransit*` arm only. But
// `compute_ship_projection` keeps `projected_state = InSystem` from the
// last known snapshot while populating `intended_state = InTransit*` —
// so the common post-dispatch projection routed through the docked
// grouping arm and the interpolation never fired.
//
// These tests pin the routing predicate that lets `draw_ships` detour
// `InSystem` projections through `projection_screen_pos` when (and only
// when) the four jointly-required conditions hold.
// ===========================================================================

/// Make a projection with `projected_state = InSystem` at `origin` but
/// `intended_state = InTransit*` heading to `destination`. The
/// helper-under-test reads only the projection itself, so we use two
/// distinguishable placeholder entities to model origin != destination.
fn make_in_system_in_transit_projection(
    intended_state: ShipSnapshotState,
    intended_takes_effect_at: Option<i64>,
    expected_arrival_at: Option<i64>,
) -> ShipProjection {
    // Two synthetic entity ids so origin != destination. The routing
    // helper never derefs these — it only compares them for equality.
    let origin_sys = Entity::from_bits(1);
    let destination_sys = Entity::from_bits(2);
    ShipProjection {
        entity: Entity::PLACEHOLDER,
        dispatched_at: 0,
        expected_arrival_at,
        expected_return_at: None,
        // The whole point: projected_state is the docked-style state
        // (carried over from the last known snapshot) while the
        // intended layer has already become a transit.
        projected_state: ShipSnapshotState::InSystem,
        projected_system: Some(origin_sys),
        intended_state: Some(intended_state),
        intended_system: Some(destination_sys),
        intended_takes_effect_at,
    }
}

/// Pre-effect: command has been dispatched but has not yet locally
/// reached the ship. The marker MUST stay docked at the projected
/// system — this preserves PR #530's no-FTL-leak contract.
#[test]
fn projected_in_system_pre_effect_remains_docked() {
    let p =
        make_in_system_in_transit_projection(ShipSnapshotState::InTransitFTL, Some(100), Some(200));
    // now=50 < takes_effect_at=100 → pre-effect.
    assert!(
        !should_draw_projected_marker_with_interpolation(&p, 50),
        "pre-effect (now < intended_takes_effect_at) must NOT route \
         through interpolation — that would reintroduce the FTL leak \
         #530 closed",
    );
}

/// Post-effect: command has locally reached the ship, the ship is in
/// transit, arrival ETA is known. The marker MUST route through
/// `projection_screen_pos` instead of the docked grouping.
#[test]
fn projected_in_system_post_effect_uses_interpolated_marker() {
    let p =
        make_in_system_in_transit_projection(ShipSnapshotState::InTransitFTL, Some(100), Some(200));
    // now=150 ∈ [100, 200] → mid-flight.
    assert!(
        should_draw_projected_marker_with_interpolation(&p, 150),
        "post-effect with transit intent + ETAs MUST route through \
         interpolation (the whole point of the #532 follow-up)",
    );

    // Sanity-check the lerp lands strictly between origin and
    // destination at mid-flight — confirms the helper is wired to the
    // same lerp the marker draw will use.
    let frac = intended_lerp_fraction(&p, 150).expect("mid-flight has a lerp fraction");
    assert!(
        (frac - 0.5).abs() < 1e-4,
        "mid-flight lerp fraction must be 0.5; got {frac}",
    );
    let pos = own_ship_marker_screen_pos(&p, &origin(), Some(&dest()), VIEW_SCALE, 150);
    assert!(
        pos.x > 0.0 && pos.x < 100.0,
        "mid-flight marker must sit strictly between origin and \
         destination (not snap to either endpoint); got {pos:?}",
    );
}

/// Missing arrival ETA: the lerp endpoint is undefined. The marker
/// MUST stay docked rather than divide-by-zero or guess. (This mirrors
/// the steady-state `own_ship_marker_screen_pos` contract at the
/// routing level.)
#[test]
fn projected_in_system_missing_arrival_eta_remains_docked() {
    let p = make_in_system_in_transit_projection(ShipSnapshotState::InTransitFTL, Some(100), None);
    // now=150, post-effect but no arrival ETA.
    assert!(
        !should_draw_projected_marker_with_interpolation(&p, 150),
        "missing expected_arrival_at must NOT trigger interpolation \
         routing — the lerp endpoint would be undefined",
    );
}

/// Reconciled state: `projected_system == intended_system`. The
/// docked grouping at the (already correct) destination is the right
/// render — interpolating would be a zero-length lerp / no-op.
#[test]
fn projected_in_system_reconciled_uses_default() {
    let same_sys = Entity::from_bits(3);
    let p = ShipProjection {
        entity: Entity::PLACEHOLDER,
        dispatched_at: 0,
        expected_arrival_at: Some(200),
        expected_return_at: None,
        projected_state: ShipSnapshotState::InSystem,
        projected_system: Some(same_sys),
        intended_state: Some(ShipSnapshotState::InTransitFTL),
        intended_system: Some(same_sys),
        intended_takes_effect_at: Some(100),
    };
    // now=250, past arrival. Projected == Intended → reconciled.
    assert!(
        !should_draw_projected_marker_with_interpolation(&p, 250),
        "reconciled projection (projected == intended) must fall back \
         to the default docked grouping — there is no divergence to \
         interpolate against",
    );
}

/// Non-transit intended state (e.g. Surveying) must NOT route through
/// interpolation. Surveying anchors the ship at the target system; the
/// marker should not slide between systems.
#[test]
fn projected_in_system_non_transit_intended_does_not_route_through_interpolation() {
    let p =
        make_in_system_in_transit_projection(ShipSnapshotState::Surveying, Some(100), Some(200));
    assert!(
        !should_draw_projected_marker_with_interpolation(&p, 150),
        "non-transit intended state must NOT trigger interpolation \
         routing — Surveying / Settling anchor the ship at the \
         intended system rather than implying motion between systems",
    );
}

/// Steady-state projection (no in-flight command) must NOT route
/// through interpolation. `intended_state = None` is the default
/// post-reconcile shape.
#[test]
fn projected_in_system_steady_state_does_not_route_through_interpolation() {
    let p = make_in_system_in_transit_projection(
        ShipSnapshotState::InTransitFTL, // overwritten below
        Some(100),
        Some(200),
    );
    let p = ShipProjection {
        intended_state: None,
        intended_system: None,
        intended_takes_effect_at: None,
        expected_arrival_at: None,
        ..p
    };
    assert!(
        !should_draw_projected_marker_with_interpolation(&p, 150),
        "steady-state projection (intended_state = None) must NOT \
         trigger interpolation routing",
    );
}
