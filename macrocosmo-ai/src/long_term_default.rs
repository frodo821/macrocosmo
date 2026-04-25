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
use crate::bus::AiBus;
use crate::condition::{Condition, ConditionAtom};
use crate::ids::{IntentId, IntentKindId, IntentTargetRef, MetricId};
use crate::intent::{IntentParams, IntentSpec, RationaleSnapshot};
use crate::projection::{
    self, ThresholdGate, TrajectoryConfig, WindowDetectionConfig, WindowKind, detect_windows,
};
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
    /// Default validity window (relative offset from `issued_at`)
    /// stamped onto every emitted intent's `expires_at_offset`.
    /// Dispatchers that compare `estimate_delay()` against expiry can
    /// drop intents whose window the carrier cannot meet — see
    /// `FixedDelayDispatcher.drop_when_expiry_exceeded`.
    ///
    /// `None` (default) means "no expiry" (intent stays valid until
    /// stale_threshold or hard expiry from supersedes). Setting this
    /// is the easiest way to opt into adaptive retry / fallback.
    pub default_validity_window: Option<Tick>,
    /// On retry after a drop, multiply the previous `expires_at_offset`
    /// by this factor. Default `2.0` (doubles each retry).
    pub retry_window_extension: f64,
    /// After this many drops for the same `(kind, metric)` pair, the
    /// long agent **stops emitting** that pursuit and falls back to
    /// the remaining pursuits. Default `2` — i.e., retry once with
    /// extended window, then surrender.
    pub max_retries_before_fallback: usize,
    /// When `true`, the long agent uses projection-driven validity
    /// windows: for each leaf metric the agent runs `project()` over
    /// recent bus history, looks for a `ThresholdRace` window via
    /// `detect_windows()`, and uses `reached_at - now` as the
    /// per-leaf `expires_at_offset`. Static `default_validity_window`
    /// is used only as a fallback when projection produces no
    /// crossing (flat trajectory, insufficient history, etc.).
    ///
    /// Default `false` to preserve the previous behavior.
    pub use_projection_window: bool,
    /// Trajectory parameters used when `use_projection_window = true`.
    pub projection_config: TrajectoryConfig,
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
            default_validity_window: None,
            retry_window_extension: 2.0,
            max_retries_before_fallback: 2,
            use_projection_window: false,
            projection_config: TrajectoryConfig::default(),
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
    /// Drop counter per `(kind, metric)` key. Each drop observed
    /// via `LongTermInput.recent_drops` increments the counter for
    /// that key. Reset is implicit — currently the agent never
    /// resets, it just stops emitting once the cap is reached.
    /// `record_minted` *could* reset on success but that's not
    /// modeled yet (would re-introduce flapping in cyclical drop
    /// patterns).
    pub drop_counts: AHashMap<Arc<str>, usize>,
}

