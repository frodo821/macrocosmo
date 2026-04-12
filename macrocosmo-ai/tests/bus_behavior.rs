//! Behavioral tests for `AiBus`: redeclaration, silent mode, partial windows.

use macrocosmo_ai::{
    AiBus, CommandKindId, CommandSpec, EvidenceKindId, EvidenceSpec, FactionId, MetricId,
    MetricSpec, Retention, StandingEvidence, WarningMode,
};

#[test]
fn redeclare_metric_preserves_history() {
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
    let id = MetricId::from("x");
    bus.declare_metric(id.clone(), MetricSpec::gauge(Retention::Short, "old"));
    bus.emit(&id, 1.0, 10);
    bus.emit(&id, 2.0, 20);
    bus.declare_metric(id.clone(), MetricSpec::gauge(Retention::Long, "new"));
    assert_eq!(bus.current(&id), Some(2.0));
}

#[test]
fn redeclare_command_preserves_pending() {
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
    let kind = CommandKindId::from("c");
    bus.declare_command(kind.clone(), CommandSpec::new("v1"));
    bus.emit_command(macrocosmo_ai::Command::new(kind.clone(), FactionId(1), 10));
    bus.declare_command(kind.clone(), CommandSpec::new("v2"));
    let drained = bus.drain_commands();
    assert_eq!(drained.len(), 1);
}

#[test]
fn redeclare_evidence_preserves_history() {
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
    let kind = EvidenceKindId::from("e");
    bus.declare_evidence(kind.clone(), EvidenceSpec::new(Retention::Short, "old"));
    bus.emit_evidence(StandingEvidence::new(
        kind.clone(),
        FactionId(1),
        FactionId(2),
        1.0,
        10,
    ));
    bus.declare_evidence(kind.clone(), EvidenceSpec::new(Retention::Long, "new"));
    let got: Vec<_> = bus.evidence_for(FactionId(1), 20, 100).collect();
    assert_eq!(got.len(), 1);
}

#[test]
fn emit_to_undeclared_metric_is_noop() {
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
    let id = MetricId::from("undeclared");
    bus.emit(&id, 42.0, 10);
    assert_eq!(bus.current(&id), None);
}

#[test]
fn time_reversed_metric_emit_drops_sample() {
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
    let id = MetricId::from("x");
    bus.declare_metric(id.clone(), MetricSpec::gauge(Retention::Long, "x"));
    bus.emit(&id, 1.0, 100);
    bus.emit(&id, 2.0, 50); // reversed
    assert_eq!(bus.current(&id), Some(1.0));
}

#[test]
fn window_wider_than_retention_returns_partial() {
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
    let id = MetricId::from("x");
    bus.declare_metric(id.clone(), MetricSpec::gauge(Retention::Custom(50), "x"));
    for t in 0..10 {
        bus.emit(&id, t as f64, t * 10);
    }
    // newest=90, retention=50 -> kept samples at t in {40..=90}
    let all: Vec<f64> = bus.window(&id, 90, 10_000).map(|tv| tv.value).collect();
    assert_eq!(all, vec![4.0, 5.0, 6.0, 7.0, 8.0, 9.0]);
}

#[test]
fn window_on_undeclared_is_empty() {
    let bus = AiBus::new();
    let got: Vec<_> = bus.window(&MetricId::from("nope"), 100, 50).collect();
    assert!(got.is_empty());
}

#[test]
fn default_warning_mode_is_enabled() {
    let bus = AiBus::new();
    assert_eq!(bus.warning_mode(), WarningMode::Enabled);
}

#[test]
fn set_warning_mode_toggles() {
    let mut bus = AiBus::new();
    assert_eq!(bus.warning_mode(), WarningMode::Enabled);
    bus.set_warning_mode(WarningMode::Silent);
    assert_eq!(bus.warning_mode(), WarningMode::Silent);
}
