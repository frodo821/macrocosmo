//! Value expressions — `f64`-valued (plus `Missing`) computations over the AI bus.
//!
//! `ValueExpr` is the building block for feasibility formulas and other
//! numeric reasoning. It is a pure evaluator.
//!
//! Phase 2 supports literals, metric lookups, arithmetic composition, clamping,
//! and the DelT lookback operator. Phase 3 (#192) adds:
//! - `Value` three-valued semantics (`Number | Missing`)
//! - Richer arithmetic algebra (`Sub`, `Div`, `Neg`, `Min`, `Max`, `Abs`)
//! - Conditional branching (`IfThenElse`) via [`Condition`]
//! - Window aggregates over metric history
//! - Dependency collection for cache invalidation
//!
//! # Missing propagation
//!
//! `Missing` propagates in the direction with zero downstream impact:
//! - Variadic ops (`Add`, `Mul`, `Min`, `Max`) skip `Missing` children; an
//!   all-`Missing` list yields `Missing`.
//! - Unary ops (`Neg`, `Abs`) propagate `Missing`.
//! - `Sub(a, b)` yields `a` when `b` is `Missing` (right-side Missing ≈ zero
//!   delta); left-side Missing propagates.
//! - `Div { num, den }` with `Missing`/zero denominator yields `Missing`.
//!   `Missing` numerator also yields `Missing`.
//! - `Clamp` propagates `Missing`.
//! - Window aggregates over empty windows return `Missing`.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::condition::Condition;
use crate::eval::EvalContext;
use crate::ids::{EvidenceKindId, MetricId};
use crate::time::Tick;

/// Reference to a metric by id. Wraps `MetricId` so that future variants
/// (e.g. faction-scoped or kind-qualified lookups) can be added without
/// breaking call sites.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MetricRef {
    pub id: MetricId,
}

impl MetricRef {
    pub fn new(id: MetricId) -> Self {
        Self { id }
    }
}

impl From<MetricId> for MetricRef {
    fn from(id: MetricId) -> Self {
        Self { id }
    }
}

/// Opaque reference to a script function evaluated outside of ai_core.
/// Phase 2 stub — `Custom` variants return `Missing`; future phases resolve
/// these via a scripting bridge (#192 / #198).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScriptRef(pub Arc<str>);

impl From<&str> for ScriptRef {
    fn from(s: &str) -> Self {
        Self(Arc::from(s))
    }
}

/// Three-valued result of evaluating a `ValueExpr`.
///
/// `Missing` represents "information is unavailable" (undeclared metric,
/// empty history window, division by zero, …). Evaluators propagate it
/// along the branch where it has zero downstream impact.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Number(f64),
    Missing,
}

impl Value {
    /// Access the underlying number, if any.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            Value::Missing => None,
        }
    }

    /// Convert to `f64`, mapping `Missing` to `0.0`.
    pub fn or_zero(self) -> f64 {
        match self {
            Value::Number(n) => n,
            Value::Missing => 0.0,
        }
    }

    /// Whether this value is `Missing`.
    pub fn is_missing(&self) -> bool {
        matches!(self, Value::Missing)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Number(v)
    }
}

