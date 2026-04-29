//! #491: Shared ship-view helpers consumed by every UI panel.
//!
//! `outline.rs` factored the original `ShipOutlineView` / `ship_outline_view`
//! / `realtime_state_to_snapshot` trio in epic #473 / #487 to fix the FTL
//! leak in the outline tree. The remaining 5 panels (`ship_panel`,
//! `context_menu`, `situation_center::ship_ops_tab`, `system_panel`,
//! `ui::mod`) still consume realtime ECS [`ShipState`] directly. Issue
//! #491 rewires them all to consume the same projection-/snapshot-mediated
//! view — but each panel needs slightly different output (status label,
//! progress fraction, ETA). This module owns the shared data shape and
//! helpers so the per-panel rewires (PR #2..#6) can share one definition.
//!
//! No behaviour change in this PR — only:
//! * Renamed `ShipOutlineView` → [`ShipView`]; `ship_outline_view` →
//!   [`ship_view`]. `outline.rs` continues to expose the old names as
//!   `pub use` aliases for backward compatibility.
//! * `realtime_state_to_snapshot` is now `pub` so non-outline panels can
//!   reuse the realtime-→-snapshot collapse used in observer / startup
//!   fallback paths.
//! * New helpers: [`ship_view_status_label`], [`ship_view_eta`],
//!   [`ship_view_progress`], [`ShipViewTiming`].

use bevy::prelude::*;

use crate::components::Position;
use crate::galaxy::{StarSystem, SystemAttributes};
use crate::knowledge::{KnowledgeStore, ShipSnapshotState};
use crate::ship::{Owner, Ship, ShipState};
use crate::time_system::GameClock;

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

/// #491: Light-coherent timing data for [`ship_view_status_label`] and
/// the progress / ETA accessors.
///
/// For own-empire ships the panel rewires (PR #2..#6) populate this from
/// [`crate::knowledge::ShipProjection`] — `started_at` from `dispatched_at`
/// (or `intended_takes_effect_at` for in-flight commands) and
/// `expected_at` from `expected_arrival_at` / `expected_return_at`. For
/// foreign ships the timing comes from `ShipSnapshot::observed_at` plus
/// any per-snapshot ETA the snapshot writer chooses to expose.
///
/// `None` everywhere means the panel cannot draw a progress bar — only
/// a static status label is rendered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShipViewTiming {
    /// Tick at which the activity began (FTL departure, settle start, etc.).
    pub started_at: i64,
    /// Tick at which the activity is expected to complete. `None` for
    /// open-ended states (e.g. loitering, no projected arrival).
    pub expected_at: Option<i64>,
}

/// #491: Display data for the per-panel status section. Mirrors the
/// shape of `ship_panel::build_status_info` (private today; PR #2 will
/// rewire to consume this struct).
#[derive(Clone, Debug, PartialEq)]
pub struct ShipStatusInfo {
    pub label: String,
    /// `(elapsed_hexadies, total_hexadies, fraction_0_1)` when the
    /// activity has bounded timing. `None` for open-ended / steady-state
    /// activities (`InSystem`, `Loitering`, `Destroyed`, `Missing`).
    pub progress: Option<(i64, i64, f32)>,
}

/// #487 / #491: Convert a realtime [`ShipState`] to the corresponding
/// [`ShipSnapshotState`] for observer-mode ground-truth rendering and
/// the `viewing_knowledge.is_none()` fallback path.
///
/// Mirrors the conversion used at observation-recording time in the
/// ship-snapshot writer. `SubLight`/`InFTL`/`Scouting` collapse into
/// the coarser `InTransit`/`Surveying` snapshot variants — so observer
/// mode and the projection-driven path render the same set of labels.
pub fn realtime_state_to_snapshot(state: &ShipState) -> (ShipSnapshotState, Option<Entity>) {
    match state {
        ShipState::InSystem { system } => (ShipSnapshotState::InSystem, Some(*system)),
        ShipState::SubLight { target_system, .. } => (ShipSnapshotState::InTransit, *target_system),
        ShipState::InFTL {
            destination_system, ..
        } => (ShipSnapshotState::InTransit, Some(*destination_system)),
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
        ShipState::Scouting { target_system, .. } => {
            (ShipSnapshotState::Surveying, Some(*target_system))
        }
    }
}

