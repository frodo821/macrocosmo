//! Condition tree and evaluator.
//!
//! A `Condition` is a boolean expression over the AI bus. It has no
//! side effects and is evaluated via `evaluate(&EvalContext)`.
//!
//! Phase 3 (#192) extends the atom vocabulary with:
//! - `Compare { left, op, right }` — `ValueExpr` comparisons with `Missing`
//!   propagating to `false`
//! - `ValueMissing(expr)` — detect Missing in an expression
//! - `MetricStale { metric, max_age }` — detect stale metric samples
//! - `EvidenceRateAbove` — evidence arrival rate over a window

use serde::{Deserialize, Serialize};

use crate::eval::EvalContext;
use crate::ids::{EvidenceKindId, FactionId, MetricId};
use crate::standing::{self, StandingSubject};
use crate::time::Tick;
use crate::value_expr::{Dependencies, Value, ValueExpr};

/// Epsilon tolerance used by `CompareOp::Eq` / `NotEq`.
pub const COMPARE_EPSILON: f64 = f64::EPSILON * 16.0;

/// Comparison operator used by `ConditionAtom::Compare`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CompareOp {
    Eq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Tree combinator: logical composition of atomic conditions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Condition {
    /// Always `true`. Useful as a default precondition.
    Always,
    /// Always `false`.
    Never,
    /// Conjunction — all children must hold.
    All(Vec<Condition>),
    /// Disjunction — at least one child must hold.
    Any(Vec<Condition>),
    /// Exclusive — exactly one child must hold.
    OneOf(Vec<Condition>),
    /// Negation.
    Not(Box<Condition>),
    /// A leaf atom interpreted against the bus.
    Atom(ConditionAtom),
}

/// Leaf atoms. Extend freely; the combinators above do not care.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConditionAtom {
    /// True iff the metric's current value exceeds `threshold`.
    MetricAbove { metric: MetricId, threshold: f64 },
    /// True iff the metric's current value is below `threshold`.
    MetricBelow { metric: MetricId, threshold: f64 },
    /// True iff the metric has an emitted value (declared and not stale-empty).
    MetricPresent { metric: MetricId },
    /// True iff the observing faction (from `ctx.faction`) has accumulated
    /// more than `threshold` evidence entries of `kind` in the last
    /// `window` ticks. Returns `false` if `ctx.faction` is unset.
    EvidenceCountExceeds {
        kind: EvidenceKindId,
        window: Tick,
        threshold: usize,
    },
    /// Generic comparison between two [`ValueExpr`]s. If either side is
    /// `Missing`, the atom evaluates to `false`. `Eq`/`NotEq` use epsilon
    /// tolerance ([`COMPARE_EPSILON`]).
    Compare {
        left: ValueExpr,
        op: CompareOp,
        right: ValueExpr,
    },
    /// True iff the expression evaluates to `Missing`.
    ValueMissing(ValueExpr),
    /// True iff the metric's latest sample is older than `max_age` ticks
    /// (i.e. `now - latest.at > max_age`). `true` if undeclared/no samples.
    MetricStale { metric: MetricId, max_age: Tick },
    /// True iff the observer accumulates evidence of `kind` at a rate above
    /// `rate_per_tick` over the window, i.e. `count / window > rate`.
    /// Requires `ctx.faction` to be set; otherwise `false`.
    EvidenceRateAbove {
        kind: EvidenceKindId,
        window: Tick,
        rate_per_tick: f64,
    },
    /// True iff the observer's perceived standing toward `target` is below
    /// `threshold`. Requires `ctx.faction`, `ctx.standing_config`, and
    /// `ctx.ai_params`; returns `false` (with a one-shot warning) otherwise.
    StandingBelow { target: FactionId, threshold: f64 },
    /// True iff the observer's perceived standing toward `target` is above
    /// `threshold`. Same ctx requirements as `StandingBelow`.
    StandingAbove { target: FactionId, threshold: f64 },
    /// True iff the observer's standing confidence toward `target` is above
    /// `threshold`. Same ctx requirements as `StandingBelow`.
    StandingConfidenceAbove { target: FactionId, threshold: f64 },
}

impl Condition {
    pub fn and(children: impl IntoIterator<Item = Condition>) -> Self {
        Condition::All(children.into_iter().collect())
    }

