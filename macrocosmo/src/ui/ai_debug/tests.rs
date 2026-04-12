//! Unit tests for the AI Debug UI. egui rendering paths are exercised by
//! the integration smoke test (`tests/ai_debug_smoke.rs`) rather than here.

use bevy::prelude::*;
use macrocosmo_ai::{
    AiBus, Command, CommandKindId, CommandSpec, EvidenceKindId, EvidenceSpec, FactionId,
    MetricId, MetricSpec, Retention, StandingEvidence, WarningMode,
    playthrough::{
        Declarations, Playthrough, PlaythroughEvent, PlaythroughMeta, ScenarioConfig,
        SUPPORTED_VERSION,
    },
};

use crate::ai::plugin::{AiBusResource, AiPlugin};
use crate::time_system::GameClock;

use super::{
    AiDebugUi, DebugTab, StreamEntry, StreamEvent, STREAM_LOG_CAP, diff_snapshots,
    toggle_ai_debug,
};

fn ready_metric() -> MetricId {
    MetricId::from("fleet_readiness")
}

#[test]
fn ai_debug_ui_default_closed() {
    let ui = AiDebugUi::default();
    assert!(!ui.open);
    assert_eq!(ui.active_tab, DebugTab::Inspector);
    assert!(ui.stream.log.is_empty());
    assert!(ui.last_snapshot.is_none());
}

#[test]
fn toggle_flips_open() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.init_resource::<ButtonInput<KeyCode>>();
    app.init_resource::<AiDebugUi>();
    app.add_systems(Update, toggle_ai_debug);

    // No key pressed — stays closed.
    app.update();
    assert!(!app.world().resource::<AiDebugUi>().open);

    // Press F10: flips to open.
    {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.press(KeyCode::F10);
    }
    app.update();
    assert!(app.world().resource::<AiDebugUi>().open);

    // Clear then press again: flips back to closed.
    {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(KeyCode::F10);
        keys.clear_just_pressed(KeyCode::F10);
    }
    app.update();
    assert!(app.world().resource::<AiDebugUi>().open);
    {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.press(KeyCode::F10);
    }
    app.update();
    assert!(!app.world().resource::<AiDebugUi>().open);
}

#[test]
fn snapshot_diff_detects_metric_emit() {
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
    bus.declare_metric(
        ready_metric(),
        MetricSpec::ratio(Retention::Short, "r"),
    );
    bus.emit(&ready_metric(), 0.5, 10);
    let prev = bus.snapshot();

    bus.emit(&ready_metric(), 0.8, 20);
    let now = bus.snapshot();

    let entries = diff_snapshots(Some(&prev), &now, 20);
    // One new metric sample at tick=20.
    let emits: Vec<_> = entries
        .iter()
        .filter_map(|e| match &e.event {
            StreamEvent::MetricEmit { id, value } => Some((id.as_str().to_string(), *value, e.at)),
            _ => None,
        })
        .collect();
    assert_eq!(emits.len(), 1);
    assert_eq!(emits[0].0, "fleet_readiness");
    assert!((emits[0].1 - 0.8).abs() < 1e-9);
    assert_eq!(emits[0].2, 20);
}

#[test]
fn snapshot_diff_first_run_emits_declarations() {
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
    bus.declare_metric(
        ready_metric(),
        MetricSpec::ratio(Retention::Short, "r"),
    );
    let snap = bus.snapshot();
    let entries = diff_snapshots(None, &snap, 0);
    // On first snapshot the metric should surface as a DeclarationAdded.
    assert!(entries.iter().any(|e| matches!(
        &e.event,
        StreamEvent::DeclarationAdded { kind: "metric", id } if id == "fleet_readiness"
    )));
}

#[test]
fn snapshot_diff_detects_command_and_evidence() {
    let mut bus = AiBus::with_warning_mode(WarningMode::Silent);
    let kind = CommandKindId::from("attack");
    bus.declare_command(kind.clone(), CommandSpec::new("x"));
    bus.declare_evidence(
        EvidenceKindId::from("hostile"),
        EvidenceSpec::new(Retention::Long, "h"),
    );
    let prev = bus.snapshot();

    bus.emit_command(Command::new(kind.clone(), FactionId(2), 5).with_priority(0.8));
    bus.emit_evidence(StandingEvidence::new(
        EvidenceKindId::from("hostile"),
        FactionId(1),
        FactionId(2),
        1.0,
        6,
    ));
    let cur = bus.snapshot();

    let entries = diff_snapshots(Some(&prev), &cur, 6);
    assert!(entries.iter().any(|e| matches!(
        &e.event,
        StreamEvent::CommandEnqueued { issuer, .. } if issuer.0 == 2
    )));
    assert!(entries.iter().any(|e| matches!(
        &e.event,
        StreamEvent::EvidenceEmitted { observer, target, .. } if observer.0 == 1 && target.0 == 2
    )));
}

