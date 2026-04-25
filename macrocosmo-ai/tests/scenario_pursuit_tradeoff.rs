//! Pursuit tradeoff — pursuing the win metric bleeds the prereq.
//!
//! This scenario surfaces a known gap in the default impls: the AI
//! pursues `econ` mechanically every tick without slowing when its
//! prereq metric (`stockpile`) approaches violation. Each command
//! costs more stockpile than passive regen replaces, so without a
//! throttling mechanism the AI runs itself into `Unreachable` —
//! losing despite making "progress" on the win metric.
//!
//! ### Setup
//! - `stockpile`: starts at 30 (via tick_fn at t=0); passive regen
//!   `+0.4 / tick` (also via tick_fn).
//! - `pursue_metric:econ`: each command applies `econ +1` and
//!   `stockpile -1.0`.
//! - `win = econ > 250`, `prereq = stockpile > 0`.
//! - duration 800 ticks.
//!
//! Net change on a pursue tick: `econ +1`, `stockpile -0.6`. Stockpile
//! depletes around tick 50; econ only hits 50 by then. Naive AI loses
//! ~5x earlier than it would have won.
//!
//! ### Phases
//! 1. **Default behavior** (this file): characterize the failure —
//!    AI hits `Unreachable`. This passes when the gap is real.
//! 2. **Guardrail-fix follow-up**: a separate scenario verifies a
//!    Mid-side guardrail (suspend `pursue_metric:*` when prereq is
//!    within `safety_margin` of threshold) lets the AI cycle and win.

#![cfg(feature = "playthrough")]

use std::collections::BTreeMap;
use std::sync::Arc;

use macrocosmo_ai::CampaignReactiveShort;
use macrocosmo_ai::FixedDelayDispatcher;
use macrocosmo_ai::ObjectiveDrivenLongTerm;
use macrocosmo_ai::OrchestratorConfig;
use macrocosmo_ai::VictoryCondition;
use macrocosmo_ai::VictoryStatus;
use macrocosmo_ai::condition::{Condition, ConditionAtom};
use macrocosmo_ai::ids::{CommandKindId, FactionId, MetricId};
use macrocosmo_ai::mid_term_default::{IntentDrivenMidTerm, MidTermDefaultConfig};
use macrocosmo_ai::playthrough::scenario::{MetricScript, SyntheticDynamics};
use macrocosmo_ai::playthrough::{
    AgentScenario, FactionAgentSpec, MetricEffect, Scenario, ScenarioConfig, run_agent_scenario,
};

fn config() -> ScenarioConfig {
    let metric_scripts = BTreeMap::new();

    let mut command_responses: BTreeMap<CommandKindId, Vec<MetricEffect>> = BTreeMap::new();
    command_responses.insert(
        CommandKindId::from("pursue_metric:econ"),
        vec![
            MetricEffect::Add {
                metric: MetricId::from("econ"),
                delta: 1.0,
            },
            MetricEffect::Add {
                metric: MetricId::from("stockpile"),
                delta: -1.0,
            },
        ],
    );

    ScenarioConfig {
        name: "pursuit_tradeoff".into(),
        seed: 0,
        duration_ticks: 800,
        factions: vec![FactionId(0)],
        dynamics: SyntheticDynamics {
            metric_scripts,
            evidence_pulses: Vec::new(),
            command_responses,
        },
    }
}

fn build_scenario_with_passive_regen() -> Scenario {
    Scenario::new(config()).with_tick_fn(Arc::new(|rb, t| {
        let stockpile = MetricId::from("stockpile");
        if t == 0 {
            rb.emit(&stockpile, 30.0, 0);
        } else {
            let cur = rb.bus().current(&stockpile).unwrap_or(0.0);
            rb.emit(&stockpile, cur + 0.4, t);
        }
    }))
}

fn build_spec() -> FactionAgentSpec {
    let victory = VictoryCondition::simple(
        Condition::Atom(ConditionAtom::MetricAbove {
            metric: MetricId::from("econ"),
            threshold: 250.0,
        }),
        Condition::Atom(ConditionAtom::MetricAbove {
            metric: MetricId::from("stockpile"),
            threshold: 0.0,
        }),
    );
    let mut spec = FactionAgentSpec::with_defaults(FactionId(0), victory, 0);
    spec.orchestrator_config.long_cadence = 5;
    spec.orchestrator_config.mid_cadence = 1;
    spec
}

