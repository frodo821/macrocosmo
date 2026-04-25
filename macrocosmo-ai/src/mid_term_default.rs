//! Default mid-term agent — translates intents into campaign ops.
//!
//! Strategy:
//!
//! 1. Drop / log each expired intent (`OverrideReason::Expired`).
//! 2. Sort remaining intents by `effective_priority(now)` descending.
//! 3. Drop / log intents below `stale_threshold`
//!    (`OverrideReason::StaleIntent`).
//! 4. For each remaining intent, synthesize an ObjectiveId from
//!    `(kind, params["metric"])` and issue `Start` + `Transition(Active)`
//!    if no matching campaign exists, or `AttachIntent` if one is
//!    already active for this objective.
//! 5. Honor `supersedes` — when an intent supersedes a prior one whose
//!    campaign is still active, `AttachIntent` the new intent id onto
//!    the existing campaign (no fresh Start). The orchestrator applies
//!    ops idempotently.
//!
//! Game-side agents replace this wholesale; see
//! `docs/ai-three-layer.md` §MidTermDefault.

use crate::agent::{
    CampaignOp, MidTermAgent, MidTermInput, MidTermOutput, OverrideEntry, OverrideReason,
};
use crate::campaign::CampaignState;
use crate::condition::{Condition, ConditionAtom};
use crate::ids::{MetricId, ObjectiveId};
use crate::intent::Intent;

/// Config for [`IntentDrivenMidTerm`].
#[derive(Debug, Clone)]
pub struct MidTermDefaultConfig {
    /// Intents with `effective_priority(now) < stale_threshold` are
    /// overridden (not applied) and logged.
    pub stale_threshold: f32,
    /// **Guardrail**: when any prereq metric is within
    /// `prereq_guardrail` of its threshold, suspend any
    /// `pursue_metric:*` campaigns until the metric recovers. When
    /// the prereq lifts back above the margin, suspended pursuits
    /// transition back to `Active`.
    ///
    /// Default `0.0` keeps the pre-tuning behavior (mid never
    /// throttles based on prereq state).
    pub prereq_guardrail: f64,
    /// Prefix used to identify campaigns governed by the guardrail.
    /// Campaigns whose `id.as_str()` starts with this prefix are
    /// candidates for throttling. Default `"pursue_metric:"` matches
    /// the [`crate::long_term_default::ObjectiveDrivenLongTerm`]
    /// pursue convention.
    pub guardrail_pursue_prefix: String,
    /// When `true`, Mid stamps `Campaign.weight` from intent
    /// `priority * importance` on Start / AttachIntent and emits
    /// `CampaignOp::SetWeight` when an attach changes the value.
    /// Short-term agents that honor weights (see
    /// `CampaignReactiveShort.priority_weighted`) use this for
    /// fractional command scheduling. Default `true` — even when
    /// short ignores weights, the data on the Campaign is harmless.
    pub stamp_weights: bool,
    /// When `true`, the mid agent reacts to a terminal
    /// `victory_status`:
    /// - `Unreachable` / `TimedOut` → transition all `Active`
    ///   campaigns to `Abandoned` (path closed, no point continuing).
    /// - `Won` → transition all `Active` campaigns to `Succeeded`
    ///   (work is done) — *only* if `treat_won_as_terminal` is also
    ///   `true`.
    ///
    /// On a terminal status the inbox is also ignored (no new
    /// campaigns are started). Default `true` — mirrors the
    /// long-term agent's `is_terminal()` short-circuit.
    pub abandon_on_terminal: bool,
    /// When `true` (default), `Won` is treated as a terminal status:
    /// Active campaigns transition to `Succeeded` and the inbox is
    /// ignored, matching `Unreachable` / `TimedOut`. Appropriate for
    /// "achieve once" goals where reaching the threshold means the
    /// objective is permanently complete.
    ///
    /// Set to `false` for **maintenance** goals where the win can be
    /// undone (adversarial scenarios, "control N% territory" style
    /// objectives). When `false`, the mid agent ignores `Won` for
    /// abandon purposes — Active campaigns stay Active so the short
    /// agent keeps emitting commands to defend the threshold against
    /// erosion. The inbox is still processed (new intents land
    /// normally). `Unreachable` / `TimedOut` remain terminal
    /// regardless.
    pub treat_won_as_terminal: bool,
}

