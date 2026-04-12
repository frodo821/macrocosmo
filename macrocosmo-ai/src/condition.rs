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
use crate::ids::{EvidenceKindId, FactionId, MetricId};
use crate::standing::{self, StandingSubject};
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
    /// True iff the observer's perceived standing toward `target` is below
    /// `threshold`. Requires `ctx.faction`, `ctx.standing_config`, and
    /// `ctx.ai_params`; returns `false` (with a one-shot warning) otherwise.
    StandingBelow {
        target: FactionId,
        threshold: f64,
    },
    /// True iff the observer's perceived standing toward `target` is above
    /// `threshold`. Same ctx requirements as `StandingBelow`.
    StandingAbove {
        target: FactionId,
        threshold: f64,
    },
    /// True iff the observer's standing confidence toward `target` is above
    /// `threshold`. Same ctx requirements as `StandingBelow`.
    StandingConfidenceAbove {
        target: FactionId,
        threshold: f64,
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
