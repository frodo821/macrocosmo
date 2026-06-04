//! Commitment ledger — the unified semantic home for "this subject has
//! already decided to do X".
//!
//! Slice 2 of the knowledge redesign
//! (`docs/knowledge-redesign-implementation-plan.md`).
//!
//! Today the same intention is represented by four overlapping markers:
//!
//! * `ai::assignments::PendingAssignment` — durable on the ship.
//! * `ai::command_consumer::PendingAiShipCommand` — in-flight Ruler→ship
//!   command holder.
//! * `ai::command_outbox::AiCommandOutbox` — pending Ruler→target queue
//!   (still used for non-ship-control kinds).
//! * `knowledge::ShipProjection.intended_*` — dispatcher-side belief.
//!
//! The ledger is the place where AI dedup and "what have I already
//! decided" reads should live. Slice 2 only adds it as an additive
//! dual-write companion so the existing markers keep their semantics
//! while later slices move the AI dedup union (Slice 4) and the
//! observation-driven resolution (Slice 3) over to it.

use bevy::prelude::*;
use std::collections::HashMap;

use super::subject::KnowledgeSubject;
use crate::ai::assignments::{AssignmentKind, AssignmentTarget};

/// Monotonic ledger-local identifier for a commitment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Reflect)]
pub struct CommitmentId(pub u64);

/// What kind of decision is being committed to.
///
/// Slice 2 covers the four kinds the AI dedup path cares about today.
/// `DeployDeliverable` is written by the macro-dispatch path for Slice 4b2;
/// `Move` remains a placeholder for a later outbox migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Reflect)]
pub enum CommitmentKind {
    Survey,
    Colonize,
    Move,
    DeployDeliverable,
}

impl From<AssignmentKind> for CommitmentKind {
    fn from(k: AssignmentKind) -> Self {
        match k {
            AssignmentKind::Survey => CommitmentKind::Survey,
            AssignmentKind::Colonize => CommitmentKind::Colonize,
        }
    }
}

/// What the commitment is targeted at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Reflect)]
pub enum CommitmentTarget {
    System(Entity),
    Planet(Entity),
}

impl From<AssignmentTarget> for CommitmentTarget {
    fn from(t: AssignmentTarget) -> Self {
        match t {
            AssignmentTarget::System(e) => CommitmentTarget::System(e),
            AssignmentTarget::Planet(e) => CommitmentTarget::Planet(e),
        }
    }
}

/// Lifecycle state of a commitment.
///
/// Active = decision issued, no terminal observation yet.
/// Resolved = observation arrived that satisfies the commitment.
/// Failed = observation arrived that contradicts the commitment
/// (ship lost, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Reflect)]
pub enum CommitmentStatus {
    Active,
    Resolved,
    Failed,
}

/// One commitment entry in the ledger.
#[derive(Debug, Clone, Reflect)]
pub struct Commitment {
    pub id: CommitmentId,
    /// The subject that issued the commitment (today: always the empire).
    pub subject: KnowledgeSubject,
    /// The acting entity, e.g. the ship carrying out a survey. `None`
    /// for commitments with no specific actor (e.g. empire-scope
    /// research, future use).
    pub actor: Option<Entity>,
    pub kind: CommitmentKind,
    pub target: CommitmentTarget,
    pub issued_at: i64,
    pub status: CommitmentStatus,
    /// Slice 1.5 review F3 — the `observed_at` of the belief this
    /// commitment was based on. Today this is the snapshot's
    /// `observed_at` for the actor ship; later `BeliefStamp` can take
    /// over. `None` when the dispatch path had no prior observation
    /// to anchor against (= freshly-spawned ship with no snapshot yet).
    pub basis_observed_at: Option<i64>,
    /// Tick at which the issuing subject expects the command to take
    /// effect (the actor's projected `intended_takes_effect_at`).
    /// `None` for kinds with no spatial effect.
    pub expected_effect_at: Option<i64>,
    /// Tick at which the issuing subject expects to *learn* the
    /// outcome (= projection's `expected_return_at` when the kind has
    /// a return leg, else `expected_arrival_at`). `None` when no
    /// resolution ETA is available.
    pub expected_resolution_at: Option<i64>,
}

/// Per-subject ledger of issued commitments.
///
/// Currently stored as a sub-field of [`super::KnowledgeStore`] keyed by
/// the empire entity. Slice 9 will move it to its own module file but
/// the storage stays unchanged.
#[derive(Debug, Clone, Default, Reflect)]
pub struct CommitmentLedger {
    next_id: u64,
    entries: HashMap<CommitmentId, Commitment>,
    /// (subject, kind, target) -> list of commitment ids carrying that
    /// triple. Used by the AI dedup path in Slice 4 so it can answer
    /// "is there an active commitment for this work?" in O(1).
    by_subject_kind_target:
        HashMap<(KnowledgeSubject, CommitmentKind, CommitmentTarget), Vec<CommitmentId>>,
}

