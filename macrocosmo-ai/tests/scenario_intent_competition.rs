//! Intent competition — two simultaneous pursuits draining a shared
//! budget metric.
//!
//! Setup:
//! - `win = All(econ > 200, military > 100)`
//! - `pursue_metric:econ`: econ +1, stockpile -0.5
//! - `pursue_metric:military`: military +0.5, stockpile -0.5
//! - `stockpile`: starts 60, passive regen +0.6/tick (via tick_fn).
//!
//! Naive default (both campaigns always Active, 1 command/tick each):
//! - per-tick stockpile delta = +0.6 - 0.5 - 0.5 = -0.4
//! - stockpile dies at tick 60 / 0.4 = ~150
//! - by then econ = 150 (still short of 200), military = 75 (short of 100)
//! - → `Unreachable`
//!
//! With `prereq_guardrail = 5.0`, both pursue campaigns suspend when
//! stockpile drops near threshold; they resume in lockstep when
//! stockpile recovers. The "lockstep" behaviour is the simplest form
//! of multi-pursuit throttling — no priority differentiation, but
//! enough to keep the prereq alive while metrics tick up over time.
//! Eventually both win conditions are satisfied → `Won`.
//!
//! What this scenario does **not** test (a future gap):
//! - When budget is too tight to support both pursuits even with
//!   throttling, the default Mid has no notion of "pick one and
//!   focus". Priority-weighted command emission is the natural
//!   follow-up.

#![cfg(feature = "playthrough")]

use std::collections::BTreeMap;
use std::sync::Arc;

use macrocosmo_ai::campaign::CampaignState;
use macrocosmo_ai::condition::{Condition, ConditionAtom};
use macrocosmo_ai::ids::{CommandKindId, FactionId, MetricId};
use macrocosmo_ai::mid_term_default::{IntentDrivenMidTerm, MidTermDefaultConfig};
use macrocosmo_ai::playthrough::scenario::SyntheticDynamics;
use macrocosmo_ai::playthrough::{
    AgentScenario, FactionAgentSpec, MetricEffect, run_agent_scenario, Scenario, ScenarioConfig,
};
use macrocosmo_ai::CampaignReactiveShort;
use macrocosmo_ai::FixedDelayDispatcher;
use macrocosmo_ai::ObjectiveDrivenLongTerm;
use macrocosmo_ai::OrchestratorConfig;
use macrocosmo_ai::VictoryCondition;
use macrocosmo_ai::VictoryStatus;

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
                delta: -0.5,
            },
        ],
    );
    command_responses.insert(
        CommandKindId::from("pursue_metric:military"),
        vec![
            MetricEffect::Add {
                metric: MetricId::from("military"),
                delta: 0.5,
            },
            MetricEffect::Add {
                metric: MetricId::from("stockpile"),
                delta: -0.5,
            },
        ],
    );

    ScenarioConfig {
        name: "intent_competition".into(),
        seed: 0,
        duration_ticks: 1500,
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
            rb.emit(&stockpile, 60.0, 0);
        } else {
            let cur = rb.bus().current(&stockpile).unwrap_or(0.0);
            rb.emit(&stockpile, cur + 0.6, t);
        }
    }))
}

fn victory() -> VictoryCondition {
    VictoryCondition::simple(
        Condition::All(vec![
            Condition::Atom(ConditionAtom::MetricAbove {
                metric: MetricId::from("econ"),
                threshold: 200.0,
            }),
            Condition::Atom(ConditionAtom::MetricAbove {
                metric: MetricId::from("military"),
                threshold: 100.0,
            }),
        ]),
        Condition::Atom(ConditionAtom::MetricAbove {
            metric: MetricId::from("stockpile"),
            threshold: 0.0,
        }),
    )
}

fn spec_with_mid(mid: IntentDrivenMidTerm) -> FactionAgentSpec {
    FactionAgentSpec {
        faction: FactionId(0),
        victory: victory(),
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
    }
}

#[test]
fn intent_competition_default_drives_to_unreachable() {
    // Naive default (no guardrail): both pursuits drain stockpile too fast.
    let pt = run_agent_scenario(AgentScenario::new(
        build_scenario_with_passive_regen(),
        vec![spec_with_mid(IntentDrivenMidTerm::new())],
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
        "default AI should hit Unreachable from competing pursuits draining stockpile. \
         won={won_tick:?}, final={:?}",
        trace.victory_timeline.last().map(|(_, s)| s.clone())
    );

    // Sanity: both pursuit campaigns existed at some point.
    let pursue_kinds: std::collections::HashSet<&str> = trace
        .command_history
        .iter()
        .map(|(_, c)| c.kind.as_str())
        .collect();
    assert!(
        pursue_kinds.contains("pursue_metric:econ"),
        "expected pursue_metric:econ commands"
    );
    assert!(
        pursue_kinds.contains("pursue_metric:military"),
        "expected pursue_metric:military commands"
    );
}

#[test]
fn intent_competition_with_guardrail_wins() {
    // With the guardrail active, both pursuits suspend together when
    // the prereq budget is in danger and resume when it recovers —
    // bleed is bounded, both metrics climb (slower) until both win
    // legs cross.
    let mid = IntentDrivenMidTerm::new().with_config(MidTermDefaultConfig {
        prereq_guardrail: 5.0,
        ..MidTermDefaultConfig::default()
    });
    let pt = run_agent_scenario(AgentScenario::new(
        build_scenario_with_passive_regen(),
        vec![spec_with_mid(mid)],
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
        unreachable_tick.is_none(),
        "guardrailed AI must not bleed out (saw Unreachable at tick {:?})",
        unreachable_tick
    );
    assert!(
        won_tick.is_some(),
        "guardrailed AI should reach Won by cycling pursue/suspend across both \
         campaigns; final={:?}",
        trace.victory_timeline.last().map(|(_, s)| s.clone())
    );

    // Both campaigns suspended at some point (lockstep guardrail).
    let suspended_econ = trace.campaign_snapshots.iter().any(|(_, snap)| {
        snap.iter().any(|c| {
            c.id.as_str() == "pursue_metric:econ" && c.state == CampaignState::Suspended
        })
    });
    let suspended_military = trace.campaign_snapshots.iter().any(|(_, snap)| {
        snap.iter().any(|c| {
            c.id.as_str() == "pursue_metric:military" && c.state == CampaignState::Suspended
        })
    });
    assert!(
        suspended_econ && suspended_military,
        "both pursuit campaigns should suspend at least once under guardrail; \
         econ_suspended={suspended_econ}, military_suspended={suspended_military}"
    );
}
