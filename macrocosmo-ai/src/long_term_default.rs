//! Default long-term agent — emits intents by traversing the
//! `VictoryCondition`.
//!
//! This is a first-cut reference implementation used by abstract
//! scenarios. It is deliberately simple:
//!
//! - Walk the `win` Condition tree; for every metric leaf not yet
//!   satisfied, emit a `pursue_metric` intent pushing in the required
//!   direction (priority/importance high).
//! - Walk the `prerequisites` Condition tree; for every metric leaf
//!   currently violated OR close to threshold, emit a
//!   `preserve_metric` intent (priority medium, importance high).
//! - Deduplicate by `(kind, metric)` — if the same pursuit intent is
//!   still in flight, use `supersedes` to replace.
//!
//! Game-side implementations can replace this wholesale with a smarter
//! policy (Nash, utility, Lua-driven), as long as they implement
//! `LongTermAgent`.
//!
//! Policy details (priority/importance/half-life/target) are
//! configurable via [`LongTermDefaultConfig`].

use std::sync::Arc;

use ahash::AHashMap;

use crate::agent::{LongTermAgent, LongTermInput, LongTermOutput};
use crate::condition::{Condition, ConditionAtom};
use crate::ids::{IntentId, IntentKindId, IntentTargetRef, MetricId};
use crate::intent::{IntentParams, IntentSpec, RationaleSnapshot};
use crate::time::Tick;
use crate::value_expr::ValueExpr;

/// Config for [`ObjectiveDrivenLongTerm`].
#[derive(Debug, Clone)]
pub struct LongTermDefaultConfig {
    /// Priority assigned to intents derived from `win` traversal.
    pub win_priority: f32,
    /// Importance assigned to intents derived from `win` traversal.
    pub win_importance: f32,
    /// Priority assigned to intents derived from `prerequisites` traversal.
    /// Typically lower than `win_priority` (acts as maintenance).
    pub prereq_priority: f32,
    /// Importance for prerequisite intents — usually high since
    /// violation makes victory `Unreachable`.
    pub prereq_importance: f32,
    /// Half-life applied to all emitted intents.
    pub half_life: Option<Tick>,
    /// Target for emitted intents.
    pub target: IntentTargetRef,
    /// Kind for win-pursuit intents.
    pub pursue_kind: IntentKindId,
    /// Kind for prerequisite-preservation intents.
    pub preserve_kind: IntentKindId,
    /// **Preemptive preservation** — emit a `preserve_metric` intent
    /// when a prereq metric is currently satisfied but within
    /// `safety_margin` of the threshold (= close to violation).
    ///
    /// With `safety_margin = 0.0` the agent only reacts to actual
    /// violations (reactive behavior — the pre-tuning default). Higher
    /// values make the agent preemptive; typical values correlate with
    /// the metric's tick-over-tick rate of change.
    ///
    /// Only affects `prerequisites` — `win` targets are always emitted
    /// strictly when unsatisfied (no preemption on the win side).
    pub safety_margin: f64,
}

impl Default for LongTermDefaultConfig {
    fn default() -> Self {
        Self {
            win_priority: 0.9,
            win_importance: 0.9,
            prereq_priority: 0.6,
            prereq_importance: 0.9,
            half_life: Some(60),
            target: IntentTargetRef::from("faction"),
            pursue_kind: IntentKindId::from("pursue_metric"),
            preserve_kind: IntentKindId::from("preserve_metric"),
            safety_margin: 0.0,
        }
    }
}

