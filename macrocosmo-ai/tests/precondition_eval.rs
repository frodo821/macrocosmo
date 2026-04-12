//! Integration tests for the precondition layer + cache.
//!
//! Exercises every new atom, Missing propagation rules, tracker behavior,
//! and cache hit/miss/invalidation characteristics.

use macrocosmo_ai::{
    precond, severity, AiBus, CompareOp, Condition, ConditionAtom, Dependencies, EvalContext,
    EvidenceKindId, EvidenceSpec, FactionId, MetricId, MetricRef, MetricSpec,
    PreconditionCacheRegistry, PreconditionSet, PreconditionTracker, Retention,
    StandingEvidence, Value, ValueExpr, WarningMode,
};

fn bus() -> AiBus {
    AiBus::with_warning_mode(WarningMode::Silent)
}

fn with_metric(name: &str, value: f64, at: i64) -> (AiBus, MetricId) {
    let mut b = bus();
    let id = MetricId::from(name);
    b.declare_metric(id.clone(), MetricSpec::gauge(Retention::Long, name));
    b.emit(&id, value, at);
    (b, id)
}

#[test]
fn value_algebra_missing_propagation() {
    let b = bus();
    let ctx = EvalContext::new(&b, 0);
    // Sub: right Missing → left
    let e = ValueExpr::Sub(
        Box::new(ValueExpr::Literal(10.0)),
        Box::new(ValueExpr::Missing),
    );
    assert_eq!(e.evaluate_value(&ctx), Value::Number(10.0));
    // Sub: left Missing → Missing
    let e = ValueExpr::Sub(
        Box::new(ValueExpr::Missing),
        Box::new(ValueExpr::Literal(10.0)),
    );
    assert_eq!(e.evaluate_value(&ctx), Value::Missing);

    // Div by zero → Missing
    assert_eq!(
        ValueExpr::Div {
            num: Box::new(ValueExpr::Literal(1.0)),
            den: Box::new(ValueExpr::Literal(0.0)),
        }
        .evaluate_value(&ctx),
        Value::Missing
    );

    // Add skips missing
    assert_eq!(
        ValueExpr::Add(vec![
            ValueExpr::Literal(1.0),
            ValueExpr::Missing,
            ValueExpr::Literal(2.0),
        ])
        .evaluate_value(&ctx),
        Value::Number(3.0)
    );
}

#[test]
fn window_aggregates_end_to_end() {
    let (mut b, id) = with_metric("w", 1.0, 10);
    b.emit(&id, 3.0, 20);
    b.emit(&id, 7.0, 30);
    let ctx = EvalContext::new(&b, 30);
    let avg = ValueExpr::WindowAvg {
        metric: MetricRef::new(id.clone()),
        window: 30,
    };
    assert_eq!(avg.evaluate_value(&ctx), Value::Number((1.0 + 3.0 + 7.0) / 3.0));
    let sum = ValueExpr::WindowSum {
        metric: MetricRef::new(id.clone()),
        window: 30,
    };
    assert_eq!(sum.evaluate_value(&ctx), Value::Number(11.0));
    let cnt = ValueExpr::WindowCount {
        metric: MetricRef::new(id.clone()),
        window: 30,
    };
    assert_eq!(cnt.evaluate_value(&ctx), Value::Number(3.0));
}

#[test]
fn compare_atom_with_missing_is_false() {
    let b = bus();
    let ctx = EvalContext::new(&b, 0);
    let c = Condition::compare(
        ValueExpr::Missing,
        CompareOp::Ge,
        ValueExpr::Literal(1.0),
    );
    assert!(!c.evaluate(&ctx));
}

#[test]
fn metric_stale_atom_fires_when_expired() {
    let (b, id) = with_metric("m", 1.0, 10);
    // now=50, latest=10, age=40 > max_age=20
    let ctx = EvalContext::new(&b, 50);
    let stale = Condition::Atom(ConditionAtom::MetricStale {
        metric: id.clone(),
        max_age: 20,
    });
    assert!(stale.evaluate(&ctx));
    let fresh = Condition::Atom(ConditionAtom::MetricStale {
        metric: id,
        max_age: 60,
    });
    assert!(!fresh.evaluate(&ctx));
}

#[test]
fn evidence_rate_above_atom() {
    let mut b = bus();
    let kind = EvidenceKindId::from("k");
    b.declare_evidence(kind.clone(), EvidenceSpec::new(Retention::Long, "k"));
    for t in 0..20 {
        b.emit_evidence(StandingEvidence::new(
            kind.clone(),
            FactionId(1),
            FactionId(2),
            1.0,
            t,
        ));
    }
    let ctx = EvalContext::new(&b, 100).with_faction(FactionId(1));
    let c = Condition::Atom(ConditionAtom::EvidenceRateAbove {
        kind,
        window: 100,
        rate_per_tick: 0.1,
    });
    // 20 / 100 = 0.2 > 0.1
    assert!(c.evaluate(&ctx));
}

