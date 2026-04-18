//! Minimal synthetic scenario harness.
//!
//! A `Scenario` pairs a `ScenarioConfig` with synthetic dynamics and an
//! optional per-tick closure for custom AI logic. `run_scenario` drives a
//! `RecordingBus` forward tick-by-tick and returns the serializable
//! `Playthrough`.
//!
//! The harness is intentionally minimal — no `macrocosmo` game state, no
//! combat, no world. It exists to exercise the record/replay pipeline with
//! fully-deterministic inputs and to provide an easy way to produce
//! playthroughs for property-based assertions.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::bus::AiBus;
use crate::evidence::StandingEvidence;
use crate::ids::{EvidenceKindId, FactionId, MetricId};
use crate::time::Tick;

use super::record::{Playthrough, PlaythroughMeta, ScenarioConfig};
use super::recorder::RecordingBus;

/// A synthetic signal driver for a metric.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MetricScript {
    /// Linearly interpolate from `from` at tick 0 to `to` at
    /// `duration_ticks`.
    Linear { from: f64, to: f64 },
    /// Start at `from`, add `slope` each tick.
    Monotone { from: f64, slope: f64 },
    /// Sinusoid with given mean, amplitude and period (in ticks).
    Sinusoid {
        mean: f64,
        amplitude: f64,
        period: Tick,
    },
    /// Constant value.
    Constant(f64),
}

impl MetricScript {
    /// Value at tick `t`, given the scenario's total duration.
    pub fn sample(&self, t: Tick, duration_ticks: Tick) -> f64 {
        match self {
            MetricScript::Constant(v) => *v,
            MetricScript::Linear { from, to } => {
                if duration_ticks <= 0 {
                    *from
                } else {
                    let frac = (t as f64) / (duration_ticks as f64);
                    from + (to - from) * frac
                }
            }
            MetricScript::Monotone { from, slope } => from + slope * (t as f64),
            MetricScript::Sinusoid {
                mean,
                amplitude,
                period,
            } => {
                if *period <= 0 {
                    *mean
                } else {
                    let phase = (t as f64) / (*period as f64) * std::f64::consts::TAU;
                    mean + amplitude * phase.sin()
                }
            }
        }
    }
}

/// A single scripted evidence emission at a specific tick.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidencePulse {
    pub kind: EvidenceKindId,
    pub observer: FactionId,
    pub target: FactionId,
    pub magnitude: f64,
    pub at: Tick,
}

/// The set of scripted inputs for a scenario run. Deterministic: identical
/// `SyntheticDynamics` + seed produces an identical playthrough.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SyntheticDynamics {
    pub metric_scripts: BTreeMap<MetricId, MetricScript>,
    pub evidence_pulses: Vec<EvidencePulse>,
}

/// Type alias for a per-tick closure. The closure may emit additional
/// commands/metrics/evidence via the `RecordingBus`.
pub type TickFn = Arc<dyn Fn(&mut RecordingBus, Tick) + Send + Sync>;

/// A scenario to run. The config is data; the `tick_fn` is an opaque closure
/// that cannot be (de)serialized — scenarios that need custom AI logic must
/// attach it in-memory.
pub struct Scenario {
    pub config: ScenarioConfig,
    pub tick_fn: Option<TickFn>,
}

impl Scenario {
    pub fn new(config: ScenarioConfig) -> Self {
        Self {
            config,
            tick_fn: None,
        }
    }

    pub fn with_tick_fn(mut self, f: TickFn) -> Self {
        self.tick_fn = Some(f);
        self
    }
}

