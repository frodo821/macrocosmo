//! Trajectory projection — **Phase 2 stub**.
//!
//! The real projection (economic / combat dynamics, #190 / #191) will land
//! here. Phase 2 provides the API shape only so callers can compile and
//! tests can assert the default empty trajectory.

use serde::{Deserialize, Serialize};

use crate::bus::AiBus;
use crate::ids::MetricId;
use crate::time::{Tick, TimestampedValue};

/// A projected time-series trajectory of a single metric.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Trajectory {
    /// Projected samples, oldest-first.
    pub points: Vec<TimestampedValue>,
}

impl Trajectory {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Project `metric` forward by `horizon` ticks.
///
/// **TODO Phase 3**: fit a dynamics model from bus history and extrapolate.
/// For now this returns an empty trajectory regardless of inputs.
#[allow(unused_variables)]
pub fn project_trajectory(bus: &AiBus, metric: &MetricId, horizon: Tick) -> Trajectory {
    Trajectory::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::warning::WarningMode;

    #[test]
    fn stub_returns_empty_trajectory() {
        let bus = AiBus::with_warning_mode(WarningMode::Silent);
        let t = project_trajectory(&bus, &MetricId::from("power"), 100);
        assert!(t.points.is_empty());
    }

    #[test]
    fn default_trajectory_is_empty() {
        let t = Trajectory::default();
        assert!(t.points.is_empty());
    }
}
