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

use bevy::prelude::*;

use crate::components::Position;
use crate::ship::{Owner, Ship, ShipState};
use crate::time_system::GameClock;

use super::{KnowledgeStore, ShipSnapshotState};

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
/// * **Observer mode** (= empire-view, viewing as another empire):
///   identical to own-empire normal play — projection / snapshot of the
///   **viewing empire** (= the observed empire whose perspective the
///   player is borrowing). The caller passes the observed empire as
///   `viewing_empire`. A separate omniscient (god-view) mode is the
///   right way to expose realtime ground truth (#490, follow-up).
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
}
