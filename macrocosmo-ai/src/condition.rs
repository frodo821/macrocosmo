//! Condition tree and evaluator.
//!
//! A `Condition` is a boolean expression over the AI bus. It has no
//! side effects and is evaluated via `evaluate(&EvalContext)`.
//!
//! Phase 1 ships a narrow atom vocabulary sufficient to express
//! preconditions and simple feasibility gates. The atom set is open — more
//! atoms can be added without touching the tree combinators.

use serde::{Deserialize, Serialize};

use crate::eval::EvalContext;
use crate::ids::{EvidenceKindId, MetricId};
use crate::time::Tick;

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

    pub fn evaluate(&self, ctx: &EvalContext) -> bool {
        match self {
            Condition::Always => true,
            Condition::Never => false,
            Condition::All(children) => children.iter().all(|c| c.evaluate(ctx)),
            Condition::Any(children) => children.iter().any(|c| c.evaluate(ctx)),
            Condition::OneOf(children) => {
                children.iter().filter(|c| c.evaluate(ctx)).count() == 1
            }
            Condition::Not(inner) => !inner.evaluate(ctx),
            Condition::Atom(a) => a.evaluate(ctx),
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::AiBus;
    use crate::evidence::StandingEvidence;
    use crate::ids::FactionId;
    use crate::retention::Retention;
    use crate::spec::{EvidenceSpec, MetricSpec};
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
        assert!(ConditionAtom::MetricAbove {
            metric: id.clone(),
            threshold: 0.5
        }
        .evaluate(&ctx));
        assert!(ConditionAtom::MetricBelow {
            metric: id.clone(),
            threshold: 1.0
        }
        .evaluate(&ctx));
        assert!(ConditionAtom::MetricPresent { metric: id.clone() }.evaluate(&ctx));
        assert!(!ConditionAtom::MetricPresent {
            metric: MetricId::from("y"),
        }
        .evaluate(&ctx));
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
        // Without faction set in ctx -> false
        let ctx_no = EvalContext::new(&b, 50);
        assert!(!atom.evaluate(&ctx_no));
        // With faction=1 matching observer -> 5 entries > 3
        let ctx_yes = EvalContext::new(&b, 50).with_faction(FactionId(1));
        assert!(atom.evaluate(&ctx_yes));
        // With faction=2 (no entries) -> false
        let ctx_other = EvalContext::new(&b, 50).with_faction(FactionId(2));
        assert!(!atom.evaluate(&ctx_other));
    }
}
