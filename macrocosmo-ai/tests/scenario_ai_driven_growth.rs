//! AI-driven growth — demonstrates the command→metric feedback loop.
//!
//! Natural evolution alone is insufficient: `econ` grows from 0 to 5
//! over 500 ticks (well below the win threshold). But the short-term
//! agent issues a `pursue_metric:econ` command every tick once the
//! corresponding campaign is active, and `command_responses` declares
//! that each firing adds `+0.5` to `econ`. Over ~200-300 ticks of
//! accumulating commands, `econ` crosses the win threshold.
//!
//! The paired baseline test runs the exact same scenario **without**
//! the command→metric feedback and asserts victory is NOT reached —
//! which pins down that the AI's intervention is what's causing the
//! pass.
//!
//! This is the first scenario that exercises the full closed loop:
//! Long emits intents → Mid starts campaign → Short issues commands
//! → commands move the metric → next tick Long observes progress →
//! win eventually reached.

#![cfg(feature = "playthrough")]

use std::collections::BTreeMap;

use macrocosmo_ai::condition::{Condition, ConditionAtom};
use macrocosmo_ai::ids::{CommandKindId, FactionId, MetricId};
use macrocosmo_ai::playthrough::scenario::{MetricScript, SyntheticDynamics};
use macrocosmo_ai::playthrough::{
    AgentScenario, FactionAgentSpec, MetricEffect, run_agent_scenario, Scenario, ScenarioConfig,
};
use macrocosmo_ai::VictoryCondition;
use macrocosmo_ai::VictoryStatus;

fn base_config(command_feedback: bool) -> ScenarioConfig {
    let mut metric_scripts = BTreeMap::new();
    // econ is **not** scripted — it accumulates only via command
    // feedback. If the scenario has command_responses, each matching
    // command adds 0.5 to econ; otherwise econ never gets a value
    // and win stays False permanently. (Scripting econ here would
    // defeat the feedback test: the script re-sets econ every tick,
    // erasing the accumulated delta.)
    //
    // Prereq always satisfied.
    metric_scripts.insert(MetricId::from("stockpile"), MetricScript::Constant(10.0));

    let mut command_responses: BTreeMap<CommandKindId, Vec<MetricEffect>> = BTreeMap::new();
    if command_feedback {
        command_responses.insert(
            CommandKindId::from("pursue_metric:econ"),
            vec![MetricEffect::Add {
                metric: MetricId::from("econ"),
                delta: 0.5,
            }],
        );
    }

    ScenarioConfig {
        name: if command_feedback {
            "ai_driven_growth".into()
        } else {
            "ai_driven_growth_baseline".into()
        },
        seed: 0,
        duration_ticks: 500,
        factions: vec![FactionId(0)],
        dynamics: SyntheticDynamics {
            metric_scripts,
            evidence_pulses: Vec::new(),
            command_responses,
        },
    }
}

fn build_spec() -> FactionAgentSpec {
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
    spec.orchestrator_config.long_cadence = 10;
    spec.orchestrator_config.mid_cadence = 1;
    spec
}

#[test]
fn ai_drives_econ_to_victory_via_commands() {
    let cfg = base_config(true);
    let base = Scenario::new(cfg);
    let pt = run_agent_scenario(AgentScenario::new(base, vec![build_spec()]));
    let trace = &pt.per_faction[0];

    // Victory reached.
    let first_won = trace
        .victory_timeline
        .iter()
        .find(|(_, s)| matches!(s, VictoryStatus::Won))
        .map(|(t, _)| *t)
        .expect(
            "AI's own commands should push econ over 100 well before \
             scenario end (natural growth alone only reaches 5)",
        );
    // Natural growth = 5 over 500 ticks = 0.01/tick. AI adds 0.5/tick
    // once the campaign is active — 100 / 0.5 ≈ 200 ticks of commands,
    // plus ~20 ticks of ramp-up. Allow margin.
    assert!(
        (150..=400).contains(&first_won),
        "expected Won around tick 200-300; got {first_won}"
    );

    // Commands actually fired — use the trace to confirm.
    let pursue_count = trace
        .command_history
        .iter()
        .filter(|(_, c)| c.kind.as_str() == "pursue_metric:econ")
        .count();
    assert!(
        pursue_count > 50,
        "expected many pursue_metric:econ commands, got {pursue_count}"
    );
}

#[test]
fn without_feedback_scenario_does_not_reach_victory() {
    // Same setup but command_responses is empty → AI's commands don't
    // affect econ → natural growth stops at 5 → never wins.
    let cfg = base_config(false);
    let base = Scenario::new(cfg);
    let pt = run_agent_scenario(AgentScenario::new(base, vec![build_spec()]));
    let trace = &pt.per_faction[0];

    let reached_won = trace
        .victory_timeline
        .iter()
        .any(|(_, s)| matches!(s, VictoryStatus::Won));
    assert!(
        !reached_won,
        "baseline (no feedback): AI cannot win without commands affecting metrics"
    );

    // Sanity: commands did fire (AI was actively trying), it just had
    // no effect without the feedback wiring.
    let pursue_count = trace
        .command_history
        .iter()
        .filter(|(_, c)| c.kind.as_str() == "pursue_metric:econ")
        .count();
    assert!(
        pursue_count > 50,
        "AI should still have emitted commands even in baseline; got {pursue_count}"
    );
}
