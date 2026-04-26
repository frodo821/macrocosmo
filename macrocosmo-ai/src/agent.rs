//! Three-layer agent traits — long-term, mid-term, short-term.
//!
//! See `docs/ai-three-layer.md` for the full design.
//!
//! Each trait is a pure transform `Input -> Output`. The trait
//! impls themselves are stateful (hold `&mut self`), but each tick
//! call is independent — the orchestrator drives cadence and
//! threads state between ticks.
//!
//! The traits are deliberately narrow: enough contract to wire the
//! orchestrator, but no more. Concrete default implementations live
//! in sibling modules (`long_term_default`, `mid_term_default`,
//! `short_term_default`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ai_params::AiParamsExt;
use crate::bus::AiBus;
use crate::campaign::{Campaign, CampaignState};
use crate::command::Command;
use crate::ids::{
    CommandKindId, FactionId, IntentId, IntentKindId, IntentTargetRef, ObjectiveId, ShortContext,
};
use crate::intent::{Intent, IntentSpec};
use crate::time::Tick;
use crate::victory::{VictoryCondition, VictoryStatus};

// ---------------------------------------------------------------------------
// Long-term
// ---------------------------------------------------------------------------

/// Strategic layer. Reads the bus + victory condition, emits
/// `IntentSpec`s targeting mid-term agents.
pub trait LongTermAgent: Send + Sync {
    fn tick(&mut self, input: LongTermInput<'_>) -> LongTermOutput;
}

pub struct LongTermInput<'a> {
    pub bus: &'a AiBus,
    pub faction: FactionId,
    pub victory: &'a VictoryCondition,
    pub victory_status: VictoryStatus,
    pub active_campaigns: &'a [&'a Campaign],
    pub now: Tick,
    pub params: Option<&'a (dyn AiParamsExt + 'a)>,
    /// Drops since last long tick. Lets the long agent see futile
    /// intent emissions and adapt — extend `expires_at_offset`,
    /// fall back to a different pursuit, or surrender that leaf.
    /// Empty by default (orchestrator passes new entries since the
    /// previous long tick).
    pub recent_drops: &'a [crate::orchestrator::DropEntry],
}

#[derive(Debug, Default)]
pub struct LongTermOutput {
    /// Unrouted intents. The orchestrator feeds each to its
    /// `IntentDispatcher` and, on success, appends the materialized
    /// `Intent` to the in-transit queue.
    pub intents: Vec<IntentSpec>,
}

// ---------------------------------------------------------------------------
// Mid-term
// ---------------------------------------------------------------------------

/// Planning layer. Consumes arrived intents, drives the campaign
/// state machine, logs overrides.
pub trait MidTermAgent: Send + Sync {
    fn tick(&mut self, input: MidTermInput<'_>) -> MidTermOutput;
}

pub struct MidTermInput<'a> {
    pub bus: &'a AiBus,
    pub faction: FactionId,
    /// Intents whose `arrives_at` has been reached. Ordered oldest
    /// first, but mid-agents typically resort by `effective_priority`.
    pub inbox: &'a [Intent],
    pub campaigns: &'a [Campaign],
    pub now: Tick,
    pub params: Option<&'a (dyn AiParamsExt + 'a)>,
    /// The faction's victory condition. Lets the mid-agent inspect
    /// prerequisite metrics directly to decide whether to throttle
    /// in-flight pursuits when prereqs approach violation. Game
    /// integrations may pass the same `VictoryCondition` value the
    /// long-term agent saw on the same tick.
    pub victory: &'a VictoryCondition,
    /// Pre-computed victory status for this tick. Mirrors what the
    /// long-term agent received — gives the mid-agent the signal it
    /// needs to abandon active campaigns when the path to victory is
    /// closed (Unreachable / TimedOut) or to mark them Succeeded
    /// when the goal is met (Won). Game integrations should pass the
    /// same value used by the long-term agent on this tick.
    pub victory_status: VictoryStatus,
}

#[derive(Debug, Default)]
pub struct MidTermOutput {
    pub campaign_ops: Vec<CampaignOp>,
    pub override_log: Vec<OverrideEntry>,
}

/// One state-machine transition the mid-term agent wants to apply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CampaignOp {
    Start {
        objective_id: ObjectiveId,
        source_intent: Option<IntentId>,
        at: Tick,
    },
    Transition {
        campaign_id: ObjectiveId,
        to: CampaignState,
        at: Tick,
    },
    AttachIntent {
        campaign_id: ObjectiveId,
        intent_id: IntentId,
    },
    /// Update the priority weight on an existing campaign. Used by
    /// short-term agents that allocate budget proportional to
    /// campaign weights.
    SetWeight {
        campaign_id: ObjectiveId,
        weight: f64,
    },
}

/// Recorded reason for not honoring an inbox intent (debug + tuning).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverrideEntry {
    pub intent_id: IntentId,
    pub intent_kind: IntentKindId,
    pub reason: OverrideReason,
    pub at: Tick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverrideReason {
    /// `effective_priority` below threshold.
    StaleIntent,
    /// Local observation contradicts the rationale snapshot.
    ConflictsWithLocalObservation,
    /// A newer intent superseded this one.
    Superseded,
    /// Hard expiry passed.
    Expired,
}

