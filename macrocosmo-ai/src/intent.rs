//! Intent — inter-layer communication packet.
//!
//! See `docs/ai-three-layer.md` for the full design.
//!
//! Long-term agents emit [`IntentSpec`] (未ルーティング). The
//! orchestrator routes each `IntentSpec` through an `IntentDispatcher`
//! (see [`crate::dispatcher`]) which returns a materialized
//! [`Intent`] with delivery delay stamped in. Mid-term agents consume
//! fully-materialized [`Intent`] values from an inbox.
//!
//! The split exists so that the long-term agent, living in the
//! engine-agnostic `macrocosmo-ai` layer, never needs to know
//! positions, courier availability, or relay topology — all of that
//! is the dispatcher's responsibility.

use std::sync::Arc;

use ahash::AHashMap;
use serde::{Deserialize, Serialize};

use crate::ids::{DeliveryHintId, IntentId, IntentKindId, IntentTargetRef, MetricId, ObjectiveId};
use crate::time::Tick;
use crate::value_expr::ValueExpr;

/// Params bag attached to an intent. Open-kind: game / scenario
/// defines what keys it uses; `macrocosmo-ai` passes through without
/// interpretation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IntentParams(pub AHashMap<Arc<str>, ValueExpr>);

impl IntentParams {
    pub fn new() -> Self {
        Self(AHashMap::new())
    }

    pub fn with(mut self, key: impl Into<Arc<str>>, value: ValueExpr) -> Self {
        self.0.insert(key.into(), value);
        self
    }

    pub fn get(&self, key: &str) -> Option<&ValueExpr> {
        self.0.get(key)
    }
}

/// Snapshot of the agent's rationale at the moment an intent was
/// emitted — used by mid-term agents to judge whether to honor vs
/// override when local observations diverge from what the long-term
/// agent saw.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RationaleSnapshot {
    /// Metric values the emitter observed, keyed by metric id.
    pub metrics_seen: AHashMap<MetricId, f64>,
    /// Optional objective the intent was emitted to pursue.
    pub objective_id: Option<ObjectiveId>,
    /// Free-form human-readable note (debugging / override logs).
    pub note: Arc<str>,
}

impl RationaleSnapshot {
    pub fn empty() -> Self {
        Self {
            metrics_seen: AHashMap::new(),
            objective_id: None,
            note: Arc::from(""),
        }
    }
}

/// Intent emitted by a long-term agent, not yet routed.
///
/// `id` / `issued_at` / `arrives_at` / `expires_at` are not present —
/// they are determined by the orchestrator + dispatcher when the
/// spec is committed. See [`Intent`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntentSpec {
    /// Open-kind kind id (`"pursue_metric"`, `"fortify"`, ...).
    pub kind: IntentKindId,
    /// Open-kind params bag.
    pub params: IntentParams,
    /// Time-discounted urgency [0.0, 1.0].
    pub priority: f32,
    /// Undiscounted strategic importance [0.0, 1.0].
    pub importance: f32,
    /// Half-life (in ticks) controlling `effective_priority` decay.
    /// `None` means no decay.
    pub half_life: Option<Tick>,
    /// Hard expiry as a **relative offset from `issued_at`**.
    /// The orchestrator converts this to absolute `expires_at` on the
    /// resulting [`Intent`].
    pub expires_at_offset: Option<Tick>,
    /// Why this intent was emitted (override-judgment input).
    pub rationale: RationaleSnapshot,
    /// Replace a previously-emitted intent with the given id.
    pub supersedes: Option<IntentId>,
    /// Delivery address (open-kind).
    pub target: IntentTargetRef,
    /// Optional delivery hint the dispatcher may honor or ignore.
    pub delivery_hint: Option<DeliveryHintId>,
}

impl IntentSpec {
    /// Minimal constructor. Callers fill the remaining fields via
    /// field-update-syntax.
    pub fn new(kind: impl Into<IntentKindId>, target: impl Into<IntentTargetRef>) -> Self {
        Self {
            kind: kind.into(),
            params: IntentParams::new(),
            priority: 0.5,
            importance: 0.5,
            half_life: None,
            expires_at_offset: None,
            rationale: RationaleSnapshot::empty(),
            supersedes: None,
            target: target.into(),
            delivery_hint: None,
        }
    }
}

