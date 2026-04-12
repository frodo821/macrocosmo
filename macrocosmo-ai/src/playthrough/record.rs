//! Data types for a serialized playthrough.
//!
//! A `Playthrough` captures every topic declaration and every accepted emit
//! produced by a run. The recorder only records events the bus would
//! actually accept (declared kind, non-reversed time), mirroring bus
//! semantics so replay produces an identical final state.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::command::SerializedCommand;
use crate::evidence::StandingEvidence;
use crate::ids::{CommandKindId, EvidenceKindId, FactionId, MetricId};
use crate::spec::{CommandSpec, EvidenceSpec, MetricSpec};
use crate::time::Tick;

/// Schema version for `Playthrough`. Bumped on incompatible changes. Replay
/// rejects mismatched versions via `ReplayError::UnsupportedVersion`.
pub const SUPPORTED_VERSION: u32 = 1;

/// Deterministic-order map type for metric declarations.
pub type MetricSpecMap = BTreeMap<MetricId, MetricSpec>;
/// Deterministic-order map type for command declarations.
pub type CommandSpecMap = BTreeMap<CommandKindId, CommandSpec>;
/// Deterministic-order map type for evidence declarations.
pub type EvidenceSpecMap = BTreeMap<EvidenceKindId, EvidenceSpec>;

/// All topic declarations captured at the start of a playthrough.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Declarations {
    pub metrics: MetricSpecMap,
    pub commands: CommandSpecMap,
    pub evidence: EvidenceSpecMap,
}

/// Metadata about the playthrough, baked into the recording for traceability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaythroughMeta {
    pub name: String,
    pub seed: u64,
    pub ai_crate_version: String,
    pub duration_ticks: Tick,
}

/// Configuration that drives a scenario run. Preserved in the playthrough so a
/// reader can reconstruct exactly how the run was produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenarioConfig {
    pub name: String,
    pub seed: u64,
    pub duration_ticks: Tick,
    pub factions: Vec<FactionId>,
    pub dynamics: super::scenario::SyntheticDynamics,
}

/// A single accepted bus event, in the order it was emitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlaythroughEvent {
    Metric {
        id: MetricId,
        value: f64,
        at: Tick,
    },
    Command(SerializedCommand),
    Evidence(StandingEvidence),
}

/// Full playthrough: schema version, metadata, scenario config, declarations,
/// and the ordered event stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Playthrough {
    pub version: u32,
    pub meta: PlaythroughMeta,
    pub config: ScenarioConfig,
    pub declarations: Declarations,
    pub events: Vec<PlaythroughEvent>,
}
