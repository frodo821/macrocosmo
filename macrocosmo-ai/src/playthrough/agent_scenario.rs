//! Agent-scenario harness — drives an `Orchestrator` per faction on
//! top of the existing scripted-dynamics `Scenario` runner.
//!
//! Per-tick flow:
//! 1. Apply scripted metrics + evidence pulses (same as `run_scenario`).
//! 2. Each faction's orchestrator ticks once against the bus.
//! 3. Short-agent commands are emitted onto the recording bus.
//! 4. Per-faction trace is updated (intents, campaigns, commands,
//!    victory status, override/drop logs).
//!
//! The harness lives under the `playthrough` module (feature-gated) so
//! downstream users pull in Serde / record types only when they opt in.

use std::collections::BTreeMap;

use crate::agent::{
    LongTermAgent, LongTermInput, LongTermOutput, MidTermAgent, MidTermInput, MidTermOutput,
    OverrideEntry, ShortTermAgent, ShortTermInput, ShortTermOutput,
};
use crate::bus::AiBus;
use crate::campaign::Campaign;
use crate::command::Command;
use crate::dispatcher::{FixedDelayDispatcher, IntentDispatcher};
use crate::evidence::StandingEvidence;
use crate::ids::FactionId;
use crate::intent::Intent;
use crate::orchestrator::{DropEntry, Orchestrator, OrchestratorConfig};
use crate::time::Tick;
use crate::victory::{VictoryCondition, VictoryStatus};
use crate::warning::WarningMode;

use super::record::{Playthrough, PlaythroughMeta};
use super::recorder::RecordingBus;
use super::scenario::{EvidencePulse, Scenario};

/// Per-faction orchestrator setup.
pub struct FactionAgentSpec {
    pub faction: FactionId,
    pub victory: VictoryCondition,
    pub long: Box<dyn LongTermAgent>,
    pub mid: Box<dyn MidTermAgent>,
    pub short: Box<dyn ShortTermAgent>,
    pub dispatcher: Box<dyn IntentDispatcher + Send + Sync>,
    pub orchestrator_config: OrchestratorConfig,
}

impl FactionAgentSpec {
    /// Convenience: default implementations + `FixedDelayDispatcher`.
    pub fn with_defaults(faction: FactionId, victory: VictoryCondition, delay: Tick) -> Self {
        Self {
            faction,
            victory,
            long: Box::new(crate::long_term_default::ObjectiveDrivenLongTerm::new()),
            mid: Box::new(crate::mid_term_default::IntentDrivenMidTerm::new()),
            short: Box::new(crate::short_term_default::CampaignReactiveShort::new()),
            dispatcher: Box::new(FixedDelayDispatcher::new(delay)),
            orchestrator_config: OrchestratorConfig::default(),
        }
    }
}

/// An `AgentScenario` pairs scripted dynamics with per-faction agents.
pub struct AgentScenario {
    pub base: Scenario,
    pub factions: Vec<FactionAgentSpec>,
}

impl AgentScenario {
    pub fn new(base: Scenario, factions: Vec<FactionAgentSpec>) -> Self {
        Self { base, factions }
    }
}

/// Per-faction time-series output from a scenario run.
#[derive(Debug, Clone)]
pub struct FactionTrace {
    pub faction: FactionId,
    pub intent_history: Vec<Intent>,
    pub campaign_snapshots: Vec<(Tick, Vec<Campaign>)>,
    pub command_history: Vec<(Tick, Command)>,
    pub victory_timeline: Vec<(Tick, VictoryStatus)>,
    pub override_log: Vec<OverrideEntry>,
    pub drop_log: Vec<DropEntry>,
}

/// Full output: bus events + per-faction traces.
#[derive(Debug, Clone)]
pub struct AgentPlaythrough {
    pub base: Playthrough,
    pub per_faction: Vec<FactionTrace>,
}

// ---------------------------------------------------------------------------
// Boxed-trait newtype wrappers
//
// `Box<dyn LongTermAgent>` does not itself implement `LongTermAgent` (the
// blanket impl would conflict). A thin newtype forwards `tick` through.
// ---------------------------------------------------------------------------

pub struct LongTermWrapper(pub Box<dyn LongTermAgent>);
pub struct MidTermWrapper(pub Box<dyn MidTermAgent>);
pub struct ShortTermWrapper(pub Box<dyn ShortTermAgent>);

impl LongTermAgent for LongTermWrapper {
    fn tick(&mut self, input: LongTermInput<'_>) -> LongTermOutput {
        self.0.tick(input)
    }
}

impl MidTermAgent for MidTermWrapper {
    fn tick(&mut self, input: MidTermInput<'_>) -> MidTermOutput {
        self.0.tick(input)
    }
}

impl ShortTermAgent for ShortTermWrapper {
    fn tick(&mut self, input: ShortTermInput<'_>) -> ShortTermOutput {
        self.0.tick(input)
    }
}

/// Bundles an orchestrator with its per-faction aux state so we can
/// iterate factions per tick without ownership contortions.
struct Paired {
    orch: Orchestrator<LongTermWrapper, MidTermWrapper, ShortTermWrapper>,
    dispatcher: Box<dyn IntentDispatcher + Send + Sync>,
    victory: VictoryCondition,
    trace_idx: usize,
}