/// Fully-materialized intent. The orchestrator/dispatcher has
/// stamped id, issued_at, arrives_at, and absolute expires_at.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Intent {
    pub id: IntentId,
    pub spec: IntentSpec,
    pub issued_at: Tick,
    pub arrives_at: Tick,
    pub expires_at: Option<Tick>,
}

impl Intent {
    /// Time-discounted effective priority at tick `now`.
    ///
    /// Applies exponential decay with the half-life from `spec.half_life`.
    /// Returns the undiscounted `priority` when no half-life is set.
    /// `now < issued_at` (shouldn't happen in practice) is clamped.
    pub fn effective_priority(&self, now: Tick) -> f32 {
        let Some(half_life) = self.spec.half_life else {
            return self.spec.priority;
        };
        if half_life <= 0 {
            return self.spec.priority;
        }
        let elapsed = (now - self.issued_at).max(0) as f32;
        let decay = (-std::f32::consts::LN_2 * elapsed / half_life as f32).exp();
        self.spec.priority * decay
    }

    /// True iff the intent has a hard expiry and it has passed.
    pub fn is_expired(&self, now: Tick) -> bool {
        self.expires_at.map(|e| now >= e).unwrap_or(false)
    }

    /// True iff the intent's delivery delay has elapsed. Mid-term agents
    /// should only see "arrived" intents in their inbox.
    pub fn has_arrived(&self, now: Tick) -> bool {
        now >= self.arrives_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(spec: IntentSpec, issued_at: Tick, arrives_at: Tick) -> Intent {
        Intent {
            id: IntentId::from("intent_0"),
            expires_at: spec.expires_at_offset.map(|o| issued_at + o),
            spec,
            issued_at,
            arrives_at,
        }
    }

    #[test]
    fn effective_priority_no_decay_when_half_life_none() {
        let mut spec = IntentSpec::new("pursue", "faction");
        spec.priority = 0.8;
        let intent = mk(spec, 0, 0);
        assert!((intent.effective_priority(100) - 0.8).abs() < 1e-6);
    }

    #[test]
    fn effective_priority_halves_at_half_life() {
        let mut spec = IntentSpec::new("pursue", "faction");
        spec.priority = 0.8;
        spec.half_life = Some(10);
        let intent = mk(spec, 0, 0);
        let p = intent.effective_priority(10);
        assert!((p - 0.4).abs() < 1e-3, "expected ~0.4, got {p}");
    }

    #[test]
    fn effective_priority_clamps_negative_elapsed() {
        let mut spec = IntentSpec::new("pursue", "faction");
        spec.priority = 0.7;
        spec.half_life = Some(10);
        let intent = mk(spec, 100, 100);
        assert!((intent.effective_priority(50) - 0.7).abs() < 1e-6);
    }

    #[test]
    fn has_arrived_true_when_now_at_or_past_arrives_at() {
        let spec = IntentSpec::new("k", "faction");
        let intent = mk(spec, 0, 10);
        assert!(!intent.has_arrived(9));
        assert!(intent.has_arrived(10));
        assert!(intent.has_arrived(11));
    }

    #[test]
    fn is_expired_respects_offset_conversion() {
        let mut spec = IntentSpec::new("k", "faction");
        spec.expires_at_offset = Some(20);
        let intent = mk(spec, 5, 5);
        // expires_at should be 5 + 20 = 25
        assert_eq!(intent.expires_at, Some(25));
        assert!(!intent.is_expired(24));
        assert!(intent.is_expired(25));
    }

    #[test]
    fn is_expired_false_when_no_expiry() {
        let spec = IntentSpec::new("k", "faction");
        let intent = mk(spec, 0, 0);
        assert!(!intent.is_expired(99_999));
    }

    #[test]
    fn intent_params_insert_and_get() {
        let p = IntentParams::new().with("target_metric", ValueExpr::Literal(1.0));
        assert!(p.get("target_metric").is_some());
        assert!(p.get("absent").is_none());
    }
}