impl ObjectiveDrivenLongTerm {
    pub fn new() -> Self {
        Self {
            config: LongTermDefaultConfig::default(),
            last_id_by_key: AHashMap::new(),
            drop_counts: AHashMap::new(),
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

/// Project per-leaf validity windows from bus history. For each
/// `(metric, threshold)` pair, fits a trajectory and checks whether
/// the projected path crosses the threshold; if so, returns
/// `reached_at - now` as the validity window.
///
/// Used by [`ObjectiveDrivenLongTerm`] when
/// `LongTermDefaultConfig.use_projection_window` is set. Metrics
/// that don't cross or lack enough history are absent from the map
/// (caller falls back to the static default).
fn project_validity_windows(
    bus: &AiBus,
    now: Tick,
    leaves: &[(MetricId, f64, bool)],
    cfg: &TrajectoryConfig,
) -> AHashMap<MetricId, Tick> {
    if leaves.is_empty() {
        return AHashMap::new();
    }
    let metric_ids: Vec<MetricId> = leaves.iter().map(|(m, _, _)| m.clone()).collect();
    let trajectories = projection::project(bus, &metric_ids, cfg, now, &[]);
    let gates: Vec<ThresholdGate> = leaves
        .iter()
        .map(|(m, t, _)| ThresholdGate {
            metric: m.clone(),
            threshold: *t,
        })
        .collect();
    let det_cfg = WindowDetectionConfig {
        threshold_gates: gates,
        // We want every crossing, not just the most intense — pass
        // through low-confidence projections too.
        min_intensity: 0.0,
        ..WindowDetectionConfig::default()
    };
    let windows = detect_windows(&trajectories, now, &det_cfg);

    let mut out = AHashMap::new();
    for w in &windows {
        if let WindowKind::ThresholdRace {
            metric, reached_at, ..
        } = &w.kind
        {
            if *reached_at > now {
                out.insert(metric.clone(), *reached_at - now);
            }
        }
    }
    out
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

        // Ingest recent drops — bump per-(kind, metric) drop counter so
        // subsequent emits for that key extend the validity window or
        // fall back. `metric_hint` is the metric encoded by our own
        // emitter (`metric:<name>` param key); when missing, the drop
        // is bucketed under the kind alone.
        for d in input.recent_drops {
            let metric_str = d
                .metric_hint
                .as_ref()
                .map(|s| s.as_ref())
                .unwrap_or("");
            let k = key(d.spec_kind.as_str(), metric_str);
            *self.drop_counts.entry(k).or_insert(0) += 1;
        }

        let mut intents = Vec::new();
        let mut win_targets = Vec::new();
        collect_metric_targets(&input.victory.win, &mut win_targets);
        let mut prereq_targets = Vec::new();
        collect_metric_targets(&input.victory.prerequisites, &mut prereq_targets);

        // Per-leaf projected validity windows. Empty when feature off
        // or projection lacks data; emit() falls back to default_window.
        let projected_windows: AHashMap<MetricId, Tick> = if self.config.use_projection_window {
            let mut all_leaves = win_targets.clone();
            all_leaves.extend(prereq_targets.iter().cloned());
            project_validity_windows(
                input.bus,
                input.now,
                &all_leaves,
                &self.config.projection_config,
            )
        } else {
            AHashMap::new()
        };

        let default_window = self.config.default_validity_window;
        let extension = self.config.retry_window_extension.max(1.0);
        let max_retries = self.config.max_retries_before_fallback;

        let emit = |kind: &IntentKindId,
                    priority: f32,
                    importance: f32,
                    metric: MetricId,
                    threshold: f64,
                    direction: bool,
                    current: f64,
                    intents: &mut Vec<IntentSpec>,
                    last_id_by_key: &AHashMap<Arc<str>, IntentId>,
                    drop_counts: &AHashMap<Arc<str>, usize>,
                    projected_windows: &AHashMap<MetricId, Tick>|
         -> bool {
            let k = key(kind.as_str(), metric.as_str());
            let drops = drop_counts.get(&k).copied().unwrap_or(0);
            // Fallback: surrender this leaf after the configured cap.
            if drops >= max_retries {
                return false;
            }

            let mut rationale_metrics = AHashMap::new();
            rationale_metrics.insert(metric.clone(), current);
            let note = Arc::from(format!(
                "{} {metric}: current={current} threshold={threshold} (dir={}, retries={drops})",
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

            // Per-leaf base window: prefer projection-driven, fall back to static.
            let base_window = projected_windows.get(&metric).copied().or(default_window);
            // Adaptive expiry: extend by `extension^drops` for each prior drop.
            let expires_at_offset = base_window.map(|w| {
                let mult = extension.powi(drops as i32);
                ((w as f64) * mult).round() as Tick
            });

            intents.push(IntentSpec {
                kind: kind.clone(),
                params,
                priority,
                importance,
                half_life: None,
                expires_at_offset,
                rationale: RationaleSnapshot {
                    metrics_seen: rationale_metrics,
                    objective_id: None,
                    note,
                },
                supersedes: last_id_by_key.get(&k).cloned(),
                target: IntentTargetRef::from("faction"),
                delivery_hint: None,
            });
            true
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
                &self.drop_counts,
                &projected_windows,
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
                &self.drop_counts,
                &projected_windows,
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
            recent_drops: &[],
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
            recent_drops: &[],
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
            recent_drops: &[],
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
            recent_drops: &[],
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
            recent_drops: &[],
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
            recent_drops: &[],
        };
        let out_b = agent.tick(input_b);
        assert_eq!(out_b.intents.len(), 1);
        assert_eq!(
            out_b.intents[0].supersedes.as_ref().map(|s| s.as_str()),
            Some("intent_prev_0")
        );
    }
}