    pub fn or(children: impl IntoIterator<Item = Condition>) -> Self {
        Condition::Any(children.into_iter().collect())
    }

    pub fn not(inner: Condition) -> Self {
        Condition::Not(Box::new(inner))
    }

    /// Ergonomic builder for `Compare`.
    pub fn compare(left: ValueExpr, op: CompareOp, right: ValueExpr) -> Self {
        Condition::Atom(ConditionAtom::Compare { left, op, right })
    }

    pub fn gt(left: ValueExpr, right: ValueExpr) -> Self {
        Self::compare(left, CompareOp::Gt, right)
    }

    pub fn ge(left: ValueExpr, right: ValueExpr) -> Self {
        Self::compare(left, CompareOp::Ge, right)
    }

    pub fn lt(left: ValueExpr, right: ValueExpr) -> Self {
        Self::compare(left, CompareOp::Lt, right)
    }

    pub fn le(left: ValueExpr, right: ValueExpr) -> Self {
        Self::compare(left, CompareOp::Le, right)
    }

    pub fn eq(left: ValueExpr, right: ValueExpr) -> Self {
        Self::compare(left, CompareOp::Eq, right)
    }

    /// `left / right >= ratio`. Encoded as `left >= right * ratio` to avoid
    /// division-by-zero pitfalls when `right` is zero.
    pub fn metric_ratio_ge(left: ValueExpr, right: ValueExpr, ratio: f64) -> Self {
        Self::ge(left, ValueExpr::Mul(vec![right, ValueExpr::Literal(ratio)]))
    }

    /// Convenience: "metric m is trending up over window" (DelT > 0).
    pub fn metric_trend_up(metric: MetricId, window: Tick) -> Self {
        Self::gt(
            ValueExpr::DelT {
                metric: crate::value_expr::MetricRef::new(metric),
                window,
            },
            ValueExpr::Literal(0.0),
        )
    }

    pub fn evaluate(&self, ctx: &EvalContext) -> bool {
        match self {
            Condition::Always => true,
            Condition::Never => false,
            Condition::All(children) => children.iter().all(|c| c.evaluate(ctx)),
            Condition::Any(children) => children.iter().any(|c| c.evaluate(ctx)),
            Condition::OneOf(children) => children.iter().filter(|c| c.evaluate(ctx)).count() == 1,
            Condition::Not(inner) => !inner.evaluate(ctx),
            Condition::Atom(a) => a.evaluate(ctx),
        }
    }

    /// Walk the tree to collect bus-topic dependencies.
    pub fn collect_deps(&self, deps: &mut Dependencies) {
        match self {
            Condition::Always | Condition::Never => {}
            Condition::All(children) | Condition::Any(children) | Condition::OneOf(children) => {
                for c in children {
                    c.collect_deps(deps);
                }
            }
            Condition::Not(inner) => inner.collect_deps(deps),
            Condition::Atom(a) => a.collect_deps(deps),
        }
    }
}

impl ConditionAtom {
    pub fn evaluate(&self, ctx: &EvalContext) -> bool {
        match self {
            ConditionAtom::MetricAbove { metric, threshold } => {
                ctx.bus.current(metric).map_or(false, |v| v > *threshold)
            }
            ConditionAtom::MetricBelow { metric, threshold } => {
                ctx.bus.current(metric).map_or(false, |v| v < *threshold)
            }
            ConditionAtom::MetricPresent { metric } => ctx.bus.current(metric).is_some(),
            ConditionAtom::EvidenceCountExceeds {
                kind,
                window,
                threshold,
            } => {
                let Some(observer) = ctx.faction else {
                    return false;
                };
                let count = ctx
                    .bus
                    .evidence_of_kind(kind, observer, ctx.now, *window)
                    .count();
                count > *threshold
            }
            ConditionAtom::Compare { left, op, right } => {
                let l = left.evaluate_value(ctx);
                let r = right.evaluate_value(ctx);
                match (l, r) {
                    (Value::Number(a), Value::Number(b)) => compare_f64(a, *op, b),
                    _ => false,
                }
            }
            ConditionAtom::ValueMissing(expr) => expr.evaluate_value(ctx).is_missing(),
            ConditionAtom::MetricStale { metric, max_age } => {
                // Undeclared / never emitted is treated as "infinitely stale".
                match ctx.bus.latest_at(metric) {
                    Some(latest_at) => ctx.now - latest_at > *max_age,
                    None => true,
                }
            }
            ConditionAtom::EvidenceRateAbove {
                kind,
                window,
                rate_per_tick,
            } => {
                let Some(observer) = ctx.faction else {
                    return false;
                };
                if *window <= 0 {
                    return false;
                }
                let count = ctx
                    .bus
                    .evidence_of_kind(kind, observer, ctx.now, *window)
                    .count();
                let rate = count as f64 / *window as f64;
                rate > *rate_per_tick
            }
            ConditionAtom::StandingBelow { target, threshold } => {
                evaluate_standing(ctx, *target, |ps| ps.inferred_standing < *threshold)
            }
            ConditionAtom::StandingAbove { target, threshold } => {
                evaluate_standing(ctx, *target, |ps| ps.inferred_standing > *threshold)
            }
            ConditionAtom::StandingConfidenceAbove { target, threshold } => {
                evaluate_standing(ctx, *target, |ps| ps.confidence > *threshold)
            }
        }
    }

