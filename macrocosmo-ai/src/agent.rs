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
    CampaignPhase, CommandKindId, FactionId, IntentId, IntentKindId, IntentTargetRef, MetricId,
    ObjectiveId, RegionId, ShortContext, StanceId, VictoryAxisId,
};
use crate::intent::{Intent, IntentSpec};
use crate::time::Tick;
use crate::victory::{VictoryCondition, VictoryStatus};

// ---------------------------------------------------------------------------
// Long-term
// ---------------------------------------------------------------------------

/// Empire-wide strategic memory threaded across long ticks.
///
/// Today the default long agent (`ObjectiveDrivenLongTerm`) holds its
/// own internal state (`drop_counts`, `last_id_by_key`); this struct is
/// the *layer-level* memory the orchestrator owns and will hand to the
/// layer through the unified `Agent` trait once #448 PR4 lands. Until
/// then it is purely additive — no existing agent reads from it.
///
/// The fields are scaffolding for #449+: their concrete schemas will
/// be filled in as long-term re-evaluation, campaign-phase tracking,
/// and victory-axis decomposition land in subsequent rounds.
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LongTermState {
    /// Metrics the long agent is currently pursuing, keyed by the
    /// concrete `IntentId` it minted to chase that metric. Seeds the
    /// next long-tick re-evaluation: a metric already present here is
    /// "in flight" and should not be re-emitted as a fresh pursuit.
    pub pursued_metrics: BTreeMap<MetricId, IntentId>,
    /// Optional empire-level campaign phase — schema TBD in #449. Set
    /// to `None` today.
    pub current_campaign_phase: Option<CampaignPhase>,
    /// Per-axis progress toward victory in `[0.0, 1.0]`. Lets the long
    /// agent prioritize axes that are stalled. Schema TBD in #449.
    pub victory_progress: BTreeMap<VictoryAxisId, f32>,
}

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

/// Region-scoped tactical memory threaded across mid ticks.
///
/// Mid-term agents today operate empire-wide (one Mid per faction);
/// `region_id` is reserved for the multi-Mid split in #449 and is
/// always `None` until then. Like `LongTermState`, this struct is
/// purely additive in PR1 — the orchestrator will start handing it to
/// the layer once the unified `Agent` trait lands in PR4.
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MidTermState {
    /// Current stance. Drives downstream priority weighting and
    /// proposal filtering once #467 phase 1 lands.
    pub stance: Stance,
    /// Active operations the mid agent is currently driving. PR2
    /// will split this into pending / committed sets backed by the
    /// new Proposal / ProposalOutcome types (#467 phase 1); a single
    /// `Vec` is sufficient for PR1's state-only scope.
    pub active_operations: Vec<ObjectiveId>,
    /// Region this Mid agent is bound to. Reserved for the multi-Mid
    /// split in #449; always `None` while every faction runs a single
    /// empire-wide Mid agent.
    pub region_id: Option<RegionId>,
}

/// Region-level posture the Mid agent operates under.
///
/// Four core variants cover the common case so the orchestrator and
/// tests can `match` exhaustively. `Custom(StanceId)` is the
/// extension hook: Lua / scenario layers can register richer stances
/// without touching the enum (mirrors the open-id pattern used
/// throughout `ids.rs`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stance {
    Expanding,
    Consolidating,
    Defending,
    Withdrawing,
    Custom(StanceId),
}

impl Default for Stance {
    /// `Consolidating` matches empire startup — early game leans on
    /// build-up before expansion or defense becomes appropriate.
    fn default() -> Self {
        Self::Consolidating
    }
}

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

    #[test]
    fn long_term_state_default_is_empty() {
        let s = LongTermState::default();
        assert!(s.pursued_metrics.is_empty());
        assert!(s.current_campaign_phase.is_none());
        assert!(s.victory_progress.is_empty());
    }

    #[test]
    fn mid_term_state_defaults_to_consolidating_with_no_region() {
        let s = MidTermState::default();
        assert_eq!(s.stance, Stance::Consolidating);
        assert!(s.active_operations.is_empty());
        assert!(s.region_id.is_none());
    }

    #[test]
    fn stance_custom_round_trips_through_serde() {
        let s = Stance::Custom(StanceId::from("aggressive_raid"));
        let json = serde_json::to_string(&s).expect("serialize Custom");
        let back: Stance = serde_json::from_str(&json).expect("deserialize Custom");
        assert_eq!(s, back);
        // Sanity: the four core variants also round-trip.
        for v in [
            Stance::Expanding,
            Stance::Consolidating,
            Stance::Defending,
            Stance::Withdrawing,
        ] {
            let j = serde_json::to_string(&v).expect("serialize core");
            let r: Stance = serde_json::from_str(&j).expect("deserialize core");
            assert_eq!(v, r);
        }
    }
}
