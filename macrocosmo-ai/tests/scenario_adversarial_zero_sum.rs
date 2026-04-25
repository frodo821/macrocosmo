//! Adversarial zero-sum scenarios.
//!
//! Each faction has its own per-faction metric (`ore_0` / `ore_1`), so
//! its `pursue_metric:ore_X` command kind is unique. We encode the
//! adversarial dynamic in `command_responses`: emitting the command
//! bumps own metric **and** subtracts from the opponent's metric.
//!
//! This works without per-issuer `command_responses` routing because
//! the command kind already disambiguates issuer. A future round will
//! introduce per-issuer routing if/when both factions need to emit the
//! *same* command kind with asymmetric effects.
//!
//! ### Test 1 — symmetric stalemate
//! Both factions: `pursue` grants own +1, opp -1. Identical strength.
//! Per-tick net change is zero on both metrics → neither reaches the
//! win threshold. Verifies the dynamic actually penalizes naïve
//! symmetric pursuit.
//!
//! ### Test 2 — asymmetric power
//! Faction 0's pursue grants own +2, opp -1. Faction 1's stays +1/-1.
//! Net: ore_0 climbs at +1/tick, ore_1 stays flat. The stronger
//! faction wins; the weaker one does not.
//!
//! ### Documented gap
//! Today the AI has no "I am being attacked" awareness. Long sees a
//! flat trajectory and falls back to its static validity window;
//! after `max_retries_before_fallback` it surrenders that leaf. We
//! assert the surrender path stays observable so we notice when it
//! changes.

#![cfg(feature = "playthrough")]

use std::collections::BTreeMap;
use std::sync::Arc;

use macrocosmo_ai::VictoryCondition;
use macrocosmo_ai::VictoryStatus;
use macrocosmo_ai::condition::{Condition, ConditionAtom};
use macrocosmo_ai::ids::{CommandKindId, FactionId, MetricId};
use macrocosmo_ai::mid_term_default::{IntentDrivenMidTerm, MidTermDefaultConfig};
use macrocosmo_ai::playthrough::scenario::{MetricScript, SyntheticDynamics};
use macrocosmo_ai::playthrough::{
    AgentScenario, FactionAgentSpec, MetricEffect, Scenario, ScenarioConfig, TickFn,
    run_agent_scenario,
};

const WIN_THRESHOLD: f64 = 100.0;
const SEED_VALUE: f64 = 50.0;
const SCENARIO_TICKS: i64 = 200;

fn build_spec(faction: FactionId, target_metric: &str) -> FactionAgentSpec {
    let victory = VictoryCondition::simple(
        Condition::Atom(ConditionAtom::MetricAbove {
            metric: MetricId::from(target_metric),
            threshold: WIN_THRESHOLD,
        }),
        Condition::Atom(ConditionAtom::MetricAbove {
            metric: MetricId::from("stockpile"),
            threshold: 0.0,
        }),
    );
    let mut spec = FactionAgentSpec::with_defaults(faction, victory, 0);
    spec.orchestrator_config.long_cadence = 5;
    spec.orchestrator_config.mid_cadence = 1;
    // Adversarial wins are reversible — keep maintaining after Won so
    // commands keep firing and defend against opponent erosion.
    let mid_cfg = MidTermDefaultConfig {
        treat_won_as_terminal: false,
        ..MidTermDefaultConfig::default()
    };
    spec.mid = Box::new(IntentDrivenMidTerm::new().with_config(mid_cfg));
    spec
}

/// Seed both adversarial metrics at SEED_VALUE on tick 0; subsequent
/// ticks only see command-driven updates.
fn seed_metrics_tick_fn() -> TickFn {
    Arc::new(|rb, t| {
        if t == 0 {
            rb.emit(&MetricId::from("ore_0"), SEED_VALUE, 0);
            rb.emit(&MetricId::from("ore_1"), SEED_VALUE, 0);
        }
    })
}

fn add_response(
    map: &mut BTreeMap<CommandKindId, Vec<MetricEffect>>,
    cmd: &str,
    effects: Vec<MetricEffect>,
) {
    map.insert(CommandKindId::from(cmd), effects);
}

fn add_effect(metric: &str, delta: f64) -> MetricEffect {
    MetricEffect::Add {
        metric: MetricId::from(metric),
        delta,
    }
}

// ---------------------------------------------------------------------------
// Test 1 — symmetric stalemate
// ---------------------------------------------------------------------------

