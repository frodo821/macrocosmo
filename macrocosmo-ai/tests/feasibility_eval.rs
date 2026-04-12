//! Integration test: feasibility evaluation end-to-end against a bus.

use macrocosmo_ai::{
    feasibility::{self, FeasibilityFormula, FeasibilityTerm},
    AiBus, MetricId, MetricRef, MetricSpec, Retention, ValueExpr, WarningMode,
};

#[test]
fn weighted_sum_end_to_end() {
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
    let readiness = MetricId::from("fleet_readiness");
    let capacity = MetricId::from("economic_capacity");
    bus.declare_metric(
        readiness.clone(),
        MetricSpec::ratio(Retention::Medium, "r"),
    );
    bus.declare_metric(
        capacity.clone(),
        MetricSpec::ratio(Retention::Medium, "c"),
    );
    bus.emit(&readiness, 0.6, 50);
    bus.emit(&capacity, 0.4, 50);

    let formula = FeasibilityFormula::WeightedSum(vec![
        FeasibilityTerm::new(0.7, ValueExpr::Metric(MetricRef::new(readiness))),
        FeasibilityTerm::new(0.3, ValueExpr::Metric(MetricRef::new(capacity))),
    ]);
    let score = feasibility::evaluate(&formula, &bus, 50, None);
    // 0.7*0.6 + 0.3*0.4 = 0.42 + 0.12 = 0.54
    assert!((score - 0.54).abs() < 1e-9, "score={score}");
}

#[test]
fn delt_in_formula() {
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
    let power = MetricId::from("power");
    bus.declare_metric(power.clone(), MetricSpec::gauge(Retention::Long, "p"));
    bus.emit(&power, 10.0, 0);
    bus.emit(&power, 30.0, 100);

    let formula = FeasibilityFormula::WeightedSum(vec![FeasibilityTerm::new(
        1.0,
        ValueExpr::DelT {
            metric: MetricRef::new(power),
            window: 100,
        },
    )]);
    let score = feasibility::evaluate(&formula, &bus, 100, None);
    // delta = 30 - 10 = 20
    assert_eq!(score, 20.0);
}
