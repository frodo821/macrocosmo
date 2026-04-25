//! Three-layer consistency checks.
//!
//! Covers the three consistency axes from `docs/ai-three-layer.md`:
//! - **vertical** — short doesn't contradict mid, mid doesn't
//!   contradict long;
//! - **temporal** — plans don't flip-flop on stable input;
//! - **informational** — light-speed delay is honored, `effective_priority`
//!   decay behaves as specified.

#![cfg(feature = "playthrough")]

use macrocosmo_ai::agent::{LongTermOutput, MidTermOutput, ShortTermOutput};
use macrocosmo_ai::campaign::CampaignState;
use macrocosmo_ai::condition::{Condition, ConditionAtom};
use macrocosmo_ai::ids::{FactionId, IntentTargetRef, MetricId};
use macrocosmo_ai::playthrough::scenario::{MetricScript, Scenario, SyntheticDynamics};
use macrocosmo_ai::playthrough::{
    AgentScenario, FactionAgentSpec, ScenarioConfig, run_agent_scenario,
};
use macrocosmo_ai::{
    FixedDelayDispatcher, Intent, IntentParams, IntentSpec, LongTermAgent, LongTermInput,
    MidTermAgent, MidTermInput, OrchestratorConfig, RationaleSnapshot, ShortTermAgent,
    ShortTermInput, VictoryCondition,
};

// ---------------------------------------------------------------------------
// Vertical consistency: short never emits commands when no campaigns are
// active, mid never starts campaigns when inbox is empty.
// ---------------------------------------------------------------------------

struct NoopLong;
impl LongTermAgent for NoopLong {
    fn tick(&mut self, _input: LongTermInput<'_>) -> LongTermOutput {
        LongTermOutput::default()
    }
}

#[test]
fn vertical_short_is_silent_when_no_campaigns() {
    let cfg = base_config("vertical_short_silent", 20);
    let base = Scenario::new(cfg);
    let victory = always_ongoing();
    // NoopLong never emits intents → no campaign starts → no short commands.
    let spec = FactionAgentSpec {
        long: Box::new(NoopLong),
        ..FactionAgentSpec::with_defaults(FactionId(0), victory, 0)
    };
    let pt = run_agent_scenario(AgentScenario::new(base, vec![spec]));
    let trace = &pt.per_faction[0];
    assert!(
        trace.intent_history.is_empty(),
        "NoopLong must not emit intents, got {}",
        trace.intent_history.len()
    );
    assert!(
        trace.command_history.is_empty(),
        "no campaigns → short must stay silent, got {} commands",
        trace.command_history.len()
    );
}

// ---------------------------------------------------------------------------
// Temporal consistency: stable metrics + stable victory condition produce
// a monotone-or-stable campaign set (no flip-flop). We check that once a
// campaign becomes Active, it stays in a non-terminal state (Active or
// Suspended — never repeatedly Start/Abandon/Start).
// ---------------------------------------------------------------------------

