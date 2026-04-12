//! Deterministic, serializable snapshot of bus state.
//!
//! `BusSnapshot` is produced by `AiBus::snapshot` and gives a read-only view
//! of every declared topic and its contents, in deterministic (BTreeMap) order
//! so equivalence comparisons across runs / record+replay are stable.
//!
//! Note: `AHashMap` iteration order is non-deterministic, so the snapshot
//! re-collects into `BTreeMap`s keyed by id.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::command::SerializedCommand;
use crate::evidence::StandingEvidence;
use crate::ids::{CommandKindId, EvidenceKindId, MetricId};
use crate::spec::{CommandSpec, EvidenceSpec, MetricSpec};
use crate::time::TimestampedValue;

/// Snapshot of a single metric topic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricSnapshot {
    pub spec: MetricSpec,
    pub history: Vec<TimestampedValue>,
}

/// Snapshot of a single evidence topic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceSnapshot {
    pub spec: EvidenceSpec,
    pub entries: Vec<StandingEvidence>,
}

/// Deterministic-order view of the entire bus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BusSnapshot {
    pub metrics: BTreeMap<MetricId, MetricSnapshot>,
    pub commands: BTreeMap<CommandKindId, CommandSpec>,
    pub pending_commands: Vec<SerializedCommand>,
    pub evidence: BTreeMap<EvidenceKindId, EvidenceSnapshot>,
}
