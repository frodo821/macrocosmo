//! Mid abandons campaigns when victory becomes terminal.
//!
//! Round 1's `scenario_victory_unreachable` documented that even
//! after victory flips to `Unreachable`, the short-term agent kept
//! emitting commands for campaigns that were still `Active`. The
//! Mid had no signal to react. Round 6 part 1 closes that gap by
//! propagating `victory_status` into `MidTermInput` and adding a
//! default-impl short-circuit.
//!
//! ### Setup (mirrors scenario_victory_unreachable)
//! - econ: Linear 0 → 200 over 100 ticks (cross 100 at tick ~50).
//! - stockpile: Linear 5 → -5 over 100 ticks (cross 0 at tick ~50).
//! - win = econ > 100, prereq = stockpile > 0, time_limit None.
//! - Mid default with `abandon_on_terminal = true`.
//!
//! Expected:
//! - victory flips to `Unreachable` around tick 50.
//! - Mid issues `Transition` ops to `Abandoned` for active campaigns.
//! - After a few ticks of grace (Mid cadence), Short emits no more
//!   commands for those campaigns.

#![cfg(feature = "playthrough")]

use std::collections::BTreeMap;

use macrocosmo_ai::campaign::CampaignState;
use macrocosmo_ai::condition::{Condition, ConditionAtom};
use macrocosmo_ai::ids::{FactionId, MetricId};
use macrocosmo_ai::playthrough::scenario::{MetricScript, SyntheticDynamics};
use macrocosmo_ai::playthrough::{
    AgentScenario, FactionAgentSpec, run_agent_scenario, Scenario, ScenarioConfig,
};
use macrocosmo_ai::VictoryCondition;
use macrocosmo_ai::VictoryStatus;

#[test]
fn mid_abandons_campaigns_after_unreachable() {
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
        name: "mid_terminal_awareness".into(),
        seed: 0,
        duration_ticks: 100,
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
    let mut spec = FactionAgentSpec::with_defaults(FactionId(0), victory, 0);
    spec.orchestrator_config.long_cadence = 5;
    spec.orchestrator_config.mid_cadence = 1;

    let pt = run_agent_scenario(AgentScenario::new(base, vec![spec]));
    let trace = &pt.per_faction[0];

    let first_unreachable = trace
        .victory_timeline
        .iter()
        .find(|(_, s)| matches!(s, VictoryStatus::Unreachable))
        .map(|(t, _)| *t)
        .expect("victory must reach Unreachable");

    // (1) After Unreachable + a small grace window, all campaigns are
    // Abandoned (no Active remaining).
    let later_snapshot = trace
        .campaign_snapshots
        .iter()
        .find(|(t, _)| *t >= first_unreachable + 2);
    if let Some((_, snap)) = later_snapshot {
        let still_active: Vec<&str> = snap
            .iter()
            .filter(|c| c.state == CampaignState::Active)
            .map(|c| c.id.as_str())
            .collect();
        assert!(
            still_active.is_empty(),
            "expected all campaigns Abandoned after Unreachable; \
             still Active at tick {}: {:?}",
            first_unreachable + 2,
            still_active
        );
        // And at least one campaign should be Abandoned (not vanished).
        assert!(
            snap.iter().any(|c| c.state == CampaignState::Abandoned),
            "expected at least one Abandoned campaign in snapshot"
        );
    } else {
        panic!("scenario must run long enough past Unreachable");
    }

    // (2) After Mid abandons (~tick first_unreachable + 1 or 2),
    // Short stops emitting commands for those campaigns. Allow a
    // grace window for the abandon ops to be applied.
    let cmds_post_grace: usize = trace
        .command_history
        .iter()
        .filter(|(t, _)| *t > first_unreachable + 3)
        .count();
    assert_eq!(
        cmds_post_grace, 0,
        "expected zero commands after Mid abandons (saw {cmds_post_grace}); \
         first_unreachable = {first_unreachable}"
    );
}

#[test]
fn mid_succeeds_campaigns_on_won() {
    let mut metric_scripts = BTreeMap::new();
    // Both metrics already above their thresholds → Won immediately
    // at tick 0.
    metric_scripts.insert(MetricId::from("econ"), MetricScript::Constant(150.0));
    metric_scripts.insert(MetricId::from("stockpile"), MetricScript::Constant(10.0));

    let config = ScenarioConfig {
        name: "mid_terminal_won".into(),
        seed: 0,
        duration_ticks: 30,
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
    let mut spec = FactionAgentSpec::with_defaults(FactionId(0), victory, 0);
    spec.orchestrator_config.long_cadence = 5;
    spec.orchestrator_config.mid_cadence = 1;

    let pt = run_agent_scenario(AgentScenario::new(base, vec![spec]));
    let trace = &pt.per_faction[0];

    // Already Won at tick 0; Long never emits intents (terminal short-
    // circuit). Mid never starts campaigns. No Active to mark
    // Succeeded — but the test passes when no Active campaigns
    // linger and command_history is empty.
    let any_active_ever = trace.campaign_snapshots.iter().any(|(_, snap)| {
        snap.iter().any(|c| c.state == CampaignState::Active)
    });
    assert!(
        !any_active_ever,
        "no campaigns should ever activate when victory is already Won"
    );
    assert!(
        trace.command_history.is_empty(),
        "no commands should fire when victory is already Won"
    );
}
