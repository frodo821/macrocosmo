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
use macrocosmo::visualization::ships::{intended_lerp_fraction, own_ship_marker_screen_pos};

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
