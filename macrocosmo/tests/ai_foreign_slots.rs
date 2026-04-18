//! Integration tests for `macrocosmo::ai::schema::foreign` — the
//! faction-awareness system that declares per-faction Tier 2 metric
//! slots on the bus.

use bevy::prelude::*;
use macrocosmo::ai::schema::foreign::{
    ForeignMetricTemplate, foreign_metric_id, foreign_metric_templates,
};
use macrocosmo::ai::{AiBusResource, AiPlugin};
use macrocosmo::player::Faction;
use macrocosmo::time_system::{GameClock, GameSpeed};
use macrocosmo_ai::WarningMode;

fn minimal_ai_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(GameClock::new(0));
    app.insert_resource(GameSpeed::default());
    app.insert_resource(AiBusResource::with_warning_mode(WarningMode::Silent));
    app.add_plugins(AiPlugin);
    app
}

#[test]
fn foreign_slots_declared_on_faction_spawn() {
    let mut app = minimal_ai_app();
    // Tick once so Startup declares Tier 1 schema.
    app.update();

    // Spawn a Faction entity.
    let entity = app
        .world_mut()
        .spawn(Faction::new("terran", "Terran Federation"))
        .id();

    // Tick again so declare_foreign_slots_on_awareness runs and picks up
    // the Added<Faction> component.
    app.update();

    let fid = macrocosmo::ai::convert::to_ai_faction(entity);
    let bus = app.world().resource::<AiBusResource>();
    for t in foreign_metric_templates() {
        let id = foreign_metric_id(&t.prefix, fid);
        assert!(
            bus.has_metric(&id),
            "missing foreign metric {id:?} for faction {fid:?}"
        );
    }
}

#[test]
fn foreign_slots_available_for_known_factions() {
    // Verify that multiple factions each get their own slot set.
    let mut app = minimal_ai_app();
    app.update();

    let a = app.world_mut().spawn(Faction::new("a", "A")).id();
    let b = app.world_mut().spawn(Faction::new("b", "B")).id();
    app.update();

    let fid_a = macrocosmo::ai::convert::to_ai_faction(a);
    let fid_b = macrocosmo::ai::convert::to_ai_faction(b);
    assert_ne!(fid_a, fid_b);

    let bus = app.world().resource::<AiBusResource>();
    for t in foreign_metric_templates() {
        let id_a = foreign_metric_id(&t.prefix, fid_a);
        let id_b = foreign_metric_id(&t.prefix, fid_b);
        assert!(bus.has_metric(&id_a));
        assert!(bus.has_metric(&id_b));
        // IDs must differ (guards against collision on `FactionId(_)` reuse).
        assert_ne!(id_a, id_b);
    }
}

#[test]
fn foreign_templates_deliver_specs() {
    for t in foreign_metric_templates() {
        let _: ForeignMetricTemplate = t.clone();
        // Run the factory — it should not panic and it should return a
        // spec with a non-empty description.
        let spec = (t.spec_factory)();
        assert!(!spec.description.is_empty());
    }
}
