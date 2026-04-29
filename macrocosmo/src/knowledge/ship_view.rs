//! #491 (D-C-1): Domain-level ship-view helpers.
//!
//! `ShipView` and `ship_view` were originally factored into `ui::ship_view`
//! during the #487 outline-tree FTL leak fix and the #491 helper extraction.
//! Adversarial review surfaced that the data shape and selection logic
//! consume only `KnowledgeStore` / `ShipProjection` / `ShipSnapshot`
//! and the realtime ECS [`ShipState`] — none of those are UI types — so
//! the helpers belong in the `knowledge` layer. The egui-adjacent
//! formatters (`ship_view_status_label`, `ship_view_progress`,
//! `ship_view_eta`) stay in `ui::ship_view`; the data shape lives here.
//!
//! `ui::ship_view` continues to re-export [`ShipView`] / [`ShipViewTiming`]
//! / [`ship_view`] / [`realtime_state_to_snapshot`] so existing callers
//! compile unchanged.
//!
//! ## Stage-1 #491 follow-up: ladder hoist
//!
//! The 4-PR sub-rewires (#500 / #501 / #502 / #503 + the upcoming
//! system-panel / threat-state / omniscient hooks) all need to project a
//! `(ShipView, ShipViewTiming)` pair from one of three sources:
//!
//! 1. own-empire `ShipProjection` — light-coherent dispatcher belief
//! 2. foreign `ShipSnapshot` — last observation by the viewing empire
//! 3. realtime ECS [`ShipState`] — Startup fallback / observer-mode ground truth
//!
//! Reinventing the source-selection ladder per panel was already happening
//! twice across the in-flight PRs — it would have happened a third time on
//! the next rewire. The constructors [`ShipViewTiming::from_projection`] /
//! [`ShipViewTiming::from_snapshot`] / [`ShipViewTiming::from_realtime`]
//! and the all-in-one [`ship_view_with_timing`] hoist that ladder into a
//! single canonical implementation.
//!
//! Panel UX also kept open-coding the "this ship is dead, don't show
//! actions" check four different ways. [`ShipView::is_actionable`] is the
//! single predicate panels gate ScrapShip / SetHomePort / Refit /
//! context-menu commands on, so terminal states (`Destroyed` / `Missing`)
//! cannot be acted on by mistake.

use bevy::prelude::*;

use crate::components::Position;
use crate::ship::{Owner, Ship, ShipState};
use crate::time_system::GameClock;

use super::{KnowledgeStore, ShipProjection, ShipSnapshot, ShipSnapshotState};

/// #487 / #491: Light-coherent rendering of a ship.
///
/// `state` is a [`ShipSnapshotState`] derived from either the viewing
/// empire's projection (own-empire ship) or `ship_snapshots` (foreign
/// ship), or — when no `KnowledgeStore` is resolved (early Startup) —
/// from the realtime ECS [`ShipState`] as a defensive fallback.
#[derive(Clone, Debug, PartialEq)]
pub struct ShipView {
    pub state: ShipSnapshotState,
    pub system: Option<Entity>,
}

impl ShipView {
    /// #491 (D-H-5): Returns the loitering coordinate, if the ship is in
    /// deep space. `None` for system-anchored states (InSystem /
    /// InTransit / Surveying / etc.) — callers use `view.system`'s
    /// `Position` instead.
    pub fn position(&self) -> Option<[f64; 3]> {
        match &self.state {
            ShipSnapshotState::Loitering { position } => Some(*position),
            _ => None,
        }
    }

    /// #491 (D-H-6): Linearly interpolate ship position between origin
    /// and destination for in-transit states.
    ///
    /// `origin_pos` and `dest_pos` are looked up by the caller (typically
    /// the same `Query<&Position, With<StarSystem>>` already in scope).
    /// Returns `None` for non-transit states — the caller should use
    /// `view.system`'s position directly. Returns `None` if either
    /// position or the timing data is unavailable.
    ///
    /// The fraction is clamped to `[0.0, 1.0]` so a stale `clock.elapsed`
    /// past `expected_tick` does not over-extrapolate; an in-flight ship
    /// is never drawn past its destination.
    pub fn estimated_position(
        &self,
        timing: Option<&ShipViewTiming>,
        clock: &GameClock,
        origin_pos: Option<&Position>,
        dest_pos: Option<&Position>,
    ) -> Option<Position> {
        if !self.state.is_in_transit() {
            return None;
        }
        let timing = timing?;
        let expected = timing.expected_tick?;
        let origin = origin_pos?;
        let dest = dest_pos?;
        let total = (expected.saturating_sub(timing.origin_tick)).max(1) as f64;
        let elapsed_raw = clock.elapsed.saturating_sub(timing.origin_tick) as f64;
        let frac = (elapsed_raw / total).clamp(0.0, 1.0);
        Some(Position {
            x: origin.x + (dest.x - origin.x) * frac,
            y: origin.y + (dest.y - origin.y) * frac,
            z: origin.z + (dest.z - origin.z) * frac,
        })
    }