    pub fn collect_deps(&self, deps: &mut Dependencies) {
        match self {
            ConditionAtom::MetricAbove { metric, .. }
            | ConditionAtom::MetricBelow { metric, .. }
            | ConditionAtom::MetricPresent { metric }
            | ConditionAtom::MetricStale { metric, .. } => {
                deps.metrics.push(metric.clone());
            }
            ConditionAtom::EvidenceCountExceeds { kind, .. }
            | ConditionAtom::EvidenceRateAbove { kind, .. } => {
                deps.evidence.push(kind.clone());
            }
            ConditionAtom::Compare { left, right, .. } => {
                left.collect_deps(deps);
                right.collect_deps(deps);
            }
            ConditionAtom::ValueMissing(expr) => expr.collect_deps(deps),
            // Standing atoms depend on all evidence currently on the bus; at
            // this granularity we cannot list specific kinds up front (the
            // aggregator consults all declared kinds via StandingConfig). We
            // conservatively contribute no explicit deps here — callers that
            // want precondition caching for standing must invalidate on any
            // evidence emit.
            ConditionAtom::StandingBelow { .. }
            | ConditionAtom::StandingAbove { .. }
            | ConditionAtom::StandingConfidenceAbove { .. } => {}
        }
    }
}

fn compare_f64(a: f64, op: CompareOp, b: f64) -> bool {
    match op {
        CompareOp::Eq => (a - b).abs() <= COMPARE_EPSILON,
        CompareOp::NotEq => (a - b).abs() > COMPARE_EPSILON,
        CompareOp::Lt => a < b,
        CompareOp::Le => a <= b,
        CompareOp::Gt => a > b,
        CompareOp::Ge => a >= b,
    }
}

/// Helper for standing atoms: verifies ctx has the required refs, computes
/// the perceived standing, and applies `predicate`. Returns `false` if any
/// required ref is missing (warning logged once per session).
fn evaluate_standing<F>(ctx: &EvalContext, target: FactionId, predicate: F) -> bool
where
    F: FnOnce(&standing::PerceivedStanding) -> bool,
{
    let Some(observer) = ctx.faction else {
        warn_standing_ctx_missing("faction");
        return false;
    };
    let Some(cfg) = ctx.standing_config else {
        warn_standing_ctx_missing("standing_config");
        return false;
    };
    let Some(params) = ctx.ai_params else {
        warn_standing_ctx_missing("ai_params");
        return false;
    };
    let ps = standing::compute(
        ctx.bus,
        observer,
        target,
        StandingSubject::ObserverSelf,
        ctx.now,
        cfg,
        params,
    );
    predicate(&ps)
}

