//! #291: Integration tests for `macrocosmo:fleet_system_entered` and
//! `macrocosmo:fleet_system_left` Lua events.

mod common;

use bevy::prelude::*;
use macrocosmo::components::Position;
use macrocosmo::event_system::{EventSystem, FLEET_SYSTEM_ENTERED_EVENT, FLEET_SYSTEM_LEFT_EVENT};
use macrocosmo::ship::fleet::{Fleet, FleetMembers};
use macrocosmo::ship::transit_events::LastDockedSystem;
use macrocosmo::ship::*;

use common::{advance_time, spawn_test_system, test_app};

/// Helper: spawn a ship with a fleet and LastDockedSystem.
fn spawn_ship_with_fleet(
    world: &mut World,
    name: &str,
    system: Entity,
    pos: [f64; 3],
    state: ShipState,
    last_docked: Option<Entity>,
) -> (Entity, Entity) {
    let ship_entity = world.spawn_empty().id();
    let fleet_entity = world.spawn_empty().id();
    world.entity_mut(ship_entity).insert((
        Ship {
            name: name.to_string(),
            design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 0.75,
            ftl_range: 15.0,
            ruler_aboard: false,
            home_port: system,
            design_revision: 0,
            fleet: Some(fleet_entity),
        },
        state,
        Position::from(pos),
        ShipHitpoints {
            hull: 50.0,
            hull_max: 50.0,
            armor: 0.0,
            armor_max: 0.0,
            shield: 0.0,
            shield_max: 0.0,
            shield_regen: 0.0,
        },
        CommandQueue::default(),
        Cargo::default(),
        ShipModifiers::default(),
        ShipStats::default(),
        RulesOfEngagement::default(),
        LastDockedSystem(last_docked),
    ));
    world.entity_mut(fleet_entity).insert((
        Fleet {
            name: name.to_string(),
            flagship: Some(ship_entity),
        },
        FleetMembers(vec![ship_entity]),
    ));
    (ship_entity, fleet_entity)
}

/// Helper: set a ship's ShipState (Bevy's `Mut<T>` auto-detects the change).
fn set_ship_state(world: &mut World, entity: Entity, new_state: ShipState) {
    let mut eref = world.entity_mut(entity);
    let mut state = eref.get_mut::<ShipState>().unwrap();
    *state = new_state;
}

// -------------------------------------------------------------------------
// Arrival events
// -------------------------------------------------------------------------

#[test]
fn ftl_arrival_fires_fleet_system_entered() {
    let mut app = test_app();

    let sys_a = spawn_test_system(app.world_mut(), "A", [0.0, 0.0, 0.0], 1.0, true, true);
    let sys_b = spawn_test_system(app.world_mut(), "B", [5.0, 0.0, 0.0], 1.0, true, true);

    let arrival_at: i64 = 10;
    spawn_ship_with_fleet(
        app.world_mut(),
        "FTL-Ship",
        sys_a,
        [0.0, 0.0, 0.0],
        ShipState::InFTL {
            origin_system: sys_a,
            destination_system: sys_b,
            departed_at: 0,
            arrival_at,
        },
        None,
    );

    // Run initial update so Changed<ShipState> base is established.
    app.update();

    // Clear any events from initial update.
    app.world_mut()
        .resource_mut::<EventSystem>()
        .fired_log
        .clear();

    // Advance to arrival.
    advance_time(&mut app, arrival_at);

    let es = app.world().resource::<EventSystem>();
    let entered: Vec<_> = es
        .fired_log
        .iter()
        .filter(|e| e.event_id == FLEET_SYSTEM_ENTERED_EVENT)
        .collect();
    assert_eq!(
        entered.len(),
        1,
        "FTL arrival should fire exactly one fleet_system_entered event"
    );
    let ctx = entered[0].payload.as_ref().expect("payload attached");
    assert_eq!(ctx.payload_get("mode").as_deref(), Some("ftl"));
}

