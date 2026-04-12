mod common;

use bevy::prelude::*;
use macrocosmo::amount::{Amt, SignedAmt};
use macrocosmo::colony::*;
use macrocosmo::modifier::ModifiedValue;
use macrocosmo::event_system::{EventDefinition, EventSystem, EventTrigger};
use macrocosmo::events::{EventLog, GameEventKind};

use macrocosmo::modifier::Modifier;

use common::{advance_time, find_planet, spawn_test_colony, spawn_test_system, test_app, test_app_with_event_log};

#[test]
fn test_expired_modifier_has_on_expire_event() {
    use common::*;

    let mut app = test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "Expire Event Test",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );

    let colony_id = spawn_test_colony(
        app.world_mut(),
        sys,
        Amt::ZERO,
        Amt::ZERO,
        vec![],
    );

    // Push a modifier with duration=5 and on_expire_event="test_event"
    {
        let mut prod = app.world_mut().get_mut::<Production>(colony_id).unwrap();
        prod.minerals_per_hexadies.push_modifier_timed(
            Modifier {
                id: "event_boost".to_string(),
                label: "Event Boost".to_string(),
                base_add: SignedAmt::units(2),
                multiplier: SignedAmt::ZERO,
                add: SignedAmt::ZERO,
                expires_at: None,
                on_expire_event: Some("test_event".to_string()),
            },
            0,
            5,
        );
    }

    // At clock=3, modifier should still be present
    advance_time(&mut app, 3);
    {
        let prod = app.world().get::<Production>(colony_id).unwrap();
        assert!(prod.minerals_per_hexadies.has_modifier("event_boost"));
    }

    // Advance past expiry (clock=6)
    advance_time(&mut app, 3);
    {
        let prod = app.world().get::<Production>(colony_id).unwrap();
        assert!(
            !prod.minerals_per_hexadies.has_modifier("event_boost"),
            "Modifier with on_expire_event should be removed after expiry"
        );
    }
}

// Periodic event fires on interval

#[test]
fn test_periodic_event_fires() {
    let mut app = test_app();

    // Register a periodic event with interval=5 hexadies
    {
        let mut event_system = app.world_mut().resource_mut::<EventSystem>();
        event_system.register(EventDefinition {
            id: "periodic_test".to_string(),
            name: "Periodic Test".to_string(),
            description: "Fires every 5 hexadies.".to_string(),
            trigger: EventTrigger::Periodic {
                interval_hexadies: 5,
                last_fired: 0,
                fire_condition: None,
                max_times: None,
                times_triggered: 0,
            },
        });
    }

    // Advance 5 hexadies -- periodic event should fire
    advance_time(&mut app, 5);

    {
        let event_system = app.world().resource::<EventSystem>();
        assert_eq!(
            event_system.fired_log.len(),
            1,
            "Periodic event should have fired once at t=5"
        );
        assert_eq!(event_system.fired_log[0].event_id, "periodic_test");
        assert_eq!(event_system.fired_log[0].fired_at, 5);
    }

    // Advance 3 more hexadies (t=8) -- should NOT fire again
    advance_time(&mut app, 3);

    {
        let event_system = app.world().resource::<EventSystem>();
        assert_eq!(
            event_system.fired_log.len(),
            1,
            "Periodic event should not have fired again at t=8"
        );
    }

    // Advance 2 more (t=10) -- should fire again
    advance_time(&mut app, 2);

    {
        let event_system = app.world().resource::<EventSystem>();
        assert_eq!(
            event_system.fired_log.len(),
            2,
            "Periodic event should have fired again at t=10"
        );
        assert_eq!(event_system.fired_log[1].event_id, "periodic_test");
        assert_eq!(event_system.fired_log[1].fired_at, 10);
    }
}

// CRITICAL: tick_timed_effects cleans all components (#1)