/// Default long-term agent: derives intents from a `VictoryCondition`.
///
/// Tracks the `IntentId` most recently emitted per `(kind, metric)`
/// key so follow-up emissions can attach `supersedes`. The actual id
/// minting is done by the orchestrator — the agent sees the previous
/// id by looking at active campaigns' `source_intent` (game layer is
/// responsible for feeding intent history back if needed). For the
/// abstract scenario harness the agent simply emits a fresh spec
/// every tick; `supersedes` is populated from an internal shadow map
/// when available.
pub struct ObjectiveDrivenLongTerm {
    pub config: LongTermDefaultConfig,
    /// Last intent id per `(kind, metric)`, keyed by
    /// `format!("{kind}/{metric}")`. Populated by the orchestrator
    /// via [`ObjectiveDrivenLongTerm::record_minted`] when the
    /// dispatcher succeeds — abstract scenarios can ignore this.
    pub last_id_by_key: AHashMap<Arc<str>, IntentId>,
}

impl ObjectiveDrivenLongTerm {
    pub fn new() -> Self {
        Self {
            config: LongTermDefaultConfig::default(),
            last_id_by_key: AHashMap::new(),
        }
    }

    pub fn with_config(mut self, config: LongTermDefaultConfig) -> Self {
        self.config = config;
        self
    }

    /// External callers (e.g. a game-side dispatcher hook) may record
    /// that a given `(kind, metric)` spec was minted as `intent_id`,
    /// so the next emission can reference it via `supersedes`.
    pub fn record_minted(&mut self, kind: &IntentKindId, metric: &MetricId, intent_id: IntentId) {
        self.last_id_by_key
            .insert(key(kind.as_str(), metric.as_str()), intent_id);
    }
}

impl Default for ObjectiveDrivenLongTerm {
    fn default() -> Self {
        Self::new()
    }
}

fn key(kind: &str, metric: &str) -> Arc<str> {
    Arc::from(format!("{kind}/{metric}"))
}

/// Walk a `Condition` tree collecting leaf `(metric, threshold, direction)`
/// entries. `direction = true` means "must be above threshold",
/// `false` means "must be below".
///
/// Ignores non-metric atoms (standing, evidence, etc.) — the default
/// agent focuses on metric-driven intents only.
fn collect_metric_targets(cond: &Condition, out: &mut Vec<(MetricId, f64, bool)>) {
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
                collect_metric_targets(c, out);
            }
        }
        Condition::Not(inner) => collect_metric_targets(inner, out),
    }
}

