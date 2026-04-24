//! Economic-growth abstract scenario.
//!
//! Single faction; `econ` grows linearly 0 → 200 over 500 ticks;
//! `stockpile` stays positive (prerequisites OK). The 3-layer stack
//! (default Long + Mid + Short) should drive `econ` past 100 and
//! produce a `Won` victory status before the scenario ends.
//!
//! This is the "happy path" smoke — all three layers cooperate, no
//! Intent competition, no prerequisites violation.

#![cfg(feature = "playthrough")]

use std::collections::BTreeMap;

use macrocosmo_ai::condition::{Condition, ConditionAtom};
use macrocosmo_ai::ids::{FactionId, MetricId};
use macrocosmo_ai::playthrough::{
    AgentScenario, FactionAgentSpec, run_agent_scenario, Scenario, ScenarioConfig,
};
use macrocosmo_ai::playthrough::scenario::{MetricScript, SyntheticDynamics};
use macrocosmo_ai::VictoryCondition;
use macrocosmo_ai::VictoryStatus;

#[test]
fn economic_growth_reaches_victory() {
    let mut metric_scripts = BTreeMap::new();
    metric_scripts.insert(
        MetricId::from("econ"),
        MetricScript::Linear {
            from: 0.0,
            to: 200.0,
        },
    );
    metric_scripts.insert(MetricId::from("stockpile"), MetricScript::Constant(5.0));

    let config = ScenarioConfig {
        name: "economic_growth".into(),
        seed: 42,
        duration_ticks: 500,
        factions: vec![FactionId(0)],
        dynamics: SyntheticDynamics {
            metric_scripts,
            evidence_pulses: Vec::new(),
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
    let mut spec = FactionAgentSpec::with_defaults(FactionId(0), victory, 5);
    spec.orchestrator_config.long_cadence = 10;
    spec.orchestrator_config.mid_cadence = 5;

    let pt = run_agent_scenario(AgentScenario::new(base, vec![spec]));
    let trace = &pt.per_faction[0];

    // Expectations (see docs/ai-three-layer.md §scenario_economic_growth):
    // - Long emits at least one intent
    assert!(
        !trace.intent_history.is_empty(),
        "long agent must emit intents while ongoing; history empty"
    );

    // - Mid starts an expand_economy-ish campaign (synthesized name)
    let has_campaign = trace
        .campaign_snapshots
        .iter()
        .any(|(_, snap)| !snap.is_empty());
    assert!(has_campaign, "mid agent must start at least one campaign");

    // - Short emits commands once the campaign is active
    assert!(
        !trace.command_history.is_empty(),
        "short agent must emit commands while a campaign is Active"
    );

    // - Victory transitions to Won by tick ~500 (econ crosses 100 at ~250)
    let first_won = trace
        .victory_timeline
        .iter()
        .find(|(_, s)| matches!(s, VictoryStatus::Won))
        .map(|(t, _)| *t);
    assert!(
        first_won.is_some(),
        "victory should reach Won; timeline: {:?}",
        trace
            .victory_timeline
            .iter()
            .map(|(t, s)| (*t, s.clone()))
            .collect::<Vec<_>>()
    );
    let won_tick = first_won.unwrap();
    assert!(
        won_tick <= 400,
        "econ crosses 100 by tick ~250, Won should land by tick ~400, got {won_tick}"
    );

    // - No drops, no stale overrides (happy path)
    assert!(
        trace.drop_log.is_empty(),
        "no intents should be dropped in happy path; got {}",
        trace.drop_log.len()
    );
}
