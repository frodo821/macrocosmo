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

use serde::{Deserialize, Serialize};

use crate::ai_params::AiParamsExt;
use crate::bus::AiBus;
use crate::campaign::{Campaign, CampaignState};
use crate::command::Command;
use crate::ids::{FactionId, IntentId, IntentKindId, IntentTargetRef, ObjectiveId, ShortContext};
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
}
