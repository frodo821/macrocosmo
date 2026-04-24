//! Mid-scenario prerequisite violation.
//!
//! `stockpile` starts positive, crosses 0 at tick 50, stays negative.
//! `econ` reaches its win threshold simultaneously at tick 50 — but
//! because `prerequisites` is checked before `win` in
//! `VictoryCondition::evaluate`, the verdict is `Unreachable`, not
//! `Won`.
//!
//! Expected behavior of the 3-layer stack:
//! - `victory_timeline` shows `Ongoing` before tick ~50 and
//!   `Unreachable` after.
//! - The default long-term agent stops emitting intents once victory
//!   status becomes terminal (`is_terminal()` guard). No intent should
//!   have `issued_at` after the flip.
//! - Existing campaigns are not auto-suspended — short continues to
//!   emit commands for the already-active campaigns. (This is the
//!   current contract; a future tuning pass may introduce an
//!   auto-suspend hook.)

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
fn victory_becomes_unreachable_when_prereq_fails() {
    let mut metric_scripts = BTreeMap::new();
    metric_scripts.insert(
        MetricId::from("econ"),
        MetricScript::Linear {
            from: 0.0,
            to: 200.0,
        },
    );
    metric_scripts.insert(
        MetricId::from("stockpile"),
        MetricScript::Linear {
            from: 5.0,
            to: -5.0,
        },
    );

    let config = ScenarioConfig {
        name: "victory_unreachable".into(),
        seed: 0,
        duration_ticks: 100,
        factions: vec![FactionId(0)],
        dynamics: SyntheticDynamics {
            metric_scripts,
            evidence_pulses: Vec::new(),
            command_responses: std::collections::BTreeMap::new(),
        },
    };
    let base = Scenario::new(config);

    // win = econ > 100 (crosses at tick 50); prereq = stockpile > 0
    // (crosses 0 at tick 50 — simultaneous).
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
    let mut spec = FactionAgentSpec::with_defaults(FactionId(0), victory, 0);
    spec.orchestrator_config.long_cadence = 5;
    spec.orchestrator_config.mid_cadence = 1;

    let pt = run_agent_scenario(AgentScenario::new(base, vec![spec]));
    let trace = &pt.per_faction[0];

    // Find the tick where victory first flips to Unreachable.
    let first_unreachable = trace
        .victory_timeline
        .iter()
        .find(|(_, s)| matches!(s, VictoryStatus::Unreachable))
        .map(|(t, _)| *t)
        .expect("victory must reach Unreachable once stockpile drops below 0");

    // Must not win (prereq pre-check takes precedence).
    assert!(
        !trace
            .victory_timeline
            .iter()
            .any(|(_, s)| matches!(s, VictoryStatus::Won)),
        "victory must not Win when prereq breaks on the same tick"
    );

    // At least one Ongoing before the flip.
    assert!(
        trace
            .victory_timeline
            .iter()
            .any(|(t, s)| *t < first_unreachable && matches!(s, VictoryStatus::Ongoing { .. })),
        "expected Ongoing status before Unreachable; timeline: {:?}",
        trace.victory_timeline
    );

    // Once Unreachable, never flips back to Ongoing / Won.
    for (t, status) in &trace.victory_timeline {
        if *t >= first_unreachable {
            assert!(
                matches!(status, VictoryStatus::Unreachable),
                "victory must stay Unreachable after tick {first_unreachable}; \
                 found {status:?} at tick {t}"
            );
        }
    }

    // The default long-term agent should stop emitting intents once
    // victory is terminal. Any intent issued at or after `first_unreachable`
    // is a bug.
    let post_flip_intents: Vec<&macrocosmo_ai::Intent> = trace
        .intent_history
        .iter()
        .filter(|i| i.issued_at >= first_unreachable)
        .collect();
    assert!(
        post_flip_intents.is_empty(),
        "long-term agent should stop emitting after Unreachable; \
         found {} intents issued at/after tick {first_unreachable}",
        post_flip_intents.len()
    );

    // Sanity: there were intents pre-flip (otherwise the test setup is wrong).
    assert!(
        !trace.intent_history.is_empty(),
        "scenario must have exercised at least some intents before the flip"
    );
}