    /// #491 follow-up: Returns `false` for terminal states ([`ShipSnapshotState::Destroyed`]
    /// / [`ShipSnapshotState::Missing`]) where the ship cannot accept
    /// commands or be the target of UI actions. UI panels SHOULD gate
    /// ScrapShip / SetHomePort / Refit / context-menu commands on this
    /// predicate to avoid dispatching against a ship the empire already
    /// believes is gone.
    ///
    /// Note that "actionable" is a UI gating predicate, not a
    /// game-logic existence test — a `Destroyed` ship may still have a
    /// live ECS entity (until ghost cleanup runs); this predicate only
    /// answers "should the player be allowed to click ScrapShip on
    /// this view?".
    pub fn is_actionable(&self) -> bool {
        !matches!(
            self.state,
            ShipSnapshotState::Destroyed | ShipSnapshotState::Missing
        )
    }
}

/// Timing context for a [`ShipView`]'s progress / ETA derivations.
///
/// `origin_tick` semantics depend on which path produced the `ShipView`:
/// * **Own-empire ship via projection**: [`crate::knowledge::ShipProjection::dispatched_at`]
///   (= dispatcher's command-send tick, light-coherent with player UI)
/// * **Foreign ship via snapshot**: [`crate::knowledge::ShipSnapshot::observed_at`]
///   (= when the viewing empire learned of the ship's state)
/// * **Realtime fallback (no KnowledgeStore)**: ECS [`ShipState`]'s
///   `departed_at` / `started_at` (ground truth, observer-only)
///
/// All three are i64 hexadies on the same [`GameClock`] — they differ
/// only in **whose timeline** they anchor progress to. Callers MUST
/// construct `ShipViewTiming` from the same source as the `ShipView`
/// itself; mixing (e.g. projection `ShipView` + snapshot timing) is a
/// programming error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShipViewTiming {
    /// Tick at which the activity began (FTL departure, settle start, etc.).
    pub origin_tick: i64,
    /// Tick at which the activity is expected to complete. `None` for
    /// open-ended states (e.g. loitering, no projected arrival).
    pub expected_tick: Option<i64>,
}

impl ShipViewTiming {
    /// Build timing from a [`ShipProjection`]. Used for own-empire ships
    /// where the projection layer carries the dispatcher's light-coherent
    /// belief about dispatch / arrival ticks.
    ///
    /// * `origin_tick` = [`ShipProjection::dispatched_at`] (= dispatcher's
    ///   command-send tick).
    /// * `expected_tick` = [`ShipProjection::expected_arrival_at`].
    pub fn from_projection(projection: &ShipProjection) -> Self {
        Self {
            origin_tick: projection.dispatched_at,
            expected_tick: projection.expected_arrival_at,
        }
    }

    /// Build timing from a [`ShipSnapshot`]. Used for foreign ships where
    /// the viewing empire's last observation supplies the timing anchor.
    ///
    /// * `origin_tick` = [`ShipSnapshot::observed_at`] (= the tick at
    ///   which the viewing empire learned of the ship's last-known state).
    /// * `expected_tick` = `None`. Snapshots are point-in-time
    ///   observations only; they do not carry a future-projected arrival
    ///   tick. Foreign ETA is unknowable from the viewing empire's
    ///   perspective without dedicated intel — progress bars and ETAs
    ///   for foreign ships should fall back to "Unknown" / no progress
    ///   until a stronger intel channel (e.g. survey, scout report)
    ///   produces a projection-equivalent estimate.
    pub fn from_snapshot(snapshot: &ShipSnapshot) -> Self {
        Self {
            origin_tick: snapshot.observed_at,
            expected_tick: None,
        }
    }

    /// Build timing from a realtime ECS [`ShipState`]. Used as the
    /// no-`KnowledgeStore` fallback (early Startup before empires are
    /// wired) and for omniscient / observer-mode ground-truth views.
    ///
    /// * `origin_tick` = the state's `departed_at` / `started_at`, or
    ///   `0` for steady-state variants ([`ShipState::InSystem`] /
    ///   [`ShipState::Loitering`]) which have no activity start.
    /// * `expected_tick` = the state's `arrival_at` / `completes_at`,
    ///   or `None` for steady-state variants.
    pub fn from_realtime(state: &ShipState) -> Self {
        match state {
            ShipState::InSystem { .. } => Self {
                origin_tick: 0,
                expected_tick: None,
            },
            ShipState::SubLight {
                departed_at,
                arrival_at,
                ..
            } => Self {
                origin_tick: *departed_at,
                expected_tick: Some(*arrival_at),
            },
            ShipState::InFTL {
                departed_at,
                arrival_at,
                ..
            } => Self {
                origin_tick: *departed_at,
                expected_tick: Some(*arrival_at),
            },
            ShipState::Surveying {
                started_at,
                completes_at,
                ..
            } => Self {
                origin_tick: *started_at,
                expected_tick: Some(*completes_at),
            },
            ShipState::Settling {
                started_at,
                completes_at,
                ..
            } => Self {
                origin_tick: *started_at,
                expected_tick: Some(*completes_at),
            },
            ShipState::Refitting {
                started_at,
                completes_at,
                ..
            } => Self {
                origin_tick: *started_at,
                expected_tick: Some(*completes_at),
            },
            ShipState::Loitering { .. } => Self {
                origin_tick: 0,
                expected_tick: None,
            },
            ShipState::Scouting {
                started_at,
                completes_at,
                ..
            } => Self {
                origin_tick: *started_at,
                expected_tick: Some(*completes_at),
            },
        }
    }
}

