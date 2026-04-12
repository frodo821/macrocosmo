//! Playthrough recording & replay for headless AI verification.
//!
//! Phase 1 + 2 + 5 of issue #196:
//! - `record`: serializable `Playthrough` data types
//! - `recorder`: `RecordingBus` decorator that wraps `AiBus`
//! - `replayer`: `replay(Playthrough) -> AiBus`
//! - `scenario`: synthetic scenario harness for deterministic playthroughs
//! - `assertions`: property assertion helpers used by verification tests
//!
//! Out of scope (deferred to a future `macrocosmo-ai-harness` crate):
//! - Lua DSL for scenarios
//! - Anomaly detection
//! - Balance sweep CLI binary

pub mod assertions;
pub mod record;
pub mod recorder;
pub mod replayer;
pub mod scenario;

pub use assertions::{
    assert_bus_equivalent, assert_command_count, assert_metric_monotone, assert_no_command_kind,
    assert_no_panics, assert_playthrough_equivalent, Direction,
};
pub use record::{
    CommandSpecMap, Declarations, EvidenceSpecMap, MetricSpecMap, Playthrough, PlaythroughEvent,
    PlaythroughMeta, SUPPORTED_VERSION,
};
// `ScenarioConfig` is re-exported at the top-level of `playthrough` for
// convenience, but also remains accessible via `playthrough::record` for
// explicit imports.
pub use record::ScenarioConfig;
pub use recorder::RecordingBus;
pub use replayer::{replay, ReplayError};
pub use scenario::{
    run_scenario, EvidencePulse, MetricScript, Scenario, SyntheticDynamics, TickFn,
};
