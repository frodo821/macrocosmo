//! Compound win condition — `win = All(A > 100, B > 50)`.
//!
//! The default long-term agent walks the condition tree and emits
//! one `pursue_metric` intent per unsatisfied leaf. If only one leaf
//! is satisfied, the other must still be pursued.
//!
//! Scenario layout:
//! - `metric_a`: Linear 0 → 200 over 400 ticks (crosses 100 at ~tick 200)
//! - `metric_b`: Linear 0 → 100 over 400 ticks (crosses 50 at ~tick 200)
//! - stockpile: Constant 5.0 (prereq always satisfied)
//!
//! Expected:
//! - Both leaves are pursued from tick 0 (two distinct campaigns,
//!   one per metric).
//! - Around tick ~200, both cross — victory Won.
//! - After each leaf is individually satisfied (say A at 200 while B
//!   still pending), the satisfied leaf's pursuit emissions **stop**
//!   (default agent's "already satisfied" short-circuit).

#![cfg(feature = "playthrough")]

use std::collections::BTreeMap;

use macrocosmo_ai::condition::{Condition, ConditionAtom};
use macrocosmo_ai::ids::{FactionId, MetricId};
use macrocosmo_ai::playthrough::scenario::{MetricScript, SyntheticDynamics};
use macrocosmo_ai::playthrough::{
    AgentScenario, FactionAgentSpec, run_agent_scenario, Scenario, ScenarioConfig,
};
use macrocosmo_ai::VictoryCondition;
use macrocosmo_ai::VictoryStatus;

#[test]
fn compound_win_pursues_each_leaf_independently() {
    let mut metric_scripts = BTreeMap::new();
    metric_scripts.insert(
        MetricId::from("metric_a"),
        MetricScript::Linear {
            from: 0.0,
            to: 200.0,
        },
    );
    metric_scripts.insert(
        MetricId::from("metric_b"),
        MetricScript::Linear {
            from: 0.0,
            to: 100.0,
        },
    );
    metric_scripts.insert(MetricId::from("stockpile"), MetricScript::Constant(5.0));

    let config = ScenarioConfig {
        name: "compound_win".into(),
        seed: 0,
        duration_ticks: 400,
        factions: vec![FactionId(0)],
        dynamics: SyntheticDynamics {
            metric_scripts,
            evidence_pulses: Vec::new(),
        },
    };
    let base = Scenario::new(config);

    let victory = VictoryCondition::simple(
        Condition::All(vec![
            Condition::Atom(ConditionAtom::MetricAbove {
                metric: MetricId::from("metric_a"),
                threshold: 100.0,
            }),
            Condition::Atom(ConditionAtom::MetricAbove {
                metric: MetricId::from("metric_b"),
                threshold: 50.0,
            }),
        ]),
        Condition::Atom(ConditionAtom::MetricAbove {
            metric: MetricId::from("stockpile"),
            threshold: 0.0,
        }),
    );
    let mut spec = FactionAgentSpec::with_defaults(FactionId(0), victory, 0);
    spec.orchestrator_config.long_cadence = 1;
    spec.orchestrator_config.mid_cadence = 1;

    let pt = run_agent_scenario(AgentScenario::new(base, vec![spec]));
    let trace = &pt.per_faction[0];

    // (1) Both metrics were pursued at some point. The default Long
    // agent encodes the target metric in the intent rationale's
    // `metrics_seen` map — that gives us an observable way to tell
    // which leaf each intent pursued without parsing params strings.
    let saw_a = trace.intent_history.iter().any(|i| {
        i.spec
            .rationale
            .metrics_seen
            .keys()
            .any(|k| k.as_str() == "metric_a")
    });
    let saw_b = trace.intent_history.iter().any(|i| {
        i.spec
            .rationale
            .metrics_seen
            .keys()
            .any(|k| k.as_str() == "metric_b")
    });
    assert!(saw_a, "metric_a leaf must be pursued at least once");
    assert!(saw_b, "metric_b leaf must be pursued at least once");

    // (2) Two distinct campaigns exist at some point (one per leaf).
    let max_campaigns = trace
        .campaign_snapshots
        .iter()
        .map(|(_, snap)| snap.len())
        .max()
        .unwrap_or(0);
    assert!(
        max_campaigns >= 2,
        "expected at least 2 campaigns (one per win leaf); peak was {max_campaigns}"
    );

    // (3) Victory reaches Won around when both leaves have crossed.
    let first_won = trace
        .victory_timeline
        .iter()
        .find(|(_, s)| matches!(s, VictoryStatus::Won))
        .map(|(t, _)| *t)
        .expect("compound win must eventually succeed");
    // metric_a crosses 100 at tick ~200; metric_b crosses 50 at tick ~200.
    // Allow a small margin for cadence granularity.
    assert!(
        (190..=220).contains(&first_won),
        "expected Won around tick 200; got {first_won}"
    );

    // (4) After a leaf becomes satisfied, its pursue intents should
    // stop being re-emitted on subsequent long ticks (short-circuit).
    // Verify by looking at intents per leaf after tick 200.
    let a_post_win: usize = trace
        .intent_history
        .iter()
        .filter(|i| i.issued_at >= 210)
        .filter(|i| {
            i.spec
                .rationale
                .metrics_seen
                .keys()
                .any(|k| k.as_str() == "metric_a")
        })
        .count();
    assert_eq!(
        a_post_win, 0,
        "no new metric_a pursuit intents should fire after tick 210 (a already above threshold)"
    );
}
