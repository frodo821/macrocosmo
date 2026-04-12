//! Mock helpers for tests and downstream verification harnesses.
//!
//! Gated behind `feature = "mock"` so downstream crates (e.g. a future
//! `ai_test_harness` or #196 Headless Verification) can depend on this
//! module explicitly via:
//!
//! ```toml
//! macrocosmo-ai = { path = "...", features = ["mock"] }
//! ```
//!
//! Tests inside this crate enable the feature transitively via the
//! `dev-dependencies` self-reference in `Cargo.toml`.

use crate::bus::AiBus;
use crate::command::Command;
use crate::evidence::StandingEvidence;
use crate::ids::{CommandKindId, EvidenceKindId, FactionId, MetricId};
use crate::retention::Retention;
use crate::spec::{CommandSpec, EvidenceSpec, MetricSpec};
use crate::time::Tick;
use crate::warning::WarningMode;

/// Common metric ids used by fixtures. Not exhaustive — add freely.
pub mod metric_ids {
    use crate::ids::MetricId;
    pub fn fleet_readiness() -> MetricId {
        MetricId::from("fleet_readiness")
    }
    pub fn economic_capacity() -> MetricId {
        MetricId::from("economic_capacity")
    }
    pub fn local_force_ratio() -> MetricId {
        MetricId::from("local_force_ratio")
    }
}

/// Common command kinds used by fixtures.
pub mod command_kinds {
    use crate::ids::CommandKindId;
    pub fn attack_target() -> CommandKindId {
        CommandKindId::from("attack_target")
    }
    pub fn reposition() -> CommandKindId {
        CommandKindId::from("reposition")
    }
    pub fn retreat() -> CommandKindId {
        CommandKindId::from("retreat")
    }
}

/// Common evidence kinds used by fixtures.
pub mod evidence_kinds {
    use crate::ids::EvidenceKindId;
    pub fn hostile_engagement() -> EvidenceKindId {
        EvidenceKindId::from("hostile_engagement")
    }
    pub fn fleet_loss() -> EvidenceKindId {
        EvidenceKindId::from("fleet_loss")
    }
}

/// Build a silent-mode bus with the canonical fixture schemas pre-declared.
/// No data is emitted — callers populate as needed.
pub fn preconfigured_bus() -> AiBus {
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);

    bus.declare_metric(
        metric_ids::fleet_readiness(),
        MetricSpec::ratio(Retention::Medium, "fleet readiness (0..1)"),
    );
    bus.declare_metric(
        metric_ids::economic_capacity(),
        MetricSpec::ratio(Retention::Medium, "economic capacity (0..1)"),
    );
    bus.declare_metric(
        metric_ids::local_force_ratio(),
        MetricSpec::gauge(Retention::Short, "own/enemy force ratio"),
    );

    bus.declare_command(
        command_kinds::attack_target(),
        CommandSpec::new("issue an attack against a target"),
    );
    bus.declare_command(
        command_kinds::reposition(),
        CommandSpec::new("move fleet to a tactical position"),
    );
    bus.declare_command(
        command_kinds::retreat(),
        CommandSpec::new("fall back from engagement"),
    );

    bus.declare_evidence(
        evidence_kinds::hostile_engagement(),
        EvidenceSpec::new(Retention::Long, "hostile engaged our assets"),
    );
    bus.declare_evidence(
        evidence_kinds::fleet_loss(),
        EvidenceSpec::new(Retention::Long, "we lost a fleet asset"),
    );

    bus
}

/// Emit a linearly interpolated series of metric samples over
/// `[start_at, start_at + steps * step]`.
pub fn emit_linear(
    bus: &mut AiBus,
    metric: &MetricId,
    from: f64,
    to: f64,
    start_at: Tick,
    step: Tick,
    steps: usize,
) {
    if steps == 0 {
        return;
    }
    let denom = if steps > 1 { (steps - 1) as f64 } else { 1.0 };
    for i in 0..steps {
        let t = start_at + (i as Tick) * step;
        let frac = i as f64 / denom;
        let v = from + (to - from) * frac;
        bus.emit(metric, v, t);
    }
}

