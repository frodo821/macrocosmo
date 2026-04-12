//! CI isolation test: proves `macrocosmo-ai` is engine-agnostic by exercising
//! the public API using only items from this crate.
//!
//! If a bevy or macrocosmo type leaks into a public signature, this file will
//! fail to compile.

use macrocosmo_ai::{
    AiBus, Command, CommandKindId, CommandSpec, EvidenceKindId, EvidenceSpec, FactionId, MetricId,
    MetricSpec, Retention, StandingEvidence, WarningMode,
};

#[test]
fn metric_round_trip() {
    let mut bus = AiBus::new();
    let id = MetricId::from("faction.power");
    bus.declare_metric(id.clone(), MetricSpec::gauge(Retention::Medium, "power"));
    bus.emit(&id, 10.0, 100);
    bus.emit(&id, 12.0, 110);
    assert_eq!(bus.current(&id), Some(12.0));
    let window: Vec<f64> = bus.window(&id, 110, 50).map(|tv| tv.value).collect();
    assert_eq!(window, vec![10.0, 12.0]);
}

#[test]
fn command_round_trip() {
    let mut bus = AiBus::new();
    let kind = CommandKindId::from("attack_target");
    bus.declare_command(kind.clone(), CommandSpec::new("attack"));
    bus.emit_command(
        Command::new(kind.clone(), FactionId(1), 10)
            .with_priority(0.9)
            .with_param("target_system", 42i64),
    );
    let drained = bus.drain_commands();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].kind, kind);
    assert!(bus.drain_commands().is_empty());
}

#[test]
fn evidence_round_trip() {
    let mut bus = AiBus::new();
    let kind = EvidenceKindId::from("hostile_engagement");
    bus.declare_evidence(kind.clone(), EvidenceSpec::new(Retention::Long, "hostile"));
    bus.emit_evidence(StandingEvidence::new(
        kind.clone(),
        FactionId(1),
        FactionId(2),
        1.5,
        10,
    ));
    let got: Vec<_> = bus.evidence_for(FactionId(1), 20, 50).collect();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].target, FactionId(2));
}

#[test]
fn silent_warning_mode_suppresses_warnings() {
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
    // Emit to undeclared — should no-op without logging.
    bus.emit(&MetricId::from("nope"), 1.0, 0);
    bus.emit_command(Command::new(CommandKindId::from("nope"), FactionId(1), 0));
    bus.emit_evidence(StandingEvidence::new(
        EvidenceKindId::from("nope"),
        FactionId(1),
        FactionId(2),
        1.0,
        0,
    ));
    // No side effects observable.
    assert_eq!(bus.current(&MetricId::from("nope")), None);
}
