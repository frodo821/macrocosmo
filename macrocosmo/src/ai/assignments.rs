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
//! 2. [`sweep_resolved_assignments`] — every tick, checks the
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
/// follow-up: Scout, Repair, etc.).
#[derive(Debug, Clone, Copy, Reflect, PartialEq, Eq, Hash)]
pub enum AssignmentKind {
    /// `survey_system` command issued — handler will read the
    /// `SurveyRequested` message and either start the survey or auto-insert
    /// a `MoveTo` first (Deferred).
    Survey,
    /// `colonize_system` command issued — #468 PR-2 added. Drained by
    /// `apply_colonize_to_ship` which emits `ColonizeRequested`. Marker
    /// dedup keyed in `npc_decision.rs::outbox_colonize_per_empire`.
    Colonize,
}

/// Where the assignment is targeted. Reserved for future expansion (deep-
/// space coordinates, etc.).
#[derive(Debug, Clone, Copy, Reflect)]
pub enum AssignmentTarget {
    /// A specific star system.
    System(Entity),
    /// A specific planet — used by `colonize_planet` (#468 PR-3). The
    /// settlement handler resolves the parent system from the planet's
    /// `Planet::system` field; `sweep_resolved_assignments` reads the
    /// planet's own colonization state rather than the system's.
    Planet(Entity),
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

    /// #468 PR-2: convenience constructor for a colonize-system
    /// assignment. Same shape as [`survey_system`] but tags the marker
    /// with `AssignmentKind::Colonize` so the dedup scan in
    /// `npc_decision.rs` can distinguish "this ship is on a survey"
    /// from "this ship is on a colonize" when both lookups need to
    /// avoid double-tasking a candidate ship.
    pub fn colonize_system(faction: Entity, target: Entity, now: i64) -> Self {
        Self {
            faction,
            kind: AssignmentKind::Colonize,
            target: AssignmentTarget::System(target),
            since: now,
        }
    }

    /// #468 PR-3: convenience constructor for a colonize-planet
    /// assignment. Identical to [`colonize_system`] except the
    /// `target` is a planet entity rather than a system —
    /// `sweep_resolved_assignments` resolves the planet's parent
    /// system to read the colonization flag, and the npc_decision
    /// dedup pools `colonize_planet` markers into the same per-empire
    /// set as `colonize_system` (the two kinds are semantically
    /// equivalent for dedup purposes — both say "this empire has
    /// already dispatched a colony attempt for that planet's
    /// system").
    pub fn colonize_planet(faction: Entity, planet: Entity, now: i64) -> Self {
        Self {
            faction,
            kind: AssignmentKind::Colonize,
            target: AssignmentTarget::Planet(planet),
            since: now,
        }
    }
}