/// An expression producing a [`Value`] when evaluated against the bus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ValueExpr {
    /// Constant literal.
    Literal(f64),
    /// Always `Missing`. Useful as a placeholder / explicit "unknown".
    Missing,
    /// Current value of a metric. `Missing` if undeclared or no samples.
    Metric(MetricRef),
    /// `current(metric) - value_at_or_before(now - window)` — the change
    /// over the given lookback window. `Missing` if either side is missing.
    DelT { metric: MetricRef, window: Tick },
    /// Sum of children. `Missing` children are skipped; all-`Missing` or
    /// empty list yields `Missing`.
    Add(Vec<ValueExpr>),
    /// Product of children. `Missing` children are skipped; all-`Missing`
    /// or empty list yields `Missing`.
    Mul(Vec<ValueExpr>),
    /// `left - right`. Missing right → `left`. Missing left → `Missing`.
    Sub(Box<ValueExpr>, Box<ValueExpr>),
    /// `num / den`. Missing either side → `Missing`. Zero den → `Missing`.
    Div {
        num: Box<ValueExpr>,
        den: Box<ValueExpr>,
    },
    /// Negation. Propagates `Missing`.
    Neg(Box<ValueExpr>),
    /// Minimum of children. Missing children are skipped.
    Min(Vec<ValueExpr>),
    /// Maximum of children. Missing children are skipped.
    Max(Vec<ValueExpr>),
    /// Absolute value. Propagates `Missing`.
    Abs(Box<ValueExpr>),
    /// Clamp `expr` into `[lo, hi]`. Propagates `Missing`.
    Clamp {
        expr: Box<ValueExpr>,
        lo: f64,
        hi: f64,
    },
    /// Branch on a [`Condition`]. Picks `then_` or `else_` based on the
    /// condition's boolean result; `Missing` inside the picked branch
    /// propagates through as usual.
    IfThenElse {
        cond: Box<Condition>,
        then_: Box<ValueExpr>,
        else_: Box<ValueExpr>,
    },
    /// Arithmetic mean of samples in `[now - window, now]` for `metric`.
    /// `Missing` if the window is empty.
    WindowAvg { metric: MetricRef, window: Tick },
    /// Minimum sample value in the window. `Missing` if empty.
    WindowMin { metric: MetricRef, window: Tick },
    /// Maximum sample value in the window. `Missing` if empty.
    WindowMax { metric: MetricRef, window: Tick },
    /// Sum of sample values in the window. `Missing` if empty.
    WindowSum { metric: MetricRef, window: Tick },
    /// Count of samples in the window (always a number; `0` is not missing).
    WindowCount { metric: MetricRef, window: Tick },
    /// External script reference. Phase 2 stub returns `Missing`.
    Custom(ScriptRef),
}

impl ValueExpr {
    /// Evaluate to an `f64`, mapping `Missing` to `0.0`. Back-compat wrapper
    /// around [`Self::evaluate_value`].
    pub fn evaluate(&self, ctx: &EvalContext) -> f64 {
        self.evaluate_value(ctx).or_zero()
    }