impl LongTermAgent for ObjectiveDrivenLongTerm {
    fn tick(&mut self, input: LongTermInput<'_>) -> LongTermOutput {
        // Only act when the victory is still in progress.
        if input.victory_status.is_terminal() {
            return LongTermOutput::default();
        }

        let mut intents = Vec::new();
        let mut win_targets = Vec::new();
        collect_metric_targets(&input.victory.win, &mut win_targets);
        let mut prereq_targets = Vec::new();
        collect_metric_targets(&input.victory.prerequisites, &mut prereq_targets);

        let emit = |kind: &IntentKindId,
                    priority: f32,
                    importance: f32,
                    metric: MetricId,
                    threshold: f64,
                    direction: bool,
                    current: f64,
                    intents: &mut Vec<IntentSpec>,
                    last_id_by_key: &AHashMap<Arc<str>, IntentId>| {
            let k = key(kind.as_str(), metric.as_str());
            let mut rationale_metrics = AHashMap::new();
            rationale_metrics.insert(metric.clone(), current);
            let note = Arc::from(format!(
                "{} {metric}: current={current} threshold={threshold} (dir={})",
                kind.as_str(),
                if direction { "above" } else { "below" }
            ));

            let mut params = IntentParams::new()
                .with("metric", ValueExpr::Literal(0.0))
                .with("threshold", ValueExpr::Literal(threshold))
                .with("direction", ValueExpr::Literal(if direction { 1.0 } else { 0.0 }));
            // Encode metric name as a params key `metric:<name>` so dispatchers
            // / mid-term agents can retrieve it without extra primitives.
            params = params.with(
                format!("metric:{}", metric.as_str()),
                ValueExpr::Literal(current),
            );

            intents.push(IntentSpec {
                kind: kind.clone(),
                params,
                priority,
                importance,
                half_life: None,
                expires_at_offset: None,
                rationale: RationaleSnapshot {
                    metrics_seen: rationale_metrics,
                    objective_id: None,
                    note,
                },
                supersedes: last_id_by_key.get(&k).cloned(),
                target: IntentTargetRef::from("faction"),
                delivery_hint: None,
            });
        };

        let target = self.config.target.clone();
        let half_life = self.config.half_life;

        for (metric, threshold, direction) in win_targets {
            let current = input.bus.current(&metric).unwrap_or(f64::NAN);
            let satisfied = if direction {
                current > threshold
            } else {
                current < threshold
            };
            if satisfied {
                continue;
            }
            emit(
                &self.config.pursue_kind,
                self.config.win_priority,
                self.config.win_importance,
                metric,
                threshold,
                direction,
                current,
                &mut intents,
                &self.last_id_by_key,
            );
        }

        // Prerequisites — preemptive when within `safety_margin`.
        //
        // For `direction = true` (must be above threshold), "close to
        // violation" means `current - threshold < safety_margin`. The
        // violated case (`current <= threshold`) is a strict subset
        // (distance is non-positive). For `direction = false`, mirror:
        // `threshold - current < safety_margin`.
        for (metric, threshold, direction) in prereq_targets {
            let current = input.bus.current(&metric).unwrap_or(f64::NAN);
            let distance = if direction {
                current - threshold
            } else {
                threshold - current
            };
            let within_margin = distance < self.config.safety_margin;
            let violated = distance <= 0.0;
            if !within_margin && !violated {
                continue;
            }
            emit(
                &self.config.preserve_kind,
                self.config.prereq_priority,
                self.config.prereq_importance,
                metric,
                threshold,
                direction,
                current,
                &mut intents,
                &self.last_id_by_key,
            );
        }

        // Apply shared policy (target, half_life).
        for spec in &mut intents {
            spec.target = target.clone();
            spec.half_life = half_life;
        }

        LongTermOutput { intents }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::AiBus;
    use crate::condition::{Condition, ConditionAtom};
    use crate::ids::{FactionId, MetricId};
    use crate::retention::Retention;
    use crate::spec::MetricSpec;
    use crate::victory::{VictoryCondition, VictoryStatus};
    use crate::warning::WarningMode;

    fn declare_and_emit(bus: &mut AiBus, metric: &str, value: f64, at: Tick) {
        let id = MetricId::from(metric);
        bus.declare_metric(id.clone(), MetricSpec::gauge(Retention::Short, "test"));
        bus.emit(&id, value, at);
    }

    fn above(m: &str, t: f64) -> Condition {
        Condition::Atom(ConditionAtom::MetricAbove {
            metric: MetricId::from(m),
            threshold: t,
        })
    }

    #[test]
    fn emits_pursue_when_win_metric_under_threshold() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        declare_and_emit(&mut bus, "econ", 20.0, 1);
        declare_and_emit(&mut bus, "stockpile", 5.0, 1);
        let victory = VictoryCondition::simple(above("econ", 100.0), above("stockpile", 0.0));

        let mut agent = ObjectiveDrivenLongTerm::new();
        let campaigns: Vec<&crate::campaign::Campaign> = vec![];
        let input = LongTermInput {
            bus: &bus,
            faction: FactionId(0),
            victory: &victory,
            victory_status: VictoryStatus::Ongoing { progress: 0.0 },
            active_campaigns: &campaigns,
            now: 1,
            params: None,
        };
        let out = agent.tick(input);
        assert_eq!(out.intents.len(), 1);
        assert_eq!(out.intents[0].kind.as_str(), "pursue_metric");
    }

    #[test]
    fn suppresses_emit_when_metric_already_satisfied() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        declare_and_emit(&mut bus, "econ", 150.0, 1);
        declare_and_emit(&mut bus, "stockpile", 5.0, 1);
        let victory = VictoryCondition::simple(above("econ", 100.0), above("stockpile", 0.0));