/// #487 / #491: Convert a realtime [`ShipState`] to the corresponding
/// [`ShipSnapshotState`] for observer-mode ground-truth rendering and
/// the `viewing_knowledge.is_none()` fallback path.
///
/// This is the single source of truth for the `ShipState` →
/// `ShipSnapshotState` collapse — readers (`ship_view`) and writers
/// (`update_ship_snapshots` in [`crate::knowledge::mod`], the buoy
/// detector in `deep_space`, the relay propagator, the scout drop-off)
/// all route through this helper so projection-driven and observer
/// renders agree label-for-label.
pub fn realtime_state_to_snapshot(state: &ShipState) -> (ShipSnapshotState, Option<Entity>) {
    match state {
        ShipState::InSystem { system } => (ShipSnapshotState::InSystem, Some(*system)),
        ShipState::SubLight { target_system, .. } => {
            (ShipSnapshotState::InTransitSubLight, *target_system)
        }
        ShipState::InFTL {
            destination_system, ..
        } => (ShipSnapshotState::InTransitFTL, Some(*destination_system)),
        ShipState::Surveying { target_system, .. } => {
            (ShipSnapshotState::Surveying, Some(*target_system))
        }
        ShipState::Settling { system, .. } => (ShipSnapshotState::Settling, Some(*system)),
        ShipState::Refitting { system, .. } => (ShipSnapshotState::Refitting, Some(*system)),
        ShipState::Loitering { position } => (
            ShipSnapshotState::Loitering {
                position: *position,
            },
            None,
        ),
        // #217: Scouting collapses into Surveying at snapshot granularity.
        ShipState::Scouting { target_system, .. } => {
            (ShipSnapshotState::Surveying, Some(*target_system))
        }
    }
}

/// #487 / #491: Compute the light-coherent view of a ship, gated by the
/// light-speed contract.
///
/// * **Own-empire ship** (in normal play): read the projected state from
///   the viewing empire's [`KnowledgeStore::projections`]. The realtime
///   ECS [`ShipState`] is intentionally ignored — that's the FTL leak
///   fix (epic #473 / #487).
/// * **Foreign ship** (in normal play): read the last-known state from
///   the viewing empire's [`KnowledgeStore::ship_snapshots`]. Unchanged
///   from the pre-#487 contract (it was already snapshot-mediated).
/// * **Observer mode** (= empire-view, viewing as another empire): the
///   *intended* contract is "treat identically to own-empire normal
///   play — projection / snapshot of the **viewing empire**" (= the
///   observed empire whose perspective the player is borrowing). The
///   caller would pass the observed empire as `viewing_empire`. A
///   separate omniscient (god-view) mode is the right way to expose
///   realtime ground truth (#490, follow-up).
///
///   **Production drift**: the current `ui::mod::draw_outline_and_tooltips_system`
///   caller passes `viewing_knowledge = None` whenever observer mode is
///   active, which falls through to the realtime ECS path below
///   (= ground-truth, not the empire-view contract above). This is
///   tracked as a follow-up to #440 (observer mode design); the helper
///   itself supports both contracts via the no-store fallback, so
///   migrating the call site is a one-line change once the design is
///   finalised. Tests that pin the empire-view contract still pass
///   because they construct `viewing_knowledge = Some(...)` explicitly.
/// * **No `KnowledgeStore` resolved** (early Startup frames before
///   empires are wired): fall back to realtime ECS state — there's no
///   light-coherent view to use yet.
///
/// Returns `None` when the ship has no entry in the viewing empire's
/// knowledge — e.g. a freshly-spawned own-ship before its seed
/// projection lands (#481), or a foreign ship the empire has never
/// observed. The caller decides how to render the absence (skip /
/// "Unknown").
///
/// #491 (D-M-9): The legacy `_is_observer` parameter is gone. Observer
/// mode is now expressed entirely through `viewing_empire = Some(...)`
/// pointing at the observed empire — the caller's responsibility to
/// resolve, not the helper's.
pub fn ship_view(
    ship_entity: Entity,
    ship: &Ship,
    realtime_state: &ShipState,
    viewing_knowledge: Option<&KnowledgeStore>,
    viewing_empire: Option<Entity>,
) -> Option<ShipView> {
    // No KnowledgeStore resolved (e.g. very early Startup frames before
    // empires are wired). Fall back to realtime ECS as a defensive path.
    if viewing_knowledge.is_none() {
        let (state, system) = realtime_state_to_snapshot(realtime_state);
        return Some(ShipView { state, system });
    }
    let store = viewing_knowledge.unwrap();
    if let Owner::Empire(owner) = ship.owner {
        if Some(owner) == viewing_empire {
            // Own ship: projection is the only legal source.
            return store.get_projection(ship_entity).map(|p| ShipView {
                state: p.projected_state.clone(),
                system: p.projected_system,
            });
        }
    }
    // Foreign ship: snapshot is the only legal source.
    store.get_ship(ship_entity).map(|s| ShipView {
        state: s.last_known_state.clone(),
        system: s.last_known_system,
    })
}

