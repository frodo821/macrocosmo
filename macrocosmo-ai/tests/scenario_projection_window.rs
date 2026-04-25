//! Projection-driven validity window — per-leaf asymmetric `expires_at_offset`.
//!
//! With `use_projection_window = true`, the long-term agent runs
//! `project()` over recent bus history for every win/prereq leaf and
//! looks for a `ThresholdRace` window via `detect_windows()`. The
//! per-leaf validity window is `reached_at - now`, so a slow-moving
//! metric naturally gets a longer window than a fast one.
//!
//! This is the missing piece for **per-leaf asymmetric fallback**:
//! when one leaf projects to cross late and another early, their
//! drop tolerances differ. A single dispatcher delay can be fatal
//! for the fast leaf (window too short) but fine for the slow leaf
//! (window comfortably exceeds delay).
//!
//! ### Setup
//! - `pursue_metric:fast`: each cmd adds +1.0 to `fast`. Runs every
//!   tick once campaign is Active → metric grows ~1/tick.
//! - `pursue_metric:slow`: each cmd adds +0.25 to `slow`. ~0.25/tick.
//! - Win = All(fast > 100, slow > 50).
//! - duration 600 ticks.
//!
//! Projected windows:
//! - fast: rate 1/tick, threshold 100 → reached_at ≈ tick 100, window ≈ 100−now
//! - slow: rate 0.25/tick, threshold 50 → reached_at ≈ tick 200, window ≈ 200−now
//!
//! At long_cadence intervals (5), expect Long to start with similar
//! windows (no history) and converge to **fast < slow** as history
//! accumulates.

#![cfg(feature = "playthrough")]

use std::collections::BTreeMap;

use macrocosmo_ai::condition::{Condition, ConditionAtom};
use macrocosmo_ai::ids::{CommandKindId, FactionId, MetricId};
use macrocosmo_ai::long_term_default::{LongTermDefaultConfig, ObjectiveDrivenLongTerm};
use macrocosmo_ai::playthrough::scenario::{MetricScript, SyntheticDynamics};
use macrocosmo_ai::playthrough::{
    AgentScenario, FactionAgentSpec, MetricEffect, Scenario, ScenarioConfig, run_agent_scenario,
};
use macrocosmo_ai::projection::TrajectoryConfig;
use macrocosmo_ai::{
    CampaignReactiveShort, FixedDelayDispatcher, IntentDrivenMidTerm, OrchestratorConfig,
    VictoryCondition, VictoryStatus,
};

fn config() -> ScenarioConfig {
    let mut metric_scripts = BTreeMap::new();
    metric_scripts.insert(MetricId::from("stockpile"), MetricScript::Constant(50.0));

    let mut command_responses: BTreeMap<CommandKindId, Vec<MetricEffect>> = BTreeMap::new();
    command_responses.insert(
        CommandKindId::from("pursue_metric:fast"),
        vec![MetricEffect::Add {
            metric: MetricId::from("fast"),
            delta: 1.0,
        }],
    );
    command_responses.insert(
        CommandKindId::from("pursue_metric:slow"),
        vec![MetricEffect::Add {
            metric: MetricId::from("slow"),
            delta: 0.25,
        }],
    );

    ScenarioConfig {
        name: "projection_window".into(),
        seed: 0,
        duration_ticks: 600,
        factions: vec![FactionId(0)],
        dynamics: SyntheticDynamics {
            metric_scripts,
            evidence_pulses: Vec::new(),
            command_responses,
        },
    }
}

fn victory() -> VictoryCondition {
    VictoryCondition::simple(
        Condition::All(vec![
            Condition::Atom(ConditionAtom::MetricAbove {
                metric: MetricId::from("fast"),
                threshold: 100.0,
            }),
            Condition::Atom(ConditionAtom::MetricAbove {
                metric: MetricId::from("slow"),
                threshold: 50.0,
            }),
        ]),
        Condition::Atom(ConditionAtom::MetricAbove {
            metric: MetricId::from("stockpile"),
            threshold: 0.0,
        }),
    )
}

