//! Entry points: [`project`] and [`project_metric`].
//!
//! Both consume an [`AiBus`] plus a [`TrajectoryConfig`] and return fully
//! populated [`Trajectory`]s. The dynamics model is selected from the
//! recent history window according to [`ProjectionFidelity`].

use ahash::AHashMap;

use crate::bus::AiBus;
use crate::ids::MetricId;
use crate::time::{Tick, TimestampedValue};

use super::confidence::{confidence_at, effective_strategic_window};
use super::fit::{detect_saturation, fit_linear, volatility};
use super::model::ProjectionModel;
use super::{CompoundDelta, CompoundEffect, ProjectionFidelity, Trajectory, TrajectoryConfig};

/// Project a batch of metrics in one call.
///
/// `compound` is an optional list of discrete future events keyed by
/// [`MetricId`]; each effect is applied to its target metric if (and only
/// if) [`TrajectoryConfig::fidelity`] is [`ProjectionFidelity::Detailed`].
pub fn project(
    bus: &AiBus,
    metrics: &[MetricId],
    config: &TrajectoryConfig,
    now: Tick,
    compound: &[CompoundEffect],
) -> AHashMap<MetricId, Trajectory> {
    let mut out = AHashMap::with_capacity(metrics.len());
    for m in metrics {
        out.insert(m.clone(), project_metric(bus, m, config, now, compound));
    }
    out
}

/// Project a single metric.
pub fn project_metric(
    bus: &AiBus,
    metric: &MetricId,
    config: &TrajectoryConfig,
    now: Tick,
    compound: &[CompoundEffect],
) -> Trajectory {
    if !bus.has_metric(metric) {
        return Trajectory::missing();
    }

    let samples: Vec<TimestampedValue> = bus
        .window(metric, now, config.history_window)
        .cloned()
        .collect();

    if samples.is_empty() {
        return Trajectory::missing();
    }

    let (model, fit_vol) = fit_model(&samples, config, now);
    let effective_window = effective_strategic_window(
        config.confidence_decay.strategic_window,
        fit_vol,
        config.volatility_penalty,
    );

    // Collect the compound effects that target this metric, pre-sorted by
    // activation tick so we can step through them during extrapolation.
    let mut my_effects: Vec<&CompoundEffect> = compound
        .iter()
        .filter(|e| &e.metric == metric && config.fidelity == ProjectionFidelity::Detailed)
        .collect();
    my_effects.sort_by_key(|e| e.activates_at);

    let step = config.step.max(1);
    let horizon = config.horizon.max(0);
    let intrinsic = model.intrinsic_confidence();

    // Per-sample additive / multiplicative adjustments accumulated from
    // compound effects. `slope_override` replaces the Linear slope once a
    // SlopeChange effect activates.
    let mut additive = 0.0;
    let mut multiplicative = 1.0;
    let mut slope_override: Option<f64> = None;

    let mut samples_out = Vec::new();
    let mut confidence_out = Vec::new();

    let mut effect_idx = 0;
    let mut delta_t = 0;
    while delta_t <= horizon {
        let at = now + delta_t;

        // Activate any compound effects whose time has arrived.
        while effect_idx < my_effects.len() && my_effects[effect_idx].activates_at <= at {
            let e = my_effects[effect_idx];
            match e.delta {
                CompoundDelta::Additive(v) => additive += v,
                CompoundDelta::Multiplicative(v) => multiplicative *= v,
                CompoundDelta::SlopeChange(s) => slope_override = Some(s),
            }
            effect_idx += 1;
        }

        let base_value = match (&model, slope_override) {
            (ProjectionModel::Linear { intercept, .. }, Some(new_slope)) => {
                Some(new_slope * (delta_t as f64) + intercept)
            }
            (m, _) => m.eval_at(delta_t, step),
        };
        let Some(base) = base_value else {
            // Missing model — bail out with an empty trajectory.
            return Trajectory::missing();
        };
        let value = base * multiplicative + additive;
        samples_out.push(TimestampedValue::new(at, value));
        let decay = confidence_at(delta_t, effective_window, config.confidence_decay);
        confidence_out.push(decay * intrinsic);

        delta_t += step;
    }

    Trajectory {
        samples: samples_out,
        confidence: confidence_out,
        model,
    }
}