/// #491 follow-up: Resolve both a [`ShipView`] and a matching
/// [`ShipViewTiming`] from the same source in one call.
///
/// Replaces the per-panel three-step ladder
/// (resolve view → resolve timing source → build timing) that was
/// already being reinvented across the in-flight sub-PRs (#500 / #501 /
/// #502 / #503). Picks the source under the same contract as
/// [`ship_view`]:
///
/// * own-empire ship → [`ShipProjection`] (via [`KnowledgeStore::get_projection`])
/// * foreign ship → [`ShipSnapshot`] (via [`KnowledgeStore::get_ship`])
/// * no `KnowledgeStore` → realtime ECS [`ShipState`] fallback
///
/// Returns `None` under the same conditions as [`ship_view`] — i.e. an
/// own-ship without a projection (freshly spawned, pre-seed) or a
/// foreign ship the empire has never observed.
///
/// Implementation note: this performs at most one `HashMap` lookup
/// against the projection / snapshot store (the same number `ship_view`
/// already does on its own), so callers can replace
/// `ship_view(...) + ladder` with a single call without paying for an
/// extra lookup.
pub fn ship_view_with_timing(
    ship_entity: Entity,
    ship: &Ship,
    realtime_state: &ShipState,
    viewing_knowledge: Option<&KnowledgeStore>,
    viewing_empire: Option<Entity>,
) -> Option<(ShipView, ShipViewTiming)> {
    // No KnowledgeStore resolved (e.g. very early Startup). Fall back to
    // realtime ECS as a defensive ground-truth path — same contract as
    // `ship_view`'s no-store branch.
    let Some(store) = viewing_knowledge else {
        let (state, system) = realtime_state_to_snapshot(realtime_state);
        return Some((
            ShipView { state, system },
            ShipViewTiming::from_realtime(realtime_state),
        ));
    };
    if let Owner::Empire(owner) = ship.owner
        && Some(owner) == viewing_empire
    {
        // Own ship: projection is the only legal source.
        let projection = store.get_projection(ship_entity)?;
        let view = ShipView {
            state: projection.projected_state.clone(),
            system: projection.projected_system,
        };
        let timing = ShipViewTiming::from_projection(projection);
        return Some((view, timing));
    }
    // Foreign ship: snapshot is the only legal source.
    let snapshot = store.get_ship(ship_entity)?;
    let view = ShipView {
        state: snapshot.last_known_state.clone(),
        system: snapshot.last_known_system,
    };
    let timing = ShipViewTiming::from_snapshot(snapshot);
    Some((view, timing))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::Position;
    use crate::knowledge::{ObservationSource, ShipProjection, ShipSnapshot};
    use crate::ship::Owner;

    fn make_ship(name: &str, owner: Entity, home: Entity) -> Ship {
        Ship {
            name: name.into(),
            design_id: "explorer_mk1".into(),
            hull_id: "frigate".into(),
            modules: Vec::new(),
            owner: Owner::Empire(owner),
            sublight_speed: 1.0,
            ftl_range: 5.0,
            ruler_aboard: false,
            home_port: home,
            design_revision: 0,
            fleet: None,
        }
    }

    // -----------------------------------------------------------------
    // realtime_state_to_snapshot — all 8 ShipState variants
    // -----------------------------------------------------------------

    #[test]
    fn realtime_in_system_round_trip() {
        let mut world = World::new();
        let sys = world.spawn_empty().id();
        let (s, sys_e) = realtime_state_to_snapshot(&ShipState::InSystem { system: sys });
        assert_eq!(s, ShipSnapshotState::InSystem);
        assert_eq!(sys_e, Some(sys));
    }

    #[test]
    fn realtime_sublight_collapses_to_in_transit_sublight() {
        let mut world = World::new();
        let target = world.spawn_empty().id();
        let (s, sys_e) = realtime_state_to_snapshot(&ShipState::SubLight {
            origin: [0.0, 0.0, 0.0],
            destination: [50.0, 0.0, 0.0],
            target_system: Some(target),
            departed_at: 0,
            arrival_at: 5,
        });
        assert_eq!(s, ShipSnapshotState::InTransitSubLight);
        assert_eq!(sys_e, Some(target));
    }

    #[test]
    fn realtime_sublight_open_target_has_none_system() {
        let (s, sys_e) = realtime_state_to_snapshot(&ShipState::SubLight {
            origin: [0.0, 0.0, 0.0],
            destination: [50.0, 0.0, 0.0],
            target_system: None,
            departed_at: 0,
            arrival_at: 5,
        });
        assert_eq!(s, ShipSnapshotState::InTransitSubLight);
        assert_eq!(sys_e, None);
    }

    #[test]
    fn realtime_inftl_collapses_to_in_transit_ftl() {
        let mut world = World::new();
        let dest = world.spawn_empty().id();
        let origin = world.spawn_empty().id();
        let (s, sys_e) = realtime_state_to_snapshot(&ShipState::InFTL {
            origin_system: origin,
            destination_system: dest,
            departed_at: 0,
            arrival_at: 5,
        });
        assert_eq!(s, ShipSnapshotState::InTransitFTL);
        assert_eq!(sys_e, Some(dest));
    }

    #[test]
    fn realtime_surveying_round_trip() {
        let mut world = World::new();
        let target = world.spawn_empty().id();
        let (s, sys_e) = realtime_state_to_snapshot(&ShipState::Surveying {
            target_system: target,
            started_at: 0,
            completes_at: 10,
        });
        assert_eq!(s, ShipSnapshotState::Surveying);
        assert_eq!(sys_e, Some(target));
    }

    #[test]
    fn realtime_settling_round_trip() {
        let mut world = World::new();
        let sys = world.spawn_empty().id();
        let planet = world.spawn_empty().id();
        let (s, sys_e) = realtime_state_to_snapshot(&ShipState::Settling {
            system: sys,
            planet: Some(planet),
            started_at: 0,
            completes_at: 20,
        });
        assert_eq!(s, ShipSnapshotState::Settling);
        assert_eq!(sys_e, Some(sys));
    }

    #[test]
    fn realtime_refitting_round_trip() {
        let mut world = World::new();
        let sys = world.spawn_empty().id();
        let (s, sys_e) = realtime_state_to_snapshot(&ShipState::Refitting {
            system: sys,
            started_at: 0,
            completes_at: 5,
            new_modules: Vec::new(),
            target_revision: 1,
        });
        assert_eq!(s, ShipSnapshotState::Refitting);
        assert_eq!(sys_e, Some(sys));
    }

    #[test]
    fn realtime_loitering_round_trip() {
        let pos = [10.5, -3.0, 7.25];
        let (s, sys_e) = realtime_state_to_snapshot(&ShipState::Loitering { position: pos });
        assert_eq!(s, ShipSnapshotState::Loitering { position: pos });
        assert_eq!(sys_e, None);
    }

    #[test]
    fn realtime_scouting_collapses_to_surveying() {
        use crate::ship::ReportMode;
        let mut world = World::new();
        let target = world.spawn_empty().id();
        let origin = world.spawn_empty().id();
        let (s, sys_e) = realtime_state_to_snapshot(&ShipState::Scouting {
            target_system: target,
            origin_system: origin,
            started_at: 0,
            completes_at: 10,
            report_mode: ReportMode::Return,
        });
        assert_eq!(s, ShipSnapshotState::Surveying);
        assert_eq!(sys_e, Some(target));
    }

    // -----------------------------------------------------------------
    // is_in_transit — covers both transit variants
    // -----------------------------------------------------------------

    #[test]
    fn is_in_transit_covers_both_variants() {
        assert!(ShipSnapshotState::InTransitSubLight.is_in_transit());
        assert!(ShipSnapshotState::InTransitFTL.is_in_transit());
        assert!(!ShipSnapshotState::InSystem.is_in_transit());
        assert!(!ShipSnapshotState::Surveying.is_in_transit());
        assert!(!ShipSnapshotState::Settling.is_in_transit());
        assert!(!ShipSnapshotState::Refitting.is_in_transit());
        assert!(!ShipSnapshotState::Destroyed.is_in_transit());
        assert!(!ShipSnapshotState::Missing.is_in_transit());
        assert!(
            !ShipSnapshotState::Loitering {
                position: [0.0, 0.0, 0.0]
            }
            .is_in_transit()
        );
    }

    // -----------------------------------------------------------------
    // ShipView::position — only Loitering returns a coordinate
    // -----------------------------------------------------------------

    #[test]
    fn position_returns_loitering_coord_only() {
        let view = ShipView {
            state: ShipSnapshotState::Loitering {
                position: [1.0, 2.0, 3.0],
            },
            system: None,
        };
        assert_eq!(view.position(), Some([1.0, 2.0, 3.0]));
        let other = ShipView {
            state: ShipSnapshotState::InSystem,
            system: None,
        };
        assert_eq!(other.position(), None);
        let in_transit = ShipView {
            state: ShipSnapshotState::InTransitFTL,
            system: None,
        };
        assert_eq!(in_transit.position(), None);
    }

    // -----------------------------------------------------------------
    // ShipView::estimated_position — lerp for in-transit, None otherwise
    // -----------------------------------------------------------------

    #[test]
    fn estimated_position_lerps_for_in_transit() {
        let view = ShipView {
            state: ShipSnapshotState::InTransitFTL,
            system: None,
        };
        let timing = ShipViewTiming {
            origin_tick: 0,
            expected_tick: Some(10),
        };
        let origin = Position::from([0.0, 0.0, 0.0]);
        let dest = Position::from([10.0, 0.0, 0.0]);
        let clock = GameClock::new(5);
        let est = view
            .estimated_position(Some(&timing), &clock, Some(&origin), Some(&dest))
            .expect("in-transit lerp should produce Some");
        assert!((est.x - 5.0).abs() < 1e-6);
    }

    #[test]
    fn estimated_position_lerps_for_in_transit_sublight() {
        let view = ShipView {
            state: ShipSnapshotState::InTransitSubLight,
            system: None,
        };
        let timing = ShipViewTiming {
            origin_tick: 0,
            expected_tick: Some(10),
        };
        let origin = Position::from([0.0, 0.0, 0.0]);
        let dest = Position::from([10.0, 20.0, 0.0]);
        let clock = GameClock::new(2);
        let est = view
            .estimated_position(Some(&timing), &clock, Some(&origin), Some(&dest))
            .expect("sublight transit should also lerp");
        assert!((est.x - 2.0).abs() < 1e-6);
        assert!((est.y - 4.0).abs() < 1e-6);
    }

    #[test]
    fn estimated_position_returns_none_for_in_system() {
        let view = ShipView {
            state: ShipSnapshotState::InSystem,
            system: None,
        };
        let timing = ShipViewTiming {
            origin_tick: 0,
            expected_tick: Some(10),
        };
        let origin = Position::from([0.0, 0.0, 0.0]);
        let dest = Position::from([10.0, 0.0, 0.0]);
        let clock = GameClock::new(5);
        assert_eq!(
            view.estimated_position(Some(&timing), &clock, Some(&origin), Some(&dest)),
            None
        );
    }

    #[test]
    fn estimated_position_clamps_overdue_to_destination() {
        let view = ShipView {
            state: ShipSnapshotState::InTransitFTL,
            system: None,
        };
        let timing = ShipViewTiming {
            origin_tick: 0,
            expected_tick: Some(10),
        };
        let origin = Position::from([0.0, 0.0, 0.0]);
        let dest = Position::from([10.0, 0.0, 0.0]);
        // Now far past expected_tick — clamp to destination, not extrapolate.
        let clock = GameClock::new(100);
        let est = view
            .estimated_position(Some(&timing), &clock, Some(&origin), Some(&dest))
            .expect("clamped lerp should still produce Some");
        assert!((est.x - 10.0).abs() < 1e-6);
    }

    // -----------------------------------------------------------------
    // ship_view — own / foreign / no-projection / no-store paths
    // -----------------------------------------------------------------

    #[test]
    fn ship_view_own_empire_uses_projection() {
        let mut world = World::new();
        let empire = world.spawn_empty().id();
        let home = world.spawn_empty().id();
        let frontier = world.spawn_empty().id();
        let ship_entity = world.spawn_empty().id();
        let ship = make_ship("Explorer", empire, home);
        // Realtime says InFTL — the projection-driven path must ignore it.
        let realtime = ShipState::InFTL {
            origin_system: home,
            destination_system: frontier,
            departed_at: 0,
            arrival_at: 5,
        };
        let mut store = KnowledgeStore::default();
        store.update_projection(ShipProjection {
            entity: ship_entity,
            dispatched_at: 0,
            expected_arrival_at: Some(10),
            expected_return_at: None,
            projected_state: ShipSnapshotState::InSystem,
            projected_system: Some(home),
            intended_state: Some(ShipSnapshotState::Surveying),
            intended_system: Some(frontier),
            intended_takes_effect_at: Some(5),
        });

        let view = ship_view(ship_entity, &ship, &realtime, Some(&store), Some(empire))
            .expect("own-ship projection must produce a view");
        assert_eq!(view.state, ShipSnapshotState::InSystem);
        assert_eq!(view.system, Some(home));
    }

    #[test]
    fn ship_view_foreign_uses_snapshot() {
        let mut world = World::new();
        let viewing_empire = world.spawn_empty().id();
        let foreign_empire = world.spawn_empty().id();
        let foreign_sys = world.spawn_empty().id();
        let ship_entity = world.spawn_empty().id();
        let ship = make_ship("EnemyScout", foreign_empire, foreign_sys);
        let realtime = ShipState::InSystem {
            system: foreign_sys,
        };
        let mut store = KnowledgeStore::default();
        store.update_ship(ShipSnapshot {
            entity: ship_entity,
            name: "EnemyScout".into(),
            design_id: "explorer_mk1".into(),
            last_known_state: ShipSnapshotState::Surveying,
            last_known_system: Some(foreign_sys),
            observed_at: 0,
            hp: 100.0,
            hp_max: 100.0,
            source: ObservationSource::Direct,
        });

        let view = ship_view(
            ship_entity,
            &ship,
            &realtime,
            Some(&store),
            Some(viewing_empire),
        )
        .expect("foreign snapshot must produce a view");
        assert_eq!(view.state, ShipSnapshotState::Surveying);
        assert_eq!(view.system, Some(foreign_sys));
    }

    #[test]
    fn ship_view_own_empire_no_projection_returns_none() {
        let mut world = World::new();
        let empire = world.spawn_empty().id();
        let home = world.spawn_empty().id();
        let ship_entity = world.spawn_empty().id();
        let ship = make_ship("Explorer", empire, home);
        let realtime = ShipState::InSystem { system: home };
        let store = KnowledgeStore::default();
        let view = ship_view(ship_entity, &ship, &realtime, Some(&store), Some(empire));
        assert!(view.is_none());
    }

    #[test]
    fn ship_view_no_knowledge_store_falls_back_to_realtime() {
        let mut world = World::new();
        let empire = world.spawn_empty().id();
        let home = world.spawn_empty().id();
        let frontier = world.spawn_empty().id();
        let ship_entity = world.spawn_empty().id();
        let ship = make_ship("Explorer", empire, home);
        let realtime = ShipState::InFTL {
            origin_system: home,
            destination_system: frontier,
            departed_at: 0,
            arrival_at: 5,
        };

        let view = ship_view(ship_entity, &ship, &realtime, None, Some(empire))
            .expect("startup fallback must produce a view");
        // No KnowledgeStore = realtime read => InTransitFTL (no longer collapsed).
        assert_eq!(view.state, ShipSnapshotState::InTransitFTL);
        assert_eq!(view.system, Some(frontier));
    }

    // -----------------------------------------------------------------
    // #491 follow-up: ShipViewTiming constructors
    // -----------------------------------------------------------------

    fn make_projection(entity: Entity, dispatched_at: i64, arrival: Option<i64>) -> ShipProjection {
        ShipProjection {
            entity,
            dispatched_at,
            expected_arrival_at: arrival,
            expected_return_at: None,
            projected_state: ShipSnapshotState::InSystem,
            projected_system: None,
            intended_state: None,
            intended_system: None,
            intended_takes_effect_at: None,
        }
    }

    fn make_snapshot(entity: Entity, observed_at: i64) -> ShipSnapshot {
        ShipSnapshot {
            entity,
            name: "Probe".into(),
            design_id: "explorer_mk1".into(),
            last_known_state: ShipSnapshotState::Surveying,
            last_known_system: None,
            observed_at,
            hp: 100.0,
            hp_max: 100.0,
            source: ObservationSource::Direct,
        }
    }

    #[test]
    fn timing_from_projection_uses_dispatched_at_and_expected_arrival_at() {
        let mut world = World::new();
        let ship_entity = world.spawn_empty().id();
        let projection = make_projection(ship_entity, 5, Some(10));
        let timing = ShipViewTiming::from_projection(&projection);
        assert_eq!(timing.origin_tick, 5);
        assert_eq!(timing.expected_tick, Some(10));
    }

    #[test]
    fn timing_from_projection_passes_through_none_arrival() {
        let mut world = World::new();
        let ship_entity = world.spawn_empty().id();
        let projection = make_projection(ship_entity, 7, None);
        let timing = ShipViewTiming::from_projection(&projection);
        assert_eq!(timing.origin_tick, 7);
        assert_eq!(timing.expected_tick, None);
    }

    #[test]
    fn timing_from_snapshot_uses_observed_at_with_none_eta() {
        let mut world = World::new();
        let ship_entity = world.spawn_empty().id();
        let snapshot = make_snapshot(ship_entity, 42);
        let timing = ShipViewTiming::from_snapshot(&snapshot);
        // Snapshots are point-in-time observations only — they do not
        // carry future-projected ETA. Foreign ETA falls back to None.
        assert_eq!(timing.origin_tick, 42);
        assert_eq!(timing.expected_tick, None);
    }

    #[test]
    fn timing_from_realtime_in_system_yields_zero_origin_no_expected() {
        let mut world = World::new();
        let sys = world.spawn_empty().id();
        let timing = ShipViewTiming::from_realtime(&ShipState::InSystem { system: sys });
        assert_eq!(timing.origin_tick, 0);
        assert_eq!(timing.expected_tick, None);
    }

    #[test]
    fn timing_from_realtime_sublight_uses_departed_arrival() {
        let mut world = World::new();
        let target = world.spawn_empty().id();
        let timing = ShipViewTiming::from_realtime(&ShipState::SubLight {
            origin: [0.0, 0.0, 0.0],
            destination: [10.0, 0.0, 0.0],
            target_system: Some(target),
            departed_at: 3,
            arrival_at: 13,
        });
        assert_eq!(timing.origin_tick, 3);
        assert_eq!(timing.expected_tick, Some(13));
    }

    #[test]
    fn timing_from_realtime_ftl_uses_departed_arrival() {
        let mut world = World::new();
        let origin = world.spawn_empty().id();
        let dest = world.spawn_empty().id();
        let timing = ShipViewTiming::from_realtime(&ShipState::InFTL {
            origin_system: origin,
            destination_system: dest,
            departed_at: 1,
            arrival_at: 6,
        });
        assert_eq!(timing.origin_tick, 1);
        assert_eq!(timing.expected_tick, Some(6));
    }

    #[test]
    fn timing_from_realtime_surveying_uses_started_completes() {
        let mut world = World::new();
        let target = world.spawn_empty().id();
        let timing = ShipViewTiming::from_realtime(&ShipState::Surveying {
            target_system: target,
            started_at: 2,
            completes_at: 8,
        });
        assert_eq!(timing.origin_tick, 2);
        assert_eq!(timing.expected_tick, Some(8));
    }

    #[test]
    fn timing_from_realtime_settling_uses_started_completes() {
        let mut world = World::new();
        let sys = world.spawn_empty().id();
        let planet = world.spawn_empty().id();
        let timing = ShipViewTiming::from_realtime(&ShipState::Settling {
            system: sys,
            planet: Some(planet),
            started_at: 4,
            completes_at: 24,
        });
        assert_eq!(timing.origin_tick, 4);
        assert_eq!(timing.expected_tick, Some(24));
    }

    #[test]
    fn timing_from_realtime_refitting_uses_started_completes() {
        let mut world = World::new();
        let sys = world.spawn_empty().id();
        let timing = ShipViewTiming::from_realtime(&ShipState::Refitting {
            system: sys,
            started_at: 9,
            completes_at: 14,
            new_modules: Vec::new(),
            target_revision: 1,
        });
        assert_eq!(timing.origin_tick, 9);
        assert_eq!(timing.expected_tick, Some(14));
    }

    #[test]
    fn timing_from_realtime_loitering_yields_zero_no_expected() {
        let timing = ShipViewTiming::from_realtime(&ShipState::Loitering {
            position: [1.0, 2.0, 3.0],
        });
        assert_eq!(timing.origin_tick, 0);
        assert_eq!(timing.expected_tick, None);
    }

    #[test]
    fn timing_from_realtime_scouting_uses_started_completes() {
        use crate::ship::ReportMode;
        let mut world = World::new();
        let target = world.spawn_empty().id();
        let origin = world.spawn_empty().id();
        let timing = ShipViewTiming::from_realtime(&ShipState::Scouting {
            target_system: target,
            origin_system: origin,
            started_at: 11,
            completes_at: 21,
            report_mode: ReportMode::Return,
        });
        assert_eq!(timing.origin_tick, 11);
        assert_eq!(timing.expected_tick, Some(21));
    }

    // -----------------------------------------------------------------
    // #491 follow-up: ship_view_with_timing
    // -----------------------------------------------------------------

    #[test]
    fn ship_view_with_timing_own_ship_uses_projection() {
        let mut world = World::new();
        let empire = world.spawn_empty().id();
        let home = world.spawn_empty().id();
        let frontier = world.spawn_empty().id();
        let ship_entity = world.spawn_empty().id();
        let ship = make_ship("Explorer", empire, home);
        // Realtime says InFTL — the projection-driven path must ignore
        // it both for the view and for the timing.
        let realtime = ShipState::InFTL {
            origin_system: home,
            destination_system: frontier,
            departed_at: 100,
            arrival_at: 200,
        };
        let mut store = KnowledgeStore::default();
        store.update_projection(ShipProjection {
            entity: ship_entity,
            dispatched_at: 5,
            expected_arrival_at: Some(15),
            expected_return_at: None,
            projected_state: ShipSnapshotState::InTransitFTL,
            projected_system: Some(frontier),
            intended_state: Some(ShipSnapshotState::Surveying),
            intended_system: Some(frontier),
            intended_takes_effect_at: Some(10),
        });

        let (view, timing) =
            ship_view_with_timing(ship_entity, &ship, &realtime, Some(&store), Some(empire))
                .expect("own-ship projection must produce both view and timing");
        assert_eq!(view.state, ShipSnapshotState::InTransitFTL);
        assert_eq!(view.system, Some(frontier));
        assert_eq!(timing.origin_tick, 5);
        assert_eq!(timing.expected_tick, Some(15));
    }

    #[test]
    fn ship_view_with_timing_foreign_ship_uses_snapshot() {
        let mut world = World::new();
        let viewing_empire = world.spawn_empty().id();
        let foreign_empire = world.spawn_empty().id();
        let foreign_sys = world.spawn_empty().id();
        let ship_entity = world.spawn_empty().id();
        let ship = make_ship("EnemyScout", foreign_empire, foreign_sys);
        let realtime = ShipState::InSystem {
            system: foreign_sys,
        };
        let mut store = KnowledgeStore::default();
        store.update_ship(ShipSnapshot {
            entity: ship_entity,
            name: "EnemyScout".into(),
            design_id: "explorer_mk1".into(),
            last_known_state: ShipSnapshotState::Surveying,
            last_known_system: Some(foreign_sys),
            observed_at: 17,
            hp: 100.0,
            hp_max: 100.0,
            source: ObservationSource::Direct,
        });

        let (view, timing) = ship_view_with_timing(
            ship_entity,
            &ship,
            &realtime,
            Some(&store),
            Some(viewing_empire),
        )
        .expect("foreign-ship snapshot must produce both view and timing");
        assert_eq!(view.state, ShipSnapshotState::Surveying);
        assert_eq!(view.system, Some(foreign_sys));
        assert_eq!(timing.origin_tick, 17);
        // Foreign ETA is unknowable from a snapshot — must fall back to None.
        assert_eq!(timing.expected_tick, None);
    }

    #[test]
    fn ship_view_with_timing_no_store_falls_back_to_realtime() {
        let mut world = World::new();
        let empire = world.spawn_empty().id();
        let home = world.spawn_empty().id();
        let frontier = world.spawn_empty().id();
        let ship_entity = world.spawn_empty().id();
        let ship = make_ship("Explorer", empire, home);
        let realtime = ShipState::InFTL {
            origin_system: home,
            destination_system: frontier,
            departed_at: 0,
            arrival_at: 5,
        };

        let (view, timing) =
            ship_view_with_timing(ship_entity, &ship, &realtime, None, Some(empire))
                .expect("no-store fallback must produce both view and timing");
        assert_eq!(view.state, ShipSnapshotState::InTransitFTL);
        assert_eq!(view.system, Some(frontier));
        assert_eq!(timing.origin_tick, 0);
        assert_eq!(timing.expected_tick, Some(5));
    }

    #[test]
    fn ship_view_with_timing_own_no_projection_returns_none() {
        let mut world = World::new();
        let empire = world.spawn_empty().id();
        let home = world.spawn_empty().id();
        let ship_entity = world.spawn_empty().id();
        let ship = make_ship("Explorer", empire, home);
        let realtime = ShipState::InSystem { system: home };
        let store = KnowledgeStore::default();
        // Own-ship + KnowledgeStore but no projection — same contract as
        // `ship_view`: returns `None`, callers decide how to render the
        // absence (skip / "Unknown").
        let result =
            ship_view_with_timing(ship_entity, &ship, &realtime, Some(&store), Some(empire));
        assert!(result.is_none());
    }

    // -----------------------------------------------------------------
    // #491 follow-up: ShipView::is_actionable
    // -----------------------------------------------------------------

    #[test]
    fn ship_view_is_actionable_returns_true_for_normal_states() {
        for state in [
            ShipSnapshotState::InSystem,
            ShipSnapshotState::InTransitSubLight,
            ShipSnapshotState::InTransitFTL,
            ShipSnapshotState::Surveying,
            ShipSnapshotState::Settling,
            ShipSnapshotState::Refitting,
            ShipSnapshotState::Loitering {
                position: [0.0, 0.0, 0.0],
            },
        ] {
            let view = ShipView {
                state: state.clone(),
                system: None,
            };
            assert!(view.is_actionable(), "state {state:?} should be actionable");
        }
    }

    #[test]
    fn ship_view_is_actionable_returns_false_for_destroyed() {
        let view = ShipView {
            state: ShipSnapshotState::Destroyed,
            system: None,
        };
        assert!(!view.is_actionable());
    }

    #[test]
    fn ship_view_is_actionable_returns_false_for_missing() {
        let view = ShipView {
            state: ShipSnapshotState::Missing,
            system: None,
        };
        assert!(!view.is_actionable());
    }
}