fn spec(use_projection: bool) -> FactionAgentSpec {
    let long_cfg = LongTermDefaultConfig {
        use_projection_window: use_projection,
        // Lower min_history_samples and step so the first projection
        // can fit within ~10-20 ticks of accumulated bus samples.
        projection_config: TrajectoryConfig {
            history_window: 30,
            min_history_samples: 3,
            step: 5,
            horizon: 400,
            ..TrajectoryConfig::default()
        },
        // Static fallback to compare against — but with use_projection
        // = true, projected windows take precedence when present.
        default_validity_window: Some(100),
        retry_window_extension: 2.0,
        max_retries_before_fallback: 2,
        half_life: None,
        ..LongTermDefaultConfig::default()
    };
    FactionAgentSpec {
        faction: FactionId(0),
        victory: victory(),
        long: Box::new(ObjectiveDrivenLongTerm::new().with_config(long_cfg)),
        mid: Box::new(IntentDrivenMidTerm::new()),
        short: Box::new(CampaignReactiveShort::new()),
        dispatcher: Box::new(FixedDelayDispatcher::zero_delay()),
        orchestrator_config: {
            let mut c = OrchestratorConfig::default();
            c.long_cadence = 10;
            c.mid_cadence = 1;
            c
        },
    }
}

fn offsets_for_metric(
    trace: &macrocosmo_ai::playthrough::FactionTrace,
    metric_id: &str,
) -> Vec<(i64, i64)> {
    // Returns (issued_at, expires_at_offset) pairs for intents that
    // mention `metric_id` in their rationale.
    trace
        .intent_history
        .iter()
        .filter(|i| {
            i.spec
                .rationale
                .metrics_seen
                .keys()
                .any(|k| k.as_str() == metric_id)
        })
        .filter_map(|i| i.spec.expires_at_offset.map(|o| (i.issued_at, o)))
        .collect()
}

#[test]
fn projection_yields_per_leaf_asymmetric_windows() {
    let pt = run_agent_scenario(AgentScenario::new(
        Scenario::new(config()),
        vec![spec(true)],
    ));
    let trace = &pt.per_faction[0];

    let fast_offsets = offsets_for_metric(trace, "fast");
    let slow_offsets = offsets_for_metric(trace, "slow");
    assert!(
        !fast_offsets.is_empty() && !slow_offsets.is_empty(),
        "expected pursue intents for both metrics"
    );

    // While projection is active (i.e., metric has not yet crossed
    // its threshold), the windows should reflect projected
    // `reached_at - now`. fast reaches its threshold around tick 100;
    // for issued_at < 100, projected window for fast is
    // `100 - issued_at` (much shorter than slow's `200 - issued_at`).
    //
    // Compare paired (same issued_at) windows to avoid noise from the
    // post-cross fallback to default_window.
    let mut paired = Vec::new();
    for (t_fast, w_fast) in &fast_offsets {
        if let Some((_, w_slow)) = slow_offsets.iter().find(|(t, _)| t == t_fast) {
            paired.push((*t_fast, *w_fast, *w_slow));
        }
    }
    assert!(
        !paired.is_empty(),
        "expected at least one tick where both intents fired"
    );

    // Find at least one tick where fast's window is strictly smaller
    // than slow's — that's the asymmetry projection introduces.
    let asymmetric_tick = paired.iter().find(|(_, f, s)| f < s);
    assert!(
        asymmetric_tick.is_some(),
        "expected at least one tick with fast<slow under projection; pairs: {:?}",
        paired
    );

    // Sanity: AI eventually wins because windows are accommodating.
    assert!(
        trace
            .victory_timeline
            .iter()
            .any(|(_, s)| matches!(s, VictoryStatus::Won)),
        "expected eventual Won; final = {:?}",
        trace.victory_timeline.last().map(|(_, s)| s.clone())
    );
}

#[test]
fn projection_off_yields_uniform_windows() {
    // When projection is OFF, every intent uses the static default
    // window (modulo retry extensions, but the zero-delay dispatcher
    // never drops, so all stay at the default).
    let pt = run_agent_scenario(AgentScenario::new(
        Scenario::new(config()),
        vec![spec(false)],
    ));
    let trace = &pt.per_faction[0];

    let fast_offsets = offsets_for_metric(trace, "fast");
    let slow_offsets = offsets_for_metric(trace, "slow");
    assert!(
        !fast_offsets.is_empty() && !slow_offsets.is_empty(),
        "expected pursue intents for both metrics"
    );

    // Every intent should have offset == 100 (the configured default).
    assert!(
        fast_offsets.iter().all(|(_, w)| *w == 100),
        "all fast windows should be the static default 100; got {:?}",
        fast_offsets
    );
    assert!(
        slow_offsets.iter().all(|(_, w)| *w == 100),
        "all slow windows should be the static default 100; got {:?}",
        slow_offsets
    );
}
