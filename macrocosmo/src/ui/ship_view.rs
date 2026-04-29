//! #491: Egui-adjacent helpers that turn a [`ShipView`] into the
//! per-panel labels and progress data the UI panels render.
//!
//! The data shape ([`ShipView`], [`ShipViewTiming`]) and the selection
//! logic ([`ship_view`], [`realtime_state_to_snapshot`]) live in
//! [`crate::knowledge::ship_view`] — they have no UI dependencies.
//! This module owns the egui-adjacent formatters that take a `Query`
//! over star systems plus a [`ShipView`] and produce the strings /
//! progress structs the panels show.
//!
//! Re-exports the data shape so callers that imported from the old
//! `ui::ship_view` location continue to compile unchanged.
//!
//! ## Production callers
//!
//! As of #491 (this prep PR), the egui-adjacent formatter helpers in
//! this module ([`ship_view_label`], [`ship_view_progress`],
//! [`ship_view_eta`], [`ship_view_state_supports_progress`],
//! [`ship_view_status_label`]) have **no production callers** —
//! they are intentionally landed ahead of the consumer panels:
//!
//! * PR #2 — `ship_panel`
//! * PR #3 — `context_menu`
//! * PR #4 — `situation_center`
//! * PR #5 — `system_panel`
//! * PR #6 — `ui::mod` map tooltip
//!
//! All current outline-tree formatting flows through the existing
//! `outline.rs` private helpers (`snapshot_status_in_transit_label` /
//! `snapshot_status_tooltip_label`); those will be migrated alongside
//! the panel rewires above. Unit tests below cover the helpers
//! exhaustively so the API freezes at a sensible shape — if PR #2..#6
//! surface design pressure, the helpers may need to widen, at which
//! point the consumer-side change should land in the same PR as the
//! helper modification (do **not** post-hoc widen the API in this
//! prep PR).
//!
//! Reviewers: this is *intentional* pre-landing, not dead code.

use bevy::prelude::*;

use crate::components::Position;
use crate::galaxy::{StarSystem, SystemAttributes};
use crate::knowledge::ShipSnapshotState;
use crate::time_system::GameClock;
use crate::ui::params::system_name;

// #491 (D-C-1): the data shape moved to `knowledge::ship_view`. Re-export
// from this module so the existing `use crate::ui::ship_view::ShipView`
// import sites keep working.
pub use crate::knowledge::ship_view::{
    ShipView, ShipViewTiming, realtime_state_to_snapshot, ship_view,
};

/// #491 (D-M-12): Progress data for an in-flight or in-progress ship
/// activity.
///
/// * `elapsed` is the **raw** delta since `origin_tick` — not clamped,
///   so callers can detect overdue activity by checking
///   `elapsed > total`.
/// * `total` is `expected_tick - origin_tick`, clamped to `>= 1` to
///   avoid division by zero on same-tick activities. Always non-negative.
/// * `fraction` is `elapsed / total`, clamped to `[0.0, 1.0]` so a stale
///   clock past `expected_tick` does not over-extrapolate progress bars.
/// * `is_overdue` is `true` when the activity should already have
///   completed (= the activity has run for longer than its expected
///   duration). Useful for highlighting stuck ships.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShipViewProgress {
    pub elapsed: i64,
    pub total: i64,
    pub fraction: f32,
    pub is_overdue: bool,
}

/// **PR #491 prep**: no production caller as of this PR; consumers land in
/// #491 PR #2..#6.
///
/// #491: ETA accessor — returns the projected / observed completion
/// tick when the panel has timing data, or `None` for open-ended /
/// steady-state activities.
///
/// Pure passthrough today. Kept as a function so future light-speed
/// adjustments (e.g. clamping foreign ETAs to the viewing empire's
/// knowledge horizon) can land here without changing every call site.
pub fn ship_view_eta(timing: Option<&ShipViewTiming>) -> Option<i64> {
    timing.and_then(|t| t.expected_tick)
}

