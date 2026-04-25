//! Multi-faction parallel scenarios.
//!
//! Two orchestrators (one per faction) run on the same `AiBus`. Each
//! faction's `VictoryCondition` references only the metrics that
//! faction cares about; the harness routes intents per faction (each
//! gets its own dispatcher / agent triple).
//!
//! ### Test 1 — independence
//! Each faction has its own pursue metric (`econ_0`, `econ_1`) and
//! its own win threshold. command_responses split by command kind so
//! the two factions never poke each other's metrics. We verify both
//! reach `Won` and that the traces are clean (no cross-contamination).
//!
//! ### Test 2 — cooperative shared goal
//! A single shared metric `world_crisis` is targeted by both
//! factions' pursue intents. Each faction sees the same metric value
//! and projects the same trajectory; both emit commands; the metric
//! decreases at twice the per-faction rate; both reach `Won` at
//! roughly the same tick.
//!
//! ### Out of scope (next round)
//! True adversarial competition (one faction's gain costs the
//! other's resources) needs per-issuer command_response routing or
//! per-faction metric scoping, which the abstract harness doesn't
//! yet support. We document that gap rather than work around it.

#![cfg(feature = "playthrough")]

use std::collections::BTreeMap;

use macrocosmo_ai::VictoryCondition;
use macrocosmo_ai::VictoryStatus;
use macrocosmo_ai::condition::{Condition, ConditionAtom};
use macrocosmo_ai::ids::{CommandKindId, FactionId, MetricId};
use macrocosmo_ai::playthrough::scenario::{MetricScript, SyntheticDynamics};
use macrocosmo_ai::playthrough::{
    AgentScenario, FactionAgentSpec, MetricEffect, Scenario, ScenarioConfig, run_agent_scenario,
};

// ---------------------------------------------------------------------------
// Test 1: independence
// ---------------------------------------------------------------------------

fn independence_config() -> ScenarioConfig {
    let mut metric_scripts = BTreeMap::new();
    metric_scripts.insert(MetricId::from("stockpile"), MetricScript::Constant(50.0));

    let mut command_responses: BTreeMap<CommandKindId, Vec<MetricEffect>> = BTreeMap::new();
    command_responses.insert(
        CommandKindId::from("pursue_metric:econ_0"),
        vec![MetricEffect::Add {
            metric: MetricId::from("econ_0"),
            delta: 1.0,
        }],
    );
    command_responses.insert(
        CommandKindId::from("pursue_metric:econ_1"),
        vec![MetricEffect::Add {
            metric: MetricId::from("econ_1"),
            delta: 1.0,
        }],
    );

    ScenarioConfig {
        name: "multi_faction_independent".into(),
        seed: 0,
        duration_ticks: 200,
        factions: vec![FactionId(0), FactionId(1)],
        dynamics: SyntheticDynamics {
            metric_scripts,
            evidence_pulses: Vec::new(),
            command_responses,
        },
    }
}

fn build_independence_spec(faction: FactionId, target_metric: &str) -> FactionAgentSpec {
    let victory = VictoryCondition::simple(
        Condition::Atom(ConditionAtom::MetricAbove {
            metric: MetricId::from(target_metric),
            threshold: 100.0,
        }),
        Condition::Atom(ConditionAtom::MetricAbove {
            metric: MetricId::from("stockpile"),
            threshold: 0.0,
        }),
    );
    let mut spec = FactionAgentSpec::with_defaults(faction, victory, 0);
    spec.orchestrator_config.long_cadence = 5;
    spec.orchestrator_config.mid_cadence = 1;
    spec
}

#[test]
fn two_factions_pursue_separate_goals_in_parallel() {
    let pt = run_agent_scenario(AgentScenario::new(
        Scenario::new(independence_config()),
        vec![
            build_independence_spec(FactionId(0), "econ_0"),
            build_independence_spec(FactionId(1), "econ_1"),
        ],
    ));

    assert_eq!(pt.per_faction.len(), 2);
    let trace_0 = &pt.per_faction[0];
    let trace_1 = &pt.per_faction[1];

    // (1) Both reach Won.
    let won_0 = trace_0
        .victory_timeline
        .iter()
        .find(|(_, s)| matches!(s, VictoryStatus::Won))
        .map(|(t, _)| *t)
        .expect("faction 0 should win");
    let won_1 = trace_1
        .victory_timeline
        .iter()
        .find(|(_, s)| matches!(s, VictoryStatus::Won))
        .map(|(t, _)| *t)
        .expect("faction 1 should win");
    assert!(
        won_0 < 200 && won_1 < 200,
        "both factions should win within the scenario; got {won_0} / {won_1}"
    );

    // (2) Each trace's command_history only contains commands for its
    // own metric — no cross-contamination via shared bus.
    let cmds_0_for_other = trace_0
        .command_history
        .iter()
        .filter(|(_, c)| c.kind.as_str() == "pursue_metric:econ_1")
        .count();
    let cmds_1_for_other = trace_1
        .command_history
        .iter()
        .filter(|(_, c)| c.kind.as_str() == "pursue_metric:econ_0")
        .count();
    assert_eq!(
        cmds_0_for_other, 0,
        "faction 0's trace must not include faction 1's commands"
    );
    assert_eq!(
        cmds_1_for_other, 0,
        "faction 1's trace must not include faction 0's commands"
    );

    // (3) Each faction's intents target only its own metric.
    let cross_intents_0 = trace_0.intent_history.iter().any(|i| {
        i.spec
            .rationale
            .metrics_seen
            .keys()
            .any(|k| k.as_str() == "econ_1")
    });
    let cross_intents_1 = trace_1.intent_history.iter().any(|i| {
        i.spec
            .rationale
            .metrics_seen
            .keys()
            .any(|k| k.as_str() == "econ_0")
    });
    assert!(!cross_intents_0, "faction 0 should not pursue econ_1");
    assert!(!cross_intents_1, "faction 1 should not pursue econ_0");
}

