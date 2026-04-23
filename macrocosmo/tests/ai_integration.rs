//! Integration tests for `macrocosmo::ai` — the AI integration layer (#203).
//!
//! These tests validate the infrastructure wiring only; no content
//! (metrics/commands/evidence) is declared in #203, so the tests stick to
//! plugin boot behaviour, `AiBusWriter` stamping, and Bevy Query conflict
//! detection via `full_test_app()`.

mod common;

use bevy::prelude::*;
use macrocosmo::ai::emit::AiBusWriter;
use macrocosmo::ai::npc_decision::{AiControlled, AiPlayerMode};
use macrocosmo::ai::schema::ids;
use macrocosmo::ai::{AiBusResource, AiPlugin};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::time_system::{GameClock, GameSpeed};
use macrocosmo_ai::{MetricId, MetricSpec, Retention, WarningMode};

use common::{full_test_app, test_app};

/// Build a minimal app carrying just the bits AiPlugin needs to boot.
fn minimal_ai_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    // #439 Phase 2: AiPlugin's decision / marker tick now run only in
    // `GameState::InGame`. Seed it so tests that touch those systems
    // (mark_npc_ai_controlled, decision tick) observe them running.
    app.add_plugins(macrocosmo::game_state::GameStatePlugin);
    app.insert_state(macrocosmo::game_state::GameState::InGame);
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
        assert_eq!(
            bus.at(&MetricId::from("ai_integration_test"), 10),
            Some(0.25)
        );
    }

    // Tick 2: clock=25, emit -> stamp at=25. Previous sample still there.
    app.world_mut().resource_mut::<GameClock>().elapsed = 25;
    app.update();
    {
        let bus = app.world().resource::<AiBusResource>();
        assert_eq!(
            bus.at(&MetricId::from("ai_integration_test"), 25),
            Some(0.25)
        );
        assert_eq!(
            bus.at(&MetricId::from("ai_integration_test"), 10),
            Some(0.25)
        );
        assert_eq!(
            bus.current(&MetricId::from("ai_integration_test")),
            Some(0.25)
        );
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

/// Startup-time schema declarations (#198) register every Tier 1 topic
/// so downstream producers and evaluators observe a stable vocabulary
/// as soon as the plugin runs its `Startup` system.
#[test]
fn ai_plugin_declares_tier1_schema_on_startup() {
    let mut app = minimal_ai_app();
    // A single tick is enough — `schema::declare_all` is registered on
    // `Startup`, which runs before the first `Update`.
    app.update();
    let bus = app
        .world()
        .get_resource::<AiBusResource>()
        .expect("AiBusResource missing after AiPlugin::build");

    // Metrics — spot-check one per catalogue category.
    assert!(bus.has_metric(&ids::metric::my_strength()));
    assert!(bus.has_metric(&ids::metric::net_production_minerals()));
    assert!(bus.has_metric(&ids::metric::stockpile_energy()));
    assert!(bus.has_metric(&ids::metric::population_total()));
    assert!(bus.has_metric(&ids::metric::colony_count()));
    assert!(bus.has_metric(&ids::metric::tech_total_researched()));
    assert!(bus.has_metric(&ids::metric::systems_with_shipyard()));
    assert!(bus.has_metric(&ids::metric::game_elapsed_time()));

    // Commands.
    assert!(bus.has_command_kind(&ids::command::attack_target()));
    assert!(bus.has_command_kind(&ids::command::colonize_system()));
    assert!(bus.has_command_kind(&ids::command::research_focus()));
    assert!(bus.has_command_kind(&ids::command::declare_war()));

    // Evidence kinds.
    assert!(bus.has_evidence_kind(&ids::evidence::direct_attack()));
    assert!(bus.has_evidence_kind(&ids::evidence::gift_given()));
    assert!(bus.has_evidence_kind(&ids::evidence::major_military_buildup()));
}

// ---------------------------------------------------------------------------
// AiControlled / AiPlayerMode tests (#398)
// ---------------------------------------------------------------------------

/// NPC empires (Empire without PlayerEmpire) are automatically marked
/// AiControlled after a tick.
#[test]
fn npc_empires_get_ai_controlled_marker() {
    let mut app = minimal_ai_app();
    // Spawn an NPC empire (Empire + Faction, no PlayerEmpire).
    let npc = app
        .world_mut()
        .spawn((
            Empire { name: "NPC".into() },
            Faction::new("test_npc", "Test NPC"),
        ))
        .id();
    app.update();
    assert!(
        app.world().get::<AiControlled>(npc).is_some(),
        "NPC empire should have AiControlled after one tick"
    );
}

/// Player empire is NOT marked AiControlled by default (AiPlayerMode(false)).
#[test]
fn player_empire_not_ai_controlled_by_default() {
    let mut app = minimal_ai_app();
    let player = app
        .world_mut()
        .spawn((
            Empire {
                name: "Player".into(),
            },
            PlayerEmpire,
            Faction::new("player", "Player"),
        ))
        .id();
    // Default AiPlayerMode is false.
    app.update();
    assert!(
        app.world().get::<AiControlled>(player).is_none(),
        "Player empire should NOT have AiControlled when AiPlayerMode is false"
    );
}

/// Player empire IS marked AiControlled when AiPlayerMode(true).
#[test]
fn player_empire_ai_controlled_when_opt_in() {
    let mut app = minimal_ai_app();
    app.insert_resource(AiPlayerMode(true));
    let player = app
        .world_mut()
        .spawn((
            Empire {
                name: "Player".into(),
            },
            PlayerEmpire,
            Faction::new("player", "Player"),
        ))
        .id();
    app.update();
    // Commands are applied at end of tick; need another tick for the
    // AiControlled component to be visible.
    app.update();
    assert!(
        app.world().get::<AiControlled>(player).is_some(),
        "Player empire should have AiControlled when AiPlayerMode(true)"
    );
}

/// Both NPC and player empire have AiControlled when AiPlayerMode(true),
/// and the decision tick processes both without panicking.
#[test]
fn ai_player_mode_ticks_both_empires() {
    let mut app = minimal_ai_app();
    app.insert_resource(AiPlayerMode(true));
    app.world_mut().spawn((
        Empire {
            name: "Player".into(),
        },
        PlayerEmpire,
        Faction::new("player", "Player"),
    ));
    app.world_mut().spawn((
        Empire { name: "NPC".into() },
        Faction::new("test_npc", "Test NPC"),
    ));

    // Several ticks — must not panic.
    for t in 1..=10 {
        app.world_mut().resource_mut::<GameClock>().elapsed = t;
        app.update();
    }

    // Both should be AiControlled.
    let ai_controlled_count = app
        .world_mut()
        .query_filtered::<Entity, With<AiControlled>>()
        .iter(app.world())
        .count();
    assert_eq!(
        ai_controlled_count, 2,
        "Both player and NPC empires should be AiControlled"
    );
}
