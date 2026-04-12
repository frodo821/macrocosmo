//! Foreign-faction metric slot templates.
//!
//! Every observed faction gets its own set of **Tier 2** metrics that
//! describe the observer's estimate of that faction's strategic state.
//! The templates in this module are the schema; the actual topics are
//! declared on the bus by
//! [`crate::ai::plugin::declare_foreign_slots_on_awareness`] whenever a new
//! `Faction` entity appears.
//!
//! Naming convention: `<prefix>.faction_<FactionId>`.
//!
//! These topics are emitted by (future) knowledge-system producers that
//! digest `KnowledgeStore` observations into estimated foreign-faction
//! metrics, including the appropriate light-speed delay. They feed into
//! the projection subsystem (#191) as inputs to `MetricPair` comparisons
//! (my_strength vs foreign.strength.faction_N), enabling Offensive /
//! Defensive window detection against real foreign factions.

use std::sync::Arc;

use macrocosmo_ai::{FactionId, MetricId, MetricSpec, Retention};

/// One foreign-metric schema entry.
///
/// `spec_factory` is a fn pointer rather than a cloned spec so the
/// template list itself remains cheap; each instantiation gets a fresh
/// `Arc<str>` description.
#[derive(Debug, Clone)]
pub struct ForeignMetricTemplate {
    /// Metric-id prefix (everything before `.faction_<id>`).
    pub prefix: Arc<str>,
    /// Factory producing the [`MetricSpec`] to declare.
    pub spec_factory: fn() -> MetricSpec,
}

/// Compose a per-faction metric id from a template prefix and a faction id.
pub fn foreign_metric_id(prefix: &str, faction: FactionId) -> MetricId {
    MetricId::from(format!("{prefix}.faction_{}", faction.0))
}

/// The canonical Tier 2 foreign-metric templates.
///
/// Each template represents one observer-side estimate (strength, fleet
/// count, colony count, …) that the AI wants to reason about
/// per-observed-faction. The list is intentionally short for MVP — new
/// estimates should be added here (and declared on the bus
/// automatically via [`crate::ai::plugin::declare_foreign_slots_on_awareness`]).
pub fn foreign_metric_templates() -> Vec<ForeignMetricTemplate> {
    vec![
        ForeignMetricTemplate {
            prefix: "foreign.strength".into(),
            spec_factory: || MetricSpec::gauge(Retention::Long, "推定総戦力"),
        },
        ForeignMetricTemplate {
            prefix: "foreign.fleet_count".into(),
            spec_factory: || MetricSpec::gauge(Retention::Long, "観測艦隊数"),
        },
        ForeignMetricTemplate {
            prefix: "foreign.colony_count".into(),
            spec_factory: || MetricSpec::gauge(Retention::Long, "観測コロニー数"),
        },
        ForeignMetricTemplate {
            prefix: "foreign.research_output".into(),
            spec_factory: || MetricSpec::gauge(Retention::VeryLong, "推定研究産出"),
        },
        ForeignMetricTemplate {
            prefix: "foreign.economy".into(),
            spec_factory: || MetricSpec::gauge(Retention::Long, "推定経済規模"),
        },
        ForeignMetricTemplate {
            prefix: "foreign.territory_pressure".into(),
            spec_factory: || MetricSpec::gauge(Retention::Medium, "隣接系影響力"),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreign_metric_id_formats_consistently() {
        let id = foreign_metric_id("foreign.strength", FactionId(42));
        assert_eq!(id.as_str(), "foreign.strength.faction_42");
    }

    #[test]
    fn foreign_templates_nonempty() {
        let t = foreign_metric_templates();
        assert!(t.len() >= 4);
        assert!(t.iter().any(|t| t.prefix.as_ref() == "foreign.strength"));
    }

    #[test]
    fn spec_factory_runs() {
        for t in foreign_metric_templates() {
            let _spec: MetricSpec = (t.spec_factory)();
            // Just assert it produces a spec without panicking.
        }
    }
}