impl Default for MidTermDefaultConfig {
    fn default() -> Self {
        Self {
            stale_threshold: 0.1,
            prereq_guardrail: 0.0,
            guardrail_pursue_prefix: "pursue_metric:".into(),
            stamp_weights: true,
            abandon_on_terminal: true,
            treat_won_as_terminal: true,
        }
    }
}

/// Default mid-term agent: `Intent → CampaignOp` translator.
#[derive(Debug, Default)]
pub struct IntentDrivenMidTerm {
    pub config: MidTermDefaultConfig,
}

impl IntentDrivenMidTerm {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(mut self, config: MidTermDefaultConfig) -> Self {
        self.config = config;
        self
    }
}

/// Walk a prereq `Condition` tree and return any metric leaves whose
/// current bus value is within `margin` of the threshold (= near or
/// past violation).
fn prereq_metrics_in_danger(
    bus: &crate::bus::AiBus,
    cond: &Condition,
    margin: f64,
) -> Vec<MetricId> {
    let mut leaves = Vec::new();
    collect_leaves(cond, &mut leaves);
    leaves
        .into_iter()
        .filter(|(metric, threshold, direction)| {
            let cur = match bus.current(metric) {
                Some(v) => v,
                None => return false,
            };
            let distance = if *direction {
                cur - *threshold
            } else {
                *threshold - cur
            };
            distance < margin
        })
        .map(|(m, _, _)| m)
        .collect()
}

fn collect_leaves(cond: &Condition, out: &mut Vec<(MetricId, f64, bool)>) {
    match cond {
        Condition::Always | Condition::Never => {}
        Condition::Atom(a) => match a {
            ConditionAtom::MetricAbove { metric, threshold } => {
                out.push((metric.clone(), *threshold, true));
            }
            ConditionAtom::MetricBelow { metric, threshold } => {
                out.push((metric.clone(), *threshold, false));
            }
            _ => {}
        },
        Condition::All(children) | Condition::Any(children) | Condition::OneOf(children) => {
            for c in children {
                collect_leaves(c, out);
            }
        }
        Condition::Not(inner) => collect_leaves(inner, out),
    }
}

/// Synthesize an ObjectiveId for an intent: `{kind}[:{metric_key}]`.
///
/// The intent's params may carry a `metric:<name>` key
/// (emitted by the default long-term agent); when present, it is
/// appended so different metrics get distinct campaigns.
fn objective_id_for(intent: &Intent) -> ObjectiveId {
    let kind = intent.spec.kind.as_str();
    let metric_key = intent
        .spec
        .params
        .0
        .keys()
        .find(|k| k.starts_with("metric:"))
        .map(|k| k.strip_prefix("metric:").unwrap_or("").to_string());
    match metric_key {
        Some(m) if !m.is_empty() => ObjectiveId::from(format!("{kind}:{m}")),
        _ => ObjectiveId::from(kind),
    }
}