/// Emit a single evidence point conveniently.
pub fn emit_evidence(
    bus: &mut AiBus,
    kind: EvidenceKindId,
    observer: FactionId,
    target: FactionId,
    magnitude: f64,
    at: Tick,
) {
    bus.emit_evidence(StandingEvidence::new(kind, observer, target, magnitude, at));
}

/// Emit a simple command with a priority.
pub fn emit_command(
    bus: &mut AiBus,
    kind: CommandKindId,
    issuer: FactionId,
    at: Tick,
    priority: f64,
) {
    bus.emit_command(Command::new(kind, issuer, at).with_priority(priority));
}

/// Build a `RecordingBus` wrapping the canonical preconfigured bus. The
/// recorder has no declarations of its own yet — the underlying bus's
/// declarations are already in place so emits will succeed, but callers
/// should re-declare via the recorder if they want those declarations to
/// appear in the resulting `Playthrough`.
#[cfg(feature = "playthrough")]
pub fn preconfigured_recording_bus() -> crate::playthrough::RecordingBus {
    use crate::playthrough::RecordingBus;

    let mut rb = RecordingBus::new(AiBus::with_warning_mode(WarningMode::Silent));

    rb.declare_metric(
        metric_ids::fleet_readiness(),
        MetricSpec::ratio(Retention::Medium, "fleet readiness (0..1)"),
    );
    rb.declare_metric(
        metric_ids::economic_capacity(),
        MetricSpec::ratio(Retention::Medium, "economic capacity (0..1)"),
    );
    rb.declare_metric(
        metric_ids::local_force_ratio(),
        MetricSpec::gauge(Retention::Short, "own/enemy force ratio"),
    );

    rb.declare_command(
        command_kinds::attack_target(),
        CommandSpec::new("issue an attack against a target"),
    );
    rb.declare_command(
        command_kinds::reposition(),
        CommandSpec::new("move fleet to a tactical position"),
    );
    rb.declare_command(
        command_kinds::retreat(),
        CommandSpec::new("fall back from engagement"),
    );

    rb.declare_evidence(
        evidence_kinds::hostile_engagement(),
        EvidenceSpec::new(Retention::Long, "hostile engaged our assets"),
    );
    rb.declare_evidence(
        evidence_kinds::fleet_loss(),
        EvidenceSpec::new(Retention::Long, "we lost a fleet asset"),
    );

    rb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preconfigured_bus_has_canonical_metrics() {
        let bus = preconfigured_bus();
        assert!(bus.has_metric(&metric_ids::fleet_readiness()));
        assert!(bus.has_metric(&metric_ids::economic_capacity()));
        assert!(bus.has_metric(&metric_ids::local_force_ratio()));
        assert!(bus.has_command_kind(&command_kinds::attack_target()));
        assert!(bus.has_evidence_kind(&evidence_kinds::hostile_engagement()));
    }

    #[test]
    fn emit_linear_produces_expected_samples() {
        let mut bus = preconfigured_bus();
        let id = metric_ids::fleet_readiness();
        emit_linear(&mut bus, &id, 0.0, 1.0, 0, 10, 5);
        let got: Vec<f64> = bus.window(&id, 40, 50).map(|tv| tv.value).collect();
        assert_eq!(got.len(), 5);
        assert!((got[0] - 0.0).abs() < 1e-9);
        assert!((got[4] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn emit_linear_single_step() {
        let mut bus = preconfigured_bus();
        let id = metric_ids::fleet_readiness();
        emit_linear(&mut bus, &id, 0.5, 0.5, 0, 10, 1);
        assert_eq!(bus.current(&id), Some(0.5));
    }

    #[test]
    fn emit_linear_zero_steps_is_noop() {
        let mut bus = preconfigured_bus();
        let id = metric_ids::fleet_readiness();
        emit_linear(&mut bus, &id, 0.0, 1.0, 0, 10, 0);
        assert_eq!(bus.current(&id), None);
    }

    #[test]
    fn helper_emit_command_enqueues() {
        let mut bus = preconfigured_bus();
        emit_command(
            &mut bus,
            command_kinds::attack_target(),
            FactionId(1),
            10,
            0.9,
        );
        let drained = bus.drain_commands();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].priority, 0.9);
    }
}
