//! Smoke-test for the `assessment` module.
//!
//! Populates a mock bus with synthetic self-metrics + foreign-faction
//! research slots, runs `build_assessment`, and asserts every derived score
//! is finite and lives in the expected bounds.

use macrocosmo_ai::ai_params::AiParamsExt;
use macrocosmo_ai::feasibility::{FeasibilityFormula, FeasibilityTerm};
use macrocosmo_ai::objective::{Objective, PreconditionSet as ObjPreconditionSet, SuccessCriteria};
use macrocosmo_ai::{
    AiBus, Assessment, AssessmentConfig, Condition, EvidenceKindId, EvidenceSpec, FactionId,
    MetricId, MetricSpec, ObjectiveId, PreconditionItem, PreconditionSet, PreconditionTracker,
    Retention, StandingEvidence, ValueExpr, WarningMode, build_assessment, severity,
};

// Custom minimal AiParams stub for the smoke test.
#[derive(Default)]
struct NeutralParams;
impl AiParamsExt for NeutralParams {
    fn ai_param_f64(&self, _key: &str, default: f64) -> f64 {
        default
    }
}

fn declare_and_emit(bus: &mut AiBus, name: &str, value: f64, at: i64) {
    let id = MetricId::from(name);
    bus.declare_metric(id.clone(), MetricSpec::gauge(Retention::Long, name));
    bus.emit(&id, value, at);
}

#[test]
fn assessment_smoke() {
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);

    // Self metrics.
    for (name, v) in [
        ("net_production_minerals", 200.0),
        ("net_production_energy", 180.0),
        ("net_production_food", 120.0),
        ("net_production_research", 40.0),
        ("net_production_authority", 5.0),
        ("stockpile_minerals", 1000.0),
        ("stockpile_energy", 800.0),
        ("stockpile_food", 600.0),
        ("population_total", 5000.0),
        ("population_growth_rate", 0.02),
        ("colony_count", 5.0),
        ("tech_total_researched", 60.0),
    ] {
        declare_and_emit(&mut bus, name, v, 100);
    }

    // Foreign faction research output slots.
    let rival_a = FactionId(1);
    let rival_b = FactionId(2);
    declare_and_emit(
        &mut bus,
        &format!("foreign.research_output.faction_{}", rival_a.0),
        35.0,
        100,
    );
    declare_and_emit(
        &mut bus,
        &format!("foreign.research_output.faction_{}", rival_b.0),
        45.0,
        100,
    );

    // A little evidence so PerceivedStanding can attach confidence.
    bus.declare_evidence(
        EvidenceKindId::from("hostile_engagement"),
        EvidenceSpec::new(Retention::Long, "x"),
    );
    bus.emit_evidence(StandingEvidence::new(
        EvidenceKindId::from("hostile_engagement"),
        FactionId(0),
        rival_a,
        1.0,
        95,
    ));

    // Objective + preconditions.
    let objective = Objective::new(
        ObjectiveId::from("economic_dominance"),
        ObjPreconditionSet::always(),
        SuccessCriteria::new(Condition::Always),
        FeasibilityFormula::WeightedSum(vec![FeasibilityTerm::new(1.0, ValueExpr::Literal(0.5))]),
    );
    let precondition_set = PreconditionSet::new(vec![
        PreconditionItem::new("alive", severity::MAJOR, Condition::Always),
        PreconditionItem::new("has_colonies", severity::MODERATE, Condition::Always),
    ]);

    let mut tracker = PreconditionTracker::new();
    let config = AssessmentConfig::default();
    let a: Assessment = build_assessment(
        &bus,
        FactionId(0),
        &[rival_a, rival_b],
        &objective,
        &precondition_set,
        &mut tracker,
        100,
        &config,
        &NeutralParams,
    );

    // Every derived score must be finite and in [0, 1].
    assert!(a.economic_capacity.is_finite());
    assert!(
        (0.0..=1.0).contains(&a.economic_capacity),
        "ec={}",
        a.economic_capacity
    );
    assert!((0.0..=1.0).contains(&a.tech_lead), "tl={}", a.tech_lead);
    assert!((0.0..=1.0).contains(&a.feasibility), "f={}", a.feasibility);
    assert!((0.0..=1.0).contains(&a.confidence), "c={}", a.confidence);
    assert_eq!(a.last_updated_at, 100);

    // Two rival research-output slots populated → known competitors.
    assert_eq!(a.tech_position.known_competitor_levels.len(), 2);

    // Perceived standing toward each rival.
    assert_eq!(a.perceived_standings.len(), 2);

    // Confidence should have some signal (> 0 since metrics are all fresh).
    assert!(a.confidence > 0.0);

    // Objective precondition summary reflects the 2 items we registered.
    assert_eq!(a.objective_precondition_summary.total, 2);
}