/// **PR #491 prep**: no production caller as of this PR; consumers land in
/// #491 PR #2..#6.
///
/// #491 (D-M-12 + B-NTF-1): Compute progress as a [`ShipViewProgress`].
///
/// * `now < origin_tick` → `elapsed = 0`, `fraction = 0.0`,
///   `is_overdue = false`. (Projection's `dispatched_at` can briefly
///   lead the local clock during reconcile.)
/// * Mid-flight → `elapsed = now - origin_tick`,
///   `fraction = elapsed / total`, `is_overdue = false`.
/// * `now == expected_tick` → just-completed: `fraction = 1.0`,
///   `is_overdue = false` (the activity finished exactly on schedule).
/// * `now > expected_tick` → `elapsed = now - origin_tick` (raw),
///   `fraction = 1.0` (clamped), `is_overdue = true`.
/// * `expected_tick == None` → `None` (open-ended activity).
/// * `timing == None` → `None`.
///
/// `total` is clamped to `>= 1` to avoid division by zero when
/// `origin_tick == expected_tick` (= a same-tick activity). All
/// arithmetic uses `saturating_sub` to handle sentinel ticks
/// (`i64::MIN` / `i64::MAX`) without overflow.
pub fn ship_view_progress(timing: Option<&ShipViewTiming>, now: i64) -> Option<ShipViewProgress> {
    let timing = timing?;
    let expected = timing.expected_tick?;
    let total = expected.saturating_sub(timing.origin_tick).max(1);
    let raw_elapsed = now.saturating_sub(timing.origin_tick).max(0);
    // Fraction uses clamped elapsed so progress bars never over-extend.
    let clamped = raw_elapsed.min(total);
    let fraction = (clamped as f32 / total as f32).clamp(0.0, 1.0);
    // #491 boundary fix: `now == expected_tick` is "just completed" —
    // not overdue. Strict `now > expected` means the next tick after
    // completion is the first one flagged as overdue.
    let is_overdue = now > expected;
    Some(ShipViewProgress {
        elapsed: raw_elapsed,
        total,
        fraction,
        is_overdue,
    })
}

/// **PR #491 prep**: no production caller as of this PR; consumers land in
/// #491 PR #2..#6.
///
/// #491: Light-coherent status label for a [`ShipView`].
///
/// Switches on `view.state` (= a [`ShipSnapshotState`]) — the projection
/// / snapshot collapse already happened upstream, so this function does
/// not see realtime [`crate::ship::ShipState`] details. Per #491 (D-H-4)
/// `InTransitSubLight` and `InTransitFTL` produce different labels
/// because the player UI must surface the FTL/sublight distinction
/// (FTL ships cannot be intercepted by game contract).
///
/// `timing` is consumed by callers via [`ship_view_progress`] /
/// [`ship_view_eta`] separately — the label itself does not include
/// progress digits any more (#491 D-H-8 / D-M-10). Callers that want
/// the legacy `"X/Y hd, Z%"` suffix concatenate the two themselves.
pub fn ship_view_label(
    view: &ShipView,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
) -> String {
    match &view.state {
        ShipSnapshotState::InSystem => {
            let name = view
                .system
                .map(|s| system_name(s, stars))
                .unwrap_or_else(|| "Unknown".to_string());
            format!("Docked at {}", name)
        }
        ShipSnapshotState::InTransitSubLight => {
            let name = view
                .system
                .map(|s| system_name(s, stars))
                .unwrap_or_else(|| "deep space".to_string());
            format!("Moving to {}", name)
        }
        ShipSnapshotState::InTransitFTL => {
            let name = view
                .system
                .map(|s| system_name(s, stars))
                .unwrap_or_else(|| "deep space".to_string());
            format!("FTL to {}", name)
        }
        ShipSnapshotState::Surveying => {
            // #491 (B-NTF-3): when system is None, surface that to the
            // player rather than label the destination "Unknown".
            match view.system {
                Some(s) => format!("Surveying {}", system_name(s, stars)),
                None => "Surveying (target unknown)".to_string(),
            }
        }
        ShipSnapshotState::Settling => {
            let name = view
                .system
                .map(|s| system_name(s, stars))
                .unwrap_or_else(|| "Unknown".to_string());
            format!("Settling {}", name)
        }
        ShipSnapshotState::Refitting => {
            let name = view
                .system
                .map(|s| system_name(s, stars))
                .unwrap_or_else(|| "Unknown".to_string());
            format!("Refitting at {}", name)
        }
        ShipSnapshotState::Loitering { position } => format!(
            "Loitering at ({:.2}, {:.2}, {:.2})",
            position[0], position[1], position[2]
        ),
        ShipSnapshotState::Destroyed => "Destroyed".to_string(),
        ShipSnapshotState::Missing => "Missing".to_string(),
    }
}

/// **PR #491 prep**: no production caller as of this PR; consumers land in
/// #491 PR #2..#6.
///
/// #491 (B-NTF-4): Per-state predicate for "should the panel render
/// progress data even if the caller passed timing?".
///
/// The progress bar is meaningful for activities with bounded duration
/// — the transit / survey / settle / refit family. Steady-state and
/// terminal variants (`InSystem`, `Loitering`, `Destroyed`, `Missing`)
/// should never show progress, even if a stale [`ShipViewTiming`] is
/// passed in alongside them.
pub fn ship_view_state_supports_progress(state: &ShipSnapshotState) -> bool {
    matches!(
        state,
        ShipSnapshotState::InTransitSubLight
            | ShipSnapshotState::InTransitFTL
            | ShipSnapshotState::Surveying
            | ShipSnapshotState::Settling
            | ShipSnapshotState::Refitting
    )
}

