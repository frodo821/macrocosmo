//! Priority-weighted command emission.
//!
//! Two pursue campaigns are simultaneously Active, with markedly
//! different priority. The default short-term agent (legacy mode)
//! emits one command per active campaign per tick — the priority is
//! ignored and the metric counts come out roughly equal. The
//! priority-weighted mode allocates per-campaign command budget
//! proportional to `Campaign.weight` (which Mid stamps from
//! `intent.priority * intent.importance`), so the high-priority
//! pursuit fires substantially more often than the low-priority one.
//!
//! ### Setup
//! - `vital`: only modifiable via `pursue_metric:vital` (each cmd +1).
//! - `cosmetic`: only modifiable via `pursue_metric:cosmetic` (each cmd +1).
//! - Long agent: a custom impl that emits two intents on tick 0 with
//!   different priorities (vital=0.9, cosmetic=0.3) and never re-emits.
//! - Win condition is irrelevant — we run for a fixed duration and
//!   measure command counts.
//!
//! Expected ratio under priority weighting: about
//! `0.9*0.9 / 0.3*0.9 = 3.0` (importance defaults to 0.9 in the
//! custom intent). We require ratio > 2.5 in weighted mode and
//! ratio in `[0.7, 1.3]` in legacy mode.

#![cfg(feature = "playthrough")]

use std::collections::BTreeMap;

use macrocosmo_ai::agent::{LongTermAgent, LongTermInput, LongTermOutput};
use macrocosmo_ai::condition::Condition;
use macrocosmo_ai::ids::{CommandKindId, FactionId, IntentKindId, IntentTargetRef, MetricId};
use macrocosmo_ai::playthrough::scenario::SyntheticDynamics;
use macrocosmo_ai::playthrough::{
    AgentScenario, FactionAgentSpec, MetricEffect, run_agent_scenario, Scenario, ScenarioConfig,
};
use macrocosmo_ai::{
    CampaignReactiveShort, FixedDelayDispatcher, IntentDrivenMidTerm, IntentParams, IntentSpec,
    OrchestratorConfig, RationaleSnapshot, ShortTermDefaultConfig, VictoryCondition,
};

/// One-shot Long agent: emits two intents at tick 0, then nothing.
struct EmitTwoOnce {
    fired: bool,
}

impl LongTermAgent for EmitTwoOnce {
    fn tick(&mut self, _input: LongTermInput<'_>) -> LongTermOutput {
        if self.fired {
            return LongTermOutput::default();
        }
        self.fired = true;
        LongTermOutput {
            intents: vec![
                IntentSpec {
                    kind: IntentKindId::from("pursue_metric"),
                    params: IntentParams::new().with(
                        "metric:vital",
                        macrocosmo_ai::ValueExpr::Literal(0.0),
                    ),
                    priority: 0.9,
                    importance: 0.9,
                    half_life: None,
                    expires_at_offset: None,
                    rationale: RationaleSnapshot::empty(),
                    supersedes: None,
                    target: IntentTargetRef::from("faction"),
                    delivery_hint: None,
                },
                IntentSpec {
                    kind: IntentKindId::from("pursue_metric"),
                    params: IntentParams::new().with(
                        "metric:cosmetic",
                        macrocosmo_ai::ValueExpr::Literal(0.0),
                    ),
                    priority: 0.3,
                    importance: 0.9,
                    half_life: None,
                    expires_at_offset: None,
                    rationale: RationaleSnapshot::empty(),
                    supersedes: None,
                    target: IntentTargetRef::from("faction"),
                    delivery_hint: None,
                },
            ],
        }
    }
}

fn config() -> ScenarioConfig {
    let metric_scripts = BTreeMap::new();
    let mut command_responses: BTreeMap<CommandKindId, Vec<MetricEffect>> = BTreeMap::new();
    command_responses.insert(
        CommandKindId::from("pursue_metric:vital"),
        vec![MetricEffect::Add {
            metric: MetricId::from("vital"),
            delta: 1.0,
        }],
    );
    command_responses.insert(
        CommandKindId::from("pursue_metric:cosmetic"),
        vec![MetricEffect::Add {
            metric: MetricId::from("cosmetic"),
            delta: 1.0,
        }],
    );
    ScenarioConfig {
        name: "priority_weighted".into(),
        seed: 0,
        duration_ticks: 200,
        factions: vec![FactionId(0)],
        dynamics: SyntheticDynamics {
            metric_scripts,
            evidence_pulses: Vec::new(),
            command_responses,
        },
    }
}

fn build_spec(weighted: bool) -> FactionAgentSpec {
    // Victory irrelevant — run to completion observing counts.
    let victory = VictoryCondition::simple(Condition::Never, Condition::Always);
    let short_cfg = ShortTermDefaultConfig {
        priority_weighted: weighted,
        ..ShortTermDefaultConfig::default()
    };
    FactionAgentSpec {
        faction: FactionId(0),
        victory,
        long: Box::new(EmitTwoOnce { fired: false }),
        mid: Box::new(IntentDrivenMidTerm::new()),
        short: Box::new(CampaignReactiveShort::new().with_config(short_cfg)),
        dispatcher: Box::new(FixedDelayDispatcher::zero_delay()),
        orchestrator_config: {
            let mut c = OrchestratorConfig::default();
            c.long_cadence = 1;
            c.mid_cadence = 1;
            c
        },
    }
}

fn count_kind(trace: &macrocosmo_ai::playthrough::FactionTrace, kind: &str) -> usize {
    trace
        .command_history
        .iter()
        .filter(|(_, c)| c.kind.as_str() == kind)
        .count()
}

#[test]
fn legacy_mode_emits_equal_counts() {
    let pt = run_agent_scenario(AgentScenario::new(
        Scenario::new(config()),
        vec![build_spec(false)],
    ));
    let trace = &pt.per_faction[0];
    let vital = count_kind(trace, "pursue_metric:vital");
    let cosmetic = count_kind(trace, "pursue_metric:cosmetic");

    assert!(vital > 100, "expected most ticks to fire vital, got {vital}");
    assert!(
        cosmetic > 100,
        "expected most ticks to fire cosmetic, got {cosmetic}"
    );
    let ratio = vital as f64 / cosmetic as f64;
    assert!(
        (0.7..=1.3).contains(&ratio),
        "legacy mode should fire roughly 1:1 (vital={vital}, cosmetic={cosmetic}, ratio={ratio:.3})"
    );
}

#[test]
fn weighted_mode_skews_toward_high_priority() {
    let pt = run_agent_scenario(AgentScenario::new(
        Scenario::new(config()),
        vec![build_spec(true)],
    ));
    let trace = &pt.per_faction[0];
    let vital = count_kind(trace, "pursue_metric:vital");
    let cosmetic = count_kind(trace, "pursue_metric:cosmetic");

    // vital weight = 0.9 * 0.9 = 0.81; cosmetic weight = 0.3 * 0.9 = 0.27.
    // Expected ratio ≈ 3.0.
    assert!(
        cosmetic > 0,
        "cosmetic should still fire occasionally, got {cosmetic}"
    );
    let ratio = vital as f64 / cosmetic as f64;
    assert!(
        ratio > 2.5,
        "weighted mode should skew >2.5:1 toward vital \
         (vital={vital}, cosmetic={cosmetic}, ratio={ratio:.3})"
    );

    // Sanity: total commands ≈ sum(weights) * duration. For 200 ticks
    // and weights summing to 1.08, expect ~216 commands. Allow margin.
    let total = vital + cosmetic;
    assert!(
        (180..=240).contains(&total),
        "weighted total should be near sum_weights * duration ≈ 216; got {total}"
    );
}