#[test]
fn sublight_arrival_at_system_fires_fleet_system_entered() {
    let mut app = test_app();

    let sys_a = spawn_test_system(app.world_mut(), "A", [0.0, 0.0, 0.0], 1.0, true, true);
    let sys_b = spawn_test_system(app.world_mut(), "B", [5.0, 0.0, 0.0], 1.0, true, true);

    let arrival_at: i64 = 10;
    spawn_ship_with_fleet(
        app.world_mut(),
        "SL-Ship",
        sys_a,
        [0.0, 0.0, 0.0],
        ShipState::SubLight {
            origin: [0.0, 0.0, 0.0],
            destination: [5.0, 0.0, 0.0],
            target_system: Some(sys_b),
            departed_at: 0,
            arrival_at,
        },
        None,
    );

    app.update();
    app.world_mut()
        .resource_mut::<EventSystem>()
        .fired_log
        .clear();

    advance_time(&mut app, arrival_at);

    let es = app.world().resource::<EventSystem>();
    let entered: Vec<_> = es
        .fired_log
        .iter()
        .filter(|e| e.event_id == FLEET_SYSTEM_ENTERED_EVENT)
        .collect();
    assert_eq!(
        entered.len(),
        1,
        "Sublight arrival at a system should fire fleet_system_entered"
    );
    let ctx = entered[0].payload.as_ref().expect("payload attached");
    assert_eq!(ctx.payload_get("mode").as_deref(), Some("sublight"));
}

#[test]
fn deep_space_loitering_arrival_does_not_fire_entered() {
    let mut app = test_app();

    let sys_a = spawn_test_system(app.world_mut(), "A", [0.0, 0.0, 0.0], 1.0, true, true);

    let arrival_at: i64 = 10;
    spawn_ship_with_fleet(
        app.world_mut(),
        "DS-Ship",
        sys_a,
        [0.0, 0.0, 0.0],
        ShipState::SubLight {
            origin: [0.0, 0.0, 0.0],
            destination: [50.0, 50.0, 0.0],
            target_system: None,
            departed_at: 0,
            arrival_at,
        },
        None,
    );

    app.update();
    app.world_mut()
        .resource_mut::<EventSystem>()
        .fired_log
        .clear();

    advance_time(&mut app, arrival_at);

    let es = app.world().resource::<EventSystem>();
    let entered: Vec<_> = es
        .fired_log
        .iter()
        .filter(|e| e.event_id == FLEET_SYSTEM_ENTERED_EVENT)
        .collect();
    assert_eq!(
        entered.len(),
        0,
        "Deep-space Loitering arrival must NOT fire fleet_system_entered"
    );
}

// -------------------------------------------------------------------------
// Departure events
// -------------------------------------------------------------------------

#[test]
fn ftl_departure_fires_fleet_system_left() {
    let mut app = test_app();

    let sys_a = spawn_test_system(app.world_mut(), "A", [0.0, 0.0, 0.0], 1.0, true, true);
    let sys_b = spawn_test_system(app.world_mut(), "B", [5.0, 0.0, 0.0], 1.0, true, true);

    let (ship_entity, _fleet_entity) = spawn_ship_with_fleet(
        app.world_mut(),
        "Departing-FTL",
        sys_a,
        [0.0, 0.0, 0.0],
        ShipState::InSystem { system: sys_a },
        Some(sys_a),
    );

    // Run initial update to establish change-detection baseline.
    app.update();
    app.world_mut()
        .resource_mut::<EventSystem>()
        .fired_log
        .clear();

    // Transition ship to InFTL.
    set_ship_state(
        app.world_mut(),
        ship_entity,
        ShipState::InFTL {
            origin_system: sys_a,
            destination_system: sys_b,
            departed_at: 0,
            arrival_at: 100,
        },
    );

    app.update();

    let es = app.world().resource::<EventSystem>();
    let left: Vec<_> = es
        .fired_log
        .iter()
        .filter(|e| e.event_id == FLEET_SYSTEM_LEFT_EVENT)
        .collect();
    assert_eq!(
        left.len(),
        1,
        "FTL departure should fire exactly one fleet_system_left event"
    );
    let ctx = left[0].payload.as_ref().expect("payload attached");
    assert_eq!(ctx.payload_get("mode").as_deref(), Some("ftl"));
}