/// **PR #491 prep**: no production caller as of this PR; consumers land in
/// #491 PR #2..#6.
///
/// #491 (D-H-8): Light-coherent status label + progress for a
/// [`ShipView`].
///
/// Convenience wrapper that calls [`ship_view_label`] and conditionally
/// [`ship_view_progress`] (gated on
/// [`ship_view_state_supports_progress`]). Callers that only need one
/// of the two should call the underlying helper directly — this
/// function exists for the per-panel "label + progress bar" pattern
/// that several panels share (#491 PR #2..#6).
pub fn ship_view_status_label(
    view: &ShipView,
    timing: Option<ShipViewTiming>,
    clock: &GameClock,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
) -> (String, Option<ShipViewProgress>) {
    let label = ship_view_label(view, stars);
    let progress = if ship_view_state_supports_progress(&view.state) {
        ship_view_progress(timing.as_ref(), clock.elapsed)
    } else {
        // #491 (B-NTF-4): force None even when caller passed stale
        // timing — non-transit states never show a progress bar.
        None
    };
    (label, progress)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::Position;
    use crate::galaxy::StarSystem;
    use bevy::ecs::system::SystemState;

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

    fn label_for(
        view: ShipView,
        timing: Option<ShipViewTiming>,
        clock: GameClock,
        world: &mut World,
    ) -> (String, Option<ShipViewProgress>) {
        let mut state: SystemState<
            Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
        > = SystemState::new(world);
        let stars = state.get(world);
        ship_view_status_label(&view, timing, &clock, &stars)
    }

    // -----------------------------------------------------------------
    // ship_view_progress — boundary / mid-flight / open-ended +
    // sentinel + overdue (D-M-12, B-NTF-1)
    // -----------------------------------------------------------------

    #[test]
    fn progress_before_start_is_zero() {
        let timing = ShipViewTiming {
            origin_tick: 5,
            expected_tick: Some(15),
        };
        let p = ship_view_progress(Some(&timing), 3).expect("bounded => Some");
        assert_eq!(p.elapsed, 0);
        assert_eq!(p.total, 10);
        assert!((p.fraction - 0.0).abs() < 1e-6);
        assert!(!p.is_overdue);
    }

    #[test]
    fn progress_mid_flight_is_fractional() {
        let timing = ShipViewTiming {
            origin_tick: 0,
            expected_tick: Some(10),
        };
        let p = ship_view_progress(Some(&timing), 3).expect("bounded => Some");
        assert_eq!(p.elapsed, 3);
        assert_eq!(p.total, 10);
        assert!((p.fraction - 0.3).abs() < 1e-6);
        assert!(!p.is_overdue);
    }

    #[test]
    fn progress_flags_overdue_when_now_exceeds_expected() {
        let timing = ShipViewTiming {
            origin_tick: 0,
            expected_tick: Some(10),
        };
        let p = ship_view_progress(Some(&timing), 25).expect("bounded => Some");
        assert!(p.is_overdue, "now=25 past expected=10 should be overdue");
        // fraction clamped, raw elapsed retained
        assert!((p.fraction - 1.0).abs() < 1e-6);
    }

    /// #491 boundary fix: strict `now > expected` semantics. `now ==
    /// expected` is "just completed" (not overdue); the first overdue
    /// tick is `now == expected + 1`. Pins the off-by-one regression
    /// flagged in adversarial review.
    #[test]
    fn progress_is_overdue_only_when_now_strictly_exceeds_expected() {
        let timing = ShipViewTiming {
            origin_tick: 0,
            expected_tick: Some(10),
        };

        // now == expected → just completed, NOT overdue.
        let p = ship_view_progress(Some(&timing), 10).expect("bounded => Some");
        assert!(
            !p.is_overdue,
            "now == expected_tick is just-completed, must not be overdue"
        );
        // Fraction is 1.0 — the activity is at completion.
        assert!((p.fraction - 1.0).abs() < 1e-6);

        // now == expected + 1 → first overdue tick.
        let p = ship_view_progress(Some(&timing), 11).expect("bounded => Some");
        assert!(
            p.is_overdue,
            "now == expected_tick + 1 must flag as overdue"
        );
    }

    #[test]
    fn progress_returns_raw_elapsed_for_overdue() {
        let timing = ShipViewTiming {
            origin_tick: 0,
            expected_tick: Some(10),
        };
        let p = ship_view_progress(Some(&timing), 25).expect("bounded => Some");
        // Raw elapsed: caller can detect "stuck" by checking elapsed > total.
        assert_eq!(p.elapsed, 25);
        assert_eq!(p.total, 10);
        assert!(p.elapsed > p.total);
    }

    #[test]
    fn progress_no_expected_is_none() {
        let timing = ShipViewTiming {
            origin_tick: 0,
            expected_tick: None,
        };
        assert_eq!(ship_view_progress(Some(&timing), 5), None);
    }

    #[test]
    fn progress_no_timing_is_none() {
        assert_eq!(ship_view_progress(None, 5), None);
    }

    #[test]
    fn progress_same_tick_total_clamped_to_one() {
        let timing = ShipViewTiming {
            origin_tick: 5,
            expected_tick: Some(5),
        };
        let p = ship_view_progress(Some(&timing), 5).expect("bounded => Some");
        assert_eq!(p.elapsed, 0);
        assert_eq!(p.total, 1);
        assert!(!p.is_overdue);
    }

    #[test]
    fn progress_handles_sentinel_ticks() {
        // B-NTF-1: extreme i64 inputs must not panic via overflow.
        let timing = ShipViewTiming {
            origin_tick: i64::MIN,
            expected_tick: Some(i64::MAX),
        };
        // Should not panic.
        let p = ship_view_progress(Some(&timing), 0).expect("bounded => Some");
        assert!(p.total >= 1);
        assert!(p.fraction >= 0.0 && p.fraction <= 1.0);
    }

    #[test]
    fn eta_returns_expected_tick() {
        let timing = ShipViewTiming {
            origin_tick: 0,
            expected_tick: Some(42),
        };
        assert_eq!(ship_view_eta(Some(&timing)), Some(42));
        assert_eq!(ship_view_eta(None), None);
        let timing_open = ShipViewTiming {
            origin_tick: 0,
            expected_tick: None,
        };
        assert_eq!(ship_view_eta(Some(&timing_open)), None);
    }

    // -----------------------------------------------------------------
    // ship_view_status_label — structural assertions per variant
    // (D-M-10)
    // -----------------------------------------------------------------

    #[test]
    fn status_label_in_system_docked() {
        let mut world = World::new();
        let sys = spawn_star(&mut world, "Sol", [0.0, 0.0, 0.0]);
        let (label, progress) = label_for(
            ShipView {
                state: ShipSnapshotState::InSystem,
                system: Some(sys),
            },
            None,
            GameClock::new(0),
            &mut world,
        );
        assert!(label.contains("Docked"));
        assert!(label.contains("Sol"));
        assert_eq!(progress, None);
    }

    #[test]
    fn status_label_in_transit_sublight_with_timing() {
        let mut world = World::new();
        let dest = spawn_star(&mut world, "Frontier", [50.0, 0.0, 0.0]);
        let (label, progress) = label_for(
            ShipView {
                state: ShipSnapshotState::InTransitSubLight,
                system: Some(dest),
            },
            Some(ShipViewTiming {
                origin_tick: 0,
                expected_tick: Some(10),
            }),
            GameClock::new(5),
            &mut world,
        );
        assert!(label.contains("Moving") || label.contains("Transit"));
        assert!(label.contains("Frontier"));
        // Critical FTL leak invariant — the label must not say "FTL"
        // for a sublight ship (#491 D-H-4).
        assert!(
            !label.contains("FTL"),
            "sublight transit must never render the 'FTL' marker"
        );
        let p = progress.expect("transit must produce progress");
        assert_eq!(p.elapsed, 5);
        assert_eq!(p.total, 10);
        assert!((p.fraction - 0.5).abs() < 1e-6);
    }

    #[test]
    fn status_label_in_transit_ftl_with_timing() {
        let mut world = World::new();
        let dest = spawn_star(&mut world, "Frontier", [50.0, 0.0, 0.0]);
        let (label, progress) = label_for(
            ShipView {
                state: ShipSnapshotState::InTransitFTL,
                system: Some(dest),
            },
            Some(ShipViewTiming {
                origin_tick: 0,
                expected_tick: Some(10),
            }),
            GameClock::new(3),
            &mut world,
        );
        assert!(label.contains("FTL"));
        assert!(label.contains("Frontier"));
        assert!(progress.is_some());
    }

    #[test]
    fn status_label_in_transit_without_timing() {
        let mut world = World::new();
        let dest = spawn_star(&mut world, "Frontier", [50.0, 0.0, 0.0]);
        let (label, progress) = label_for(
            ShipView {
                state: ShipSnapshotState::InTransitSubLight,
                system: Some(dest),
            },
            None,
            GameClock::new(5),
            &mut world,
        );
        assert!(label.contains("Frontier"));
        assert_eq!(progress, None);
    }

    #[test]
    fn status_label_surveying_with_timing() {
        let mut world = World::new();
        let target = spawn_star(&mut world, "Frontier", [50.0, 0.0, 0.0]);
        let (label, progress) = label_for(
            ShipView {
                state: ShipSnapshotState::Surveying,
                system: Some(target),
            },
            Some(ShipViewTiming {
                origin_tick: 0,
                expected_tick: Some(10),
            }),
            GameClock::new(3),
            &mut world,
        );
        assert!(label.contains("Surveying"));
        assert!(label.contains("Frontier"));
        assert!(progress.is_some());
    }

    #[test]
    fn status_label_surveying_with_none_system() {
        // B-NTF-3: explicit signal that the target is unknown rather
        // than literal "Unknown".
        let mut world = World::new();
        let (label, _) = label_for(
            ShipView {
                state: ShipSnapshotState::Surveying,
                system: None,
            },
            None,
            GameClock::new(0),
            &mut world,
        );
        assert!(label.contains("Surveying"));
        assert!(
            label.contains("unknown") || label.contains("Unknown"),
            "label should signal that the survey target is unknown: got {label:?}"
        );
    }

    #[test]
    fn status_label_loitering() {
        let mut world = World::new();
        let (label, progress) = label_for(
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
        assert!(label.contains("Loitering"));
        assert_eq!(progress, None);
    }

    #[test]
    fn status_label_destroyed() {
        let mut world = World::new();
        let (label, progress) = label_for(
            ShipView {
                state: ShipSnapshotState::Destroyed,
                system: None,
            },
            None,
            GameClock::new(0),
            &mut world,
        );
        assert_eq!(label, "Destroyed");
        assert_eq!(progress, None);
    }

    #[test]
    fn status_label_missing() {
        let mut world = World::new();
        let (label, progress) = label_for(
            ShipView {
                state: ShipSnapshotState::Missing,
                system: None,
            },
            None,
            GameClock::new(0),
            &mut world,
        );
        assert_eq!(label, "Missing");
        assert_eq!(progress, None);
    }

    // -----------------------------------------------------------------
    // B-NTF-4: stale timing is ignored for non-transit states
    // -----------------------------------------------------------------

    #[test]
    fn destroyed_with_stale_timing_yields_none_progress() {
        let mut world = World::new();
        let stale_timing = ShipViewTiming {
            origin_tick: 0,
            expected_tick: Some(10),
        };
        let (_, progress) = label_for(
            ShipView {
                state: ShipSnapshotState::Destroyed,
                system: None,
            },
            Some(stale_timing),
            GameClock::new(5),
            &mut world,
        );
        assert_eq!(
            progress, None,
            "Destroyed must never surface progress even with stale timing"
        );
    }

    #[test]
    fn missing_with_stale_timing_yields_none_progress() {
        let mut world = World::new();
        let stale_timing = ShipViewTiming {
            origin_tick: 0,
            expected_tick: Some(10),
        };
        let (_, progress) = label_for(
            ShipView {
                state: ShipSnapshotState::Missing,
                system: None,
            },
            Some(stale_timing),
            GameClock::new(5),
            &mut world,
        );
        assert_eq!(progress, None);
    }

    #[test]
    fn in_system_with_stale_timing_yields_none_progress() {
        let mut world = World::new();
        let sys = spawn_star(&mut world, "Sol", [0.0, 0.0, 0.0]);
        let stale_timing = ShipViewTiming {
            origin_tick: 0,
            expected_tick: Some(10),
        };
        let (_, progress) = label_for(
            ShipView {
                state: ShipSnapshotState::InSystem,
                system: Some(sys),
            },
            Some(stale_timing),
            GameClock::new(5),
            &mut world,
        );
        assert_eq!(
            progress, None,
            "InSystem must never surface progress (steady-state)"
        );
    }

    #[test]
    fn loitering_with_stale_timing_yields_none_progress() {
        let mut world = World::new();
        let stale_timing = ShipViewTiming {
            origin_tick: 0,
            expected_tick: Some(10),
        };
        let (_, progress) = label_for(
            ShipView {
                state: ShipSnapshotState::Loitering {
                    position: [0.0, 0.0, 0.0],
                },
                system: None,
            },
            Some(stale_timing),
            GameClock::new(5),
            &mut world,
        );
        assert_eq!(progress, None);
    }
}