#[test]
fn precondition_set_weighted() {
    let (b, id) = with_metric("m", 0.5, 0);
    let set = PreconditionSet::new(vec![
        precond(
            "ok",
            severity::MAJOR,
            Condition::Atom(ConditionAtom::MetricPresent { metric: id.clone() }),
        ),
        precond(
            "high_val",
            severity::MODERATE,
            Condition::gt(
                ValueExpr::Metric(MetricRef::new(id)),
                ValueExpr::Literal(10.0),
            ),
        ),
    ]);
    let summary = set.evaluate(&EvalContext::new(&b, 0));
    assert_eq!(summary.total, 2);
    assert_eq!(summary.satisfied, 1);
    // 0.7 / (0.7 + 0.5) ≈ 0.5833
    assert!((summary.weighted_satisfaction - (0.7 / 1.2)).abs() < 1e-5);
    assert!(summary.critical_violations.is_empty());
}

#[test]
fn tracker_tracks_violation_duration() {
    let (b, _) = with_metric("m", 0.5, 0);
    let failing = PreconditionSet::new(vec![precond(
        "fail",
        severity::CRITICAL,
        Condition::Never,
    )]);
    let mut tracker = PreconditionTracker::new();

    let r = failing.evaluate_detailed(&EvalContext::new(&b, 5));
    tracker.record(&r, 5);
    assert_eq!(tracker.violated_for("fail", 5), Some(0));

    let r = failing.evaluate_detailed(&EvalContext::new(&b, 100));
    tracker.record(&r, 100);
    assert_eq!(tracker.violated_for("fail", 100), Some(95));
}

#[test]
fn cache_hit_when_versions_unchanged() {
    let (b, id) = with_metric("m", 0.5, 0);
    let mut reg = PreconditionCacheRegistry::new();
    let c = Condition::Atom(ConditionAtom::MetricPresent { metric: id });
    reg.evaluate(&c, &EvalContext::new(&b, 10));
    reg.evaluate(&c, &EvalContext::new(&b, 20));
    reg.evaluate(&c, &EvalContext::new(&b, 30));
    let s = reg.stats();
    assert_eq!(s.misses, 1);
    assert_eq!(s.hits, 2);
}

#[test]
fn cache_miss_after_reemit() {
    let (mut b, id) = with_metric("m", 0.1, 0);
    let mut reg = PreconditionCacheRegistry::new();
    let c = Condition::gt(
        ValueExpr::Metric(MetricRef::new(id.clone())),
        ValueExpr::Literal(1.0),
    );
    assert!(!reg.evaluate(&c, &EvalContext::new(&b, 10)));
    assert!(!reg.evaluate(&c, &EvalContext::new(&b, 20)));
    assert_eq!(reg.stats().hits, 1);

    b.emit(&id, 5.0, 30);
    assert!(reg.evaluate(&c, &EvalContext::new(&b, 40)));
    let s = reg.stats();
    assert_eq!(s.misses, 2);
}

#[test]
fn cache_respects_faction_key() {
    let mut b = bus();
    let kind = EvidenceKindId::from("k");
    b.declare_evidence(kind.clone(), EvidenceSpec::new(Retention::Long, "k"));
    for t in 0..5 {
        b.emit_evidence(StandingEvidence::new(
            kind.clone(),
            FactionId(1),
            FactionId(2),
            1.0,
            t,
        ));
    }
    let mut reg = PreconditionCacheRegistry::new();
    let c = Condition::Atom(ConditionAtom::EvidenceCountExceeds {
        kind,
        window: 100,
        threshold: 2,
    });
    // faction(1) sees 5 events
    assert!(reg.evaluate(&c, &EvalContext::new(&b, 10).with_faction(FactionId(1))));
    // faction(2) sees 0 — a miss (different cache key)
    assert!(!reg.evaluate(&c, &EvalContext::new(&b, 10).with_faction(FactionId(2))));
    assert_eq!(reg.stats().misses, 2);
}

#[test]
fn dependencies_dedup() {
    let c = Condition::All(vec![
        Condition::Atom(ConditionAtom::MetricPresent {
            metric: MetricId::from("x"),
        }),
        Condition::gt(
            ValueExpr::Metric(MetricRef::new(MetricId::from("x"))),
            ValueExpr::Literal(0.0),
        ),
    ]);
    let mut deps = Dependencies::new();
    c.collect_deps(&mut deps);
    assert_eq!(deps.metrics.len(), 2);
    deps.dedup();
    assert_eq!(deps.metrics.len(), 1);
}

