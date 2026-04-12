//! Topic specs — schemas declared up-front via `AiBus::declare_*`.
//!
//! Specs describe *what* a topic holds and *how long* history is kept,
//! without touching the values themselves. They are plain data.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::retention::Retention;

/// Semantic kind of a metric value. Does not affect storage (all values are `f64`),
/// but documents intent and enables UI / validation downstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetricType {
    /// Point-in-time scalar (e.g., "current fleet strength").
    Gauge,
    /// Monotonically increasing counter (e.g., "ships lost").
    Counter,
    /// Bounded 0..=1 ratio (e.g., "fleet readiness").
    Ratio,
    /// Untyped raw value; no interpretation.
    Raw,
}

/// Declaration schema for a metric topic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricSpec {
    pub kind: MetricType,
    pub retention: Retention,
    pub description: Arc<str>,
}

impl MetricSpec {
    pub fn gauge(retention: Retention, description: impl Into<Arc<str>>) -> Self {
        Self {
            kind: MetricType::Gauge,
            retention,
            description: description.into(),
        }
    }

    pub fn ratio(retention: Retention, description: impl Into<Arc<str>>) -> Self {
        Self {
            kind: MetricType::Ratio,
            retention,
            description: description.into(),
        }
    }
}

/// Declaration schema for a command kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandSpec {
    pub description: Arc<str>,
}

impl CommandSpec {
    pub fn new(description: impl Into<Arc<str>>) -> Self {
        Self {
            description: description.into(),
        }
    }
}

/// Declaration schema for an evidence kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceSpec {
    pub retention: Retention,
    pub description: Arc<str>,
}

impl EvidenceSpec {
    pub fn new(retention: Retention, description: impl Into<Arc<str>>) -> Self {
        Self {
            retention,
            description: description.into(),
        }
    }
}