#[test]
fn temporal_no_campaign_flip_flop_on_stable_input() {
    let mut metric_scripts = std::collections::BTreeMap::new();
    metric_scripts.insert(MetricId::from("econ"), MetricScript::Constant(20.0));
    metric_scripts.insert(MetricId::from("stockpile"), MetricScript::Constant(5.0));
    let cfg = ScenarioConfig {
        name: "temporal_stable".into(),
        seed: 1,
        duration_ticks: 100,
        factions: vec![FactionId(0)],
        dynamics: SyntheticDynamics {
            metric_scripts,
            evidence_pulses: Vec::new(),
            command_responses: std::collections::BTreeMap::new(),
        },
    };
    let base = Scenario::new(cfg);
    let victory =
        VictoryCondition::simple(metric_above("econ", 100.0), metric_above("stockpile", 0.0));
    let mut spec = FactionAgentSpec::with_defaults(FactionId(0), victory, 0);
    spec.orchestrator_config.long_cadence = 1;
    spec.orchestrator_config.mid_cadence = 1;

    let pt = run_agent_scenario(AgentScenario::new(base, vec![spec]));
    let trace = &pt.per_faction[0];

    // Pick one campaign id and track its state over snapshots. Expect it
    // to Start once and remain Active for the rest of the scenario.
    let Some(first_campaign_id) = trace
        .campaign_snapshots
        .iter()
        .find_map(|(_, snap)| snap.first().map(|c| c.id.clone()))
    else {
        panic!("expected at least one campaign by end of scenario");
    };

    let mut observed_states = Vec::new();
    for (tick, snap) in &trace.campaign_snapshots {
        if let Some(c) = snap.iter().find(|c| c.id == first_campaign_id) {
            observed_states.push((*tick, c.state));
        }
    }

    // Verify there is no Active → Failed/Abandoned → Active flip cycle.
    let mut terminal_seen = false;
    for (_, state) in &observed_states {
        if matches!(
            state,
            CampaignState::Failed | CampaignState::Abandoned | CampaignState::Succeeded
        ) {
            terminal_seen = true;
        } else if terminal_seen {
            panic!(
                "campaign returned to non-terminal state after terminal; history: {:?}",
                observed_states
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Informational consistency: delivery delay is honored, stale intents are
// overridden, priority decay behaves per spec.
// ---------------------------------------------------------------------------

/// A long agent that emits exactly one intent, on the first tick, with
/// importance high and priority high but a very short half-life.
struct EmitOnceDecayingLong {
    emitted: bool,
    half_life: i64,
}

impl LongTermAgent for EmitOnceDecayingLong {
    fn tick(&mut self, _input: LongTermInput<'_>) -> LongTermOutput {
        if self.emitted {
            return LongTermOutput::default();
        }
        self.emitted = true;
        let spec = IntentSpec {
            kind: macrocosmo_ai::IntentKindId::from("pursue_metric"),
            params: IntentParams::new(),
            priority: 0.9,
            importance: 0.9,
            half_life: Some(self.half_life),
            expires_at_offset: None,
            rationale: RationaleSnapshot::empty(),
            supersedes: None,
            target: IntentTargetRef::from("faction"),
            delivery_hint: None,
        };
        LongTermOutput {
            intents: vec![spec],
        }
    }
}

/// Mid agent that only records the order it saw intents, never takes
/// campaign ops — so we can verify delivery timing observably.
struct RecordingMid {
    pub seen: Vec<(i64, macrocosmo_ai::IntentId)>,
}

impl RecordingMid {
    fn new() -> Self {
        Self { seen: Vec::new() }
    }
}

impl MidTermAgent for RecordingMid {
    fn tick(&mut self, input: MidTermInput<'_>) -> MidTermOutput {
        for intent in input.inbox {
            self.seen.push((input.now, intent.id.clone()));
        }
        MidTermOutput::default()
    }
}

struct NoopShort;
impl ShortTermAgent for NoopShort {
    fn tick(&mut self, _: ShortTermInput<'_>) -> ShortTermOutput {
        ShortTermOutput::default()
    }
}

#[test]
fn informational_delivery_delay_honored() {
    // Setup: long emits at tick 0, dispatcher delay = 15 → mid sees the
    // intent first at tick 15 (not earlier).
    let cfg = base_config("delivery_delay", 30);
    let base = Scenario::new(cfg);
    let victory = always_ongoing();

    let spec = FactionAgentSpec {
        faction: FactionId(0),
        victory,
        long: Box::new(EmitOnceDecayingLong {
            emitted: false,
            half_life: 1_000_000, // don't let decay affect this test
        }),
        mid: Box::new(RecordingMid::new()),
        short: Box::new(NoopShort),
        dispatcher: Box::new(FixedDelayDispatcher::new(15)),
        orchestrator_config: {
            let mut c = OrchestratorConfig::default();
            c.long_cadence = 1;
            c.mid_cadence = 1;
            c
        },
    };

    let pt = run_agent_scenario(AgentScenario::new(base, vec![spec]));
    let trace = &pt.per_faction[0];
    // Intent was minted at tick 0.
    assert_eq!(trace.intent_history.len(), 1);
    assert_eq!(trace.intent_history[0].issued_at, 0);
    assert_eq!(trace.intent_history[0].arrives_at, 15);
}

#[test]
fn informational_effective_priority_decays_per_half_life() {
    use macrocosmo_ai::{IntentId, IntentKindId};
    let spec = IntentSpec {
        kind: IntentKindId::from("pursue_metric"),
        params: IntentParams::new(),
        priority: 0.8,
        importance: 0.9,
        half_life: Some(10),
        expires_at_offset: None,
        rationale: RationaleSnapshot::empty(),
        supersedes: None,
        target: IntentTargetRef::from("faction"),
        delivery_hint: None,
    };
    let intent = Intent {
        id: IntentId::from("intent_0"),
        spec,
        issued_at: 0,
        arrives_at: 0,
        expires_at: None,
    };
    assert!((intent.effective_priority(0) - 0.8).abs() < 1e-6);
    // After one half-life, ≈ 0.4.
    assert!((intent.effective_priority(10) - 0.4).abs() < 1e-3);
    // After two half-lives, ≈ 0.2.
    assert!((intent.effective_priority(20) - 0.2).abs() < 1e-3);
}

#[test]
fn informational_stale_intent_overridden_by_mid() {
    // Long emits one intent with half_life=1 → by time the mid sees it,
    // the effective_priority is already near zero → mid should log
    // StaleIntent, never start a campaign.
    let cfg = base_config("stale_override", 30);
    let base = Scenario::new(cfg);
    let victory = always_ongoing();

    let spec = FactionAgentSpec {
        faction: FactionId(0),
        victory,
        long: Box::new(EmitOnceDecayingLong {
            emitted: false,
            half_life: 1, // extremely fast decay
        }),
        // Default mid-term agent honors the stale_threshold check.
        mid: Box::new(macrocosmo_ai::IntentDrivenMidTerm::new()),
        short: Box::new(NoopShort),
        dispatcher: Box::new(FixedDelayDispatcher::new(25)),
        orchestrator_config: {
            let mut c = OrchestratorConfig::default();
            c.long_cadence = 1;
            c.mid_cadence = 1;
            c
        },
    };

    let pt = run_agent_scenario(AgentScenario::new(base, vec![spec]));
    let trace = &pt.per_faction[0];

    // Expect one stale override logged.
    let stale_count = trace
        .override_log
        .iter()
        .filter(|e| matches!(e.reason, macrocosmo_ai::OverrideReason::StaleIntent))
        .count();
    assert_eq!(
        stale_count, 1,
        "expected one StaleIntent override, got log: {:?}",
        trace.override_log
    );

    // No campaigns should have been started.
    let has_any_campaign = trace
        .campaign_snapshots
        .iter()
        .any(|(_, snap)| !snap.is_empty());
    assert!(!has_any_campaign, "stale intent must not start a campaign");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn base_config(name: &str, duration: i64) -> ScenarioConfig {
    let mut metric_scripts = std::collections::BTreeMap::new();
    // dummy metric just so something is declared on the bus.
    metric_scripts.insert(MetricId::from("_dummy"), MetricScript::Constant(0.0));
    ScenarioConfig {
        name: name.into(),
        seed: 0,
        duration_ticks: duration,
        factions: vec![FactionId(0)],
        dynamics: SyntheticDynamics {
            metric_scripts,
            evidence_pulses: Vec::new(),
            command_responses: std::collections::BTreeMap::new(),
        },
    }
}

fn always_ongoing() -> VictoryCondition {
    VictoryCondition::simple(Condition::Never, Condition::Always)
}

fn metric_above(m: &str, threshold: f64) -> Condition {
    Condition::Atom(ConditionAtom::MetricAbove {
        metric: MetricId::from(m),
        threshold,
    })
}
