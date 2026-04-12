//! Dynamics model fit to a metric's recent history.

use serde::{Deserialize, Serialize};

use crate::time::Tick;

/// The dynamics model the projector uses for a single metric.
///
/// All variants produce an `f64` value for any `Δt = t − now`, in ticks
/// (where `now` is the tick used at projection time).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ProjectionModel {
    /// No fit available (no samples, or single sample at fidelity Rough).
    /// Evaluating yields the fixed `value`.
    Constant { value: f64 },
    /// Least-squares linear fit: `y = slope * (t - now) + intercept`.
    /// `r_squared ∈ [0, 1]` records fit quality and is reused as a
    /// confidence multiplier for the Linear model.
    Linear {
        slope: f64,
        intercept: f64,
        r_squared: f64,
    },
    /// Exponential approach to an asymptote:
    /// `y = asymptote − (asymptote − baseline) · exp(−rate · (t − now))`.
    /// `baseline` is the observed value at the reference time.
    Saturating {
        asymptote: f64,
        rate: f64,
        baseline: f64,
    },
    /// Geometric compounding at a fixed per-step rate:
    /// `y = base_rate · (1 + growth)^((t − now) / step)`.
    /// `step` here is provided externally by [`TrajectoryConfig::step`](
    /// crate::projection::TrajectoryConfig::step).
    Compound { base_rate: f64, growth: f64 },
    /// The metric is undeclared or history is empty — projection yields
    /// [`Value::Missing`](crate::value_expr::Value::Missing) at every step.
    Missing,
}

impl ProjectionModel {
    /// Evaluate the model at `Δt = t − now`.
    ///
    /// For [`ProjectionModel::Compound`] the caller must pass `step` so the
    /// exponent maps from ticks to step-count. Other variants ignore `step`.
    ///
    /// Returns `None` for [`ProjectionModel::Missing`].
    pub fn eval_at(&self, delta_t: Tick, step: Tick) -> Option<f64> {
        match self {
            Self::Constant { value } => Some(*value),
            Self::Linear {
                slope, intercept, ..
            } => Some(slope * (delta_t as f64) + intercept),
            Self::Saturating {
                asymptote,
                rate,
                baseline,
            } => {
                let t = delta_t as f64;
                Some(asymptote - (asymptote - baseline) * (-rate * t).exp())
            }
            Self::Compound { base_rate, growth } => {
                let s = step.max(1) as f64;
                let n = (delta_t as f64) / s;
                Some(base_rate * (1.0 + growth).powf(n))
            }
            Self::Missing => None,
        }
    }

    /// Confidence multiplier intrinsic to the model (not the decay over time).
    ///
    /// - Linear: `r_squared`
    /// - Saturating: fixed `0.8` (saturation detection is heuristic)
    /// - Compound: fixed `0.7` (extrapolated growth is uncertain)
    /// - Constant: `1.0` when derived from real samples; the caller may
    ///   override (e.g. to `0.5`) when constant is a fallback from missing
    ///   data.
    /// - Missing: `0.0`.
    pub fn intrinsic_confidence(&self) -> f32 {
        match self {
            Self::Constant { .. } => 1.0,
            Self::Linear { r_squared, .. } => (*r_squared as f32).clamp(0.0, 1.0),
            Self::Saturating { .. } => 0.8,
            Self::Compound { .. } => 0.7,
            Self::Missing => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_eval_is_flat() {
        let m = ProjectionModel::Constant { value: 3.14 };
        assert_eq!(m.eval_at(0, 5), Some(3.14));
        assert_eq!(m.eval_at(100, 5), Some(3.14));
    }

    #[test]
    fn linear_eval_interpolates() {
        let m = ProjectionModel::Linear {
            slope: 2.0,
            intercept: 10.0,
            r_squared: 1.0,
        };
        assert_eq!(m.eval_at(0, 5), Some(10.0));
        assert_eq!(m.eval_at(5, 5), Some(20.0));
    }

    #[test]
    fn saturating_eval_approaches_asymptote() {
        let m = ProjectionModel::Saturating {
            asymptote: 100.0,
            rate: 0.1,
            baseline: 0.0,
        };
        assert_eq!(m.eval_at(0, 5), Some(0.0));
        let v_far = m.eval_at(1000, 5).unwrap();
        assert!((v_far - 100.0).abs() < 0.01);
    }

    #[test]
    fn compound_eval_doubles_on_100pct_growth() {
        let m = ProjectionModel::Compound {
            base_rate: 1.0,
            growth: 1.0,
        };
        // (1+1)^((10-0)/5) = 2^2 = 4
        assert!((m.eval_at(10, 5).unwrap() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn missing_eval_is_none() {
        assert!(ProjectionModel::Missing.eval_at(10, 5).is_none());
    }

    #[test]
    fn intrinsic_confidence_ranges() {
        assert_eq!(ProjectionModel::Constant { value: 0.0 }.intrinsic_confidence(), 1.0);
        assert_eq!(
            ProjectionModel::Linear {
                slope: 1.0,
                intercept: 0.0,
                r_squared: 0.5,
            }
            .intrinsic_confidence(),
            0.5
        );
        assert_eq!(ProjectionModel::Missing.intrinsic_confidence(), 0.0);
    }
}