/// #487 / #491: Compute the light-coherent view of a ship, gated by the
/// light-speed contract.
///
/// * **Own-empire ship** (in normal play): read the projected state from
///   the viewing empire's `KnowledgeStore::projections`. The realtime ECS
///   [`ShipState`] is intentionally ignored — that's the FTL leak fix
///   (epic #473 / #487).
/// * **Foreign ship** (in normal play): read the last-known state from
///   the viewing empire's `KnowledgeStore::ship_snapshots`. Unchanged
///   from the pre-#487 contract (it was already snapshot-mediated).
/// * **Observer mode** (= empire-view, viewing as another empire):
///   treated identically to own-empire normal play — projection /
///   snapshot of the **viewing empire** (= the observed empire whose
///   perspective the player is borrowing). Light-speed coherent. A
///   separate omniscient (god-view) mode is the right way to expose
///   realtime ground truth (#490, follow-up).
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
/// `_is_observer` is preserved to keep the shape of the API stable
/// while #497 separately removes it (it is currently unused inside the
/// helper — the projection / snapshot routing already covers observer
/// mode).
pub fn ship_view(
    ship_entity: Entity,
    ship: &Ship,
    realtime_state: &ShipState,
    viewing_knowledge: Option<&KnowledgeStore>,
    viewing_empire: Option<Entity>,
    _is_observer: bool,
) -> Option<ShipView> {
    // No KnowledgeStore resolved (e.g. very early Startup frames before
    // empires are wired). Fall back to realtime ECS as a defensive path.
    // Observer mode (#440) is NOT a fall-through: it still uses the
    // viewing empire's KnowledgeStore — that's the whole point of
    // empire-view observer being light-coherent.
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

/// Resolve an Entity to a star system name, falling back to "Unknown".
///
/// Local mirror of `ship_panel::system_name` — the two will be
/// consolidated when the ship-panel rewire lands (PR #2 in the #491
/// epic). Kept here so this module is self-contained and the unit
/// tests below don't need to import from `ship_panel`.
fn system_name(
    entity: Entity,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
) -> String {
    stars
        .get(entity)
        .map(|(_, s, _, _)| s.name.clone())
        .unwrap_or_else(|_| "Unknown".to_string())
}

/// #491: ETA accessor — returns the projected / observed completion
/// tick when the panel has timing data, or `None` for open-ended /
/// steady-state activities.
///
/// Pure passthrough today. Kept as a function so future light-speed
/// adjustments (e.g. clamping foreign ETAs to the viewing empire's
/// knowledge horizon) can land here without changing every call site.
pub fn ship_view_eta(timing: Option<&ShipViewTiming>) -> Option<i64> {
    timing.and_then(|t| t.expected_at)
}

/// #491: Compute progress as `(elapsed, total, fraction)`.
///
/// * `now < started_at` → `(0, total, 0.0)` (clamped — projection's
///   `dispatched_at` can briefly lead the local clock during reconcile).
/// * `now > expected_at` → `(total, total, 1.0)`.
/// * Mid-flight → `((now - started_at), (expected_at - started_at),
///   fraction)`.
/// * `expected_at == None` → `None` (open-ended activity, no progress).
///
/// `total` is clamped to `>= 1` to avoid division by zero when
/// `started_at == expected_at` (= a same-tick activity).
pub fn ship_view_progress(timing: Option<&ShipViewTiming>, now: i64) -> Option<(i64, i64, f32)> {
    let timing = timing?;
    let expected = timing.expected_at?;
    let total = (expected - timing.started_at).max(1);
    let elapsed = (now - timing.started_at).clamp(0, total);
    let pct = elapsed as f32 / total as f32;
    Some((elapsed, total, pct))
}

/// #491: Light-coherent status label + progress for a [`ShipView`].
///
/// Replaces the per-panel `build_status_info` family by switching on
/// `view.state` (= a [`ShipSnapshotState`]) instead of a realtime
/// [`ShipState`]. The coarser snapshot variants mean some labels
/// collapse — `SubLight` / `InFTL` both render as `"In Transit"`,
/// `Scouting` collapses into `Surveying`. That's the design intent: the
/// player can't tell `SubLight` from `InFTL` for a remote ship anyway
/// (light-speed coherence).
///
/// `timing` carries the dispatch / arrival ticks the caller resolved
/// from `ShipProjection` (own ships) or `ShipSnapshot` (foreign). When
/// `None` the label shows no `(X/Y hd, Z%)` suffix — useful for snapshot
/// states without committed ETAs.
pub fn ship_view_status_label(
    view: &ShipView,
    timing: Option<ShipViewTiming>,
    clock: &GameClock,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
) -> ShipStatusInfo {
    let progress = ship_view_progress(timing.as_ref(), clock.elapsed);

    let label = match &view.state {
        ShipSnapshotState::InSystem => {
            let name = view
                .system
                .map(|s| system_name(s, stars))
                .unwrap_or_else(|| "Unknown".to_string());
            format!("Docked at {}", name)
        }
        ShipSnapshotState::InTransit => {
            let name = view
                .system
                .map(|s| system_name(s, stars))
                .unwrap_or_else(|| "deep space".to_string());
            match progress {
                Some((elapsed, total, pct)) => format!(
                    "In Transit to {} ({}/{} hd, {:.0}%)",
                    name,
                    elapsed,
                    total,
                    pct * 100.0
                ),
                None => format!("In Transit to {}", name),
            }
        }
        ShipSnapshotState::Surveying => {
            let name = view
                .system
                .map(|s| system_name(s, stars))
                .unwrap_or_else(|| "Unknown".to_string());
            match progress {
                Some((elapsed, total, pct)) => format!(
                    "Surveying {} ({}/{} hd, {:.0}%)",
                    name,
                    elapsed,
                    total,
                    pct * 100.0
                ),
                None => format!("Surveying {} ...", name),
            }
        }
        ShipSnapshotState::Settling => {
            let name = view
                .system
                .map(|s| system_name(s, stars))
                .unwrap_or_else(|| "Unknown".to_string());
            match progress {
                Some((elapsed, total, pct)) => format!(
                    "Settling {} ({}/{} hd, {:.0}%)",
                    name,
                    elapsed,
                    total,
                    pct * 100.0
                ),
                None => format!("Settling {} ...", name),
            }
        }
        ShipSnapshotState::Refitting => {
            let name = view
                .system
                .map(|s| system_name(s, stars))
                .unwrap_or_else(|| "Unknown".to_string());
            match progress {
                Some((elapsed, total, pct)) => format!(
                    "Refitting at {} ({}/{} hd, {:.0}%)",
                    name,
                    elapsed,
                    total,
                    pct * 100.0
                ),
                None => format!("Refitting at {} ...", name),
            }
        }
        ShipSnapshotState::Loitering { position } => format!(
            "Loitering at ({:.2}, {:.2}, {:.2})",
            position[0], position[1], position[2]
        ),
        ShipSnapshotState::Destroyed => "Destroyed".to_string(),
        ShipSnapshotState::Missing => "Missing".to_string(),
    };

    ShipStatusInfo { label, progress }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::Position;
    use crate::galaxy::StarSystem;
    use crate::knowledge::{ObservationSource, ShipProjection, ShipSnapshot};
    use crate::ship::Owner;
    use bevy::ecs::system::SystemState;

    /// Spawn a minimal star system entity with just the components
    /// `system_name` consumes (no Sovereignty / TechKnowledge / etc.,
    /// which the `tests/common::spawn_test_system` helper would pull
    /// in). Unit tests don't need the full bundle.
    fn spawn_star(world: &mut World, name: &str, pos: [f64; 3]) -> Entity {
        world
            .spawn((
                StarSystem {
                    name: name.to_string(),
                    surveyed: true,
                    is_capital: false,
                    star_type: "default".into(),
                },
                Position::from(pos),
            ))
            .id()
    }

    /// Build a minimal `Ship` Component with sane defaults, owned by
    /// the given empire entity. Unit tests don't need fleets / cargo /
    /// hp because `ship_view` only reads `Ship.owner`.
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
    // realtime_state_to_snapshot — all 8 variants
    // -----------------------------------------------------------------

    #[test]
    fn realtime_state_to_snapshot_in_system() {
        let mut world = World::new();
        let sys = world.spawn_empty().id();
        let (s, sys_e) = realtime_state_to_snapshot(&ShipState::InSystem { system: sys });
        assert_eq!(s, ShipSnapshotState::InSystem);
        assert_eq!(sys_e, Some(sys));
    }

    #[test]
    fn realtime_state_to_snapshot_sublight() {
        let mut world = World::new();
        let target = world.spawn_empty().id();
        let (s, sys_e) = realtime_state_to_snapshot(&ShipState::SubLight {
            origin: [0.0, 0.0, 0.0],
            destination: [50.0, 0.0, 0.0],
            target_system: Some(target),
            departed_at: 0,
            arrival_at: 5,
        });
        assert_eq!(s, ShipSnapshotState::InTransit);
        assert_eq!(sys_e, Some(target));
    }

    #[test]
    fn realtime_state_to_snapshot_sublight_open_target() {
        // SubLight to a deep-space coordinate — no target system.
        let (s, sys_e) = realtime_state_to_snapshot(&ShipState::SubLight {
            origin: [0.0, 0.0, 0.0],
            destination: [50.0, 0.0, 0.0],
            target_system: None,
            departed_at: 0,
            arrival_at: 5,
        });
        assert_eq!(s, ShipSnapshotState::InTransit);
        assert_eq!(sys_e, None);
    }

    #[test]
    fn realtime_state_to_snapshot_inftl() {
        let mut world = World::new();
        let dest = world.spawn_empty().id();
        let origin = world.spawn_empty().id();
        let (s, sys_e) = realtime_state_to_snapshot(&ShipState::InFTL {
            origin_system: origin,
            destination_system: dest,
            departed_at: 0,
            arrival_at: 5,
        });
        assert_eq!(s, ShipSnapshotState::InTransit);
        assert_eq!(sys_e, Some(dest));
    }

    #[test]
    fn realtime_state_to_snapshot_surveying() {
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
    fn realtime_state_to_snapshot_settling() {
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
    fn realtime_state_to_snapshot_refitting() {
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
    fn realtime_state_to_snapshot_loitering() {
        let pos = [10.5, -3.0, 7.25];
        let (s, sys_e) = realtime_state_to_snapshot(&ShipState::Loitering { position: pos });
        assert_eq!(s, ShipSnapshotState::Loitering { position: pos });
        assert_eq!(sys_e, None);
    }

    #[test]
    fn realtime_state_to_snapshot_scouting() {
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
        // Scouting collapses into Surveying at snapshot granularity.
        assert_eq!(s, ShipSnapshotState::Surveying);
        assert_eq!(sys_e, Some(target));
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

        let view = ship_view(
            ship_entity,
            &ship,
            &realtime,
            Some(&store),
            Some(empire),
            false,
        )
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
            false,
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
        // Empty store — no projection populated.
        let store = KnowledgeStore::default();
        let view = ship_view(
            ship_entity,
            &ship,
            &realtime,
            Some(&store),
            Some(empire),
            false,
        );
        assert!(
            view.is_none(),
            "ship_view returns None for own-ship without projection (caller skips)"
        );
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

        let view = ship_view(ship_entity, &ship, &realtime, None, Some(empire), false)
            .expect("startup fallback must produce a view");
        // No KnowledgeStore = realtime read => InTransit (collapsed from InFTL).
        assert_eq!(view.state, ShipSnapshotState::InTransit);
        assert_eq!(view.system, Some(frontier));
    }

    // -----------------------------------------------------------------
    // ship_view_progress — boundary / mid-flight / open-ended
    // -----------------------------------------------------------------

    #[test]
    fn ship_view_progress_before_start_is_zero() {
        let timing = ShipViewTiming {
            started_at: 5,
            expected_at: Some(15),
        };
        let p = ship_view_progress(Some(&timing), 3).expect("bounded => Some");
        assert_eq!(p, (0, 10, 0.0));
    }

    #[test]
    fn ship_view_progress_after_expected_is_one() {
        let timing = ShipViewTiming {
            started_at: 0,
            expected_at: Some(10),
        };
        let p = ship_view_progress(Some(&timing), 25).expect("bounded => Some");
        assert_eq!(p, (10, 10, 1.0));
    }

    #[test]
    fn ship_view_progress_mid_flight_is_fractional() {
        let timing = ShipViewTiming {
            started_at: 0,
            expected_at: Some(10),
        };
        let p = ship_view_progress(Some(&timing), 3).expect("bounded => Some");
        assert_eq!(p.0, 3);
        assert_eq!(p.1, 10);
        assert!(
            (p.2 - 0.3).abs() < 1e-6,
            "fraction = 3/10 = 0.3, got {}",
            p.2
        );
    }

    #[test]
    fn ship_view_progress_no_expected_is_none() {
        let timing = ShipViewTiming {
            started_at: 0,
            expected_at: None,
        };
        assert_eq!(ship_view_progress(Some(&timing), 5), None);
    }

    #[test]
    fn ship_view_progress_no_timing_is_none() {
        assert_eq!(ship_view_progress(None, 5), None);
    }

    #[test]
    fn ship_view_progress_same_tick_total_clamped_to_one() {
        // started_at == expected_at — `total` clamps to 1 to avoid div-by-zero.
        let timing = ShipViewTiming {
            started_at: 5,
            expected_at: Some(5),
        };
        let p = ship_view_progress(Some(&timing), 5).expect("bounded => Some");
        // elapsed = (5 - 5) clamped to [0, 1] = 0; total = max(0, 1) = 1.
        assert_eq!(p, (0, 1, 0.0));
    }

    #[test]
    fn ship_view_eta_returns_expected_at() {
        let timing = ShipViewTiming {
            started_at: 0,
            expected_at: Some(42),
        };
        assert_eq!(ship_view_eta(Some(&timing)), Some(42));
        assert_eq!(ship_view_eta(None), None);
        let timing_open = ShipViewTiming {
            started_at: 0,
            expected_at: None,
        };
        assert_eq!(ship_view_eta(Some(&timing_open)), None);
    }

    // -----------------------------------------------------------------
    // ship_view_status_label — label shape per variant
    // -----------------------------------------------------------------

    /// Build a (`SystemState`, `clock`) pair suitable for invoking
    /// `ship_view_status_label`. Returns the world plus a `SystemState`
    /// that resolves the `stars` query — caller drives via `get(world)`.
    fn label_for(
        view: ShipView,
        timing: Option<ShipViewTiming>,
        clock: GameClock,
        world: &mut World,
    ) -> ShipStatusInfo {
        let mut state: SystemState<
            Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
        > = SystemState::new(world);
        let stars = state.get(world);
        ship_view_status_label(&view, timing, &clock, &stars)
    }

    #[test]
    fn ship_view_status_label_in_system() {
        let mut world = World::new();
        let sys = spawn_star(&mut world, "Sol", [0.0, 0.0, 0.0]);
        let info = label_for(
            ShipView {
                state: ShipSnapshotState::InSystem,
                system: Some(sys),
            },
            None,
            GameClock::new(0),
            &mut world,
        );
        assert_eq!(info.label, "Docked at Sol");
        assert_eq!(info.progress, None);
    }

    #[test]
    fn ship_view_status_label_in_transit_with_timing() {
        let mut world = World::new();
        let dest = spawn_star(&mut world, "Frontier", [50.0, 0.0, 0.0]);
        let info = label_for(
            ShipView {
                state: ShipSnapshotState::InTransit,
                system: Some(dest),
            },
            Some(ShipViewTiming {
                started_at: 0,
                expected_at: Some(10),
            }),
            GameClock::new(5),
            &mut world,
        );
        assert_eq!(info.label, "In Transit to Frontier (5/10 hd, 50%)");
        assert_eq!(info.progress, Some((5, 10, 0.5)));
    }

    #[test]
    fn ship_view_status_label_in_transit_without_timing() {
        let mut world = World::new();
        let dest = spawn_star(&mut world, "Frontier", [50.0, 0.0, 0.0]);
        let info = label_for(
            ShipView {
                state: ShipSnapshotState::InTransit,
                system: Some(dest),
            },
            None,
            GameClock::new(5),
            &mut world,
        );
        assert_eq!(info.label, "In Transit to Frontier");
        assert_eq!(info.progress, None);
    }

    #[test]
    fn ship_view_status_label_surveying_with_timing() {
        let mut world = World::new();
        let target = spawn_star(&mut world, "Frontier", [50.0, 0.0, 0.0]);
        let info = label_for(
            ShipView {
                state: ShipSnapshotState::Surveying,
                system: Some(target),
            },
            Some(ShipViewTiming {
                started_at: 0,
                expected_at: Some(10),
            }),
            GameClock::new(3),
            &mut world,
        );
        assert_eq!(info.label, "Surveying Frontier (3/10 hd, 30%)");
        assert!(info.progress.is_some());
    }

    #[test]
    fn ship_view_status_label_loitering() {
        let mut world = World::new();
        let info = label_for(
            ShipView {
                state: ShipSnapshotState::Loitering {
                    position: [1.5, 2.5, 3.5],
                },
                system: None,
            },
            None,
            GameClock::new(0),
            &mut world,
        );
        assert_eq!(info.label, "Loitering at (1.50, 2.50, 3.50)");
        assert_eq!(info.progress, None);
    }

    #[test]
    fn ship_view_status_label_destroyed() {
        let mut world = World::new();
        let info = label_for(
            ShipView {
                state: ShipSnapshotState::Destroyed,
                system: None,
            },
            None,
            GameClock::new(0),
            &mut world,
        );
        assert_eq!(info.label, "Destroyed");
        assert_eq!(info.progress, None);
    }
}