/// Fit a dynamics model and return it alongside the observed volatility.
fn fit_model(
    samples: &[TimestampedValue],
    config: &TrajectoryConfig,
    now: Tick,
) -> (ProjectionModel, f64) {
    let current = samples
        .last()
        .map(|s| s.value)
        .expect("fit_model called with empty samples");

    // Rough: constant when fewer than min_history_samples, else linear.
    // Standard/Detailed: linear + optional saturation detection.
    match config.fidelity {
        ProjectionFidelity::Rough => {
            if samples.len() < 2 {
                return (ProjectionModel::Constant { value: current }, 0.0);
            }
            match fit_linear(samples, now) {
                Some(fit) => {
                    let vol = volatility(samples, Some(fit));
                    (
                        ProjectionModel::Linear {
                            slope: fit.slope,
                            intercept: fit.intercept,
                            r_squared: fit.r_squared,
                        },
                        vol,
                    )
                }
                None => (ProjectionModel::Constant { value: current }, 0.0),
            }
        }
        ProjectionFidelity::Standard | ProjectionFidelity::Detailed => {
            if samples.len() < config.min_history_samples {
                if samples.len() >= 2 {
                    if let Some(fit) = fit_linear(samples, now) {
                        let vol = volatility(samples, Some(fit));
                        return (
                            ProjectionModel::Linear {
                                slope: fit.slope,
                                intercept: fit.intercept,
                                r_squared: fit.r_squared,
                            },
                            vol,
                        );
                    }
                }
                return (ProjectionModel::Constant { value: current }, 0.0);
            }

            let fit = match fit_linear(samples, now) {
                Some(f) => f,
                None => return (ProjectionModel::Constant { value: current }, 0.0),
            };
            let vol = volatility(samples, Some(fit));

            // Heuristic saturation detection. If present, convert to
            // Saturating model using current value as baseline and a rough
            // rate proxy (positive slope → rising to asymptote).
            let tail = (samples.len() / 3).max(2);
            if detect_saturation(samples, fit, tail, 0.3) {
                // Estimate asymptote as current + residual mean projected.
                // Cheap heuristic: use mean of tail samples + their average
                // residual deviation.
                let baseline = current;
                // Approach rate ~ |slope| / (asymptote − baseline) with a
                // small guard so we don't blow up on flat tails.
                let asymptote_guess = baseline + fit.slope.abs() * 20.0;
                let rate = if (asymptote_guess - baseline).abs() > f64::EPSILON {
                    (fit.slope.abs() / (asymptote_guess - baseline).abs()).max(0.01)
                } else {
                    0.01
                };
                return (
                    ProjectionModel::Saturating {
                        asymptote: asymptote_guess,
                        rate,
                        baseline,
                    },
                    vol,
                );
            }

            (
                ProjectionModel::Linear {
                    slope: fit.slope,
                    intercept: fit.intercept,
                    r_squared: fit.r_squared,
                },
                vol,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::MetricId;
    use crate::retention::Retention;
    use crate::spec::MetricSpec;
    use crate::warning::WarningMode;

    fn bus_with_metric(id: &MetricId) -> AiBus {
        let mut b = AiBus::with_warning_mode(WarningMode::Silent);
        b.declare_metric(id.clone(), MetricSpec::gauge(Retention::VeryLong, "test"));
        b
    }

    #[test]
    fn project_undeclared_metric_is_missing() {
        let b = AiBus::with_warning_mode(WarningMode::Silent);
        let t = project_metric(
            &b,
            &MetricId::from("none"),
            &TrajectoryConfig::default(),
            0,
            &[],
        );
        assert_eq!(t.model, ProjectionModel::Missing);
        assert!(t.is_empty());
    }

    #[test]
    fn project_empty_history_is_missing() {
        let id = MetricId::from("m");
        let b = bus_with_metric(&id);
        let t = project_metric(&b, &id, &TrajectoryConfig::default(), 0, &[]);
        assert_eq!(t.model, ProjectionModel::Missing);
    }

    #[test]
    fn project_constant_metric_is_flat() {
        let id = MetricId::from("m");
        let mut b = bus_with_metric(&id);
        for t in 0..10 {
            b.emit(&id, 42.0, t * 5);
        }
        let cfg = TrajectoryConfig {
            horizon: 50,
            step: 5,
            ..Default::default()
        };
        let tr = project_metric(&b, &id, &cfg, 45, &[]);
        assert!(matches!(
            tr.model,
            ProjectionModel::Linear { .. }
                | ProjectionModel::Constant { .. }
                | ProjectionModel::Saturating { .. }
        ));
        assert_eq!(tr.samples.len(), 11); // 0..=50 inclusive at step 5
        for s in &tr.samples {
            assert!((s.value - 42.0).abs() < 1e-6);
        }
    }

    #[test]
    fn project_linear_extrapolates() {
        let id = MetricId::from("m");
        let mut b = bus_with_metric(&id);
        // y = 2t, history window covers recent samples
        for i in 0..10 {
            let at = i * 5;
            b.emit(&id, 2.0 * at as f64, at);
        }
        let cfg = TrajectoryConfig {
            horizon: 20,
            step: 5,
            history_window: 60,
            fidelity: ProjectionFidelity::Standard,
            ..Default::default()
        };
        let tr = project_metric(&b, &id, &cfg, 45, &[]);
        match tr.model {
            ProjectionModel::Linear { slope, .. } => assert!((slope - 2.0).abs() < 1e-6),
            other => panic!("expected Linear, got {other:?}"),
        }
        // At now=45 (delta=0) value ~ 90; at delta=20 -> value ~ 130
        assert!((tr.samples[0].value - 90.0).abs() < 1e-6);
        assert!((tr.samples[4].value - 130.0).abs() < 1e-6);
    }

    #[test]
    fn project_detailed_applies_additive_compound() {
        let id = MetricId::from("m");
        let mut b = bus_with_metric(&id);
        for i in 0..6 {
            b.emit(&id, 10.0, i * 5);
        }
        let cfg = TrajectoryConfig {
            horizon: 30,
            step: 5,
            history_window: 60,
            fidelity: ProjectionFidelity::Detailed,
            ..Default::default()
        };
        let now = 25;
        let effect = CompoundEffect {
            activates_at: now + 10,
            metric: id.clone(),
            delta: CompoundDelta::Additive(5.0),
        };
        let tr = project_metric(&b, &id, &cfg, now, &[effect]);
        // delta=0 -> 10, delta=5 -> 10, delta=10 -> 15, delta=15 -> 15, ...
        assert!((tr.samples[0].value - 10.0).abs() < 1e-6);
        assert!((tr.samples[1].value - 10.0).abs() < 1e-6);
        assert!((tr.samples[2].value - 15.0).abs() < 1e-6);
        assert!((tr.samples[6].value - 15.0).abs() < 1e-6);
    }

    #[test]
    fn project_compound_effect_ignored_below_detailed() {
        let id = MetricId::from("m");
        let mut b = bus_with_metric(&id);
        for i in 0..6 {
            b.emit(&id, 10.0, i * 5);
        }
        let cfg = TrajectoryConfig {
            horizon: 20,
            step: 5,
            history_window: 60,
            fidelity: ProjectionFidelity::Standard,
            ..Default::default()
        };
        let effect = CompoundEffect {
            activates_at: 30,
            metric: id.clone(),
            delta: CompoundDelta::Additive(5.0),
        };
        let tr = project_metric(&b, &id, &cfg, 25, &[effect]);
        // No additive should apply — all values flat at 10.
        for s in &tr.samples {
            assert!((s.value - 10.0).abs() < 1e-6);
        }
    }

    #[test]
    fn project_batch_returns_all_requested_metrics() {
        let a = MetricId::from("a");
        let b_id = MetricId::from("b");
        let mut bus = bus_with_metric(&a);
        bus.declare_metric(
            b_id.clone(),
            MetricSpec::gauge(Retention::VeryLong, "test"),
        );
        for i in 0..4 {
            bus.emit(&a, 1.0, i * 5);
            bus.emit(&b_id, 2.0, i * 5);
        }
        let cfg = TrajectoryConfig {
            horizon: 10,
            step: 5,
            ..Default::default()
        };
        let out = project(&bus, &[a.clone(), b_id.clone()], &cfg, 15, &[]);
        assert!(out.contains_key(&a));
        assert!(out.contains_key(&b_id));
    }
}