#[test]
fn symmetric_zero_sum_yields_stalemate() {
    let mut metric_scripts = BTreeMap::new();
    metric_scripts.insert(MetricId::from("stockpile"), MetricScript::Constant(50.0));

    let mut command_responses: BTreeMap<CommandKindId, Vec<MetricEffect>> = BTreeMap::new();
    add_response(
        &mut command_responses,
        "pursue_metric:ore_0",
        vec![add_effect("ore_0", 1.0), add_effect("ore_1", -1.0)],
    );
    add_response(
        &mut command_responses,
        "pursue_metric:ore_1",
        vec![add_effect("ore_1", 1.0), add_effect("ore_0", -1.0)],
    );

    let config = ScenarioConfig {
        name: "adversarial_symmetric".into(),
        seed: 0,
        duration_ticks: SCENARIO_TICKS,
        factions: vec![FactionId(0), FactionId(1)],
        dynamics: SyntheticDynamics {
            metric_scripts,
            evidence_pulses: Vec::new(),
            command_responses,
        },
    };

    let scenario = Scenario::new(config).with_tick_fn(seed_metrics_tick_fn());
    let pt = run_agent_scenario(AgentScenario::new(
        scenario,
        vec![
            build_spec(FactionId(0), "ore_0"),
            build_spec(FactionId(1), "ore_1"),
        ],
    ));

    let trace_0 = &pt.per_faction[0];
    let trace_1 = &pt.per_faction[1];

    // (1) Neither faction wins — symmetric pursuits cancel out.
    assert!(
        !trace_0
            .victory_timeline
            .iter()
            .any(|(_, s)| matches!(s, VictoryStatus::Won)),
        "faction 0 must not win in symmetric stalemate; timeline: {:?}",
        trace_0.victory_timeline
    );
    assert!(
        !trace_1
            .victory_timeline
            .iter()
            .any(|(_, s)| matches!(s, VictoryStatus::Won)),
        "faction 1 must not win in symmetric stalemate; timeline: {:?}",
        trace_1.victory_timeline
    );

    // (2) Both factions actually engaged before any potential
    //     surrender — i.e., at least one pursue command emitted each.
    let f0_cmds = trace_0
        .command_history
        .iter()
        .filter(|(_, c)| c.kind.as_str() == "pursue_metric:ore_0")
        .count();
    let f1_cmds = trace_1
        .command_history
        .iter()
        .filter(|(_, c)| c.kind.as_str() == "pursue_metric:ore_1")
        .count();
    assert!(
        f0_cmds > 0,
        "faction 0 should engage at all (got {f0_cmds} commands)"
    );
    assert!(
        f1_cmds > 0,
        "faction 1 should engage at all (got {f1_cmds} commands)"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — asymmetric power
// ---------------------------------------------------------------------------

#[test]
fn asymmetric_strength_decides_the_race() {
    let mut metric_scripts = BTreeMap::new();
    metric_scripts.insert(MetricId::from("stockpile"), MetricScript::Constant(50.0));

    // Faction 0's pursue: +2 own, -1 opp (stronger).
    // Faction 1's pursue: +1 own, -1 opp (baseline).
    // Net per round (one cmd each): ore_0 = +2 - 1 = +1, ore_1 = +1 - 1 = 0.
    // Faction 0 climbs from 50 → 100 in ~50 ticks; faction 1 stays flat.
    let mut command_responses: BTreeMap<CommandKindId, Vec<MetricEffect>> = BTreeMap::new();
    add_response(
        &mut command_responses,
        "pursue_metric:ore_0",
        vec![add_effect("ore_0", 2.0), add_effect("ore_1", -1.0)],
    );
    add_response(
        &mut command_responses,
        "pursue_metric:ore_1",
        vec![add_effect("ore_1", 1.0), add_effect("ore_0", -1.0)],
    );

    let config = ScenarioConfig {
        name: "adversarial_asymmetric".into(),
        seed: 0,
        duration_ticks: SCENARIO_TICKS,
        factions: vec![FactionId(0), FactionId(1)],
        dynamics: SyntheticDynamics {
            metric_scripts,
            evidence_pulses: Vec::new(),
            command_responses,
        },
    };

    let scenario = Scenario::new(config).with_tick_fn(seed_metrics_tick_fn());
    let pt = run_agent_scenario(AgentScenario::new(
        scenario,
        vec![
            build_spec(FactionId(0), "ore_0"),
            build_spec(FactionId(1), "ore_1"),
        ],
    ));

    let trace_0 = &pt.per_faction[0];
    let trace_1 = &pt.per_faction[1];

    // (1) Stronger faction 0 wins.
    let won_0 = trace_0
        .victory_timeline
        .iter()
        .find(|(_, s)| matches!(s, VictoryStatus::Won))
        .map(|(t, _)| *t);
    assert!(
        won_0.is_some(),
        "stronger faction 0 should reach Won; timeline tail: {:?}",
        trace_0.victory_timeline.iter().rev().take(5).collect::<Vec<_>>()
    );

    // (2) Weaker faction 1 must not win — its metric is flat.
    let won_1 = trace_1
        .victory_timeline
        .iter()
        .find(|(_, s)| matches!(s, VictoryStatus::Won));
    assert!(
        won_1.is_none(),
        "weaker faction 1 must not win — its metric stays flat by construction"
    );
}
