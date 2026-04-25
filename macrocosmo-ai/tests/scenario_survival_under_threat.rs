//! Survival-under-threat abstract scenario.
//!
//! 2 factions. The observed faction's `my_strength` stays constant; the
//! rival's strength oscillates (sinusoid). Victory = survive `now > T`;
//! prerequisites = `my_strength > 0` (won't actually break in this
//! scenario — the sinusoid doesn't modify `my_strength`).
//!
//! Expected behavior:
//! - Long emits intents continuously while ongoing (at least on
//!   threat spikes if it were threat-aware; the default impl is not,
//!   so it simply emits pursue_metric intents for the `now > T` leaf
//!   whenever the condition is unsatisfied).
//! - Mid starts a campaign tied to the intent.
//! - Victory reaches Won at exactly `time_limit - 1` (or whenever
//!   `now > 500` first evaluates True).
//!
//! This scenario is a second smoke: it exercises multi-faction setup
//! (foreign metric populated) and a `win` condition tied to tick count
//! rather than a metric, ensuring `now > T` works through the
//! Condition tree.

#![cfg(feature = "playthrough")]

use std::collections::BTreeMap;

use macrocosmo_ai::VictoryCondition;
use macrocosmo_ai::VictoryStatus;
use macrocosmo_ai::condition::{Condition, ConditionAtom};
use macrocosmo_ai::ids::{FactionId, MetricId};
use macrocosmo_ai::playthrough::scenario::{MetricScript, SyntheticDynamics};
use macrocosmo_ai::playthrough::{
    AgentScenario, FactionAgentSpec, Scenario, ScenarioConfig, run_agent_scenario,
};

#[test]
fn survival_under_threat_reaches_time_based_victory() {
    let mut metric_scripts = BTreeMap::new();
    // Observer's own strength: constant.
    metric_scripts.insert(MetricId::from("my_strength"), MetricScript::Constant(30.0));
    // Rival's strength (foreign slot): sinusoidal threat.
    metric_scripts.insert(
        MetricId::from("foreign.my_strength.faction_1"),
        MetricScript::Sinusoid {
            mean: 50.0,
            amplitude: 40.0,
            period: 100,
        },
    );
    // A metric representing "time elapsed"; we'll drive win against it.
    metric_scripts.insert(
        MetricId::from("elapsed"),
        MetricScript::Monotone {
            from: 0.0,
            slope: 1.0,
        },
    );

    let config = ScenarioConfig {
        name: "survival_under_threat".into(),
        seed: 7,
        duration_ticks: 600,
        factions: vec![FactionId(0), FactionId(1)],
        dynamics: SyntheticDynamics {
            metric_scripts,
            evidence_pulses: Vec::new(),
            command_responses: BTreeMap::new(),
        },
    };
    let base = Scenario::new(config);

    // Win: elapsed > 500 (= survived the T-tick window).
    // Prerequisites: my_strength > 0 (won't break in this scenario).
    let victory = VictoryCondition::simple(
        Condition::Atom(ConditionAtom::MetricAbove {
            metric: MetricId::from("elapsed"),
            threshold: 500.0,
        }),
        Condition::Atom(ConditionAtom::MetricAbove {
            metric: MetricId::from("my_strength"),
            threshold: 0.0,
        }),
    );
    let mut spec = FactionAgentSpec::with_defaults(FactionId(0), victory, 3);
    spec.orchestrator_config.long_cadence = 20;
    spec.orchestrator_config.mid_cadence = 5;

    let pt = run_agent_scenario(AgentScenario::new(base, vec![spec]));
    let trace = &pt.per_faction[0];

    // - Long emits intents while ongoing
    assert!(
        !trace.intent_history.is_empty(),
        "long must emit intents while ongoing"
    );

    // - Victory reaches Won sometime after elapsed > 500 (around tick 501+)
    let first_won = trace
        .victory_timeline
        .iter()
        .find(|(_, s)| matches!(s, VictoryStatus::Won))
        .map(|(t, _)| *t);
    assert!(
        first_won.is_some(),
        "victory must reach Won by end of scenario"
    );
    let won_tick = first_won.unwrap();
    assert!(
        won_tick > 500,
        "elapsed crosses 500 around tick 501, got won_tick={won_tick}"
    );
    assert!(
        won_tick <= 520,
        "Won should land shortly after elapsed>500; got {won_tick}"
    );

    // - No drop entries (no dispatcher failures in the fixed-delay setup)
    assert!(
        trace.drop_log.is_empty(),
        "no drops expected with FixedDelayDispatcher"
    );

    // - Not a flood of stale overrides (allow a small number due to
    //   half-life decay, but no more than a handful)
    let stale = trace
        .override_log
        .iter()
        .filter(|e| matches!(e.reason, macrocosmo_ai::OverrideReason::StaleIntent))
        .count();
    assert!(
        stale <= 5,
        "only a few stale overrides expected; got {stale}"
    );
}