/// Slice 1.5 review F3 — bundle of optional basis / ETA fields
/// captured at dispatch time. Kept in its own struct so call sites can
/// build it from whatever projection / snapshot data they already have
/// without bloating `record_issued`'s positional signature.
#[derive(Debug, Default, Clone, Copy)]
pub struct CommitmentBasis {
    pub basis_observed_at: Option<i64>,
    pub expected_effect_at: Option<i64>,
    pub expected_resolution_at: Option<i64>,
}

impl CommitmentLedger {
    /// Record a freshly issued commitment.
    ///
    /// Slice 2 dual-writes from the AI dispatch path — see
    /// `macrocosmo/src/ai/command_outbox.rs` for the call sites.
    pub fn record_issued(
        &mut self,
        subject: KnowledgeSubject,
        actor: Option<Entity>,
        kind: CommitmentKind,
        target: CommitmentTarget,
        issued_at: i64,
    ) -> CommitmentId {
        self.record_issued_with_basis(
            subject,
            actor,
            kind,
            target,
            issued_at,
            CommitmentBasis::default(),
        )
    }

    /// Slice 1.5 review F3 — record a commitment with basis / ETA
    /// fields. Existing callers that don't have the data yet can keep
    /// calling [`record_issued`]; the new fields default to `None`.
    pub fn record_issued_with_basis(
        &mut self,
        subject: KnowledgeSubject,
        actor: Option<Entity>,
        kind: CommitmentKind,
        target: CommitmentTarget,
        issued_at: i64,
        basis: CommitmentBasis,
    ) -> CommitmentId {
        let id = CommitmentId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        let entry = Commitment {
            id,
            subject,
            actor,
            kind,
            target,
            issued_at,
            status: CommitmentStatus::Active,
            basis_observed_at: basis.basis_observed_at,
            expected_effect_at: basis.expected_effect_at,
            expected_resolution_at: basis.expected_resolution_at,
        };
        self.entries.insert(id, entry);
        self.by_subject_kind_target
            .entry((subject, kind, target))
            .or_default()
            .push(id);
        id
    }

    /// Mark a commitment as resolved (the target outcome is now observed).
    ///
    /// Returns `true` if a matching commitment was found and transitioned
    /// to [`CommitmentStatus::Resolved`].
    pub fn mark_resolved(&mut self, id: CommitmentId) -> bool {
        if let Some(c) = self.entries.get_mut(&id) {
            if c.status == CommitmentStatus::Active {
                c.status = CommitmentStatus::Resolved;
                return true;
            }
        }
        false
    }

    /// Mark a commitment as failed (the actor is lost, etc.).
    pub fn mark_failed(&mut self, id: CommitmentId) -> bool {
        if let Some(c) = self.entries.get_mut(&id) {
            if c.status == CommitmentStatus::Active {
                c.status = CommitmentStatus::Failed;
                return true;
            }
        }
        false
    }