#[test]
fn sublight_departure_from_system_fires_fleet_system_left() {
    let mut app = test_app();

    let sys_a = spawn_test_system(app.world_mut(), "A", [0.0, 0.0, 0.0], 1.0, true, true);
    let sys_b = spawn_test_system(app.world_mut(), "B", [5.0, 0.0, 0.0], 1.0, true, true);

    let (ship_entity, _fleet_entity) = spawn_ship_with_fleet(
        app.world_mut(),
        "Departing-SL",
        sys_a,
        [0.0, 0.0, 0.0],
        ShipState::InSystem { system: sys_a },
        Some(sys_a),
    );

    app.update();
    app.world_mut()
        .resource_mut::<EventSystem>()
        .fired_log
        .clear();

    // Transition ship to SubLight.
    set_ship_state(
        app.world_mut(),
        ship_entity,
        ShipState::SubLight {
            origin: [0.0, 0.0, 0.0],
            destination: [5.0, 0.0, 0.0],
            target_system: Some(sys_b),
            departed_at: 0,
            arrival_at: 100,
        },
    );

    app.update();

    let es = app.world().resource::<EventSystem>();
    let left: Vec<_> = es
        .fired_log
        .iter()
        .filter(|e| e.event_id == FLEET_SYSTEM_LEFT_EVENT)
        .collect();
    assert_eq!(
        left.len(),
        1,
        "Sublight departure from a system should fire fleet_system_left"
    );
    let ctx = left[0].payload.as_ref().expect("payload attached");
    assert_eq!(ctx.payload_get("mode").as_deref(), Some("sublight"));
}

#[test]
fn loitering_departure_does_not_fire_left() {
    let mut app = test_app();

    let sys_a = spawn_test_system(app.world_mut(), "A", [0.0, 0.0, 0.0], 1.0, true, true);

    let (ship_entity, _fleet_entity) = spawn_ship_with_fleet(
        app.world_mut(),
        "Loitering-Ship",
        sys_a,
        [50.0, 50.0, 0.0],
        ShipState::Loitering {
            position: [50.0, 50.0, 0.0],
        },
        None, // was loitering, not in a system
    );

    app.update();
    app.world_mut()
        .resource_mut::<EventSystem>()
        .fired_log
        .clear();

    // Transition from Loitering to SubLight.
    set_ship_state(
        app.world_mut(),
        ship_entity,
        ShipState::SubLight {
            origin: [50.0, 50.0, 0.0],
            destination: [0.0, 0.0, 0.0],
            target_system: Some(sys_a),
            departed_at: 0,
            arrival_at: 100,
        },
    );

    app.update();

    let es = app.world().resource::<EventSystem>();
    let left: Vec<_> = es
        .fired_log
        .iter()
        .filter(|e| e.event_id == FLEET_SYSTEM_LEFT_EVENT)
        .collect();
    assert_eq!(
        left.len(),
        0,
        "Departure from Loitering must NOT fire fleet_system_left"
    );
}

// -------------------------------------------------------------------------
// Arrival event carries correct system and fleet entity
// -------------------------------------------------------------------------

#[test]
fn arrival_event_carries_correct_system_and_fleet() {
    let mut app = test_app();

    let sys_a = spawn_test_system(app.world_mut(), "A", [0.0, 0.0, 0.0], 1.0, true, true);
    let sys_b = spawn_test_system(app.world_mut(), "B", [5.0, 0.0, 0.0], 1.0, true, true);

    let arrival_at: i64 = 10;
    let (_ship_entity, fleet_entity) = spawn_ship_with_fleet(
        app.world_mut(),
        "FTL-Ship",
        sys_a,
        [0.0, 0.0, 0.0],
        ShipState::InFTL {
            origin_system: sys_a,
            destination_system: sys_b,
            departed_at: 0,
            arrival_at,
        },
        None,
    );

    app.update();
    app.world_mut()
        .resource_mut::<EventSystem>()
        .fired_log
        .clear();

    advance_time(&mut app, arrival_at);

    let es = app.world().resource::<EventSystem>();
    let entered: Vec<_> = es
        .fired_log
        .iter()
        .filter(|e| e.event_id == FLEET_SYSTEM_ENTERED_EVENT)
        .collect();
    assert_eq!(entered.len(), 1);

    let ctx = entered[0].payload.as_ref().unwrap();
    assert_eq!(
        ctx.payload_get("system_entity").as_deref(),
        Some(&sys_b.to_bits().to_string()[..]),
        "system_entity should be the destination system"
    );
    assert_eq!(
        ctx.payload_get("fleet_entity").as_deref(),
        Some(&fleet_entity.to_bits().to_string()[..]),
        "fleet_entity should match the ship's fleet"
    );
}