impl MidTermAgent for IntentDrivenMidTerm {
    fn tick(&mut self, input: MidTermInput<'_>) -> MidTermOutput {
        let mut ops = Vec::new();
        let mut log = Vec::new();

        // Terminal short-circuit (mirror of `LongTermAgent.is_terminal()`):
        // when victory is Won / Unreachable / TimedOut, transition
        // active campaigns and stop processing the inbox. Saves the
        // short-term agent from emitting commands for campaigns that
        // can no longer matter.
        if self.config.abandon_on_terminal {
            use crate::victory::VictoryStatus::*;
            let target_state = match input.victory_status {
                Won if self.config.treat_won_as_terminal => Some(CampaignState::Succeeded),
                Won => None,
                Unreachable | TimedOut => Some(CampaignState::Abandoned),
                Ongoing { .. } => None,
            };
            if let Some(to) = target_state {
                for c in input.campaigns {
                    if c.state == CampaignState::Active {
                        ops.push(CampaignOp::Transition {
                            campaign_id: c.id.clone(),
                            to,
                            at: input.now,
                        });
                    }
                }
                return MidTermOutput {
                    campaign_ops: ops,
                    override_log: log,
                };
            }
        }

        // Collect (score, intent) after filtering expired.
        let mut scored: Vec<(f32, &Intent)> = Vec::with_capacity(input.inbox.len());
        for intent in input.inbox {
            if intent.is_expired(input.now) {
                log.push(OverrideEntry {
                    intent_id: intent.id.clone(),
                    intent_kind: intent.spec.kind.clone(),
                    reason: OverrideReason::Expired,
                    at: input.now,
                });
                continue;
            }
            let score = intent.effective_priority(input.now);
            scored.push((score, intent));
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        for (score, intent) in scored {
            if score < self.config.stale_threshold {
                log.push(OverrideEntry {
                    intent_id: intent.id.clone(),
                    intent_kind: intent.spec.kind.clone(),
                    reason: OverrideReason::StaleIntent,
                    at: input.now,
                });
                continue;
            }

            let objective_id = objective_id_for(intent);

            // weight = priority * importance. Acts as the campaign's
            // share of short-agent command budget. Float-clamped non-
            // negative; safe to skip when stamp_weights is off.
            let intent_weight = (intent.spec.priority * intent.spec.importance).max(0.0) as f64;

            // Supersedes handling: if the supersedes target's campaign
            // is still live, re-point it at this new intent.
            if let Some(superseded_id) = &intent.spec.supersedes {
                if let Some(existing) = input
                    .campaigns
                    .iter()
                    .find(|c| c.source_intent.as_ref() == Some(superseded_id))
                {
                    log.push(OverrideEntry {
                        intent_id: superseded_id.clone(),
                        intent_kind: intent.spec.kind.clone(),
                        reason: OverrideReason::Superseded,
                        at: input.now,
                    });
                    ops.push(CampaignOp::AttachIntent {
                        campaign_id: existing.id.clone(),
                        intent_id: intent.id.clone(),
                    });
                    if self.config.stamp_weights
                        && (existing.weight - intent_weight).abs() > f64::EPSILON
                    {
                        ops.push(CampaignOp::SetWeight {
                            campaign_id: existing.id.clone(),
                            weight: intent_weight,
                        });
                    }
                    // Ensure active (in case it was suspended).
                    if existing.state != CampaignState::Active {
                        ops.push(CampaignOp::Transition {
                            campaign_id: existing.id.clone(),
                            to: CampaignState::Active,
                            at: input.now,
                        });
                    }
                    continue;
                }
            }

            // If a campaign with this objective exists, attach the new
            // intent (and re-activate if needed).
            if let Some(existing) = input.campaigns.iter().find(|c| c.id == objective_id) {
                if existing.source_intent.as_ref() != Some(&intent.id) {
                    ops.push(CampaignOp::AttachIntent {
                        campaign_id: existing.id.clone(),
                        intent_id: intent.id.clone(),
                    });
                }
                if self.config.stamp_weights
                    && (existing.weight - intent_weight).abs() > f64::EPSILON
                {
                    ops.push(CampaignOp::SetWeight {
                        campaign_id: existing.id.clone(),
                        weight: intent_weight,
                    });
                }
                if existing.state != CampaignState::Active && !existing.state.is_terminal() {
                    ops.push(CampaignOp::Transition {
                        campaign_id: existing.id.clone(),
                        to: CampaignState::Active,
                        at: input.now,
                    });
                }
            } else {
                // Fresh campaign.
                ops.push(CampaignOp::Start {
                    objective_id: objective_id.clone(),
                    source_intent: Some(intent.id.clone()),
                    at: input.now,
                });
                if self.config.stamp_weights {
                    ops.push(CampaignOp::SetWeight {
                        campaign_id: objective_id.clone(),
                        weight: intent_weight,
                    });
                }
                ops.push(CampaignOp::Transition {
                    campaign_id: objective_id,
                    to: CampaignState::Active,
                    at: input.now,
                });
            }
        }

        // Guardrail pass: walk prereq metric leaves, suspend / resume
        // pursue campaigns based on whether each leaf is within the
        // configured margin. We treat the post-ops campaign view (the
        // `Start` we just queued may not have applied yet, but
        // `apply_campaign_op` is idempotent so a redundant Transition
        // here is fine).
        if self.config.prereq_guardrail > 0.0 {
            let in_danger = prereq_metrics_in_danger(
                input.bus,
                &input.victory.prerequisites,
                self.config.prereq_guardrail,
            );
            for c in input.campaigns {
                if !c
                    .id
                    .as_str()
                    .starts_with(&self.config.guardrail_pursue_prefix)
                {
                    continue;
                }
                let should_suspend = !in_danger.is_empty();
                match c.state {
                    CampaignState::Active if should_suspend => {
                        ops.push(CampaignOp::Transition {
                            campaign_id: c.id.clone(),
                            to: CampaignState::Suspended,
                            at: input.now,
                        });
                    }
                    CampaignState::Suspended if !should_suspend => {
                        ops.push(CampaignOp::Transition {
                            campaign_id: c.id.clone(),
                            to: CampaignState::Active,
                            at: input.now,
                        });
                    }
                    _ => {}
                }
            }
        }

        MidTermOutput {
            campaign_ops: ops,
            override_log: log,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(unused_imports)]
    use super::*;
    use crate::bus::AiBus;
    use crate::campaign::Campaign;
    use crate::ids::{FactionId, IntentId, IntentKindId, IntentTargetRef};
    use crate::intent::{IntentParams, IntentSpec, RationaleSnapshot};
    use crate::value_expr::ValueExpr;
    use crate::victory::VictoryCondition;
    use crate::warning::WarningMode;

    fn test_victory() -> VictoryCondition {
        VictoryCondition::simple(Condition::Always, Condition::Always)
    }

    fn make_intent(id: &str, kind: &str, issued_at: i64, arrives_at: i64) -> Intent {
        let spec = IntentSpec {
            kind: IntentKindId::from(kind),
            params: IntentParams::new().with("metric:econ", ValueExpr::Literal(1.0)),
            priority: 0.8,
            importance: 0.9,
            half_life: None,
            expires_at_offset: None,
            rationale: RationaleSnapshot::empty(),
            supersedes: None,
            target: IntentTargetRef::from("faction"),
            delivery_hint: None,
        };
        Intent {
            id: IntentId::from(id),
            spec,
            issued_at,
            arrives_at,
            expires_at: None,
        }
    }

    #[test]
    fn starts_fresh_campaign_from_intent() {
        let bus = AiBus::with_warning_mode(WarningMode::Silent);
        let mut agent = IntentDrivenMidTerm::new();
        let intent = make_intent("intent_1", "pursue_metric", 0, 1);
        let input = MidTermInput {
            bus: &bus,
            faction: FactionId(0),
            inbox: std::slice::from_ref(&intent),
            campaigns: &[],
            now: 1,
            params: None,
            victory: &test_victory(),
            victory_status: crate::victory::VictoryStatus::Ongoing { progress: 0.0 },
        };
        let out = agent.tick(input);
        // Start + SetWeight + Transition (stamp_weights default = true).
        assert_eq!(out.campaign_ops.len(), 3);
        matches!(out.campaign_ops[0], CampaignOp::Start { .. });
        matches!(out.campaign_ops[1], CampaignOp::SetWeight { .. });
        matches!(out.campaign_ops[2], CampaignOp::Transition { .. });
    }

    #[test]
    fn attaches_intent_to_existing_campaign_with_same_objective() {
        let bus = AiBus::with_warning_mode(WarningMode::Silent);
        let mut agent = IntentDrivenMidTerm::new();
        let intent = make_intent("intent_2", "pursue_metric", 10, 11);
        let mut existing = Campaign::new(objective_id_for(&intent), 0);
        existing.state = CampaignState::Active;
        existing.source_intent = Some(IntentId::from("intent_1"));
        let input = MidTermInput {
            bus: &bus,
            faction: FactionId(0),
            inbox: std::slice::from_ref(&intent),
            campaigns: std::slice::from_ref(&existing),
            now: 11,
            params: None,
            victory: &test_victory(),
            victory_status: crate::victory::VictoryStatus::Ongoing { progress: 0.0 },
        };
        let out = agent.tick(input);
        // AttachIntent + SetWeight (existing.weight=1.0, intent
        // weight=0.8*0.9=0.72 ≠ 1.0).
        assert_eq!(out.campaign_ops.len(), 2);
        match &out.campaign_ops[0] {
            CampaignOp::AttachIntent { intent_id, .. } => {
                assert_eq!(intent_id.as_str(), "intent_2");
            }
            other => panic!("expected AttachIntent, got {other:?}"),
        }
        matches!(out.campaign_ops[1], CampaignOp::SetWeight { .. });
    }

    #[test]
    fn stale_intent_is_overridden_not_applied() {
        let bus = AiBus::with_warning_mode(WarningMode::Silent);
        let mut agent = IntentDrivenMidTerm::new();
        let mut intent = make_intent("intent_3", "pursue_metric", 0, 1);
        intent.spec.priority = 0.05; // below default 0.1 stale threshold
        let input = MidTermInput {
            bus: &bus,
            faction: FactionId(0),
            inbox: std::slice::from_ref(&intent),
            campaigns: &[],
            now: 1,
            params: None,
            victory: &test_victory(),
            victory_status: crate::victory::VictoryStatus::Ongoing { progress: 0.0 },
        };
        let out = agent.tick(input);
        assert_eq!(out.campaign_ops.len(), 0);
        assert_eq!(out.override_log.len(), 1);
        assert_eq!(out.override_log[0].reason, OverrideReason::StaleIntent);
    }

    #[test]
    fn expired_intent_is_overridden_not_applied() {
        let bus = AiBus::with_warning_mode(WarningMode::Silent);
        let mut agent = IntentDrivenMidTerm::new();
        let mut intent = make_intent("intent_4", "pursue_metric", 0, 1);
        intent.expires_at = Some(5);
        let input = MidTermInput {
            bus: &bus,
            faction: FactionId(0),
            inbox: std::slice::from_ref(&intent),
            campaigns: &[],
            now: 10,
            params: None,
            victory: &test_victory(),
            victory_status: crate::victory::VictoryStatus::Ongoing { progress: 0.0 },
        };
        let out = agent.tick(input);
        assert_eq!(out.campaign_ops.len(), 0);
        assert_eq!(out.override_log.len(), 1);
        assert_eq!(out.override_log[0].reason, OverrideReason::Expired);
    }

    #[test]
    fn supersedes_reroutes_existing_campaign() {
        let bus = AiBus::with_warning_mode(WarningMode::Silent);
        let mut agent = IntentDrivenMidTerm::new();
        let mut intent = make_intent("intent_new", "pursue_metric", 20, 21);
        intent.spec.supersedes = Some(IntentId::from("intent_old"));
        let old_objective = ObjectiveId::from("old_campaign");
        let mut existing = Campaign::new(old_objective.clone(), 0);
        existing.state = CampaignState::Active;
        existing.source_intent = Some(IntentId::from("intent_old"));
        let input = MidTermInput {
            bus: &bus,
            faction: FactionId(0),
            inbox: std::slice::from_ref(&intent),
            campaigns: std::slice::from_ref(&existing),
            now: 21,
            params: None,
            victory: &test_victory(),
            victory_status: crate::victory::VictoryStatus::Ongoing { progress: 0.0 },
        };
        let out = agent.tick(input);
        // One Superseded log entry + AttachIntent + SetWeight (existing
        // campaign was still Active so no additional Transition; weight
        // changes from 1.0 default to 0.72).
        assert_eq!(out.override_log.len(), 1);
        assert_eq!(out.override_log[0].reason, OverrideReason::Superseded);
        assert_eq!(out.campaign_ops.len(), 2);
        match &out.campaign_ops[0] {
            CampaignOp::AttachIntent {
                campaign_id,
                intent_id,
            } => {
                assert_eq!(campaign_id, &old_objective);
                assert_eq!(intent_id.as_str(), "intent_new");
            }
            other => panic!("expected AttachIntent, got {other:?}"),
        }
    }
}