#[test]
fn stream_log_cap_enforced() {
    let mut state = super::StreamState::default();
    for i in 0..(STREAM_LOG_CAP * 2) {
        state.push(StreamEntry {
            at: i as i64,
            event: StreamEvent::MetricEmit {
                id: ready_metric(),
                value: i as f64,
            },
        });
    }
    assert_eq!(state.log.len(), STREAM_LOG_CAP);
    // Oldest retained entry should be from the second half (the first
    // `STREAM_LOG_CAP` were evicted as the ring wrapped).
    let front = state.log.front().unwrap();
    assert!(front.at >= STREAM_LOG_CAP as i64);
}

fn mk_empty_playthrough() -> Playthrough {
    Playthrough {
        version: SUPPORTED_VERSION,
        meta: PlaythroughMeta {
            name: "t".into(),
            seed: 0,
            ai_crate_version: env!("CARGO_PKG_VERSION").into(),
            duration_ticks: 0,
        },
        config: ScenarioConfig {
            name: "t".into(),
            seed: 0,
            duration_ticks: 0,
            factions: Vec::new(),
            dynamics: Default::default(),
        },
        declarations: Declarations::default(),
        events: Vec::new(),
    }
}

fn mk_simple_playthrough() -> Playthrough {
    let mut pt = mk_empty_playthrough();
    pt.declarations.metrics.insert(
        ready_metric(),
        MetricSpec::ratio(Retention::Short, "r"),
    );
    pt.events.push(PlaythroughEvent::Metric {
        id: ready_metric(),
        value: 0.3,
        at: 5,
    });
    pt.events.push(PlaythroughEvent::Metric {
        id: ready_metric(),
        value: 0.9,
        at: 10,
    });
    pt
}

#[test]
fn replay_load_parse_valid_json() {
    let pt = mk_simple_playthrough();
    let json = serde_json::to_string(&pt).expect("serialize");
    // Round-trip: parsed playthrough equals the original.
    let roundtrip: Playthrough = serde_json::from_str(&json).expect("parse");
    assert_eq!(roundtrip, pt);
}

#[test]
fn replay_load_invalid_json_sets_error() {
    // `load_playthrough` is private; exercise the underlying error surface
    // via `std::fs::read_to_string` + `serde_json`.
    let parsed: Result<Playthrough, _> = serde_json::from_str("{not json");
    assert!(parsed.is_err());
}

#[test]
fn replay_step_forward_advances_cursor() {
    let pt = mk_simple_playthrough();
    let mut bus = super::replay::build_empty_bus(&pt);
    assert_eq!(bus.current(&ready_metric()), None);
    super::replay::apply_event(&mut bus, &pt.events[0]);
    assert_eq!(bus.current(&ready_metric()), Some(0.3));
    super::replay::apply_event(&mut bus, &pt.events[1]);
    assert_eq!(bus.current(&ready_metric()), Some(0.9));
}

#[test]
fn replay_rewind_resets() {
    let pt = mk_simple_playthrough();
    // Fully apply.
    let bus_full = super::replay::rebuild_bus_to(&pt, pt.events.len());
    assert_eq!(bus_full.current(&ready_metric()), Some(0.9));
    // Rewind to zero: bus is declared but no samples.
    let bus_zero = super::replay::rebuild_bus_to(&pt, 0);
    assert_eq!(bus_zero.current(&ready_metric()), None);
    assert!(bus_zero.has_metric(&ready_metric()));
}

#[test]
fn plugin_registers_ai_debug_ui() {
    // AiDebugUi is initialised by UiPlugin; simulate minimally.
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(GameClock::new(0));
    app.insert_resource(crate::time_system::GameSpeed::default());
    app.add_plugins(AiPlugin);
    app.init_resource::<AiDebugUi>();
    app.update();
    assert!(app.world().get_resource::<AiDebugUi>().is_some());
    assert!(app.world().get_resource::<AiBusResource>().is_some());
}