/// Knowledge-driven cleanup: remove a `PendingAssignment` once the issuing
/// empire's `KnowledgeStore` reflects the work's completion. This is the
/// AI memory dissolving as the empire *learns* the result, not when the
/// handler fires (which is too eager — the success fact still has to
/// propagate at light speed back to the issuing Ruler). Without this, NPC
/// AI re-emits the same target for the entire propagation window and the
/// surveyor loops every 30 hexadies.
///
/// #468 PR-2: covers both `Survey` (target's `surveyed` flag) and
/// `Colonize` (target's `colonized` flag). The body branches on
/// `AssignmentKind` to pick the relevant resolution predicate; PR-3
/// will extend with further kinds.
///
/// Failure path (ship lost to hostiles before completion) is handled
/// by Bevy's automatic component cleanup on `despawn` for the survey
/// kind, and explicitly by `handle_colonize_requested`'s reject-branch
/// marker removal for the colonize kind (the colonize handler runs
/// after a Ruler→ship courier window so the ship is still alive when
/// rejections happen). A future extension may also drop the marker
/// when `KnowledgeStore` records a `ShipDestroyed` fact for the
/// carrying ship — then the NPC explicitly knows "scout was lost" and
/// can re-emit immediately rather than waiting for the ship entity
/// itself to vanish.
pub fn sweep_resolved_assignments(
    mut commands: Commands,
    assignments: Query<(Entity, &PendingAssignment)>,
    knowledge: Query<&crate::knowledge::KnowledgeStore>,
    planets: Query<&crate::galaxy::Planet>,
) {
    for (ship_entity, pa) in &assignments {
        // #468 PR-3: resolve the system to look up in the issuing
        // empire's KnowledgeStore. For `System` targets it's the
        // target directly; for `Planet` targets (colonize_planet) we
        // follow the planet's parent-system reference, since
        // SystemSnapshot.colonized is keyed per-system not
        // per-planet.
        let system = match pa.target {
            AssignmentTarget::System(t) => t,
            AssignmentTarget::Planet(p) => match planets.get(p) {
                Ok(planet) => planet.system,
                Err(_) => {
                    // #468 PR-3 NICE-TO-FIX #8 fold-in: the planet
                    // despawned (rare — planets are usually durable for
                    // the game's life, but a future "planet destroyed"
                    // event could trigger this). The legacy `continue`
                    // left the marker stamped on the ship forever,
                    // permanently excluding the ship from future
                    // `colonize_*` dispatches. Drop the marker here so
                    // the NPC can re-task the ship to a different
                    // target. Bevy's automatic cleanup on ship despawn
                    // is the only other removal path; without this
                    // explicit drop the ship would have to be
                    // destroyed for the marker to clear.
                    commands.entity(ship_entity).remove::<PendingAssignment>();
                    continue;
                }
            },
        };
        let Ok(store) = knowledge.get(pa.faction) else {
            continue;
        };
        let snapshot = store.get(system);
        // #468 PR-2/PR-3: extended to drop the marker when the kind's
        // target-state has been learned by the issuing empire. For
        // `Survey` that's `surveyed = true`; for `Colonize` that's
        // `colonized = true` (covers both colonize_system AND
        // colonize_planet — both succeed when *any* planet in the
        // system is colonized, which is the granularity of
        // `SystemSnapshot.colonized`).
        let resolved = match pa.kind {
            AssignmentKind::Survey => snapshot.map(|sk| sk.data.surveyed).unwrap_or(false),
            AssignmentKind::Colonize => snapshot.map(|sk| sk.data.colonized).unwrap_or(false),
        };
        if resolved {
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
            AssignmentTarget::Planet(_) => panic!("expected System target"),
        }
        assert_eq!(pa.since, 100);
    }

    #[test]
    fn colonize_system_constructor_sets_fields() {
        let faction = Entity::from_raw_u32(3).unwrap();
        let target = Entity::from_raw_u32(4).unwrap();
        let pa = PendingAssignment::colonize_system(faction, target, 200);
        assert_eq!(pa.faction, faction);
        assert!(matches!(pa.kind, AssignmentKind::Colonize));
        match pa.target {
            AssignmentTarget::System(e) => assert_eq!(e, target),
            AssignmentTarget::Planet(_) => panic!("expected System target"),
        }
        assert_eq!(pa.since, 200);
    }

    /// #468 PR-3: planet-target colonize marker carries the planet entity
    /// (not the system) so the sweeper can resolve the parent system via
    /// the `Planet` component when reading the empire's KnowledgeStore.
    #[test]
    fn colonize_planet_constructor_sets_planet_target() {
        let faction = Entity::from_raw_u32(5).unwrap();
        let planet = Entity::from_raw_u32(6).unwrap();
        let pa = PendingAssignment::colonize_planet(faction, planet, 300);
        assert_eq!(pa.faction, faction);
        // Kind is the same Colonize variant — dedup pools `colonize_planet`
        // with `colonize_system` per #468 PR-3 (both mean "this empire
        // already dispatched a colony attempt for that system").
        assert!(matches!(pa.kind, AssignmentKind::Colonize));
        match pa.target {
            AssignmentTarget::Planet(e) => assert_eq!(e, planet),
            AssignmentTarget::System(_) => panic!("expected Planet target"),
        }
        assert_eq!(pa.since, 300);
    }
}
