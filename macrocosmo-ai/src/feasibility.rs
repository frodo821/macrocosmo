//! Feasibility formula: how an `Objective` is scored against the current
//! bus state. Pure data + pure evaluator.
//!
//! `WeightedSum` is the canonical formula — a linear combination of
//! `ValueExpr` terms. `Custom(ScriptRef)` is a Phase-later stub.
//!
//! `prior` is reserved for future EMA-style smoothing. Phase 2 ignores it.

use serde::{Deserialize, Serialize};

use crate::bus::AiBus;
use crate::eval::EvalContext;
use crate::time::Tick;
use crate::value_expr::{ScriptRef, Value, ValueExpr};

/// A single weighted term in a `WeightedSum` formula.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeasibilityTerm {
    pub weight: f64,
    pub expr: ValueExpr,
}

impl FeasibilityTerm {
    pub fn new(weight: f64, expr: ValueExpr) -> Self {
        Self { weight, expr }
    }
}

/// Feasibility formula. Evaluated against an `AiBus` + `now`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FeasibilityFormula {
    /// Linear combination of weighted `ValueExpr` terms.
    WeightedSum(Vec<FeasibilityTerm>),
    /// External script reference. Phase 2 stub returns `0.0`.
    Custom(ScriptRef),
}

/// Evaluate a feasibility formula.
///
/// `prior` (reserved) is not used in Phase 2; future phases may combine the
/// fresh score with a stored prior via an EMA or similar smoother.
pub fn evaluate(
    formula: &FeasibilityFormula,
    bus: &AiBus,
    now: Tick,
    _prior: Option<f64>,
) -> f64 {
    let ctx = EvalContext::new(bus, now);
    match formula {
        FeasibilityFormula::WeightedSum(terms) => terms
            .iter()
            .filter_map(|t| match t.expr.evaluate_value(&ctx) {
                Value::Number(v) => Some(t.weight * v),
                Value::Missing => None,
            })
            .sum(),
        FeasibilityFormula::Custom(_) => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::MetricId;
    use crate::retention::Retention;
    use crate::spec::MetricSpec;
    use crate::value_expr::MetricRef;
    use crate::warning::WarningMode;

    fn bus() -> AiBus {
        AiBus::with_warning_mode(WarningMode::Silent)
    }

    #[test]
    fn empty_weighted_sum_is_zero() {
        let b = bus();
        let f = FeasibilityFormula::WeightedSum(vec![]);
        assert_eq!(evaluate(&f, &b, 0, None), 0.0);
    }

    #[test]
    fn weighted_sum_of_literals() {
        let b = bus();
        let f = FeasibilityFormula::WeightedSum(vec![
            FeasibilityTerm::new(0.5, ValueExpr::Literal(2.0)),
            FeasibilityTerm::new(0.25, ValueExpr::Literal(4.0)),
        ]);
        // 0.5*2 + 0.25*4 = 1.0 + 1.0 = 2.0
        assert_eq!(evaluate(&f, &b, 0, None), 2.0);
    }

    #[test]
    fn weighted_sum_over_metrics() {
        let mut b = bus();
        let readiness = MetricId::from("readiness");
        let force = MetricId::from("force");
        b.declare_metric(
            readiness.clone(),
            MetricSpec::ratio(Retention::Short, "r"),
        );
        b.declare_metric(force.clone(), MetricSpec::gauge(Retention::Short, "f"));
        b.emit(&readiness, 0.8, 10);
        b.emit(&force, 100.0, 10);
        let f = FeasibilityFormula::WeightedSum(vec![
            FeasibilityTerm::new(1.0, ValueExpr::Metric(MetricRef::new(readiness))),
            FeasibilityTerm::new(
                0.01,
                ValueExpr::Metric(MetricRef::new(force)),
            ),
        ]);
        // 1.0*0.8 + 0.01*100 = 0.8 + 1.0 = 1.8
        assert!((evaluate(&f, &b, 10, None) - 1.8).abs() < 1e-9);
    }

    #[test]
    fn custom_formula_is_stub_zero() {
        let b = bus();
        let f = FeasibilityFormula::Custom(ScriptRef::from("myscript"));
        assert_eq!(evaluate(&f, &b, 0, None), 0.0);
    }

    #[test]
    fn weighted_sum_skips_missing_term() {
        let b = bus();
        // Missing term contributes 0; other terms dominate.
        let f = FeasibilityFormula::WeightedSum(vec![
            FeasibilityTerm::new(1.0, ValueExpr::Literal(2.0)),
            FeasibilityTerm::new(100.0, ValueExpr::Missing),
            FeasibilityTerm::new(1.0, ValueExpr::Literal(3.0)),
        ]);
        assert_eq!(evaluate(&f, &b, 0, None), 5.0);
    }

    #[test]
    fn prior_is_ignored_in_phase2() {
        let b = bus();
        let f = FeasibilityFormula::WeightedSum(vec![FeasibilityTerm::new(
            1.0,
            ValueExpr::Literal(0.5),
        )]);
        assert_eq!(evaluate(&f, &b, 0, None), 0.5);
        assert_eq!(evaluate(&f, &b, 0, Some(0.9)), 0.5);
    }
}