    /// Is there an *active* commitment matching (subject, kind, target)?
    ///
    /// This is the dedup query that Slice 4 will use to replace the
    /// manual outbox + pending-assignment union in `npc_decision_tick`.
    pub fn has_active(
        &self,
        subject: KnowledgeSubject,
        kind: CommitmentKind,
        target: CommitmentTarget,
    ) -> bool {
        self.by_subject_kind_target
            .get(&(subject, kind, target))
            .map(|ids| {
                ids.iter().any(|id| {
                    self.entries
                        .get(id)
                        .map(|c| c.status == CommitmentStatus::Active)
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    }

    /// All active commitments where `subject` issued the commitment AND
    /// `actor` is the executing entity (e.g. the ship carrying out a
    /// survey).
    ///
    /// The `subject` filter is intentional: once knowledge merge
    /// (Slice 7) lets a single ledger hold entries for multiple
    /// subjects, callers must specify whose commitments they want.
    /// Today each ledger holds exactly one subject so the filter is a
    /// no-op, but pinning the API shape now avoids a silent
    /// behavior change when merge lands.
    pub fn active_for_actor(
        &self,
        subject: KnowledgeSubject,
        actor: Entity,
    ) -> impl Iterator<Item = &Commitment> {
        self.entries.values().filter(move |c| {
            c.subject == subject && c.status == CommitmentStatus::Active && c.actor == Some(actor)
        })
    }

    /// All commitments matching (subject, kind, target), regardless of
    /// status. Used by Slice 3's resolution system to flip the right
    /// entry when an observation lands.
    pub fn ids_for(
        &self,
        subject: KnowledgeSubject,
        kind: CommitmentKind,
        target: CommitmentTarget,
    ) -> &[CommitmentId] {
        self.by_subject_kind_target
            .get(&(subject, kind, target))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Read-only access to a single commitment.
    pub fn get(&self, id: CommitmentId) -> Option<&Commitment> {
        self.entries.get(&id)
    }

    /// Total entries (active + resolved + failed). Mostly for tests.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate all entries. Mostly for tests / debug.
    pub fn iter(&self) -> impl Iterator<Item = &Commitment> {
        self.entries.values()
    }

    /// Next ledger-local id to allocate. Used by save/load so post-load
    /// commitments do not reuse an existing persisted id.
    pub fn next_id_for_persistence(&self) -> u64 {
        self.next_id
    }

    /// Rebuild a ledger from persisted entries, including the secondary
    /// `(subject, kind, target)` index used by dedup queries.
    pub fn from_persisted(next_id: u64, entries: impl IntoIterator<Item = Commitment>) -> Self {
        let mut ledger = Self {
            next_id,
            entries: HashMap::new(),
            by_subject_kind_target: HashMap::new(),
        };
        let mut max_seen_next = next_id;
        for entry in entries {
            if entry.id.0 < u64::MAX {
                max_seen_next = max_seen_next.max(entry.id.0 + 1);
            } else {
                max_seen_next = u64::MAX;
            }
            ledger
                .by_subject_kind_target
                .entry((entry.subject, entry.kind, entry.target))
                .or_default()
                .push(entry.id);
            ledger.entries.insert(entry.id, entry);
        }
        ledger.next_id = max_seen_next;
        ledger
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(i: u32) -> Entity {
        Entity::from_raw_u32(i).expect("nonzero")
    }

    fn empire_subject(i: u32) -> KnowledgeSubject {
        KnowledgeSubject::Empire(e(i))
    }

    #[test]
    fn record_and_query_active() {
        let mut led = CommitmentLedger::default();
        let id = led.record_issued(
            empire_subject(1),
            Some(e(2)),
            CommitmentKind::Survey,
            CommitmentTarget::System(e(3)),
            10,
        );
        assert!(led.has_active(
            empire_subject(1),
            CommitmentKind::Survey,
            CommitmentTarget::System(e(3)),
        ));
        assert!(!led.has_active(
            empire_subject(99),
            CommitmentKind::Survey,
            CommitmentTarget::System(e(3)),
        ));
        let c = led.get(id).expect("present");
        assert_eq!(c.status, CommitmentStatus::Active);
    }

    #[test]
    fn resolve_flips_status_and_clears_has_active() {
        let mut led = CommitmentLedger::default();
        let id = led.record_issued(
            empire_subject(1),
            Some(e(2)),
            CommitmentKind::Survey,
            CommitmentTarget::System(e(3)),
            10,
        );
        assert!(led.mark_resolved(id));
        assert!(!led.has_active(
            empire_subject(1),
            CommitmentKind::Survey,
            CommitmentTarget::System(e(3)),
        ));
        assert_eq!(led.get(id).unwrap().status, CommitmentStatus::Resolved);
    }

    #[test]
    fn fail_flips_status_and_clears_has_active() {
        let mut led = CommitmentLedger::default();
        let id = led.record_issued(
            empire_subject(1),
            Some(e(2)),
            CommitmentKind::Survey,
            CommitmentTarget::System(e(3)),
            10,
        );
        assert!(led.mark_failed(id));
        assert!(!led.has_active(
            empire_subject(1),
            CommitmentKind::Survey,
            CommitmentTarget::System(e(3)),
        ));
        assert_eq!(led.get(id).unwrap().status, CommitmentStatus::Failed);
    }

    #[test]
    fn active_for_actor_filters_by_actor_and_status() {
        let mut led = CommitmentLedger::default();
        let _a = led.record_issued(
            empire_subject(1),
            Some(e(10)),
            CommitmentKind::Survey,
            CommitmentTarget::System(e(20)),
            1,
        );
        let b = led.record_issued(
            empire_subject(1),
            Some(e(10)),
            CommitmentKind::Survey,
            CommitmentTarget::System(e(21)),
            2,
        );
        let _c = led.record_issued(
            empire_subject(1),
            Some(e(11)),
            CommitmentKind::Survey,
            CommitmentTarget::System(e(22)),
            3,
        );
        led.mark_resolved(b);
        let actor_10: Vec<_> = led.active_for_actor(empire_subject(1), e(10)).collect();
        assert_eq!(actor_10.len(), 1);
        let actor_11: Vec<_> = led.active_for_actor(empire_subject(1), e(11)).collect();
        assert_eq!(actor_11.len(), 1);
        // Cross-subject scope check: searching by a different subject
        // returns zero matches even when the actor entity is the same.
        let other_subject: Vec<_> = led.active_for_actor(empire_subject(99), e(10)).collect();
        assert!(
            other_subject.is_empty(),
            "active_for_actor must respect the subject filter"
        );
    }

    #[test]
    fn assignment_kind_target_conversion() {
        assert_eq!(
            CommitmentKind::from(AssignmentKind::Survey),
            CommitmentKind::Survey
        );
        assert_eq!(
            CommitmentKind::from(AssignmentKind::Colonize),
            CommitmentKind::Colonize
        );
        assert_eq!(
            CommitmentTarget::from(AssignmentTarget::System(e(5))),
            CommitmentTarget::System(e(5))
        );
        assert_eq!(
            CommitmentTarget::from(AssignmentTarget::Planet(e(6))),
            CommitmentTarget::Planet(e(6))
        );
    }
}