/// Run a scenario to completion, returning the recorded `Playthrough`.
///
/// Scripted dynamics are applied *before* the user-supplied `tick_fn` each
/// tick — custom AI logic observes the updated metrics / evidence first.
pub fn run_scenario(scenario: &Scenario) -> Playthrough {
    let mut rb = RecordingBus::new(AiBus::with_warning_mode(
        crate::warning::WarningMode::Silent,
    ));

    // Declare every metric referenced in `metric_scripts`.
    for id in scenario.config.dynamics.metric_scripts.keys() {
        rb.declare_metric(
            id.clone(),
            crate::spec::MetricSpec::gauge(
                crate::retention::Retention::Long,
                format!("scripted:{id}"),
            ),
        );
    }

    // Declare every evidence kind referenced in `evidence_pulses`.
    let mut evidence_kinds = std::collections::BTreeSet::new();
    for pulse in &scenario.config.dynamics.evidence_pulses {
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

    let duration = scenario.config.duration_ticks;

    // Pre-bucket evidence pulses by tick for predictable ordering.
    let mut pulses_by_tick: BTreeMap<Tick, Vec<&EvidencePulse>> = BTreeMap::new();
    for p in &scenario.config.dynamics.evidence_pulses {
        pulses_by_tick.entry(p.at).or_default().push(p);
    }

    for t in 0..=duration {
        // Scripted metrics (emit in BTreeMap order for determinism).
        for (id, script) in &scenario.config.dynamics.metric_scripts {
            let v = script.sample(t, duration);
            rb.emit(id, v, t);
        }

        // Scripted evidence pulses at this tick.
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

        // User tick closure.
        if let Some(tf) = &scenario.tick_fn {
            (tf)(&mut rb, t);
        }
    }

    let meta = PlaythroughMeta {
        name: scenario.config.name.clone(),
        seed: scenario.config.seed,
        ai_crate_version: env!("CARGO_PKG_VERSION").into(),
        duration_ticks: duration,
    };

    rb.finish(meta, scenario.config.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retention::Retention;

    fn simple_config() -> ScenarioConfig {
        let mut metric_scripts = BTreeMap::new();
        metric_scripts.insert(MetricId::from("readiness"), MetricScript::Constant(0.5));
        ScenarioConfig {
            name: "test".into(),
            seed: 42,
            duration_ticks: 5,
            factions: vec![FactionId(1)],
            dynamics: SyntheticDynamics {
                metric_scripts,
                evidence_pulses: Vec::new(),
            },
        }
    }

    #[test]
    fn run_scenario_produces_expected_event_count() {
        let scenario = Scenario::new(simple_config());
        let pt = run_scenario(&scenario);
        // 6 ticks (0..=5), one metric each.
        assert_eq!(pt.events.len(), 6);
    }

    #[test]
    fn linear_script_interpolates() {
        let s = MetricScript::Linear {
            from: 0.0,
            to: 10.0,
        };
        assert!((s.sample(0, 10) - 0.0).abs() < 1e-9);
        assert!((s.sample(5, 10) - 5.0).abs() < 1e-9);
        assert!((s.sample(10, 10) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn monotone_script_is_strict() {
        let s = MetricScript::Monotone {
            from: 1.0,
            slope: 0.1,
        };
        assert!(s.sample(1, 100) > s.sample(0, 100));
        assert!(s.sample(50, 100) > s.sample(1, 100));
    }

    #[test]
    fn tick_fn_runs_each_tick() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let counter = Arc::new(AtomicUsize::new(0));
        let cc = counter.clone();
        let scenario = Scenario::new(simple_config()).with_tick_fn(Arc::new(move |_rb, _t| {
            cc.fetch_add(1, Ordering::SeqCst);
        }));
        let _ = run_scenario(&scenario);
        assert_eq!(counter.load(Ordering::SeqCst), 6);
    }

    #[test]
    fn evidence_pulse_is_recorded() {
        use super::super::record::PlaythroughEvent;
        let mut cfg = simple_config();
        cfg.dynamics.evidence_pulses.push(EvidencePulse {
            kind: EvidenceKindId::from("incident"),
            observer: FactionId(1),
            target: FactionId(2),
            magnitude: 1.5,
            at: 3,
        });
        // Ensure there's an extra scripted metric to avoid interaction.
        let _ = Retention::Long;
        let scenario = Scenario::new(cfg);
        let pt = run_scenario(&scenario);
        let has_ev = pt
            .events
            .iter()
            .any(|e| matches!(e, PlaythroughEvent::Evidence(_)));
        assert!(has_ev);
    }
}
