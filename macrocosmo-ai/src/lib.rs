//! macrocosmo-ai — engine-agnostic AI core for macrocosmo.
//!
//! 設計方針:
//! - **Typed topic bus** アーキテクチャ。`AiBus` に metric / command / evidence の
//!   3 種類のトピックを型付きで保持する。
//! - **Callback は ai_core に流れ込まない**。game 側は bus に emit し、
//!   純粋関数 (feasibility::evaluate, nash::solve_*, Condition::evaluate 等) を
//!   呼ぶ。ai_core から game への呼び戻しは存在しない。
//! - **依存方向**: `macrocosmo → macrocosmo-ai` (逆は禁止)。本 crate は
//!   bevy / macrocosmo / mlua に依存しない。型変換レイヤは macrocosmo 側の
//!   `src/ai/` モジュール (#203) で実装される。
//!
//! Phase 1 + 2 の範囲については issue #195 を参照。

pub mod ai_params;
pub mod assessment;
pub mod bus;
pub mod campaign;
pub mod command;
pub mod condition;
pub mod eval;
pub mod evidence;
pub mod feasibility;
pub mod ids;
pub mod nash;
pub mod objective;
pub mod precondition;
pub mod precondition_cache;
pub mod projection;
pub mod retention;
pub mod spec;
pub mod standing;
pub mod time;
pub mod value_expr;
pub mod warning;

#[cfg(any(test, feature = "mock"))]
pub mod mock;

#[cfg(feature = "playthrough")]
pub mod playthrough;

pub use bus::AiBus;
pub use bus::snapshot::{BusSnapshot, EvidenceSnapshot, MetricSnapshot};
pub use condition::{CompareOp, Condition, ConditionAtom};
pub use eval::EvalContext;
pub use precondition::{
    PreconditionEvalResult, PreconditionHistory, PreconditionItem, PreconditionSet,
    PreconditionSummary, PreconditionTracker, precond, severity,
};
pub use precondition_cache::{CacheStats, PreconditionCacheRegistry};
pub use value_expr::{Dependencies, MetricRef, ScriptRef, Value, ValueExpr};

pub use assessment::{
    Assessment, AssessmentConfig, EconomicBaseline, EconomicCapacityWeights, EconomicSnapshot,
    FleetSnapshot, ObjectiveKind, ResourceVector, TechLeadWeights, TechPositionSnapshot,
    build_assessment, build_economic_snapshot, build_tech_position_snapshot,
    compute_economic_capacity, compute_feasibility, compute_fleet_readiness,
    compute_overall_confidence, compute_tech_lead, compute_threat_level,
    critical_violation_penalty, gather_trajectory_metric_ids, objective_kind,
};
pub use command::{Command, CommandParams, CommandValue, SerializedCommand};
pub use evidence::StandingEvidence;
pub use ids::{
    CommandKindId, EntityRef, EvidenceKindId, FactionId, FactionRef, IntentId, MetricId,
    ObjectiveId, SystemRef,
};
pub use projection::{
    CompoundDelta, CompoundEffect, ConfidenceDecay, LinearFit, MetricPair, ProjectionFidelity,
    ProjectionModel, ProjectionNaming, StrategicWindow, ThresholdGate, Trajectory,
    TrajectoryConfig, WindowDetectionConfig, WindowKind, WindowRationale, confidence_at,
    detect_windows, effective_strategic_window, emit_projections_to_bus, fit_linear, project,
    project_metric, volatility,
};
pub use retention::Retention;
pub use spec::{CommandSpec, EvidenceSpec, MetricSpec, MetricType};
pub use standing::{
    EvidenceContribution, EvidenceKindConfig, PerceivedStanding, StandingConfig, StandingLevel,
    StandingLevelThresholds, StandingSubject,
};
pub use time::{Tick, TimestampedValue};
pub use warning::WarningMode;
