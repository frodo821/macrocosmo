//! Integration tests for `macrocosmo::ai` — the AI integration layer (#203).
//!
//! These tests validate the infrastructure wiring only; no content
//! (metrics/commands/evidence) is declared in #203, so the tests stick to
//! plugin boot behaviour, `AiBusWriter` stamping, and Bevy Query conflict
//! detection via `full_test_app()`.

mod common;

use bevy::prelude::*;
use macrocosmo::ai::emit::AiBusWriter;
use macrocosmo::ai::{AiBusResource, AiPlugin};
use macrocosmo::time_system::{GameClock, GameSpeed};
use macrocosmo_ai::{MetricId, MetricSpec, Retention, WarningMode};

use common::{full_test_app, test_app};

/// Build a minimal app carrying just the bits AiPlugin needs to boot.
fn minimal_ai_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(GameClock::new(0));
    app.insert_resource(GameSpeed::default());
    app.add_plugins(AiPlugin);
    app
}

#[test]
fn ai_plugin_boots_and_ticks_empty_bus() {
    let mut app = minimal_ai_app();

    // Run Startup + a handful of Update ticks. With nothing registered
    // under `AiTickSet::*` this must be a no-op that doesn't panic.
    for _ in 0..5 {
        app.update();
    }

    // AiBusResource must exist and have no declared content.
    let bus = app
        .world()
        .get_resource::<AiBusResource>()
        .expect("AiBusResource missing after AiPlugin::build");
    assert!(!bus.has_metric(&MetricId::from("unused")));
}

/// Confirm `AiBusWriter::emit` stamps with the current `GameClock` tick,
/// and that `emit_at` respects caller-supplied ticks.
#[test]
fn ai_bus_writer_stamps_current_tick() {
    let mut app = minimal_ai_app();

    // Declare a test metric directly on the bus resource. Use Silent mode
    // to keep test output clean; re-declaration in later ticks is a
    // no-op for this test.
    {
        let mut bus = app.world_mut().resource_mut::<AiBusResource>();
        bus.0.set_warning_mode(WarningMode::Silent);
        bus.0.declare_metric(
            MetricId::from("ai_integration_test"),
            MetricSpec::ratio(Retention::Long, "test"),
        );
    }

    // System that emits 0.25 at the current tick.
    fn emit_at_now(mut writer: AiBusWriter) {
        writer.emit(&MetricId::from("ai_integration_test"), 0.25);
    }
    app.add_systems(Update, emit_at_now);

    // Tick 1: clock=10, emit -> stamp at=10.
    app.world_mut().resource_mut::<GameClock>().elapsed = 10;
    app.update();
    {
        let bus = app.world().resource::<AiBusResource>();
        assert_eq!(bus.at(&MetricId::from("ai_integration_test"), 10), Some(0.25));
    }

    // Tick 2: clock=25, emit -> stamp at=25. Previous sample still there.
    app.world_mut().resource_mut::<GameClock>().elapsed = 25;
    app.update();
    {
        let bus = app.world().resource::<AiBusResource>();
        assert_eq!(bus.at(&MetricId::from("ai_integration_test"), 25), Some(0.25));
        assert_eq!(bus.at(&MetricId::from("ai_integration_test"), 10), Some(0.25));
        assert_eq!(bus.current(&MetricId::from("ai_integration_test")), Some(0.25));
    }
}

/// `full_test_app()` includes visualization + every game system registered
/// side-by-side. If AiPlugin introduces a Query conflict (B0001) with any
/// of them, Bevy will panic at schedule build / tick time.
#[test]
fn ai_plugin_no_query_conflict_with_full_test_app() {
    let mut app = full_test_app();
    for _ in 0..3 {
        app.update();
    }
    assert!(app.world().get_resource::<AiBusResource>().is_some());
}

/// `test_app()` also embeds `AiPlugin`; make sure the headless logic app
/// still boots cleanly so pre-existing tests keep working.
#[test]
fn ai_plugin_coexists_with_test_app() {
    let mut app = test_app();
    for _ in 0..3 {
        app.update();
    }
    assert!(app.world().get_resource::<AiBusResource>().is_some());
}
