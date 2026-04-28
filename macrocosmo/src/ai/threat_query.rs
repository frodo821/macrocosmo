//! #480: ThreatState Suspected seed-signal query helpers.
//!
//! This module is the **AI-side consumer** of the producer infrastructure
//! landed by epic #473 (PRs #474 / #475 / #476):
//!
//! - #474 introduced [`ShipProjection`](crate::knowledge::ShipProjection)
//!   as a per-empire dispatcher-side belief about an own-empire ship's
//!   trajectory.
//! - #475 wired dispatch-time projection writes so a non-`None`
//!   `expected_return_at` records the dispatcher's "the ship was sent on
//!   a return-leg mission and should be back by tick T" expectation.
//! - #476 added the per-empire reconciler that clears the relevant
//!   `expected_*_at` / `intended_*` fields when a matching observation
//!   fact (`ShipArrived`, `SurveyComplete`, `ShipDestroyed`,
//!   `ShipMissing`) arrives at the dispatcher.
//!
//! This file gives the AI a **light-speed-coherent overdue signal**:
//! [`is_ship_overdue`] reads ONLY the dispatcher's
//! [`KnowledgeStore`](crate::knowledge::KnowledgeStore) plus the current
//! tick — never the ship's realtime ECS [`ShipState`]. That is the entire
//! point of #480: a future `ThreatStates` updater (#466 Phase 2) needs a
//! Suspected seed signal that does NOT FTL-leak through realtime ECS
//! component reads.
//!
//! # Why no realtime read?
//!
//! Reading `ShipState` directly would let the dispatcher "know" that a
//! courier has docked back home before its observation light has actually
//! reached the ruler — collapsing the entire light-speed information
//! constraint that the rest of the engine carefully preserves. Suspected
//! is meant to fire when the dispatcher's *belief* says the ship should
//! have returned but the dispatcher has not received any reconciling
//! observation yet, regardless of the ship's true ECS state.
//!
//! Behaviour summary:
//!
//! - No projection for `ship` → not overdue (dispatcher isn't tracking
//!   this ship as a return-leg mission).
//! - `expected_return_at = None` → not overdue (e.g. one-way commands
//!   like `colonize_system`, or already-reconciled return).
//! - `now <= expected_return_at + tolerance` → not overdue (within grace).
//! - `now > expected_return_at + tolerance` → **overdue**.
//!
//! The full ThreatState mechanism (`ThreatStates` Component, transition
//! rules, ROE wiring) is intentionally out of scope and lives in a
//! follow-up PR under epic #466. This file deliberately exposes only the
//! callable helper plus a small constant; today there is no production
//! call site, but the helper is ready for the Phase 2 PR to plug into.

use bevy::prelude::Entity;

use crate::knowledge::{KnowledgeStore, MISSING_GRACE_HEXADIES};

/// Tolerance (in hexadies) added to `expected_return_at` before declaring
/// a ship overdue.
///
/// Sized at `2 × MISSING_GRACE_HEXADIES` (= 10 hexadies as of writing) on
/// the rationale that:
///
/// - `MISSING_GRACE_HEXADIES` is the existing "ship destroyed but no light
///   arrived yet" pause — the engine already treats this much delay as
///   normal noise.
/// - The projection's `expected_return_at` is itself a best-effort
///   light-delay estimate computed at dispatch time
///   (see [`compute_ship_projection`](crate::knowledge::compute_ship_projection)),
///   so a small additional grace absorbs FTL/sublight pathing variance,
///   refit detours, and reconciler/event-emit ordering jitter without
///   firing Suspected on healthy returns.
///
/// The eventual Phase 2 `ThreatStates` updater is free to plumb through a
/// per-state-machine override; this constant is just the default the
/// helper assumes when the caller wants a sensible starting tunable.
pub const OVERDUE_TOLERANCE_HEXADIES: i64 = 2 * MISSING_GRACE_HEXADIES;

/// Returns `true` when the dispatcher's projection of `ship` is overdue.
///
/// **Local-info-only.** This helper consults *exclusively* the
/// dispatcher's [`KnowledgeStore`] and the supplied `now` tick. It does
/// NOT and MUST NOT consult any realtime ECS state of the ship — see the
/// module docs for the rationale.
///
/// # Semantics
///
/// - No projection for `ship` → returns `false`. The dispatcher isn't
///   tracking this ship as a current return-leg mission, so there is
///   nothing to be "overdue" against.
/// - Projection exists but `expected_return_at` is `None` → returns
///   `false`. Either the command had no return leg (e.g.
///   `colonize_system`) or the reconciler already cleared the
///   expectation in response to a matching observation.
/// - Projection exists with `expected_return_at = Some(t)` → returns
///   `true` iff `now > t + tolerance`. The strict `>` (rather than `>=`)
///   matches the convention "fire only after the grace tick has fully
///   elapsed."
///
/// # Arguments
///
/// * `knowledge` — the dispatcher empire's `KnowledgeStore`. The caller
///   is responsible for selecting the correct empire's store; see
///   `tests/ai_ship_overdue.rs` for examples.
/// * `ship` — the ship entity to query.
/// * `now` — the current `GameClock.elapsed` in hexadies.
/// * `tolerance` — grace period (in hexadies) added to
///   `expected_return_at` before the ship counts as overdue. Pass
///   [`OVERDUE_TOLERANCE_HEXADIES`] for the default.
///
/// # Example
///
/// ```ignore
/// use macrocosmo::ai::threat_query::{is_ship_overdue, OVERDUE_TOLERANCE_HEXADIES};
///
/// fn ship_should_seed_suspected(
///     knowledge: &macrocosmo::knowledge::KnowledgeStore,
///     ship: bevy::prelude::Entity,
///     clock: &macrocosmo::time_system::GameClock,
/// ) -> bool {
///     is_ship_overdue(knowledge, ship, clock.elapsed, OVERDUE_TOLERANCE_HEXADIES)
/// }
/// ```
pub fn is_ship_overdue(knowledge: &KnowledgeStore, ship: Entity, now: i64, tolerance: i64) -> bool {
    let Some(proj) = knowledge.get_projection(ship) else {
        return false;
    };
    proj.expected_return_at.is_some_and(|t| now > t + tolerance)
}