fn warn_standing_ctx_missing(field: &str) {
    use std::sync::OnceLock;
    // Separate OnceLock per field so each missing field warns once.
    static FACTION: OnceLock<()> = OnceLock::new();
    static CFG: OnceLock<()> = OnceLock::new();
    static PARAMS: OnceLock<()> = OnceLock::new();
    let cell = match field {
        "faction" => &FACTION,
        "standing_config" => &CFG,
        "ai_params" => &PARAMS,
        _ => return,
    };
    let _ = cell.get_or_init(|| {
        log::warn!(
            "condition: standing atom evaluated without ctx.{field}; returning false. Further warnings for this field will be suppressed."
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::AiBus;
    use crate::evidence::StandingEvidence;
    use crate::ids::FactionId;
    use crate::retention::Retention;
    use crate::spec::{EvidenceSpec, MetricSpec};
    use crate::value_expr::MetricRef;
    use crate::warning::WarningMode;

    fn bus() -> AiBus {
        AiBus::with_warning_mode(WarningMode::Silent)
    }

    #[test]
    fn always_never() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        assert!(Condition::Always.evaluate(&ctx));
        assert!(!Condition::Never.evaluate(&ctx));
    }

    #[test]
    fn all_and_any() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        let t = Condition::Always;
        let f = Condition::Never;
        assert!(Condition::All(vec![t.clone(), t.clone()]).evaluate(&ctx));
        assert!(!Condition::All(vec![t.clone(), f.clone()]).evaluate(&ctx));
        assert!(Condition::Any(vec![t.clone(), f.clone()]).evaluate(&ctx));
        assert!(!Condition::Any(vec![f.clone(), f.clone()]).evaluate(&ctx));
    }

    #[test]
    fn one_of() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        let t = Condition::Always;
        let f = Condition::Never;
        assert!(Condition::OneOf(vec![t.clone(), f.clone()]).evaluate(&ctx));
        assert!(!Condition::OneOf(vec![t.clone(), t.clone()]).evaluate(&ctx));
        assert!(!Condition::OneOf(vec![f.clone(), f.clone()]).evaluate(&ctx));
    }

    #[test]
    fn not_inverts() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        assert!(Condition::not(Condition::Never).evaluate(&ctx));
        assert!(!Condition::not(Condition::Always).evaluate(&ctx));
    }

    #[test]
    fn metric_above_below_present() {
        let mut b = bus();
        let id = MetricId::from("x");
        b.declare_metric(id.clone(), MetricSpec::gauge(Retention::Short, "x"));
        b.emit(&id, 0.7, 10);
        let ctx = EvalContext::new(&b, 10);
        assert!(
            ConditionAtom::MetricAbove {
                metric: id.clone(),
                threshold: 0.5
            }
            .evaluate(&ctx)
        );
        assert!(
            ConditionAtom::MetricBelow {
                metric: id.clone(),
                threshold: 1.0
            }
            .evaluate(&ctx)
        );
        assert!(ConditionAtom::MetricPresent { metric: id.clone() }.evaluate(&ctx));
        assert!(
            !ConditionAtom::MetricPresent {
                metric: MetricId::from("y"),
            }
            .evaluate(&ctx)
        );
    }

    #[test]
    fn evidence_count_requires_faction_context() {
        let mut b = bus();
        let kind = EvidenceKindId::from("hostile");
        b.declare_evidence(kind.clone(), EvidenceSpec::new(Retention::Long, "h"));
        for t in 0..5 {
            b.emit_evidence(StandingEvidence::new(
                kind.clone(),
                FactionId(1),
                FactionId(2),
                1.0,
                t * 10,
            ));
        }
        let atom = ConditionAtom::EvidenceCountExceeds {
            kind: kind.clone(),
            window: 100,
            threshold: 3,
        };
        let ctx_no = EvalContext::new(&b, 50);
        assert!(!atom.evaluate(&ctx_no));
        let ctx_yes = EvalContext::new(&b, 50).with_faction(FactionId(1));
        assert!(atom.evaluate(&ctx_yes));
        let ctx_other = EvalContext::new(&b, 50).with_faction(FactionId(2));
        assert!(!atom.evaluate(&ctx_other));
    }

    #[test]
    fn compare_missing_side_is_false() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        let c = Condition::gt(ValueExpr::Missing, ValueExpr::Literal(0.0));
        assert!(!c.evaluate(&ctx));
        let c2 = Condition::gt(ValueExpr::Literal(5.0), ValueExpr::Missing);
        assert!(!c2.evaluate(&ctx));
    }

    #[test]
    fn compare_eq_within_epsilon() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        // Introduce a tiny rounding error.
        let a = 0.1 + 0.2;
        let c = Condition::eq(ValueExpr::Literal(a), ValueExpr::Literal(0.3));
        assert!(c.evaluate(&ctx));
        let c2 = Condition::eq(ValueExpr::Literal(1.0), ValueExpr::Literal(2.0));
        assert!(!c2.evaluate(&ctx));
    }

    #[test]
    fn compare_basic_ops() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        assert!(Condition::gt(ValueExpr::Literal(2.0), ValueExpr::Literal(1.0)).evaluate(&ctx));
        assert!(Condition::ge(ValueExpr::Literal(2.0), ValueExpr::Literal(2.0)).evaluate(&ctx));
        assert!(Condition::lt(ValueExpr::Literal(1.0), ValueExpr::Literal(2.0)).evaluate(&ctx));
        assert!(Condition::le(ValueExpr::Literal(2.0), ValueExpr::Literal(2.0)).evaluate(&ctx));
    }

    #[test]
    fn value_missing_atom() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        assert!(Condition::Atom(ConditionAtom::ValueMissing(ValueExpr::Missing)).evaluate(&ctx));
        assert!(
            !Condition::Atom(ConditionAtom::ValueMissing(ValueExpr::Literal(1.0))).evaluate(&ctx)
        );
    }

    #[test]
    fn metric_stale_age_threshold() {
        let mut b = bus();
        let id = MetricId::from("m");
        b.declare_metric(id.clone(), MetricSpec::gauge(Retention::Long, "m"));
        b.emit(&id, 1.0, 10);
        // now=50, latest_at=10, age=40
        let ctx = EvalContext::new(&b, 50);
        let fresh = Condition::Atom(ConditionAtom::MetricStale {
            metric: id.clone(),
            max_age: 100,
        });
        assert!(!fresh.evaluate(&ctx));
        let stale = Condition::Atom(ConditionAtom::MetricStale {
            metric: id.clone(),
            max_age: 30,
        });
        assert!(stale.evaluate(&ctx));
        let never = Condition::Atom(ConditionAtom::MetricStale {
            metric: MetricId::from("never"),
            max_age: 10,
        });
        assert!(never.evaluate(&ctx));
    }

    #[test]
    fn evidence_rate_above() {
        let mut b = bus();
        let kind = EvidenceKindId::from("k");
        b.declare_evidence(kind.clone(), EvidenceSpec::new(Retention::Long, "k"));
        for t in 0..10 {
            b.emit_evidence(StandingEvidence::new(
                kind.clone(),
                FactionId(1),
                FactionId(2),
                1.0,
                t,
            ));
        }
        // 10 events over window 100 → rate = 0.1
        let ctx = EvalContext::new(&b, 100).with_faction(FactionId(1));
        let hit = Condition::Atom(ConditionAtom::EvidenceRateAbove {
            kind: kind.clone(),
            window: 100,
            rate_per_tick: 0.05,
        });
        assert!(hit.evaluate(&ctx));
        let miss = Condition::Atom(ConditionAtom::EvidenceRateAbove {
            kind: kind.clone(),
            window: 100,
            rate_per_tick: 0.5,
        });
        assert!(!miss.evaluate(&ctx));
        // No faction → false.
        let ctx_no = EvalContext::new(&b, 100);
        assert!(!hit.evaluate(&ctx_no));
    }

    #[test]
    fn metric_ratio_ge_avoids_divide_by_zero() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        let c = Condition::metric_ratio_ge(ValueExpr::Literal(0.0), ValueExpr::Literal(0.0), 0.7);
        // 0 >= 0*0.7=0 → true
        assert!(c.evaluate(&ctx));
        let c2 = Condition::metric_ratio_ge(ValueExpr::Literal(5.0), ValueExpr::Literal(10.0), 0.7);
        // 5 >= 10*0.7=7 → false
        assert!(!c2.evaluate(&ctx));
    }

    #[test]
    fn metric_trend_up_detects_growth() {
        let mut b = bus();
        let id = MetricId::from("m");
        b.declare_metric(id.clone(), MetricSpec::gauge(Retention::Long, "m"));
        b.emit(&id, 1.0, 0);
        b.emit(&id, 3.0, 100);
        let ctx = EvalContext::new(&b, 100);
        assert!(Condition::metric_trend_up(id.clone(), 100).evaluate(&ctx));
    }

    #[test]
    fn collect_deps_on_conditions() {
        let c = Condition::All(vec![
            Condition::Atom(ConditionAtom::MetricAbove {
                metric: MetricId::from("a"),
                threshold: 0.0,
            }),
            Condition::gt(
                ValueExpr::Metric(MetricRef::new(MetricId::from("b"))),
                ValueExpr::Literal(0.0),
            ),
        ]);
        let mut deps = Dependencies::new();
        c.collect_deps(&mut deps);
        assert_eq!(deps.metrics.len(), 2);
    }

    mod standing_atoms {
        use super::*;
        use crate::ai_params::AiParamsExt;
        use crate::standing::{EvidenceKindConfig, StandingConfig};
        use std::collections::HashMap;

        #[derive(Default)]
        struct StubParams {
            map: HashMap<String, f64>,
        }

        impl AiParamsExt for StubParams {
            fn ai_param_f64(&self, key: &str, default: f64) -> f64 {
                self.map.get(key).copied().unwrap_or(default)
            }
        }

        #[test]
        fn standing_below_atom_true_when_score_under_threshold() {
            let mut b = bus();
            let kind = EvidenceKindId::from("attack");
            b.declare_evidence(kind.clone(), EvidenceSpec::new(Retention::Long, "a"));
            b.emit_evidence(StandingEvidence::new(
                kind.clone(),
                FactionId(1),
                FactionId(2),
                1.0,
                10,
            ));
            let mut cfg = StandingConfig::default();
            cfg.kinds.insert(
                kind,
                EvidenceKindConfig {
                    base_weight: -0.5,
                    ambiguous: false,
                    interpretation_key: None,
                },
            );
            cfg.use_personality_decay = false;
            let params = StubParams::default();
            let ctx = EvalContext::new(&b, 10)
                .with_faction(FactionId(1))
                .with_standing_config(&cfg)
                .with_ai_params(&params);
            let atom = ConditionAtom::StandingBelow {
                target: FactionId(2),
                threshold: -0.1,
            };
            assert!(atom.evaluate(&ctx));
            let atom_high = ConditionAtom::StandingAbove {
                target: FactionId(2),
                threshold: 0.0,
            };
            assert!(!atom_high.evaluate(&ctx));
        }

        #[test]
        fn standing_atoms_false_without_config_on_ctx() {
            let b = bus();
            let atom = ConditionAtom::StandingBelow {
                target: FactionId(2),
                threshold: 0.0,
            };
            let ctx_no_faction = EvalContext::new(&b, 0);
            assert!(!atom.evaluate(&ctx_no_faction));
            let ctx_only_faction = EvalContext::new(&b, 0).with_faction(FactionId(1));
            assert!(!atom.evaluate(&ctx_only_faction));
            let cfg = StandingConfig::default();
            let ctx_no_params = EvalContext::new(&b, 0)
                .with_faction(FactionId(1))
                .with_standing_config(&cfg);
            assert!(!atom.evaluate(&ctx_no_params));
        }

        #[test]
        fn standing_confidence_above_atom_works() {
            let mut b = bus();
            let kind = EvidenceKindId::from("signal");
            b.declare_evidence(kind.clone(), EvidenceSpec::new(Retention::Long, "s"));
            for t in 0..30 {
                b.emit_evidence(StandingEvidence::new(
                    kind.clone(),
                    FactionId(1),
                    FactionId(2),
                    0.01,
                    t,
                ));
            }
            let mut cfg = StandingConfig::default();
            cfg.kinds.insert(
                kind,
                EvidenceKindConfig {
                    base_weight: 0.1,
                    ambiguous: false,
                    interpretation_key: None,
                },
            );
            let params = StubParams::default();
            let ctx = EvalContext::new(&b, 30)
                .with_faction(FactionId(1))
                .with_standing_config(&cfg)
                .with_ai_params(&params);
            let atom = ConditionAtom::StandingConfidenceAbove {
                target: FactionId(2),
                threshold: 0.5,
            };
            assert!(atom.evaluate(&ctx));
        }
    }
}
