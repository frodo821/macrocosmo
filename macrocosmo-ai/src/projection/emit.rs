//! Emit projected trajectories back onto the bus as metric samples.
//!
//! The emitted topics follow a simple naming convention driven by
//! [`ProjectionNaming`]. Feasibility formulas can then reference the future
//! explicitly (`ValueExpr::Metric("projection.net_production_minerals.horizon_end")`)
//! just like live metrics.
//!
//! Topics are **auto-declared** with [`Retention::Medium`] the first time
//! they are written — callers don't need to declare them up-front. Existing
//! samples at the same tick are not reinserted (the bus's monotonicity
//! check drops duplicates, see [`AiBus::emit`]).

use std::sync::Arc;

use ahash::AHashMap;
use serde::{Deserialize, Serialize};

use crate::bus::AiBus;
use crate::ids::MetricId;
use crate::retention::Retention;
use crate::spec::MetricSpec;

use super::Trajectory;

/// How [`emit_projections_to_bus`] names the emitted topics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProjectionNaming {
    /// Emit one sample per projection step:
    /// `<prefix>.<metric>.t_plus_<step>`.
    PerStep { prefix: Arc<str> },
    /// Emit only the horizon endpoint:
    /// `<prefix>.<metric>.horizon_end`.
    HorizonEnd { prefix: Arc<str> },
    /// Emit both naming schemes.
    Both { prefix: Arc<str> },
}

impl ProjectionNaming {
    /// Default — both schemes under the `"projection"` prefix.
    pub fn default_both() -> Self {
        Self::Both {
            prefix: Arc::from("projection"),
        }
    }

    fn prefix(&self) -> &str {
        match self {
            Self::PerStep { prefix } | Self::HorizonEnd { prefix } | Self::Both { prefix } => {
                prefix
            }
        }
    }

    fn emit_per_step(&self) -> bool {
        matches!(self, Self::PerStep { .. } | Self::Both { .. })
    }

    fn emit_horizon_end(&self) -> bool {
        matches!(self, Self::HorizonEnd { .. } | Self::Both { .. })
    }
}

impl Default for ProjectionNaming {
    fn default() -> Self {
        Self::default_both()
    }
}

/// Emit a batch of projected trajectories to `bus`.
///
/// - Empty trajectories (`Trajectory::missing()`) are skipped.
/// - Topics are auto-declared on first use with [`Retention::Medium`].
pub fn emit_projections_to_bus(
    bus: &mut AiBus,
    trajectories: &AHashMap<MetricId, Trajectory>,
    naming: ProjectionNaming,
) {
    let prefix = naming.prefix();
    for (metric, tr) in trajectories.iter() {
        if tr.samples.is_empty() {
            continue;
        }

        // Horizon-end topic.
        if naming.emit_horizon_end() {
            let id = MetricId::from(format!("{prefix}.{metric}.horizon_end"));
            ensure_declared(bus, &id);
            if let Some(last) = tr.samples.last() {
                bus.emit(&id, last.value, last.at);
            }
        }

        // Per-step topics.
        if naming.emit_per_step() {
            if let Some(first) = tr.samples.first() {
                let now = first.at;
                for s in &tr.samples {
                    let delta = s.at - now;
                    let id = MetricId::from(format!("{prefix}.{metric}.t_plus_{delta}"));
                    ensure_declared(bus, &id);
                    bus.emit(&id, s.value, s.at);
                }
            }
        }
    }
}

fn ensure_declared(bus: &mut AiBus, id: &MetricId) {
    if !bus.has_metric(id) {
        bus.declare_metric(
            id.clone(),
            MetricSpec::gauge(Retention::Medium, "projected metric (auto-declared)"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::model::ProjectionModel;
    use crate::time::TimestampedValue;
    use crate::warning::WarningMode;

    fn sample_trajectory() -> Trajectory {
        Trajectory {
            samples: vec![
                TimestampedValue::new(10, 5.0),
                TimestampedValue::new(15, 6.0),
                TimestampedValue::new(20, 7.0),
            ],
            confidence: vec![1.0, 0.9, 0.8],
            model: ProjectionModel::Linear {
                slope: 0.2,
                intercept: 5.0,
                r_squared: 1.0,
            },
        }
    }

    #[test]
    fn emit_auto_declares_and_emits_horizon_end() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        let mut map = AHashMap::new();
        map.insert(MetricId::from("metric_a"), sample_trajectory());
        emit_projections_to_bus(
            &mut bus,
            &map,
            ProjectionNaming::HorizonEnd {
                prefix: Arc::from("projection"),
            },
        );
        let id = MetricId::from("projection.metric_a.horizon_end");
        assert!(bus.has_metric(&id));
        assert_eq!(bus.current(&id), Some(7.0));
    }

    #[test]
    fn emit_per_step_creates_topic_per_offset() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        let mut map = AHashMap::new();
        map.insert(MetricId::from("m"), sample_trajectory());
        emit_projections_to_bus(
            &mut bus,
            &map,
            ProjectionNaming::PerStep {
                prefix: Arc::from("projection"),
            },
        );
        assert_eq!(
            bus.current(&MetricId::from("projection.m.t_plus_0")),
            Some(5.0)
        );
        assert_eq!(
            bus.current(&MetricId::from("projection.m.t_plus_5")),
            Some(6.0)
        );
        assert_eq!(
            bus.current(&MetricId::from("projection.m.t_plus_10")),
            Some(7.0)
        );
    }

    #[test]
    fn emit_both_writes_both_families() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        let mut map = AHashMap::new();
        map.insert(MetricId::from("m"), sample_trajectory());
        emit_projections_to_bus(&mut bus, &map, ProjectionNaming::default_both());
        assert!(bus.has_metric(&MetricId::from("projection.m.horizon_end")));
        assert!(bus.has_metric(&MetricId::from("projection.m.t_plus_0")));
    }

    #[test]
    fn emit_skips_missing_trajectories() {
        let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
        let mut map = AHashMap::new();
        map.insert(MetricId::from("m"), Trajectory::missing());
        emit_projections_to_bus(&mut bus, &map, ProjectionNaming::default_both());
        assert!(!bus.has_metric(&MetricId::from("projection.m.horizon_end")));
    }
}
