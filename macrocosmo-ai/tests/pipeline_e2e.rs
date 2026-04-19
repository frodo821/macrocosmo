//! End-to-end pipeline test: stub metrics → projection → assessment →
//! feasibility → campaign transition → command emission → drain.
//!
//! Verifies that the full AI decision pipeline produces actionable output
//! when fed synthetic time-series data, without any game-engine dependency.

use macrocosmo_ai::ai_params::AiParamsExt;
use macrocosmo_ai::campaign::{Campaign, CampaignState};
use macrocosmo_ai::command::Command;
use macrocosmo_ai::condition::ConditionAtom;
use macrocosmo_ai::feasibility::{FeasibilityFormula, FeasibilityTerm};
use macrocosmo_ai::objective::{Objective, PreconditionSet as ObjPreconditionSet, SuccessCriteria};
use macrocosmo_ai::projection::{
    ProjectionNaming, TrajectoryConfig, emit_projections_to_bus, project,
};
use macrocosmo_ai::projection::window::{MetricPair, WindowDetectionConfig, detect_windows};
use macrocosmo_ai::value_expr::MetricRef;
use macrocosmo_ai::{
    AiBus, AssessmentConfig, Condition, FactionId, MetricId, MetricSpec, ObjectiveId,
    PreconditionItem, PreconditionSet, PreconditionTracker, Retention, WarningMode,
    build_assessment, severity,
};

#[derive(Default)]
struct StubParams;
impl AiParamsExt for StubParams {
    fn ai_param_f64(&self, _key: &str, default: f64) -> f64 {
        default
    }
}

fn declare_and_emit(bus: &mut AiBus, name: &str, value: f64, at: i64) {
    let id = MetricId::from(name);
    if !bus.has_metric(&id) {
        bus.declare_metric(id.clone(), MetricSpec::gauge(Retention::Long, name));
    }
    bus.emit(&id, value, at);
}

fn emit_stub_economy(bus: &mut AiBus, tick: i64, minerals_rate: f64, energy_rate: f64) {
    declare_and_emit(bus, "net_production_minerals", minerals_rate, tick);
    declare_and_emit(bus, "net_production_energy", energy_rate, tick);
    declare_and_emit(bus, "net_production_food", 80.0, tick);
    declare_and_emit(bus, "net_production_research", 30.0, tick);
    declare_and_emit(bus, "net_production_authority", 3.0, tick);
    declare_and_emit(bus, "stockpile_minerals", 500.0 + minerals_rate * tick as f64, tick);
    declare_and_emit(bus, "stockpile_energy", 400.0 + energy_rate * tick as f64, tick);
    declare_and_emit(bus, "stockpile_food", 300.0, tick);
    declare_and_emit(bus, "population_total", 3000.0 + tick as f64 * 10.0, tick);
    declare_and_emit(bus, "population_growth_rate", 0.03, tick);
    declare_and_emit(bus, "colony_count", 4.0, tick);
    declare_and_emit(bus, "tech_total_researched", 40.0 + tick as f64 * 0.5, tick);
    declare_and_emit(bus, "my_total_ships", 5.0, tick);
    declare_and_emit(bus, "my_strength", 100.0, tick);
}