#[test]
fn pursuit_tradeoff_default_drives_to_unreachable() {
    // Characterize the failure mode: default AI bleeds prereq dry by
    // pursuing every tick without throttling.
    let pt = run_agent_scenario(AgentScenario::new(
        build_scenario_with_passive_regen(),
        vec![build_spec()],
    ));
    let trace = &pt.per_faction[0];

    let unreachable_tick = trace
        .victory_timeline
        .iter()
        .find(|(_, s)| matches!(s, VictoryStatus::Unreachable))
        .map(|(t, _)| *t);

    let won_tick = trace
        .victory_timeline
        .iter()
        .find(|(_, s)| matches!(s, VictoryStatus::Won))
        .map(|(t, _)| *t);

    assert!(
        unreachable_tick.is_some(),
        "default AI should hit Unreachable when pursuing too aggressively. \
         won={won_tick:?}, total_pursue_cmds={}, final_status={:?}",
        trace
            .command_history
            .iter()
            .filter(|(_, c)| c.kind.as_str() == "pursue_metric:econ")
            .count(),
        trace.victory_timeline.last().map(|(_, s)| s.clone()),
    );

    if let Some(won) = won_tick {
        assert!(
            won > unreachable_tick.unwrap(),
            "AI should not have already won before bleeding out (saw Won \
             at {won}, Unreachable at {})",
            unreachable_tick.unwrap()
        );
    }

    // Sanity: AI emitted a meaningful number of pursue commands before
    // losing.
    let pursue_count = trace
        .command_history
        .iter()
        .filter(|(_, c)| c.kind.as_str() == "pursue_metric:econ")
        .count();
    assert!(
        pursue_count >= 30,
        "expected >=30 pursue commands before bleed-out, got {pursue_count}"
    );
}

/// Same scenario as above, but Mid runs with `prereq_guardrail = 5.0`
/// — pursue campaigns suspend whenever stockpile is within 5.0 of
/// the threshold. Pursue can then breathe in step with passive regen,
/// and the AI eventually wins.
#[test]
fn pursuit_tradeoff_with_guardrail_wins() {
    let victory = VictoryCondition::simple(
        Condition::Atom(ConditionAtom::MetricAbove {
            metric: MetricId::from("econ"),
            threshold: 250.0,
        }),
        Condition::Atom(ConditionAtom::MetricAbove {
            metric: MetricId::from("stockpile"),
            threshold: 0.0,
        }),
    );
    let mid = IntentDrivenMidTerm::new().with_config(MidTermDefaultConfig {
        prereq_guardrail: 5.0,
        ..MidTermDefaultConfig::default()
    });
    let spec = FactionAgentSpec {
        faction: FactionId(0),
        victory,
        long: Box::new(ObjectiveDrivenLongTerm::new()),
        mid: Box::new(mid),
        short: Box::new(CampaignReactiveShort::new()),
        dispatcher: Box::new(FixedDelayDispatcher::zero_delay()),
        orchestrator_config: {
            let mut c = OrchestratorConfig::default();
            c.long_cadence = 5;
            c.mid_cadence = 1;
            c
        },
    };

    let pt = run_agent_scenario(AgentScenario::new(
        build_scenario_with_passive_regen(),
        vec![spec],
    ));
    let trace = &pt.per_faction[0];

    let won_tick = trace
        .victory_timeline
        .iter()
        .find(|(_, s)| matches!(s, VictoryStatus::Won))
        .map(|(t, _)| *t);
    let unreachable_tick = trace
        .victory_timeline
        .iter()
        .find(|(_, s)| matches!(s, VictoryStatus::Unreachable))
        .map(|(t, _)| *t);

    assert!(
        unreachable_tick.is_none(),
        "guardrailed AI must not bleed out (got Unreachable at tick {:?})",
        unreachable_tick
    );
    assert!(
        won_tick.is_some(),
        "guardrailed AI should still reach victory by cycling pursue/suspend; \
         final={:?}",
        trace.victory_timeline.last().map(|(_, s)| s.clone())
    );

    // Sanity: Mid did issue Suspend transitions on the pursue campaign.
    let suspend_ops_in_log = trace
        .campaign_snapshots
        .iter()
        .filter_map(|(_, snap)| {
            snap.iter()
                .find(|c| {
                    c.id.as_str().starts_with("pursue_metric:")
                        && c.state == macrocosmo_ai::campaign::CampaignState::Suspended
                })
                .map(|_| ())
        })
        .count();
    assert!(
        suspend_ops_in_log > 0,
        "expected the guardrail to have suspended pursue at least once across \
         the campaign snapshots"
    );
}
