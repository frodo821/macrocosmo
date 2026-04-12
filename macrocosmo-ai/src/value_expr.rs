//! Value expressions — `f64`-valued computations over the AI bus.
//!
//! `ValueExpr` is the building block for feasibility formulas and other
//! numeric reasoning. It is a pure evaluator: `evaluate(&EvalContext)` reads
//! the bus and returns a scalar. Phase 2 supports literals, metric lookups,
//! arithmetic composition, clamping, and the DelT lookback operator.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::eval::EvalContext;
use crate::ids::MetricId;
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
/// Phase 2 stub — `Custom` variants return `0.0`; future phases resolve
/// these via a scripting bridge (#192 / #198).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScriptRef(pub Arc<str>);

impl From<&str> for ScriptRef {
    fn from(s: &str) -> Self {
        Self(Arc::from(s))
    }
}

/// An expression producing an `f64` when evaluated against the bus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ValueExpr {
    /// Constant literal.
    Literal(f64),
    /// Current value of a metric. `0.0` if undeclared or no samples.
    Metric(MetricRef),
    /// `current(metric) - value_at_or_before(now - window)` — the change
    /// over the given lookback window. `0.0` if either side is missing.
    ///
    /// Implementation uses `bus.current` for the right-hand side and
    /// `bus.at_or_before(metric, now - window)` for the left. This matches
    /// the #192 DelT semantics at Short/Medium/Long/VeryLong windows.
    DelT { metric: MetricRef, window: Tick },
    /// Sum of children (empty sum = 0.0).
    Add(Vec<ValueExpr>),
    /// Product of children (empty product = 1.0).
    Mul(Vec<ValueExpr>),
    /// Clamp `expr` into `[lo, hi]`.
    Clamp {
        expr: Box<ValueExpr>,
        lo: f64,
        hi: f64,
    },
    /// External script reference. Phase 2 stub returns `0.0`.
    Custom(ScriptRef),
}

impl ValueExpr {
    pub fn evaluate(&self, ctx: &EvalContext) -> f64 {
        match self {
            ValueExpr::Literal(v) => *v,
            ValueExpr::Metric(m) => ctx.bus.current(&m.id).unwrap_or(0.0),
            ValueExpr::DelT { metric, window } => {
                let current = ctx.bus.current(&metric.id).unwrap_or(0.0);
                let lookback_at = ctx.now.saturating_sub(*window);
                let prev = ctx.bus.at_or_before(&metric.id, lookback_at).unwrap_or(0.0);
                current - prev
            }
            ValueExpr::Add(children) => children.iter().map(|c| c.evaluate(ctx)).sum(),
            ValueExpr::Mul(children) => {
                if children.is_empty() {
                    1.0
                } else {
                    children.iter().map(|c| c.evaluate(ctx)).product()
                }
            }
            ValueExpr::Clamp { expr, lo, hi } => {
                let v = expr.evaluate(ctx);
                v.max(*lo).min(*hi)
            }
            ValueExpr::Custom(_) => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::AiBus;
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
    }

    #[test]
    fn metric_lookup_default_zero_if_missing() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        let m = MetricRef::new(MetricId::from("missing"));
        assert_eq!(ValueExpr::Metric(m).evaluate(&ctx), 0.0);
    }

    #[test]
    fn metric_lookup_returns_current() {
        let mut b = bus();
        let id = MetricId::from("x");
        b.declare_metric(id.clone(), MetricSpec::gauge(Retention::Long, "x"));
        b.emit(&id, 4.2, 10);
        let ctx = EvalContext::new(&b, 10);
        assert_eq!(
            ValueExpr::Metric(MetricRef::new(id)).evaluate(&ctx),
            4.2
        );
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
        assert_eq!(ValueExpr::Add(vec![]).evaluate(&ctx), 0.0);
        assert_eq!(ValueExpr::Mul(vec![]).evaluate(&ctx), 1.0);
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
    }

    #[test]
    fn custom_is_stub_zero() {
        let b = bus();
        let ctx = EvalContext::new(&b, 0);
        assert_eq!(ValueExpr::Custom(ScriptRef::from("x")).evaluate(&ctx), 0.0);
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
    fn delt_zero_when_no_prior() {
        let mut b = bus();
        let id = MetricId::from("x");
        b.declare_metric(id.clone(), MetricSpec::gauge(Retention::Long, "x"));
        b.emit(&id, 10.0, 100);
        let ctx = EvalContext::new(&b, 100);
        let delt = ValueExpr::DelT {
            metric: MetricRef::new(id.clone()),
            window: 50,
        };
        // at_or_before(50) -> None -> 0.0, current=10 -> 10
        assert_eq!(delt.evaluate(&ctx), 10.0);
    }
}