        let mut agent = ObjectiveDrivenLongTerm::new();
        let campaigns: Vec<&crate::campaign::Campaign> = vec![];
        let input = LongTermInput {
            bus: &bus,
            faction: FactionId(0),
            victory: &victory,
            victory_status: VictoryStatus::Ongoing { progress: 0.0 },
            active_campaigns: &campaigns,
            now: 1,
            params: None,
        };
        let out = agent.tick(input);
        assert_eq!(out.intents.len(), 0, "both satisfied, no emit");
    }

    #[test]
    fn emits_preserve_when_prereq_under_threshold() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        declare_and_emit(&mut bus, "econ", 150.0, 1); // win satisfied
        declare_and_emit(&mut bus, "stockpile", -5.0, 1); // prereq violated
        let victory = VictoryCondition::simple(above("econ", 100.0), above("stockpile", 0.0));

        let mut agent = ObjectiveDrivenLongTerm::new();
        let campaigns: Vec<&crate::campaign::Campaign> = vec![];
        let input = LongTermInput {
            bus: &bus,
            faction: FactionId(0),
            victory: &victory,
            // Unreachable would be the real status here — but long should
            // still be resilient. Use Ongoing to exercise the preserve path.
            victory_status: VictoryStatus::Ongoing { progress: 0.0 },
            active_campaigns: &campaigns,
            now: 1,
            params: None,
        };
        let out = agent.tick(input);
        assert_eq!(out.intents.len(), 1);
        assert_eq!(out.intents[0].kind.as_str(), "preserve_metric");
    }

    #[test]
    fn no_emit_when_victory_terminal() {
        let bus = AiBus::with_warning_mode(WarningMode::Silent);
        let victory = VictoryCondition::simple(above("econ", 100.0), above("stockpile", 0.0));
        let mut agent = ObjectiveDrivenLongTerm::new();
        let campaigns: Vec<&crate::campaign::Campaign> = vec![];
        let input = LongTermInput {
            bus: &bus,
            faction: FactionId(0),
            victory: &victory,
            victory_status: VictoryStatus::Won,
            active_campaigns: &campaigns,
            now: 1,
            params: None,
        };
        let out = agent.tick(input);
        assert_eq!(out.intents.len(), 0);
    }

    #[test]
    fn records_minted_populates_supersedes_next_tick() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        declare_and_emit(&mut bus, "econ", 20.0, 1);
        declare_and_emit(&mut bus, "stockpile", 5.0, 1);
        let victory = VictoryCondition::simple(above("econ", 100.0), above("stockpile", 0.0));

        let mut agent = ObjectiveDrivenLongTerm::new();
        let campaigns: Vec<&crate::campaign::Campaign> = vec![];
        let input_a = LongTermInput {
            bus: &bus,
            faction: FactionId(0),
            victory: &victory,
            victory_status: VictoryStatus::Ongoing { progress: 0.0 },
            active_campaigns: &campaigns,
            now: 1,
            params: None,
        };
        let out_a = agent.tick(input_a);
        assert_eq!(out_a.intents.len(), 1);
        assert!(out_a.intents[0].supersedes.is_none());
        agent.record_minted(
            &out_a.intents[0].kind,
            &MetricId::from("econ"),
            IntentId::from("intent_prev_0"),
        );

        let input_b = LongTermInput {
            bus: &bus,
            faction: FactionId(0),
            victory: &victory,
            victory_status: VictoryStatus::Ongoing { progress: 0.0 },
            active_campaigns: &campaigns,
            now: 31,
            params: None,
        };
        let out_b = agent.tick(input_b);
        assert_eq!(out_b.intents.len(), 1);
        assert_eq!(
            out_b.intents[0].supersedes.as_ref().map(|s| s.as_str()),
            Some("intent_prev_0")
        );
    }
}