    /// Evaluate with full three-valued semantics.
    pub fn evaluate_value(&self, ctx: &EvalContext) -> Value {
        match self {
            ValueExpr::Literal(v) => Value::Number(*v),
            ValueExpr::Missing => Value::Missing,
            ValueExpr::Metric(m) => match ctx.bus.current(&m.id) {
                Some(v) => Value::Number(v),
                None => Value::Missing,
            },
            ValueExpr::DelT { metric, window } => {
                let Some(current) = ctx.bus.current(&metric.id) else {
                    return Value::Missing;
                };
                let lookback_at = ctx.now.saturating_sub(*window);
                match ctx.bus.at_or_before(&metric.id, lookback_at) {
                    Some(prev) => Value::Number(current - prev),
                    None => Value::Missing,
                }
            }
            ValueExpr::Add(children) => variadic_collect(children, ctx, 0.0, |acc, v| acc + v),
            ValueExpr::Mul(children) => variadic_collect(children, ctx, 1.0, |acc, v| acc * v),
            ValueExpr::Sub(left, right) => {
                let l = left.evaluate_value(ctx);
                let r = right.evaluate_value(ctx);
                match (l, r) {
                    (Value::Missing, _) => Value::Missing,
                    (Value::Number(a), Value::Missing) => Value::Number(a),
                    (Value::Number(a), Value::Number(b)) => Value::Number(a - b),
                }
            }
            ValueExpr::Div { num, den } => {
                let n = num.evaluate_value(ctx);
                let d = den.evaluate_value(ctx);
                match (n, d) {
                    (Value::Number(a), Value::Number(b)) if b != 0.0 => Value::Number(a / b),
                    _ => Value::Missing,
                }
            }
            ValueExpr::Neg(inner) => match inner.evaluate_value(ctx) {
                Value::Number(v) => Value::Number(-v),
                Value::Missing => Value::Missing,
            },
            ValueExpr::Min(children) => variadic_fold(children, ctx, f64::min),
            ValueExpr::Max(children) => variadic_fold(children, ctx, f64::max),
            ValueExpr::Abs(inner) => match inner.evaluate_value(ctx) {
                Value::Number(v) => Value::Number(v.abs()),
                Value::Missing => Value::Missing,
            },
            ValueExpr::Clamp { expr, lo, hi } => match expr.evaluate_value(ctx) {
                Value::Number(v) => Value::Number(v.max(*lo).min(*hi)),
                Value::Missing => Value::Missing,
            },
            ValueExpr::IfThenElse { cond, then_, else_ } => {
                if cond.evaluate(ctx) {
                    then_.evaluate_value(ctx)
                } else {
                    else_.evaluate_value(ctx)
                }
            }
            ValueExpr::WindowAvg { metric, window } => {
                let samples: Vec<f64> = ctx
                    .bus
                    .window(&metric.id, ctx.now, *window)
                    .map(|tv| tv.value)
                    .collect();
                if samples.is_empty() {
                    Value::Missing
                } else {
                    let sum: f64 = samples.iter().sum();
                    Value::Number(sum / samples.len() as f64)
                }
            }
            ValueExpr::WindowMin { metric, window } => {
                let mut min: Option<f64> = None;
                for tv in ctx.bus.window(&metric.id, ctx.now, *window) {
                    min = Some(min.map_or(tv.value, |m| m.min(tv.value)));
                }
                min.map(Value::Number).unwrap_or(Value::Missing)
            }
            ValueExpr::WindowMax { metric, window } => {
                let mut max: Option<f64> = None;
                for tv in ctx.bus.window(&metric.id, ctx.now, *window) {
                    max = Some(max.map_or(tv.value, |m| m.max(tv.value)));
                }
                max.map(Value::Number).unwrap_or(Value::Missing)
            }
            ValueExpr::WindowSum { metric, window } => {
                let mut sum = 0.0f64;
                let mut any = false;
                for tv in ctx.bus.window(&metric.id, ctx.now, *window) {
                    sum += tv.value;
                    any = true;
                }
                if any {
                    Value::Number(sum)
                } else {
                    Value::Missing
                }
            }
            ValueExpr::WindowCount { metric, window } => {
                let count = ctx.bus.window(&metric.id, ctx.now, *window).count();
                Value::Number(count as f64)
            }
            ValueExpr::Custom(_) => Value::Missing,
        }
    }

    /// Walk the expression tree collecting (topic, kind) dependencies used
    /// for cache invalidation. Metric and window-aggregate reads record
    /// their metric ids; `IfThenElse` recurses into the condition as well
    /// as both branches.
    pub fn collect_deps(&self, deps: &mut Dependencies) {
        match self {
            ValueExpr::Literal(_) | ValueExpr::Missing | ValueExpr::Custom(_) => {}
            ValueExpr::Metric(m)
            | ValueExpr::DelT { metric: m, .. }
            | ValueExpr::WindowAvg { metric: m, .. }
            | ValueExpr::WindowMin { metric: m, .. }
            | ValueExpr::WindowMax { metric: m, .. }
            | ValueExpr::WindowSum { metric: m, .. }
            | ValueExpr::WindowCount { metric: m, .. } => {
                deps.metrics.push(m.id.clone());
            }
            ValueExpr::Add(children)
            | ValueExpr::Mul(children)
            | ValueExpr::Min(children)
            | ValueExpr::Max(children) => {
                for c in children {
                    c.collect_deps(deps);
                }
            }
            ValueExpr::Sub(a, b) => {
                a.collect_deps(deps);
                b.collect_deps(deps);
            }
            ValueExpr::Div { num, den } => {
                num.collect_deps(deps);
                den.collect_deps(deps);
            }
            ValueExpr::Neg(inner) | ValueExpr::Abs(inner) => inner.collect_deps(deps),
            ValueExpr::Clamp { expr, .. } => expr.collect_deps(deps),
            ValueExpr::IfThenElse { cond, then_, else_ } => {
                cond.collect_deps(deps);
                then_.collect_deps(deps);
                else_.collect_deps(deps);
            }
        }
    }
}

