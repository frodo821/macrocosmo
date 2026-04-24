//! Preemptive prerequisite preservation.
//!
//! The default long-term agent currently emits `preserve_metric`
//! intents **only when the prereq is already violated** — i.e., only
//! once victory is already `Unreachable`. That's too late: the real-
//! world use-case (e.g. "steer the crisis event toward ending A
//! before B locks in") needs intents emitted *while* the prereq is
//! still satisfied but trending toward violation.
//!
//! This test formalizes the expectation: when a prereq is within
//! `safety_margin` of its threshold, the long agent should already
//! be emitting `preserve_metric` intents.
//!
//! Scenario layout:
//! - `stockpile`: Linear from=20.0 to=-5.0 over 200 ticks. Crosses 0
//!   around tick 160. Within 5.0 of the threshold from tick ~120
//!   onward.
//! - `econ`: Constant 50 (win never satisfied — we don't care).
//!
//! Expected (after the Long-default safety-margin feature lands):
//! - At least one `preserve_metric` intent for `stockpile` is emitted
//!   **before tick 160** (i.e. while the prereq is still technically
//!   satisfied but within the safety margin).

#![cfg(feature = "playthrough")]

use std::collections::BTreeMap;

use macrocosmo_ai::condition::{Condition, ConditionAtom};
use macrocosmo_ai::ids::{FactionId, MetricId};
use macrocosmo_ai::long_term_default::{LongTermDefaultConfig, ObjectiveDrivenLongTerm};
use macrocosmo_ai::playthrough::scenario::{MetricScript, SyntheticDynamics};
use macrocosmo_ai::playthrough::{
    AgentScenario, FactionAgentSpec, run_agent_scenario, Scenario, ScenarioConfig,
};
use macrocosmo_ai::{FixedDelayDispatcher, OrchestratorConfig, VictoryCondition};

#[test]
fn long_emits_preserve_before_prereq_violates() {
    let mut metric_scripts = BTreeMap::new();
    metric_scripts.insert(
        MetricId::from("stockpile"),
        MetricScript::Linear {
            from: 20.0,
            to: -5.0,
        },
    );
    metric_scripts.insert(MetricId::from("econ"), MetricScript::Constant(50.0));

    let config = ScenarioConfig {
        name: "preemptive_preserve".into(),
        seed: 0,
        duration_ticks: 200,
        factions: vec![FactionId(0)],
        dynamics: SyntheticDynamics {
            metric_scripts,
            evidence_pulses: Vec::new(),
            command_responses: BTreeMap::new(),
        },
    };
    let base = Scenario::new(config);

    let victory = VictoryCondition::simple(
        Condition::Atom(ConditionAtom::MetricAbove {
            metric: MetricId::from("econ"),
            threshold: 100.0,
        }),
        Condition::Atom(ConditionAtom::MetricAbove {
            metric: MetricId::from("stockpile"),
            threshold: 0.0,
        }),
    );

    // Configure Long with a safety_margin of 5.0. The Long-default
    // should emit preserve_metric intents for stockpile once the
    // metric falls within 5.0 of the threshold (i.e. from tick ~120
    // onward — well before the tick 160 violation).
    let cfg = LongTermDefaultConfig {
        safety_margin: 5.0,
        ..LongTermDefaultConfig::default()
    };
    let long = ObjectiveDrivenLongTerm::new().with_config(cfg);

    let spec = FactionAgentSpec {
        faction: FactionId(0),
        victory,
        long: Box::new(long),
        mid: Box::new(macrocosmo_ai::IntentDrivenMidTerm::new()),
        short: Box::new(macrocosmo_ai::CampaignReactiveShort::new()),
        dispatcher: Box::new(FixedDelayDispatcher::zero_delay()),
        orchestrator_config: {
            let mut c = OrchestratorConfig::default();
            c.long_cadence = 5;
            c.mid_cadence = 1;
            c
        },
    };

    let pt = run_agent_scenario(AgentScenario::new(base, vec![spec]));
    let trace = &pt.per_faction[0];

    // Find preserve intents emitted before tick 160 (violation tick).
    // By construction, the Linear script for `stockpile` drops below
    // the safety margin (`5.0`) at tick 120 — we allow ±10 slack.
    let preserve_before_violation: Vec<&macrocosmo_ai::Intent> = trace
        .intent_history
        .iter()
        .filter(|i| i.spec.kind.as_str() == "preserve_metric")
        .filter(|i| i.issued_at < 160)
        .collect();

    assert!(
        !preserve_before_violation.is_empty(),
        "expected at least one preserve_metric intent before tick 160; \
         all intents: {:?}",
        trace
            .intent_history
            .iter()
            .map(|i| (i.spec.kind.as_str().to_string(), i.issued_at))
            .collect::<Vec<_>>()
    );

    // The earliest preserve intent should fire within the margin
    // window (tick ~120 onward, with generous slack for cadence=5).
    let earliest_preserve = preserve_before_violation
        .iter()
        .map(|i| i.issued_at)
        .min()
        .unwrap();
    assert!(
        (110..=155).contains(&earliest_preserve),
        "earliest preserve_metric should be within [110, 155] (margin entered at ~120); \
         got {earliest_preserve}"
    );
}