#[test]
fn test_tick_timed_effects_cleans_all_components() {
    use macrocosmo::modifier::Modifier;
    use macrocosmo::amount::SignedAmt;

    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Test System",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );

    let colony = spawn_test_colony(
        app.world_mut(),
        sys,
        Amt::units(500),
        Amt::units(500),
        vec![],
    );

    // Push timed modifiers (duration=5, so expires_at = 0 + 5 = 5) to all three components
    {
        let mut prod = app.world_mut().get_mut::<Production>(colony).unwrap();
        prod.minerals_per_hexadies.push_modifier_timed(
            Modifier {
                id: "timed_prod".to_string(),
                label: "Timed production bonus".to_string(),
                base_add: SignedAmt::units(10),
                multiplier: SignedAmt::ZERO,
                add: SignedAmt::ZERO,
                expires_at: None,
                on_expire_event: None,
            },
            0,
            5,
        );
        assert_eq!(prod.minerals_per_hexadies.modifiers().len(), 1);
    }
    {
        let mut maint = app.world_mut().get_mut::<MaintenanceCost>(colony).unwrap();
        maint.energy_per_hexadies.push_modifier_timed(
            Modifier {
                id: "timed_maint".to_string(),
                label: "Timed maintenance cost".to_string(),
                base_add: SignedAmt::units(2),
                multiplier: SignedAmt::ZERO,
                add: SignedAmt::ZERO,
                expires_at: None,
                on_expire_event: None,
            },
            0,
            5,
        );
        assert_eq!(maint.energy_per_hexadies.modifiers().len(), 1);
    }
    {
        let mut fc = app.world_mut().get_mut::<FoodConsumption>(colony).unwrap();
        fc.food_per_hexadies.push_modifier_timed(
            Modifier {
                id: "timed_food".to_string(),
                label: "Timed food consumption".to_string(),
                base_add: SignedAmt::units(3),
                multiplier: SignedAmt::ZERO,
                add: SignedAmt::ZERO,
                expires_at: None,
                on_expire_event: None,
            },
            0,
            5,
        );
        assert_eq!(fc.food_per_hexadies.modifiers().len(), 1);
    }

    // Advance 6 hexadies -- all three should expire (expires_at=5, clock=6)
    advance_time(&mut app, 6);

    // Verify Production modifier removed
    let prod = app.world().get::<Production>(colony).unwrap();
    assert_eq!(
        prod.minerals_per_hexadies.modifiers().iter().filter(|m| m.id == "timed_prod").count(),
        0,
        "Production timed modifier should have been removed"
    );

    // Verify MaintenanceCost modifier removed
    let maint = app.world().get::<MaintenanceCost>(colony).unwrap();
    assert_eq!(
        maint.energy_per_hexadies.modifiers().iter().filter(|m| m.id == "timed_maint").count(),
        0,
        "MaintenanceCost timed modifier should have been removed"
    );

    // Verify FoodConsumption modifier removed
    let fc = app.world().get::<FoodConsumption>(colony).unwrap();
    assert_eq!(
        fc.food_per_hexadies.modifiers().iter().filter(|m| m.id == "timed_food").count(),
        0,
        "FoodConsumption timed modifier should have been removed"
    );
}

// MAJOR: on_expire_event fires named event (#6)

#[test]
fn test_on_expire_event_fires_named_event() {
    use macrocosmo::event_system::{EventDefinition, EventSystem, EventTrigger};
    use macrocosmo::modifier::Modifier;
    use macrocosmo::amount::SignedAmt;

    let mut app = test_app();

    // Register an event definition
    {
        let mut event_system = app.world_mut().resource_mut::<EventSystem>();
        event_system.register(EventDefinition {
            id: "test_expire_event".to_string(),
            name: "Test Expire Event".to_string(),
            description: "Fires when a modifier expires.".to_string(),
            trigger: EventTrigger::Manual,
        });
    }

    let sys = spawn_test_system(
        app.world_mut(),
        "Test System",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );

    let colony = spawn_test_colony(
        app.world_mut(),
        sys,
        Amt::units(500),
        Amt::units(500),
        vec![],
    );

    // Push modifier with duration=3 and on_expire_event
    {
        let mut prod = app.world_mut().get_mut::<Production>(colony).unwrap();
        prod.minerals_per_hexadies.push_modifier_timed(
            Modifier {
                id: "expiring_mod".to_string(),
                label: "Expiring modifier".to_string(),
                base_add: SignedAmt::units(5),
                multiplier: SignedAmt::ZERO,
                add: SignedAmt::ZERO,
                expires_at: None,
                on_expire_event: Some("test_expire_event".to_string()),
            },
            0,
            3,
        );
    }

    // Advance 4 hexadies to trigger expiration
    advance_time(&mut app, 4);

    // Check EventSystem.fired_log contains our event
    let event_system = app.world().resource::<EventSystem>();
    let found = event_system
        .fired_log
        .iter()
        .any(|e| e.event_id == "test_expire_event");
    assert!(
        found,
        "EventSystem.fired_log should contain 'test_expire_event' after modifier expires"
    );
}