/// Dependency summary collected by walking a `ValueExpr` / `Condition` tree.
/// Used by the precondition cache to determine when cached results become
/// stale (bus version bumped for a referenced topic).
#[derive(Debug, Default, Clone)]
pub struct Dependencies {
    pub metrics: Vec<MetricId>,
    pub evidence: Vec<EvidenceKindId>,
}

impl Dependencies {
    pub fn new() -> Self {
        Self::default()
    }

    /// Deduplicate in place, preserving first-seen order.
    pub fn dedup(&mut self) {
        let mut seen = ahash::AHashSet::new();
        self.metrics.retain(|m| seen.insert(m.clone()));
        let mut seen2 = ahash::AHashSet::new();
        self.evidence.retain(|e| seen2.insert(e.clone()));
    }
}

fn variadic_collect<F>(children: &[ValueExpr], ctx: &EvalContext, identity: f64, op: F) -> Value
where
    F: Fn(f64, f64) -> f64,
{
    let mut acc = identity;
    let mut any = false;
    for c in children {
        if let Value::Number(v) = c.evaluate_value(ctx) {
            acc = op(acc, v);
            any = true;
        }
    }
    if any {
        Value::Number(acc)
    } else {
        Value::Missing
    }
}

fn variadic_fold<F>(children: &[ValueExpr], ctx: &EvalContext, op: F) -> Value
where
    F: Fn(f64, f64) -> f64,
{
    let mut acc: Option<f64> = None;
    for c in children {
        if let Value::Number(v) = c.evaluate_value(ctx) {
            acc = Some(acc.map_or(v, |a| op(a, v)));
        }
    }
    acc.map(Value::Number).unwrap_or(Value::Missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::AiBus;
    use crate::condition::{Condition, ConditionAtom};
    use crate::retention::Retention;
    use crate::spec::MetricSpec;
    use crate::warning::WarningMode;

    fn bus() -> AiBus {
        AiBus::with_warning_mode(WarningMode::Silent)
    }

    #[test]
    fn literal() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        assert_eq!(ValueExpr::Literal(3.5).evaluate(&ctx), 3.5);
        assert_eq!(
            ValueExpr::Literal(3.5).evaluate_value(&ctx),
            Value::Number(3.5)
        );
    }

    #[test]
    fn missing_literal() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        assert_eq!(ValueExpr::Missing.evaluate_value(&ctx), Value::Missing);
        assert_eq!(ValueExpr::Missing.evaluate(&ctx), 0.0);
    }

    #[test]
    fn metric_lookup_missing_if_undeclared() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        let m = MetricRef::new(MetricId::from("missing"));
        assert_eq!(ValueExpr::Metric(m).evaluate_value(&ctx), Value::Missing);
    }

    #[test]
    fn metric_lookup_returns_current() {
        let mut b = bus();
        let id = MetricId::from("x");
        b.declare_metric(id.clone(), MetricSpec::gauge(Retention::Long, "x"));
        b.emit(&id, 4.2, 10);
        let ctx = EvalContext::new(&b, 10);
        assert_eq!(ValueExpr::Metric(MetricRef::new(id)).evaluate(&ctx), 4.2);
    }

    #[test]
    fn add_and_mul() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        let add = ValueExpr::Add(vec![
            ValueExpr::Literal(1.0),
            ValueExpr::Literal(2.5),
            ValueExpr::Literal(0.5),
        ]);
        assert_eq!(add.evaluate(&ctx), 4.0);
        let mul = ValueExpr::Mul(vec![ValueExpr::Literal(2.0), ValueExpr::Literal(3.0)]);
        assert_eq!(mul.evaluate(&ctx), 6.0);
        // Empty list is Missing in the new semantics (no terms to combine).
        assert_eq!(ValueExpr::Add(vec![]).evaluate_value(&ctx), Value::Missing);
        assert_eq!(ValueExpr::Mul(vec![]).evaluate_value(&ctx), Value::Missing);
    }

    #[test]
    fn add_with_missing_skips_term() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        let add = ValueExpr::Add(vec![
            ValueExpr::Literal(1.0),
            ValueExpr::Missing,
            ValueExpr::Literal(2.0),
        ]);
        assert_eq!(add.evaluate_value(&ctx), Value::Number(3.0));
    }

    #[test]
    fn mul_with_missing_skips_term() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        let mul = ValueExpr::Mul(vec![
            ValueExpr::Literal(2.0),
            ValueExpr::Missing,
            ValueExpr::Literal(3.0),
        ]);
        assert_eq!(mul.evaluate_value(&ctx), Value::Number(6.0));
    }

    #[test]
    fn sub_missing_right_yields_left() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        let e = ValueExpr::Sub(
            Box::new(ValueExpr::Literal(5.0)),
            Box::new(ValueExpr::Missing),
        );
        assert_eq!(e.evaluate_value(&ctx), Value::Number(5.0));
    }

    #[test]
    fn sub_missing_left_propagates() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        let e = ValueExpr::Sub(
            Box::new(ValueExpr::Missing),
            Box::new(ValueExpr::Literal(3.0)),
        );
        assert_eq!(e.evaluate_value(&ctx), Value::Missing);
    }

    #[test]
    fn value_div_by_zero_is_missing() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        let e = ValueExpr::Div {
            num: Box::new(ValueExpr::Literal(10.0)),
            den: Box::new(ValueExpr::Literal(0.0)),
        };
        assert_eq!(e.evaluate_value(&ctx), Value::Missing);
    }

    #[test]
    fn div_missing_side_is_missing() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        let e = ValueExpr::Div {
            num: Box::new(ValueExpr::Missing),
            den: Box::new(ValueExpr::Literal(2.0)),
        };
        assert_eq!(e.evaluate_value(&ctx), Value::Missing);
    }

    #[test]
    fn div_normal() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        let e = ValueExpr::Div {
            num: Box::new(ValueExpr::Literal(10.0)),
            den: Box::new(ValueExpr::Literal(4.0)),
        };
        assert_eq!(e.evaluate_value(&ctx), Value::Number(2.5));
    }

    #[test]
    fn neg_and_abs() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        assert_eq!(
            ValueExpr::Neg(Box::new(ValueExpr::Literal(3.0))).evaluate_value(&ctx),
            Value::Number(-3.0)
        );
        assert_eq!(
            ValueExpr::Abs(Box::new(ValueExpr::Literal(-4.5))).evaluate_value(&ctx),
            Value::Number(4.5)
        );
        assert!(
            ValueExpr::Neg(Box::new(ValueExpr::Missing))
                .evaluate_value(&ctx)
                .is_missing()
        );
        assert!(
            ValueExpr::Abs(Box::new(ValueExpr::Missing))
                .evaluate_value(&ctx)
                .is_missing()
        );
    }

    #[test]
    fn value_min_max_of_empty_is_missing() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        assert!(ValueExpr::Min(vec![]).evaluate_value(&ctx).is_missing());
        assert!(ValueExpr::Max(vec![]).evaluate_value(&ctx).is_missing());
    }

    #[test]
    fn min_max_skip_missing() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        let min = ValueExpr::Min(vec![
            ValueExpr::Literal(3.0),
            ValueExpr::Missing,
            ValueExpr::Literal(1.0),
            ValueExpr::Literal(2.0),
        ]);
        assert_eq!(min.evaluate_value(&ctx), Value::Number(1.0));
        let max = ValueExpr::Max(vec![
            ValueExpr::Missing,
            ValueExpr::Literal(-5.0),
            ValueExpr::Literal(3.0),
        ]);
        assert_eq!(max.evaluate_value(&ctx), Value::Number(3.0));
    }

    #[test]
    fn clamp_bounds_value() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        let e = ValueExpr::Clamp {
            expr: Box::new(ValueExpr::Literal(5.0)),
            lo: 0.0,
            hi: 1.0,
        };
        assert_eq!(e.evaluate(&ctx), 1.0);
        let e2 = ValueExpr::Clamp {
            expr: Box::new(ValueExpr::Literal(-5.0)),
            lo: 0.0,
            hi: 1.0,
        };
        assert_eq!(e2.evaluate(&ctx), 0.0);
        let e3 = ValueExpr::Clamp {
            expr: Box::new(ValueExpr::Missing),
            lo: 0.0,
            hi: 1.0,
        };
        assert!(e3.evaluate_value(&ctx).is_missing());
    }

    #[test]
    fn custom_is_stub_missing() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        assert_eq!(
            ValueExpr::Custom(ScriptRef::from("x")).evaluate_value(&ctx),
            Value::Missing
        );
        assert_eq!(ValueExpr::Custom(ScriptRef::from("x")).evaluate(&ctx), 0.0);
    }

    #[test]
    fn if_then_else_picks_branch() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        let e = ValueExpr::IfThenElse {
            cond: Box::new(Condition::Always),
            then_: Box::new(ValueExpr::Literal(1.0)),
            else_: Box::new(ValueExpr::Literal(2.0)),
        };
        assert_eq!(e.evaluate_value(&ctx), Value::Number(1.0));
        let e2 = ValueExpr::IfThenElse {
            cond: Box::new(Condition::Never),
            then_: Box::new(ValueExpr::Literal(1.0)),
            else_: Box::new(ValueExpr::Literal(2.0)),
        };
        assert_eq!(e2.evaluate_value(&ctx), Value::Number(2.0));
    }

    #[test]
    fn if_then_else_missing_inside_branch_propagates() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        let e = ValueExpr::IfThenElse {
            cond: Box::new(Condition::Always),
            then_: Box::new(ValueExpr::Missing),
            else_: Box::new(ValueExpr::Literal(2.0)),
        };
        assert!(e.evaluate_value(&ctx).is_missing());
    }

    fn window_bus() -> (AiBus, MetricId) {
        let mut b = bus();
        let id = MetricId::from("w");
        b.declare_metric(id.clone(), MetricSpec::gauge(Retention::Long, "w"));
        b.emit(&id, 1.0, 10);
        b.emit(&id, 3.0, 20);
        b.emit(&id, 5.0, 30);
        b.emit(&id, 7.0, 40);
        (b, id)
    }

    #[test]
    fn window_avg_matches_manual_mean() {
        let (b, id) = window_bus();
        let ctx = EvalContext::new(&b, 40);
        // window 20 -> samples at 20,30,40 = 3,5,7 mean=5
        let e = ValueExpr::WindowAvg {
            metric: MetricRef::new(id),
            window: 20,
        };
        assert_eq!(e.evaluate_value(&ctx), Value::Number(5.0));
    }

    #[test]
    fn window_avg_empty_window_is_missing() {
        let b = bus();
        let id = MetricId::from("nope");
        let ctx = EvalContext::new(&b, 100);
        let e = ValueExpr::WindowAvg {
            metric: MetricRef::new(id),
            window: 50,
        };
        assert_eq!(e.evaluate_value(&ctx), Value::Missing);
    }

    #[test]
    fn window_min_max_sum_count() {
        let (b, id) = window_bus();
        let ctx = EvalContext::new(&b, 40);
        let wmin = ValueExpr::WindowMin {
            metric: MetricRef::new(id.clone()),
            window: 20,
        };
        let wmax = ValueExpr::WindowMax {
            metric: MetricRef::new(id.clone()),
            window: 20,
        };
        let wsum = ValueExpr::WindowSum {
            metric: MetricRef::new(id.clone()),
            window: 20,
        };
        let wcnt = ValueExpr::WindowCount {
            metric: MetricRef::new(id.clone()),
            window: 20,
        };
        assert_eq!(wmin.evaluate_value(&ctx), Value::Number(3.0));
        assert_eq!(wmax.evaluate_value(&ctx), Value::Number(7.0));
        assert_eq!(wsum.evaluate_value(&ctx), Value::Number(15.0));
        assert_eq!(wcnt.evaluate_value(&ctx), Value::Number(3.0));
    }

    #[test]
    fn window_count_empty_is_zero_not_missing() {
        let b = bus();
        let ctx = EvalContext::new(&b, 100);
        let e = ValueExpr::WindowCount {
            metric: MetricRef::new(MetricId::from("nope")),
            window: 50,
        };
        assert_eq!(e.evaluate_value(&ctx), Value::Number(0.0));
    }

    #[test]
    fn delt_basic_delta_across_window() {
        let mut b = bus();
        let id = MetricId::from("x");
        b.declare_metric(id.clone(), MetricSpec::gauge(Retention::Long, "x"));
        b.emit(&id, 10.0, 0);
        b.emit(&id, 15.0, 50);
        b.emit(&id, 30.0, 100);
        let ctx = EvalContext::new(&b, 100);
        let delt = ValueExpr::DelT {
            metric: MetricRef::new(id.clone()),
            window: 100,
        };
        // current=30, at_or_before(now-100)=at_or_before(0)=10 -> 20
        assert_eq!(delt.evaluate(&ctx), 20.0);
    }

    #[test]
    fn delt_uses_at_or_before_for_gap() {
        let mut b = bus();
        let id = MetricId::from("x");
        b.declare_metric(id.clone(), MetricSpec::gauge(Retention::Long, "x"));
        b.emit(&id, 5.0, 10);
        b.emit(&id, 25.0, 100);
        let ctx = EvalContext::new(&b, 100);
        let delt = ValueExpr::DelT {
            metric: MetricRef::new(id.clone()),
            window: 50,
        };
        // current=25, now-window=50, at_or_before(50)=5 -> 20
        assert_eq!(delt.evaluate(&ctx), 20.0);
    }

    #[test]
    fn delt_missing_when_no_prior() {
        let mut b = bus();
        let id = MetricId::from("x");
        b.declare_metric(id.clone(), MetricSpec::gauge(Retention::Long, "x"));
        b.emit(&id, 10.0, 100);
        let ctx = EvalContext::new(&b, 100);
        let delt = ValueExpr::DelT {
            metric: MetricRef::new(id.clone()),
            window: 50,
        };
        // at_or_before(50) -> None -> Missing
        assert_eq!(delt.evaluate_value(&ctx), Value::Missing);
    }

    #[test]
    fn collect_deps_walks_tree() {
        let e = ValueExpr::Add(vec![
            ValueExpr::Metric(MetricRef::new(MetricId::from("a"))),
            ValueExpr::Sub(
                Box::new(ValueExpr::Metric(MetricRef::new(MetricId::from("b")))),
                Box::new(ValueExpr::WindowAvg {
                    metric: MetricRef::new(MetricId::from("c")),
                    window: 10,
                }),
            ),
        ]);
        let mut deps = Dependencies::new();
        e.collect_deps(&mut deps);
        let ids: Vec<String> = deps.metrics.iter().map(|m| m.to_string()).collect();
        assert!(ids.contains(&"a".to_string()));
        assert!(ids.contains(&"b".to_string()));
        assert!(ids.contains(&"c".to_string()));
    }

    #[test]
    fn collect_deps_includes_ifthenelse_branches() {
        let e = ValueExpr::IfThenElse {
            cond: Box::new(Condition::Atom(ConditionAtom::MetricPresent {
                metric: MetricId::from("cond_m"),
            })),
            then_: Box::new(ValueExpr::Metric(MetricRef::new(MetricId::from("then_m")))),
            else_: Box::new(ValueExpr::Metric(MetricRef::new(MetricId::from("else_m")))),
        };
        let mut deps = Dependencies::new();
        e.collect_deps(&mut deps);
        let ids: Vec<String> = deps.metrics.iter().map(|m| m.to_string()).collect();
        assert!(ids.contains(&"cond_m".to_string()));
        assert!(ids.contains(&"then_m".to_string()));
        assert!(ids.contains(&"else_m".to_string()));
    }
}