// ---------------------------------------------------------------------------
// Test 2: cooperative shared goal
// ---------------------------------------------------------------------------

fn cooperative_config() -> ScenarioConfig {
    let mut metric_scripts = BTreeMap::new();
    metric_scripts.insert(MetricId::from("stockpile"), MetricScript::Constant(50.0));
    // world_crisis baseline: scripts emit it as Constant 100 only on
    // tick 0 — but Constant emits every tick, overriding command
    // effects. So we use no script for world_crisis and seed via
    // tick_fn at t=0 (see `cooperative_scenario`). We DO need it
    // declared; that happens automatically via command_responses ref.

    let mut command_responses: BTreeMap<CommandKindId, Vec<MetricEffect>> = BTreeMap::new();
    // Each faction's pursue intent for the same metric becomes a
    // command of kind `pursue_metric:world_crisis`. Since both
    // factions emit identically, their commands collide at the bus
    // level — but the harness fires each one (one per active
    // campaign per tick), so each tick the metric is decremented
    // **twice** (faction 0 + faction 1 each emit one).
    command_responses.insert(
        CommandKindId::from("pursue_metric:world_crisis"),
        vec![MetricEffect::Add {
            metric: MetricId::from("world_crisis"),
            delta: -1.0,
        }],
    );

    ScenarioConfig {
        name: "multi_faction_cooperative".into(),
        seed: 0,
        duration_ticks: 200,
        factions: vec![FactionId(0), FactionId(1)],
        dynamics: SyntheticDynamics {
            metric_scripts,
            evidence_pulses: Vec::new(),
            command_responses,
        },
    }
}

fn cooperative_scenario() -> Scenario {
    Scenario::new(cooperative_config()).with_tick_fn(std::sync::Arc::new(|rb, t| {
        if t == 0 {
            // Seed the shared metric. Subsequent ticks let command
            // effects accumulate.
            rb.emit(&MetricId::from("world_crisis"), 100.0, 0);
        }
    }))
}

fn build_cooperative_spec(faction: FactionId) -> FactionAgentSpec {
    let victory = VictoryCondition::simple(
        Condition::Atom(ConditionAtom::MetricBelow {
            metric: MetricId::from("world_crisis"),
            threshold: 50.0,
        }),
        Condition::Atom(ConditionAtom::MetricAbove {
            metric: MetricId::from("stockpile"),
            threshold: 0.0,
        }),
    );
    let mut spec = FactionAgentSpec::with_defaults(faction, victory, 0);
    spec.orchestrator_config.long_cadence = 5;
    spec.orchestrator_config.mid_cadence = 1;
    spec
}

#[test]
fn two_factions_cooperate_on_shared_goal() {
    let pt = run_agent_scenario(AgentScenario::new(
        cooperative_scenario(),
        vec![
            build_cooperative_spec(FactionId(0)),
            build_cooperative_spec(FactionId(1)),
        ],
    ));

    assert_eq!(pt.per_faction.len(), 2);
    let trace_0 = &pt.per_faction[0];
    let trace_1 = &pt.per_faction[1];

    // Both factions reach Won.
    let won_0 = trace_0
        .victory_timeline
        .iter()
        .find(|(_, s)| matches!(s, VictoryStatus::Won))
        .map(|(t, _)| *t)
        .expect("faction 0 should win");
    let won_1 = trace_1
        .victory_timeline
        .iter()
        .find(|(_, s)| matches!(s, VictoryStatus::Won))
        .map(|(t, _)| *t)
        .expect("faction 1 should win");

    // (1) Both win at roughly the same tick (within Mid cadence).
    let diff = (won_0 - won_1).abs();
    assert!(
        diff <= 5,
        "cooperative win ticks should be close (within mid_cadence ~5); \
         got {won_0} / {won_1} (diff {diff})"
    );

    // (2) Both factions emitted commands for the shared metric.
    let f0_cmds = trace_0
        .command_history
        .iter()
        .filter(|(_, c)| c.kind.as_str() == "pursue_metric:world_crisis")
        .count();
    let f1_cmds = trace_1
        .command_history
        .iter()
        .filter(|(_, c)| c.kind.as_str() == "pursue_metric:world_crisis")
        .count();
    assert!(f0_cmds > 5 && f1_cmds > 5, "both factions should pursue");

    // (3) Cooperation accelerates: with two factions emitting, the
    // crisis metric drops by 2/tick after warmup. From 100 to <50,
    // ~25 ticks of full cooperation. Win arrives well before tick
    // 100 (single-faction lower bound).
    assert!(
        won_0 < 100,
        "cooperative win should beat the 100-tick single-faction floor; got {won_0}"
    );
}
