//! Per-ship "pending AI assignment" markers (Round 9 PR #2 Step 4).
//!
//! Background: NPC decision ticks ranked unsurveyed systems and zipped them
//! with the per-faction set of idle surveyors. With overlapping ticks (a
//! ship dispatched on tick N still appears as an unsurveyed-target ship on
//! tick N+1 if the courier delay is long enough — and the destination star
//! stays unsurveyed until the surveyor actually arrives) the policy could
//! re-dispatch a *second* surveyor to the same target. Combined with no
//! handler-side dedup, Vesk Scout-2 was observed lining up behind Vesk
//! Scout-1 in a 30-hexadies loop.
//!
//! Fix shape: when the command consumer translates a `survey_system`
//! command into a `SurveyRequested` event, also stamp the dispatching ship
//! with a [`PendingAssignment`]. The NPC decision tick then filters out:
//!
//! 1. Target systems that already have a live `PendingAssignment` for this
//!    faction's surveyors.
//! 2. Ships that are themselves already carrying a `PendingAssignment`.
//!
//! The marker is removed by [`crate::ship::handlers::handle_survey_requested`]
//! on terminal results (Ok / Rejected) and swept after [`PendingAssignment::stale_at`]
//! by [`sweep_stale_assignments`] in case a handler never fires (ship
//! despawned mid-flight, etc.).
//!
//! The marker is **per faction** — when observer mode runs multiple
//! factions, each faction only sees its own pending dispatches.

use bevy::prelude::*;

use crate::time_system::GameClock;

/// What kind of work has been queued. Reserved for future expansion (#189
/// follow-up: Colonize, Scout, Repair, etc.).
#[derive(Debug, Clone, Copy, Reflect, PartialEq, Eq, Hash)]
pub enum AssignmentKind {
    /// `survey_system` command issued — handler will read the
    /// `SurveyRequested` message and either start the survey or auto-insert
    /// a `MoveTo` first (Deferred).
    Survey,
}

/// Where the assignment is targeted. Reserved for future expansion (planet-
/// level colonization, deep-space coordinates, etc.).
#[derive(Debug, Clone, Copy, Reflect)]
pub enum AssignmentTarget {
    /// A specific star system.
    System(Entity),
}

/// AI-side mirror of "this faction's ship has already been ordered to do
/// this thing — don't re-assign another ship to the same task next tick."
///
/// Attached to the **ship** entity. Removed when the underlying handler
/// resolves the request (Ok / Rejected) or when stale.
///
/// Per-faction so observer-mode multi-faction works: each NPC sees only its
/// own pending assignments, not other factions'.
#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
pub struct PendingAssignment {
    /// Empire entity that issued the command.
    pub faction: Entity,
    /// Kind of work queued.
    pub kind: AssignmentKind,
    /// Target of the work.
    pub target: AssignmentTarget,
    /// Hexadies tick at which the assignment was created.
    pub since: i64,
    /// Sweeper removes the marker after `clock.elapsed > stale_at`.
    pub stale_at: i64,
}

impl PendingAssignment {
    /// Convenience constructor for a survey-system assignment that grants
    /// `lifetime` hexadies before the sweeper considers it stale.
    pub fn survey_system(faction: Entity, target: Entity, now: i64, lifetime: i64) -> Self {
        Self {
            faction,
            kind: AssignmentKind::Survey,
            target: AssignmentTarget::System(target),
            since: now,
            stale_at: now + lifetime,
        }
    }
}

/// Default lifetime granted to a freshly-issued `survey_system`
/// `PendingAssignment`. Chosen to cover worst-case courier delay
/// (commands traverse light-speed) plus the survey itself
/// (`survey_duration_base = 30` hexadies in the default balance) plus
/// slack for hostile re-routing. If a handler resolves earlier — and it
/// almost always does — the marker is removed cleanly.
pub const SURVEY_ASSIGNMENT_LIFETIME: i64 = 90;

/// Sweep `PendingAssignment` markers whose `stale_at` tick has passed.
/// Bevy's despawn already drops the component for vanished ships; this
/// system handles the "handler never fired but ship still exists" path
/// (e.g. message reader queue overran, ship state mutated externally).
pub fn sweep_stale_assignments(
    mut commands: Commands,
    clock: Res<GameClock>,
    q: Query<(Entity, &PendingAssignment)>,
) {
    let now = clock.elapsed;
    for (entity, pa) in &q {
        if now > pa.stale_at {
            commands.entity(entity).remove::<PendingAssignment>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn survey_system_constructor_sets_lifetime() {
        let faction = Entity::from_raw_u32(1).unwrap();
        let target = Entity::from_raw_u32(2).unwrap();
        let pa = PendingAssignment::survey_system(faction, target, 100, 90);
        assert_eq!(pa.faction, faction);
        assert!(matches!(pa.kind, AssignmentKind::Survey));
        match pa.target {
            AssignmentTarget::System(e) => assert_eq!(e, target),
        }
        assert_eq!(pa.since, 100);
        assert_eq!(pa.stale_at, 190);
    }

    #[test]
    fn sweep_stale_assignments_removes_only_expired() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(GameClock::new(200));
        app.add_systems(Update, sweep_stale_assignments);

        let faction = Entity::from_raw_u32(1).unwrap();
        let target = Entity::from_raw_u32(99).unwrap();

        let fresh = app
            .world_mut()
            .spawn(PendingAssignment::survey_system(faction, target, 150, 90))
            .id();
        let expired = app
            .world_mut()
            .spawn(PendingAssignment::survey_system(faction, target, 50, 90))
            .id();

        app.update();

        // Fresh marker survives (stale_at = 240 > now = 200).
        assert!(app.world().get::<PendingAssignment>(fresh).is_some());
        // Expired marker is swept (stale_at = 140 < now = 200).
        assert!(app.world().get::<PendingAssignment>(expired).is_none());
    }
}