/// Full pipeline: emit time-series → project → assess → check feasibility.
#[test]
fn stub_metrics_produce_valid_assessment_and_projections() {
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
    let me = FactionId(0);
    let rival = FactionId(1);

    // Emit 20 ticks of growing economy for projection fit.
    for tick in (10..=200).step_by(10) {
        let minerals = 100.0 + tick as f64 * 1.5;
        let energy = 80.0 + tick as f64 * 1.0;
        emit_stub_economy(&mut bus, tick, minerals, energy);
    }

    // Rival metrics (foreign slots).
    for tick in (10..=200).step_by(10) {
        declare_and_emit(
            &mut bus,
            &format!("foreign.research_output.faction_{}", rival.0),
            20.0 + tick as f64 * 0.3,
            tick,
        );
    }

    let now = 200;

    // --- Projection ---
    let target_metrics = vec![
        MetricId::from("net_production_minerals"),
        MetricId::from("net_production_energy"),
        MetricId::from("my_strength"),
    ];
    let traj_config = TrajectoryConfig::default();
    let trajectories = project(&bus, &target_metrics, &traj_config, now, &[]);

    // Minerals should have a linear model (growing steadily).
    let minerals_traj = &trajectories[&MetricId::from("net_production_minerals")];
    assert!(
        !minerals_traj.samples.is_empty(),
        "projection should produce samples from time-series data"
    );
    let horizon_val = minerals_traj.samples.last().unwrap().value;
    assert!(
        horizon_val > 400.0,
        "minerals projection at horizon should extrapolate growth, got {horizon_val}"
    );

    // Emit projections back to bus.
    emit_projections_to_bus(&mut bus, &trajectories, ProjectionNaming::default_both());

    // Verify projection topics exist on bus.
    let proj_minerals_end = MetricId::from("projection.net_production_minerals.horizon_end");
    assert!(
        bus.has_metric(&proj_minerals_end),
        "projection.*.horizon_end should be auto-declared on bus"
    );
    let projected_val = bus.current(&proj_minerals_end).unwrap();
    assert!(
        projected_val > 400.0,
        "projection bus value should match trajectory endpoint"
    );

    // --- Assessment ---
    let objective = Objective::new(
        ObjectiveId::from("economic_growth"),
        ObjPreconditionSet::always(),
        SuccessCriteria::new(Condition::Atom(ConditionAtom::MetricAbove {
            metric: MetricId::from("net_production_minerals"),
            threshold: 500.0,
        })),
        FeasibilityFormula::WeightedSum(vec![
            FeasibilityTerm::new(0.6, macrocosmo_ai::ValueExpr::Metric(
                MetricRef::new(MetricId::from("projection.net_production_minerals.horizon_end")),
            )),
            FeasibilityTerm::new(0.001, macrocosmo_ai::ValueExpr::Literal(1.0)),
        ]),
    );
    let precondition_set = PreconditionSet::new(vec![
        PreconditionItem::new("has_colonies", severity::MODERATE, Condition::Atom(ConditionAtom::MetricAbove {
            metric: MetricId::from("colony_count"),
            threshold: 1.0,
        })),
    ]);
    let mut tracker = PreconditionTracker::new();
    let config = AssessmentConfig::default();

    let assessment = build_assessment(
        &bus,
        me,
        &[rival],
        &objective,
        &precondition_set,
        &mut tracker,
        now,
        &config,
        &StubParams,
    );

    assert!(assessment.economic_capacity > 0.0, "economy should be non-trivial");
    assert!(assessment.feasibility > 0.0, "feasibility should be positive with growing economy");
    assert!(assessment.confidence > 0.0, "confidence should be non-zero with fresh data");
    assert_eq!(assessment.objective_precondition_summary.satisfied, 1);
    assert_eq!(assessment.objective_precondition_summary.total, 1);
}

/// Pipeline through campaign state machine: feasibility drives transitions.
#[test]
fn feasibility_drives_campaign_activation() {
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
    let me = FactionId(0);

    for tick in (10..=100).step_by(10) {
        emit_stub_economy(&mut bus, tick, 150.0, 100.0);
    }

    let now = 100;
    let objective = Objective::new(
        ObjectiveId::from("expand"),
        ObjPreconditionSet::always(),
        SuccessCriteria::new(Condition::Always),
        FeasibilityFormula::WeightedSum(vec![
            FeasibilityTerm::new(1.0, macrocosmo_ai::ValueExpr::Literal(0.8)),
        ]),
    );
    let precondition_set = PreconditionSet::new(vec![]);
    let mut tracker = PreconditionTracker::new();

    let assessment = build_assessment(
        &bus, me, &[], &objective, &precondition_set, &mut tracker, now,
        &AssessmentConfig::default(), &StubParams,
    );

    // Campaign starts as Proposed.
    let mut campaign = Campaign::new(ObjectiveId::from("expand"), now);
    assert_eq!(campaign.state, CampaignState::Proposed);

    // Feasibility > threshold → activate.
    let activation_threshold = 0.3;
    if assessment.feasibility > activation_threshold {
        campaign.transition(CampaignState::Active, now).unwrap();
    }
    assert_eq!(campaign.state, CampaignState::Active, "campaign should activate when feasibility exceeds threshold");
}

