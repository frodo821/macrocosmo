//! Integration tests for `macrocosmo_ai::projection` — trajectory extrapolation.
//!
//! These tests exercise `project` / `project_metric` via the public API
//! (lib.rs re-exports) and verify feasibility wiring via
//! [`emit_projections_to_bus`].

use macrocosmo_ai::feasibility::{self, FeasibilityFormula, FeasibilityTerm};
use macrocosmo_ai::{
    emit_projections_to_bus, project, project_metric, AiBus, CompoundDelta, CompoundEffect,
    MetricId, MetricRef, MetricSpec, ProjectionFidelity, ProjectionModel, ProjectionNaming,
    Retention, TrajectoryConfig, Value, ValueExpr, WarningMode,
};

fn bus_with(metric: &MetricId) -> AiBus {
    let mut b = AiBus::with_warning_mode(WarningMode::Silent);
    b.declare_metric(metric.clone(), MetricSpec::gauge(Retention::VeryLong, "t"));
    b
}

#[test]
fn project_constant_metric_returns_flat_trajectory() {
    let id = MetricId::from("m");
    let mut bus = bus_with(&id);
    for i in 0..8 {
        bus.emit(&id, 50.0, i * 5);
    }
    let cfg = TrajectoryConfig {
        horizon: 30,
        step: 5,
        fidelity: ProjectionFidelity::Standard,
        ..Default::default()
    };
    let tr = project_metric(&bus, &id, &cfg, 35, &[]);
    assert!(!tr.is_empty());
    for s in &tr.samples {
        assert!((s.value - 50.0).abs() < 1e-6);
    }
}

#[test]
fn project_linear_growth_extrapolates() {
    let id = MetricId::from("research");
    let mut bus = bus_with(&id);
    // y = 3 t
    for i in 0..8 {
        let at = i * 5;
        bus.emit(&id, 3.0 * at as f64, at);
    }
    let cfg = TrajectoryConfig {
        horizon: 20,
        step: 5,
        history_window: 60,
        fidelity: ProjectionFidelity::Standard,
        ..Default::default()
    };
    let now = 35;
    let tr = project_metric(&bus, &id, &cfg, now, &[]);
    match tr.model {
        ProjectionModel::Linear { slope, .. } => assert!((slope - 3.0).abs() < 1e-6),
        other => panic!("expected Linear, got {other:?}"),
    }
    // At t=55 the extrapolated value should be 3*55 = 165.
    let last = tr.samples.last().unwrap();
    assert_eq!(last.at, 55);
    assert!((last.value - 165.0).abs() < 1e-6);
}

#[test]
fn project_with_compound_effect_applies_after_activation() {
    let id = MetricId::from("m");
    let mut bus = bus_with(&id);
    for i in 0..6 {
        bus.emit(&id, 100.0, i * 5);
    }
    let cfg = TrajectoryConfig {
        horizon: 30,
        step: 5,
        fidelity: ProjectionFidelity::Detailed,
        ..Default::default()
    };
    let now = 25;
    let effect = CompoundEffect {
        activates_at: now + 10,
        metric: id.clone(),
        delta: CompoundDelta::Multiplicative(2.0),
    };
    let tr = project_metric(&bus, &id, &cfg, now, &[effect]);
    // Before activation: 100; after: 200.
    assert!((tr.samples[0].value - 100.0).abs() < 1e-6);
    assert!((tr.samples[1].value - 100.0).abs() < 1e-6);
    assert!((tr.samples[2].value - 200.0).abs() < 1e-6);
    assert!((tr.samples.last().unwrap().value - 200.0).abs() < 1e-6);
}

#[test]
fn project_empty_history_yields_missing_model() {
    let id = MetricId::from("m");
    let bus = bus_with(&id);
    let tr = project_metric(&bus, &id, &TrajectoryConfig::default(), 0, &[]);
    assert_eq!(tr.model, ProjectionModel::Missing);
    assert!(tr.is_empty());
}

#[test]
fn project_batch_returns_all_requested_metrics() {
    let a = MetricId::from("a");
    let b = MetricId::from("b");
    let mut bus = bus_with(&a);
    bus.declare_metric(b.clone(), MetricSpec::gauge(Retention::VeryLong, "t"));
    for i in 0..4 {
        bus.emit(&a, 1.0, i * 5);
        bus.emit(&b, 2.0, i * 5);
    }
    let cfg = TrajectoryConfig {
        horizon: 10,
        step: 5,
        ..Default::default()
    };
    let trs = project(&bus, &[a.clone(), b.clone()], &cfg, 15, &[]);
    assert_eq!(trs.len(), 2);
    assert!(trs.contains_key(&a));
    assert!(trs.contains_key(&b));
}

#[test]
fn emit_projections_round_trips_via_feasibility() {
    // Project a linear metric, emit the horizon-end to the bus, then let a
    // feasibility formula pick it up like any other metric.
    let id = MetricId::from("m");
    let mut bus = bus_with(&id);
    for i in 0..8 {
        let at = i * 5;
        bus.emit(&id, at as f64, at);
    }
    let cfg = TrajectoryConfig {
        horizon: 20,
        step: 5,
        fidelity: ProjectionFidelity::Standard,
        ..Default::default()
    };
    let now = 35;
    let trs = project(&bus, &[id.clone()], &cfg, now, &[]);

    emit_projections_to_bus(&mut bus, &trs, ProjectionNaming::default_both());

    // The horizon-end topic should be `projection.m.horizon_end` with
    // value equal to the extrapolated slope*55 = 55.
    let horizon_id = MetricId::from("projection.m.horizon_end");
    assert!(bus.has_metric(&horizon_id));
    let v = bus.current(&horizon_id).unwrap();
    assert!((v - 55.0).abs() < 1e-6);

    // Feasibility that references the projected metric.
    let formula = FeasibilityFormula::WeightedSum(vec![FeasibilityTerm::new(
        1.0,
        ValueExpr::Metric(MetricRef::new(horizon_id.clone())),
    )]);
    let score = feasibility::evaluate(&formula, &bus, now + 20, None);
    assert!((score - 55.0).abs() < 1e-6);

    // Sanity: evaluating the ValueExpr directly also yields the projected
    // value, not Missing.
    let ctx = macrocosmo_ai::EvalContext::new(&bus, now + 20);
    let v = ValueExpr::Metric(MetricRef::new(horizon_id)).evaluate_value(&ctx);
    assert!(matches!(v, Value::Number(n) if (n - 55.0).abs() < 1e-6));
}
