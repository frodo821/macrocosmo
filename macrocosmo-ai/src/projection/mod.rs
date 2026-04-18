//! Trajectory projection — economic/metric extrapolation and Strategic
//! Window detection (issue #191).
//!
//! # Overview
//!
//! The projector operates on any metric history stored in [`AiBus`] and
//! produces a [`Trajectory`] of the metric's expected future values. The
//! implementation is deliberately minimal — no per-domain knowledge — so the
//! same machinery can project self-metrics, foreign-faction metric slots
//! (see `macrocosmo::ai::schema::foreign`), combat outputs, or any other
//! `f64` time series.
//!
//! A projection is produced in three stages:
//!
//! 1. **Fit**: [`fit`] selects a [`ProjectionModel`] (Constant / Linear /
//!    Saturating / Compound) from the recent history window.
//! 2. **Extrapolate**: [`project_fn`] evaluates the model at each step
//!    between `now` and `now + horizon`, optionally splicing discrete
//!    [`CompoundEffect`]s on the way.
//! 3. **Decay**: [`confidence`] attaches a per-sample confidence weight
//!    that decays past the *effective strategic window* (itself narrowed
//!    by observed volatility).
//!
//! Optionally the resulting trajectories feed into [`window::detect_windows`]
//! which classifies inter-metric patterns into the five
//! [`WindowKind`](window::WindowKind)s required by the Strategic Window
//! subsystem of issue #191.
//!
//! # Emitting projections back onto the bus
//!
//! [`emit::emit_projections_to_bus`] re-injects the projected values as
//! metric samples so that feasibility formulas can reference the future
//! explicitly — e.g. `projection.net_production_minerals.horizon_end` —
//! using the same [`ValueExpr::Metric`](crate::value_expr::ValueExpr::Metric)
//! path as live metrics.
//!
//! # Non-goals (Phase 1)
//!
//! - No `ValueExpr::ProjectedAt` variant; feasibility formulas consume
//!   emitted projection metrics instead. (Can be added later.)
//! - No observation-confidence shrinkage for foreign-faction metrics
//!   (light-speed delay is modelled on the emitter side in macrocosmo).

pub mod confidence;
pub mod emit;
pub mod fit;
pub mod model;
pub mod project_fn;
pub mod window;

use serde::{Deserialize, Serialize};

use crate::ids::MetricId;
use crate::time::{Tick, TimestampedValue};

pub use confidence::{confidence_at, effective_strategic_window};
pub use emit::{ProjectionNaming, emit_projections_to_bus};
pub use fit::{LinearFit, fit_linear, volatility};
pub use model::ProjectionModel;
pub use project_fn::{project, project_metric};
pub use window::{
    MetricPair, StrategicWindow, ThresholdGate, WindowDetectionConfig, WindowKind, WindowRationale,
    detect_windows,
};

/// Fidelity knob for the projector.
///
/// Distinct from AI "difficulty" — it is a **cost/quality** trade-off:
/// higher fidelity uses richer model fits and splices compound effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectionFidelity {
    /// Cheapest: constant or crude linear extrapolation.
    Rough,
    /// Default: OLS linear fit + saturation detection.
    Standard,
    /// `Standard` + discrete compound-effect splicing.
    Detailed,
}

impl Default for ProjectionFidelity {
    fn default() -> Self {
        ProjectionFidelity::Standard
    }
}

/// Parameters controlling a projection run.
///
/// All fields have sensible defaults via [`Default`]; override selectively.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrajectoryConfig {
    /// How far forward (in ticks) to project. Default: `200`.
    pub horizon: Tick,
    /// Sample spacing (ticks). Default: `5`.
    pub step: Tick,
    /// How far back into bus history to look when fitting. Default: `60`.
    pub history_window: Tick,
    /// Cost / quality selector. Default: [`ProjectionFidelity::Standard`].
    pub fidelity: ProjectionFidelity,
    /// Confidence-decay parameters. Default: see [`ConfidenceDecay::default`].
    pub confidence_decay: ConfidenceDecay,
    /// Minimum samples required before fitting non-constant models.
    /// Default: `3`.
    pub min_history_samples: usize,
    /// Coefficient `α` in the volatility-shrink formula for the effective
    /// strategic window (see [`effective_strategic_window`]). Default: `2.0`.
    pub volatility_penalty: f64,
}

impl Default for TrajectoryConfig {
    fn default() -> Self {
        Self {
            horizon: 200,
            step: 5,
            history_window: 60,
            fidelity: ProjectionFidelity::default(),
            confidence_decay: ConfidenceDecay::default(),
            min_history_samples: 3,
            volatility_penalty: 2.0,
        }
    }
}

/// Confidence-decay parameters for a projection.
///
/// Confidence is full-strength (`1.0`) for `Δt ≤ strategic_window`; past
/// that it decays exponentially with the given `half_life`, flooring at
/// `floor`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ConfidenceDecay {
    pub strategic_window: Tick,
    pub half_life: Tick,
    pub floor: f32,
}

impl Default for ConfidenceDecay {
    fn default() -> Self {
        Self {
            strategic_window: 60,
            half_life: 80,
            floor: 0.1,
        }
    }
}

/// Discrete change to a metric that activates at a known future tick.
///
/// Used by [`ProjectionFidelity::Detailed`] to splice planned
/// building-completions, tech-unlocks, etc. into an otherwise continuous
/// projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompoundEffect {
    pub activates_at: Tick,
    pub metric: MetricId,
    pub delta: CompoundDelta,
}

/// Kind of delta that a [`CompoundEffect`] applies.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum CompoundDelta {
    /// Add a fixed amount to the projected value from `activates_at` onward.
    Additive(f64),
    /// Multiply subsequent values by a factor.
    Multiplicative(f64),
    /// Replace the trajectory's underlying slope (linear models only).
    SlopeChange(f64),
}

/// A projected future time-series for a single metric.
///
/// `samples` and `confidence` are the same length; `samples[i]` has
/// confidence `confidence[i]`. `model` records the dynamics model fit to
/// the metric's history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trajectory {
    pub samples: Vec<TimestampedValue>,
    pub confidence: Vec<f32>,
    pub model: ProjectionModel,
}

impl Trajectory {
    /// Empty trajectory with [`ProjectionModel::Missing`].
    pub fn missing() -> Self {
        Self {
            samples: Vec::new(),
            confidence: Vec::new(),
            model: ProjectionModel::Missing,
        }
    }

    /// Number of projected samples.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether this trajectory has no samples.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}

impl Default for Trajectory {
    fn default() -> Self {
        Self::missing()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_trajectory_is_missing() {
        let t = Trajectory::default();
        assert!(t.is_empty());
        assert_eq!(t.model, ProjectionModel::Missing);
    }

    #[test]
    fn default_config_defaults() {
        let c = TrajectoryConfig::default();
        assert_eq!(c.horizon, 200);
        assert_eq!(c.step, 5);
        assert_eq!(c.history_window, 60);
        assert_eq!(c.fidelity, ProjectionFidelity::Standard);
        assert_eq!(c.confidence_decay.strategic_window, 60);
        assert_eq!(c.confidence_decay.half_life, 80);
        assert!((c.confidence_decay.floor - 0.1).abs() < 1e-6);
        assert_eq!(c.min_history_samples, 3);
        assert!((c.volatility_penalty - 2.0).abs() < 1e-9);
    }
}