// =========================================================================
// Resource depletion alerts (#80)
// =========================================================================

#[test]
fn test_food_depletion_alert() {
    let mut app = test_app_with_event_log();
    let sys = spawn_test_system(
        app.world_mut(),
        "Starving",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );

    // Colony with food = 0
    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().entity_mut(sys).insert((ResourceStockpile {
            minerals: Amt::units(500),
            energy: Amt::units(500),
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        }, ResourceCapacity::default()));
    let _colony = app.world_mut().spawn((
        Colony { planet: planet_sys, population: 100.0, growth_rate: 0.01 },
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::units(5)),
            energy_per_hexadies: ModifiedValue::new(Amt::units(5)),
            research_per_hexadies: ModifiedValue::new(Amt::units(1)),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![] },
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    )).id();

    advance_time(&mut app, 1);
    app.update();

    let log = app.world().resource::<EventLog>();
    let alerts: Vec<_> = log.entries.iter()
        .filter(|e| e.kind == GameEventKind::ResourceAlert)
        .collect();
    assert!(!alerts.is_empty(), "Expected a food depletion alert");
    assert!(alerts[0].description.contains("Starvation"), "Alert should mention starvation");
    assert!(alerts[0].related_system == Some(sys));
}

#[test]
fn test_energy_depletion_alert() {
    let mut app = test_app_with_event_log();
    let sys = spawn_test_system(
        app.world_mut(),
        "NoPower",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );

    // Colony with energy = 0
    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().entity_mut(sys).insert((ResourceStockpile {
            minerals: Amt::units(500),
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        }, ResourceCapacity::default()));
    let _colony = app.world_mut().spawn((
        Colony { planet: planet_sys, population: 100.0, growth_rate: 0.01 },
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::units(5)),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::units(1)),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![] },
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    )).id();

    advance_time(&mut app, 1);
    // Second update so collect_events picks up messages from previous frame
    app.update();

    let log = app.world().resource::<EventLog>();
    let alerts: Vec<_> = log.entries.iter()
        .filter(|e| e.kind == GameEventKind::ResourceAlert)
        .collect();
    assert!(!alerts.is_empty(), "Expected an energy depletion alert, got: {:?}", alerts.iter().map(|a| &a.description).collect::<Vec<_>>());
    let energy_alerts: Vec<_> = alerts.iter().filter(|a| a.description.contains("Energy depleted")).collect();
    assert!(!energy_alerts.is_empty(), "Alert should mention energy depletion, got: {:?}", alerts.iter().map(|a| &a.description).collect::<Vec<_>>());
    assert!(energy_alerts[0].related_system == Some(sys));
}

#[test]
fn test_alert_cooldown() {
    let mut app = test_app_with_event_log();
    let sys = spawn_test_system(
        app.world_mut(),
        "Starving",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    // Colony with food = 0 and no food production
    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().entity_mut(sys).insert((ResourceStockpile {
            minerals: Amt::units(500),
            energy: Amt::units(500),
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        }, ResourceCapacity::default()));
    let _colony = app.world_mut().spawn((
        Colony { planet: planet_sys, population: 100.0, growth_rate: 0.01 },
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::units(5)),
            energy_per_hexadies: ModifiedValue::new(Amt::units(5)),
            research_per_hexadies: ModifiedValue::new(Amt::units(1)),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![] },
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    )).id();

    // First tick: alert fires
    advance_time(&mut app, 1);
    app.update(); // collect messages
    let count_1 = app.world().resource::<EventLog>().entries.iter()
        .filter(|e| e.kind == GameEventKind::ResourceAlert)
        .count();
    assert_eq!(count_1, 1, "First tick should produce exactly one food alert");

    // Advance less than 30 hexadies: no duplicate
    advance_time(&mut app, 10);
    app.update(); // collect messages
    let count_2 = app.world().resource::<EventLog>().entries.iter()
        .filter(|e| e.kind == GameEventKind::ResourceAlert)
        .count();
    assert_eq!(count_2, 1, "Alert should not repeat within cooldown period");

    // Advance past 30 hexadies total from first alert: alert fires again
    advance_time(&mut app, 25);
    app.update(); // collect messages
    let count_3 = app.world().resource::<EventLog>().entries.iter()
        .filter(|e| e.kind == GameEventKind::ResourceAlert)
        .count();
    assert!(count_3 >= 2, "Alert should fire again after cooldown expires");
}