/// Full pipeline to command drain: assessment → command emit → drain.
#[test]
fn pipeline_emits_and_drains_commands() {
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
    let me = FactionId(0);

    for tick in (10..=100).step_by(10) {
        emit_stub_economy(&mut bus, tick, 200.0, 150.0);
    }

    let now = 100;

    // Declare command kind.
    let attack_kind = macrocosmo_ai::CommandKindId::from("attack_target");
    bus.declare_command(
        attack_kind.clone(),
        macrocosmo_ai::spec::CommandSpec::new("attack a target system"),
    );

    // Simulate AI decision: assessment passes threshold → emit command.
    let objective = Objective::new(
        ObjectiveId::from("conquer"),
        ObjPreconditionSet::always(),
        SuccessCriteria::new(Condition::Always),
        FeasibilityFormula::WeightedSum(vec![
            FeasibilityTerm::new(1.0, macrocosmo_ai::ValueExpr::Literal(0.9)),
        ]),
    );
    let precondition_set = PreconditionSet::new(vec![]);
    let mut tracker = PreconditionTracker::new();

    let assessment = build_assessment(
        &bus, me, &[], &objective, &precondition_set, &mut tracker, now,
        &AssessmentConfig::default(), &StubParams,
    );
    assert!(assessment.feasibility > 0.5);

    // AI decides to attack — emit command.
    let cmd = Command::new(attack_kind.clone(), me, now)
        .with_param("target_system", macrocosmo_ai::ids::SystemRef(42))
        .with_priority(assessment.feasibility as f64);
    bus.emit_command(cmd);

    // Verify command is pending.
    let pending = bus.pending_commands();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].kind, attack_kind);
    assert_eq!(pending[0].issuer, me);

    // Drain (game-side consume).
    let drained = bus.drain_commands();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].kind, attack_kind);

    // After drain, queue is empty.
    assert!(bus.pending_commands().is_empty());
}

/// Strategic window detection from diverging stub trajectories.
#[test]
fn projection_detects_offensive_window() {
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);

    // My strength growing, rival's declining.
    for tick in (10..=200).step_by(10) {
        declare_and_emit(&mut bus, "my_strength", 50.0 + tick as f64 * 2.0, tick);
        declare_and_emit(&mut bus, "rival_strength", 400.0 - tick as f64 * 0.5, tick);
    }

    let now = 200;
    let metrics = vec![
        MetricId::from("my_strength"),
        MetricId::from("rival_strength"),
    ];
    let trajectories = project(&bus, &metrics, &TrajectoryConfig::default(), now, &[]);

    let window_config = WindowDetectionConfig {
        min_intensity: 0.01,
        pairs: vec![MetricPair {
            mine: MetricId::from("my_strength"),
            theirs: MetricId::from("rival_strength"),
        }],
        ..Default::default()
    };

    let windows = detect_windows(&trajectories, now, &window_config);
    assert!(
        !windows.is_empty(),
        "should detect at least one strategic window from diverging trajectories"
    );
}

/// Verify that projection → bus round-trip preserves data for feasibility reference.
#[test]
fn feasibility_reads_projected_bus_values() {
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);

    for tick in (10..=100).step_by(10) {
        declare_and_emit(&mut bus, "net_production_minerals", 100.0 + tick as f64 * 3.0, tick);
    }

    let now = 100;
    let trajectories = project(
        &bus,
        &[MetricId::from("net_production_minerals")],
        &TrajectoryConfig::default(),
        now,
        &[],
    );
    emit_projections_to_bus(&mut bus, &trajectories, ProjectionNaming::default_both());

    // Feasibility formula references the projected value.
    let formula = FeasibilityFormula::WeightedSum(vec![FeasibilityTerm::new(
        0.001,
        macrocosmo_ai::ValueExpr::Metric(
            MetricRef::new(
                MetricId::from("projection.net_production_minerals.horizon_end"),
            ),
        ),
    )]);

    let score = macrocosmo_ai::feasibility::evaluate(&formula, &bus, now, None);
    assert!(
        score > 0.3,
        "feasibility should read projected horizon value (minerals growing at 3/tick), got {score}"
    );
}