/// Drive an agent scenario to completion.
pub fn run_agent_scenario(scenario: AgentScenario) -> AgentPlaythrough {
    let AgentScenario { base, factions } = scenario;

    let mut rb = RecordingBus::new(AiBus::with_warning_mode(WarningMode::Silent));

    // Declare scripted metric topics.
    for id in base.config.dynamics.metric_scripts.keys() {
        rb.declare_metric(
            id.clone(),
            crate::spec::MetricSpec::gauge(
                crate::retention::Retention::Long,
                format!("scripted:{id}"),
            ),
        );
    }

    // Declare scripted evidence kinds.
    let mut evidence_kinds = std::collections::BTreeSet::new();
    for pulse in &base.config.dynamics.evidence_pulses {
        evidence_kinds.insert(pulse.kind.clone());
    }
    for kind in evidence_kinds {
        rb.declare_evidence(
            kind.clone(),
            crate::spec::EvidenceSpec::new(
                crate::retention::Retention::Long,
                format!("scripted:{kind}"),
            ),
        );
    }

    // Bucket evidence pulses by tick.
    let mut pulses_by_tick: BTreeMap<Tick, Vec<EvidencePulse>> = BTreeMap::new();
    for p in &base.config.dynamics.evidence_pulses {
        pulses_by_tick.entry(p.at).or_default().push(p.clone());
    }

    // Build traces + paired orchestrators in a single pass.
    let mut traces: Vec<FactionTrace> = Vec::with_capacity(factions.len());
    let mut paired: Vec<Paired> = Vec::with_capacity(factions.len());
    for (i, spec) in factions.into_iter().enumerate() {
        traces.push(FactionTrace {
            faction: spec.faction,
            intent_history: Vec::new(),
            campaign_snapshots: Vec::new(),
            command_history: Vec::new(),
            victory_timeline: Vec::new(),
            override_log: Vec::new(),
            drop_log: Vec::new(),
        });
        let orch = Orchestrator::new(
            spec.faction,
            LongTermWrapper(spec.long),
            MidTermWrapper(spec.mid),
            ShortTermWrapper(spec.short),
        )
        .with_config(spec.orchestrator_config);
        paired.push(Paired {
            orch,
            dispatcher: spec.dispatcher,
            victory: spec.victory,
            trace_idx: i,
        });
    }

    let duration = base.config.duration_ticks;

    for t in 0..=duration {
        // 1. Scripted metrics.
        for (id, script) in &base.config.dynamics.metric_scripts {
            let v = script.sample(t, duration);
            rb.emit(id, v, t);
        }

        // 2. Scripted evidence pulses.
        if let Some(pulses) = pulses_by_tick.get(&t) {
            for p in pulses {
                rb.emit_evidence(StandingEvidence::new(
                    p.kind.clone(),
                    p.observer,
                    p.target,
                    p.magnitude,
                    p.at,
                ));
            }
        }

        // 3. Per-faction orchestrator tick.
        for p in paired.iter_mut() {
            let out = p.orch.tick(
                rb.bus_mut(),
                p.dispatcher.as_mut(),
                &p.victory,
                None,
                t,
            );
            let trace = &mut traces[p.trace_idx];

            for intent in &out.intents_sent {
                trace.intent_history.push(intent.clone());
            }
            for ov in &out.override_events {
                trace.override_log.push(ov.clone());
            }
            for drop in &out.drop_events {
                trace.drop_log.push(drop.clone());
            }
            if let Some(vs) = &out.victory_status {
                trace.victory_timeline.push((t, vs.clone()));
            }
            for cmd in &out.commands {
                trace.command_history.push((t, cmd.clone()));
                // Also emit onto the recording bus so the serialized
                // playthrough carries the command.
                rb.emit_command(cmd.clone());
            }
            // Snapshot current campaigns.
            let snapshot: Vec<Campaign> = p.orch.state.campaigns.clone();
            trace.campaign_snapshots.push((t, snapshot));
        }

        // 4. User tick closure (if provided).
        if let Some(tf) = &base.tick_fn {
            (tf)(&mut rb, t);
        }
    }

    let meta = PlaythroughMeta {
        name: base.config.name.clone(),
        seed: base.config.seed,
        ai_crate_version: env!("CARGO_PKG_VERSION").into(),
        duration_ticks: duration,
    };

    let config = base.config.clone();
    AgentPlaythrough {
        base: rb.finish(meta, config),
        per_faction: traces,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::condition::{Condition, ConditionAtom};
    use crate::ids::MetricId;
    use crate::playthrough::record::ScenarioConfig;
    use crate::playthrough::scenario::{MetricScript, SyntheticDynamics};
    use crate::victory::VictoryStatus;

    #[test]
    fn agent_scenario_runs_and_records_commands() {
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
            MetricScript::Constant(5.0),
        );
        let config = ScenarioConfig {
            name: "growth".into(),
            seed: 1,
            duration_ticks: 50,
            factions: vec![FactionId(0)],
            dynamics: SyntheticDynamics {
                metric_scripts,
                evidence_pulses: Vec::new(),
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
        spec.orchestrator_config.long_cadence = 1;
        spec.orchestrator_config.mid_cadence = 1;

        let pt = run_agent_scenario(AgentScenario::new(base, vec![spec]));

        let trace = &pt.per_faction[0];
        assert_eq!(trace.faction, FactionId(0));
        assert!(
            !trace.intent_history.is_empty(),
            "expected intents to be emitted during Ongoing phase"
        );
        assert!(
            !trace.command_history.is_empty(),
            "expected short-agent commands once a campaign is Active"
        );
        assert!(
            trace.victory_timeline.iter().any(|(_, s)| matches!(s, VictoryStatus::Won)),
            "expected victory to transition to Won by end of scenario; timeline: {:?}",
            trace.victory_timeline.iter().map(|(t, s)| (*t, s.clone())).collect::<Vec<_>>()
        );
    }
}
