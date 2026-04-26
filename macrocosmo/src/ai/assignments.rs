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
//! The marker is the NPC's **decision memory** — it lives from the moment
//! `handle_survey_system` dispatches the command until the issuing empire
//! actually *knows* the assignment is resolved. Removal paths:
//!
//! 1. `handle_survey_requested` Rejected (start_survey failed at handler
//!    time, e.g. ship gone) — immediate remove (emit failed; AI is free
//!    to re-emit next tick).
//! 2. [`sweep_resolved_survey_assignments`] — every tick, checks the
//!    issuing empire's `KnowledgeStore`. If the target system is now
//!    `surveyed = true` from that empire's perspective (success arrived
//!    via fact propagation) → remove.
//! 3. Bevy's automatic component cleanup — when the carrying ship is
//!    despawned (combat loss, scuttling, etc.) the marker goes with it.
//!
//! There is intentionally **no** time-based fallback sweeper: the previous
//! `SURVEY_ASSIGNMENT_LIFETIME = 200` was shorter than a realistic sublight
//! survey round-trip (~1700 hex observed) and would prematurely strip the
//! marker mid-flight, re-opening the double-dispatch race the marker exists
//! to prevent. The "ship is alive but never returns" corner case is
//! deferred until `KnowledgeFact::ShipDestroyed` gains per-faction
//! propagation, at which point a knowledge-driven sweep can clear the
//! marker the moment the issuer learns the ship is lost.
//!
//! The marker is **per faction** — when observer mode runs multiple
//! factions, each faction only sees its own pending dispatches.

use bevy::prelude::*;

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
/// resolves the request (Ok / Rejected) or via knowledge-driven cleanup
/// once the issuing empire learns the result.
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
}

impl PendingAssignment {
    /// Convenience constructor for a survey-system assignment.
    pub fn survey_system(faction: Entity, target: Entity, now: i64) -> Self {
        Self {
            faction,
            kind: AssignmentKind::Survey,
            target: AssignmentTarget::System(target),
            since: now,
        }
    }
}

/// Knowledge-driven cleanup: remove a `PendingAssignment` once the issuing
/// empire's `KnowledgeStore` reflects the survey completion. This is the
/// AI memory dissolving as the empire *learns* the result, not when the
/// handler fires (which is too eager — the success fact still has to
/// propagate at light speed back to the issuing Ruler). Without this, NPC
/// AI re-emits the same target for the entire propagation window and the
/// surveyor loops every 30 hexadies.
///
/// Failure path (ship lost to hostiles before completion) is currently
/// handled by Bevy's automatic component cleanup on `despawn`. A future
/// extension may also drop the marker when `KnowledgeStore` records a
/// `ShipDestroyed` fact for the carrying ship — then the NPC explicitly
/// knows "scout was lost" and can re-emit immediately rather than waiting
/// for the ship entity itself to vanish.
pub fn sweep_resolved_survey_assignments(
    mut commands: Commands,
    assignments: Query<(Entity, &PendingAssignment)>,
    knowledge: Query<&crate::knowledge::KnowledgeStore>,
) {
    for (ship_entity, pa) in &assignments {
        if !matches!(pa.kind, AssignmentKind::Survey) {
            continue;
        }
        let target = match pa.target {
            AssignmentTarget::System(t) => t,
        };
        let Ok(store) = knowledge.get(pa.faction) else {
            continue;
        };
        if store
            .get(target)
            .map(|sk| sk.data.surveyed)
            .unwrap_or(false)
        {
            commands.entity(ship_entity).remove::<PendingAssignment>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn survey_system_constructor_sets_fields() {
        let faction = Entity::from_raw_u32(1).unwrap();
        let target = Entity::from_raw_u32(2).unwrap();
        let pa = PendingAssignment::survey_system(faction, target, 100);
        assert_eq!(pa.faction, faction);
        assert!(matches!(pa.kind, AssignmentKind::Survey));
        match pa.target {
            AssignmentTarget::System(e) => assert_eq!(e, target),
        }
        assert_eq!(pa.since, 100);
    }
}