// ---------------------------------------------------------------------------
// Short-term
// ---------------------------------------------------------------------------

/// Per-`ShortContext` persistent execution state for the short-term agent.
///
/// `PlanState` is a small store of *not-yet-issued* primitive commands
/// produced by decomposing a higher-level (macro) command into a sequence
/// of executable steps. Today it is a deterministic `BTreeMap` keyed by
/// `(macro_kind, ObjectiveId)` whose values are the queued primitives
/// waiting to be drained. Decomposition logic itself lands in F2+ — F1
/// only introduces the type and its plumbing through `ShortTermInput`.
///
/// The default impl in `short_term_default` discards `plan_state`
/// entirely, so the field is purely additive and does not change
/// behavior for existing agents.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanState {
    /// Queued primitive commands per `(macro_kind, objective)` slot.
    /// `BTreeMap` is chosen over `AHashMap` so `serde` round-trips and
    /// `Debug` snapshots are deterministic — record/replay scenarios
    /// rely on stable iteration order.
    pub pending: BTreeMap<(CommandKindId, ObjectiveId), Vec<Command>>,
}

impl PlanState {
    /// Construct an empty `PlanState` (same as `Default::default()`).
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` if no slot has any queued primitive commands.
    pub fn is_empty(&self) -> bool {
        self.pending.values().all(|v| v.is_empty())
    }

    /// Number of queued primitives across all slots.
    pub fn total_len(&self) -> usize {
        self.pending.values().map(|v| v.len()).sum()
    }
}

/// Execution layer. Emits commands reacting to active campaigns.
///
/// A single `faction` may host several short-term agents simultaneously
/// (e.g. one `FleetShort` per fleet and one `ColonyShort` per colony).
/// The `context` field distinguishes them.
pub trait ShortTermAgent: Send + Sync {
    fn tick(&mut self, input: ShortTermInput<'_>) -> ShortTermOutput;
}

pub struct ShortTermInput<'a> {
    pub bus: &'a AiBus,
    pub faction: FactionId,
    /// Open-kind instance label: `"fleet:42"`, `"colony:sol"`,
    /// `"faction"`, etc. The short agent interprets as needed.
    pub context: ShortContext,
    pub active_campaigns: &'a [&'a Campaign],
    pub now: Tick,
    /// Per-`ShortContext` persistent plan state. The orchestrator owns
    /// one `PlanState` per `ShortContext` and threads a mutable
    /// borrow in here every short tick so the agent can drain or
    /// extend its queued primitive commands. Default agents that do
    /// not decompose commands simply ignore the field.
    pub plan_state: &'a mut PlanState,
    /// Optional decomposition registry. When present, decomposition-
    /// aware short agents can `lookup` a `DecompositionRule` for a
    /// macro `CommandKindId` and expand it into primitive commands.
    /// `None` (the default) preserves legacy no-decomposition behavior;
    /// agents that do not implement decomposition simply ignore it.
    pub decomp: Option<&'a dyn crate::decomposition::DecompositionRegistry>,
}

#[derive(Debug, Default)]
pub struct ShortTermOutput {
    pub commands: Vec<Command>,
}

// ---------------------------------------------------------------------------
// Routing target helpers
// ---------------------------------------------------------------------------

/// Convenience: the conventional `IntentTargetRef` for a faction-wide
/// (not-address-specific) intent. Abstract scenarios default to this.
pub fn target_faction_wide() -> IntentTargetRef {
    IntentTargetRef::from("faction")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn campaign_op_start_preserves_fields() {
        let op = CampaignOp::Start {
            objective_id: ObjectiveId::from("expand"),
            source_intent: Some(IntentId::from("intent_1")),
            at: 42,
        };
        if let CampaignOp::Start {
            objective_id,
            source_intent,
            at,
        } = &op
        {
            assert_eq!(objective_id.as_str(), "expand");
            assert_eq!(source_intent.as_ref().unwrap().as_str(), "intent_1");
            assert_eq!(*at, 42);
        } else {
            panic!("expected Start");
        }
    }

    #[test]
    fn override_reason_distinct() {
        assert_ne!(OverrideReason::StaleIntent, OverrideReason::Expired);
    }

    #[test]
    fn target_faction_wide_is_canonical_string() {
        assert_eq!(target_faction_wide().as_str(), "faction");
    }

    #[test]
    fn plan_state_default_is_empty() {
        let ps = PlanState::default();
        assert!(ps.pending.is_empty());
        assert!(ps.is_empty());
        assert_eq!(ps.total_len(), 0);
    }

    #[test]
    fn plan_state_tracks_pending_primitives() {
        let mut ps = PlanState::new();
        let key = (
            CommandKindId::from("colonize_system"),
            ObjectiveId::from("expand"),
        );
        ps.pending
            .entry(key.clone())
            .or_default()
            .push(Command::new(
                CommandKindId::from("build_deliverable"),
                FactionId(0),
                0,
            ));
        assert!(!ps.is_empty());
        assert_eq!(ps.total_len(), 1);
        assert_eq!(ps.pending.get(&key).unwrap().len(), 1);
    }
}
