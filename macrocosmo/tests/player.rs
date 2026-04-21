mod common;

use bevy::prelude::*;
use macrocosmo::amount::Amt;
use macrocosmo::components::Position;
use macrocosmo::events::{EventLog, GameEventKind};
use macrocosmo::galaxy::{AtSystem, Hostile, HostileHitpoints, HostileStats};
use macrocosmo::player::{AboardShip, Player, Ruler, StationedAt};
use macrocosmo::ship::*;

use common::{advance_time, spawn_test_system, test_app, test_app_with_event_log};

/// Helper: spawn a player entity stationed at the given system.
fn spawn_player(world: &mut World, system: Entity) -> Entity {
    let empire = world
        .query_filtered::<Entity, With<macrocosmo::player::PlayerEmpire>>()
        .iter(world)
        .next()
        .unwrap_or(Entity::PLACEHOLDER);
    world
        .spawn((
            Player,
            Ruler {
                name: "Test Player".into(),
                empire,
            },
            StationedAt { system },
        ))
        .id()
}

/// Helper: spawn a basic ship docked at the given system.
fn spawn_basic_ship(world: &mut World, name: &str, system: Entity) -> Entity {
    world
        .spawn((
            Ship {
                name: name.to_string(),
                design_id: "explorer_mk1".to_string(),
                hull_id: "corvette".to_string(),
                modules: Vec::new(),
                owner: Owner::Neutral,
                sublight_speed: 0.75,
                ftl_range: 10.0,
                ruler_aboard: false,
                home_port: system,
                design_revision: 0,
                fleet: None,
            },
            ShipState::InSystem { system },
            Position::from([0.0, 0.0, 0.0]),
            ShipHitpoints {
                hull: 50.0,
                hull_max: 50.0,
                armor: 0.0,
                armor_max: 0.0,
                shield: 0.0,
                shield_max: 0.0,
                shield_regen: 0.0,
            },
            ShipModifiers::default(),
            CommandQueue::default(),
            Cargo::default(),
        ))
        .id()
}

#[test]
fn test_player_board_ship() {
    let mut app = test_app();

    let sys = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 0.7, true, false);

    let player_entity = spawn_player(app.world_mut(), sys);
    let ship_entity = spawn_basic_ship(app.world_mut(), "Scout-1", sys);

    // Board the ship
    {
        let mut ship = app.world_mut().get_mut::<Ship>(ship_entity).unwrap();
        ship.ruler_aboard = true;
    }
    app.world_mut()
        .entity_mut(player_entity)
        .insert(AboardShip { ship: ship_entity });

    // Verify
    let ship = app.world().get::<Ship>(ship_entity).unwrap();
    assert!(ship.ruler_aboard, "Ship should have ruler_aboard = true");

    let aboard = app.world().get::<AboardShip>(player_entity).unwrap();
    assert_eq!(
        aboard.ship, ship_entity,
        "AboardShip should reference the correct ship"
    );
}

#[test]
fn test_player_disembark() {
    let mut app = test_app();

    let sys_a = spawn_test_system(
        app.world_mut(),
        "System-A",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );
    let sys_b = spawn_test_system(
        app.world_mut(),
        "System-B",
        [5.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    let player_entity = spawn_player(app.world_mut(), sys_a);
    let ship_entity = spawn_basic_ship(app.world_mut(), "Scout-1", sys_b);

    // Board the ship (player aboard ship at sys_b)
    {
        let mut ship = app.world_mut().get_mut::<Ship>(ship_entity).unwrap();
        ship.ruler_aboard = true;
    }
    app.world_mut()
        .entity_mut(player_entity)
        .insert(AboardShip { ship: ship_entity });

    // Run one update so update_ruler_location fires
    advance_time(&mut app, 1);

    // StationedAt should now be sys_b (ship is docked there)
    let stationed = app.world().get::<StationedAt>(player_entity).unwrap();
    assert_eq!(
        stationed.system, sys_b,
        "StationedAt should update to ship's docked system"
    );

    // Now disembark
    {
        let mut ship = app.world_mut().get_mut::<Ship>(ship_entity).unwrap();
        ship.ruler_aboard = false;
    }
    app.world_mut()
        .entity_mut(player_entity)
        .remove::<AboardShip>();

    // Player should no longer have AboardShip
    assert!(
        app.world().get::<AboardShip>(player_entity).is_none(),
        "Player should not have AboardShip after disembarking"
    );

    // StationedAt should still be sys_b
    let stationed = app.world().get::<StationedAt>(player_entity).unwrap();
    assert_eq!(
        stationed.system, sys_b,
        "StationedAt should remain at disembark location"
    );
}

#[test]
fn test_player_location_updates_with_ship() {
    let mut app = test_app();

    let sys_a = spawn_test_system(app.world_mut(), "Origin", [0.0, 0.0, 0.0], 0.7, true, false);
    let sys_b = spawn_test_system(
        app.world_mut(),
        "Destination",
        [5.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    let player_entity = spawn_player(app.world_mut(), sys_a);
    let ship_entity = spawn_basic_ship(app.world_mut(), "Explorer-1", sys_a);

    // Board the ship
    {
        let mut ship = app.world_mut().get_mut::<Ship>(ship_entity).unwrap();
        ship.ruler_aboard = true;
    }
    app.world_mut()
        .entity_mut(player_entity)
        .insert(AboardShip { ship: ship_entity });

    // Simulate ship arriving at sys_b by changing its state to Docked at sys_b
    {
        let mut state = app.world_mut().get_mut::<ShipState>(ship_entity).unwrap();
        *state = ShipState::InSystem { system: sys_b };
    }

    // Run update to trigger update_ruler_location
    advance_time(&mut app, 1);

    // StationedAt should follow the ship
    let stationed = app.world().get::<StationedAt>(player_entity).unwrap();
    assert_eq!(
        stationed.system, sys_b,
        "Player's StationedAt should follow ship to new docked system"
    );
}

#[test]
fn test_player_location_stays_during_transit() {
    let mut app = test_app();

    let sys_a = spawn_test_system(app.world_mut(), "Origin", [0.0, 0.0, 0.0], 0.7, true, false);
    let sys_b = spawn_test_system(
        app.world_mut(),
        "Destination",
        [5.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    let player_entity = spawn_player(app.world_mut(), sys_a);
    let ship_entity = spawn_basic_ship(app.world_mut(), "Explorer-1", sys_a);

    // Board ship
    {
        let mut ship = app.world_mut().get_mut::<Ship>(ship_entity).unwrap();
        ship.ruler_aboard = true;
    }
    app.world_mut()
        .entity_mut(player_entity)
        .insert(AboardShip { ship: ship_entity });

    // Ship goes into FTL
    {
        let mut state = app.world_mut().get_mut::<ShipState>(ship_entity).unwrap();
        *state = ShipState::InFTL {
            origin_system: sys_a,
            destination_system: sys_b,
            departed_at: 0,
            arrival_at: 10,
        };
    }

    advance_time(&mut app, 1);

    // StationedAt should still be sys_a (last docked system)
    let stationed = app.world().get::<StationedAt>(player_entity).unwrap();
    assert_eq!(
        stationed.system, sys_a,
        "Player's StationedAt should stay at origin during FTL transit"
    );
}

#[test]
fn test_player_respawn_on_ship_destruction() {
    let mut app = test_app();

    // Capital system
    let capital = {
        let sys = app
            .world_mut()
            .spawn((
                macrocosmo::galaxy::StarSystem {
                    name: "Capital".to_string(),
                    surveyed: true,
                    is_capital: true,
                    star_type: "default".to_string(),
                },
                Position::from([0.0, 0.0, 0.0]),
                macrocosmo::galaxy::Sovereignty::default(),
                macrocosmo::technology::TechKnowledge::default(),
                macrocosmo::galaxy::SystemModifiers::default(),
            ))
            .id();
        // Spawn planet for the system
        app.world_mut().spawn((
            macrocosmo::galaxy::Planet {
                name: "Capital I".to_string(),
                system: sys,
                planet_type: "default".to_string(),
            },
            macrocosmo::galaxy::SystemAttributes {
                habitability: 0.7,
                mineral_richness: 0.5,
                energy_potential: 0.5,
                research_potential: 0.5,
                max_building_slots: 4,
            },
            Position::from([0.0, 0.0, 0.0]),
        ));
        sys
    };

    // Remote system with hostile
    let remote = spawn_test_system(
        app.world_mut(),
        "Danger-Zone",
        [10.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    let player_entity = spawn_player(app.world_mut(), remote);

    // Spawn a weak ship at the remote system
    let ship_entity = app
        .world_mut()
        .spawn((
            Ship {
                name: "Flagship".to_string(),
                design_id: "explorer_mk1".to_string(),
                hull_id: "corvette".to_string(),
                modules: Vec::new(),
                owner: Owner::Neutral,
                sublight_speed: 0.75,
                ftl_range: 10.0,
                ruler_aboard: true,
                home_port: capital,
                design_revision: 0,
                fleet: None,
            },
            ShipState::InSystem { system: remote },
            Position::from([10.0, 0.0, 0.0]),
            ShipHitpoints {
                hull: 0.01,
                hull_max: 50.0,
                armor: 0.0,
                armor_max: 0.0,
                shield: 0.0,
                shield_max: 0.0,
                shield_regen: 0.0,
            },
            ShipModifiers::default(),
            CommandQueue::default(),
            Cargo::default(),
        ))
        .id();

    // Player is aboard the ship
    app.world_mut()
        .entity_mut(player_entity)
        .insert(AboardShip { ship: ship_entity });

    // Spawn a powerful hostile
    let _ = common::spawn_raw_hostile(
        app.world_mut(),
        remote,
        1000.0,
        1000.0,
        100.0,
        0.0,
        "space_creature",
    );

    // Run combat
    advance_time(&mut app, 1);

    // Ship should be destroyed
    assert!(
        app.world().get_entity(ship_entity).is_err(),
        "Player's ship should be destroyed"
    );

    // Player should be respawned at capital
    let stationed = app.world().get::<StationedAt>(player_entity).unwrap();
    assert_eq!(
        stationed.system, capital,
        "Player should be respawned at capital system"
    );

    // AboardShip should be removed
    assert!(
        app.world().get::<AboardShip>(player_entity).is_none(),
        "AboardShip should be removed after ship destruction"
    );
}

#[test]
fn test_player_respawn_event_fires() {
    let mut app = test_app_with_event_log();

    // Capital system
    let capital = {
        let sys = app
            .world_mut()
            .spawn((
                macrocosmo::galaxy::StarSystem {
                    name: "Capital".to_string(),
                    surveyed: true,
                    is_capital: true,
                    star_type: "default".to_string(),
                },
                Position::from([0.0, 0.0, 0.0]),
                macrocosmo::galaxy::Sovereignty::default(),
                macrocosmo::technology::TechKnowledge::default(),
                macrocosmo::galaxy::SystemModifiers::default(),
            ))
            .id();
        app.world_mut().spawn((
            macrocosmo::galaxy::Planet {
                name: "Capital I".to_string(),
                system: sys,
                planet_type: "default".to_string(),
            },
            macrocosmo::galaxy::SystemAttributes {
                habitability: 0.7,
                mineral_richness: 0.5,
                energy_potential: 0.5,
                research_potential: 0.5,
                max_building_slots: 4,
            },
            Position::from([0.0, 0.0, 0.0]),
        ));
        sys
    };

    let remote = spawn_test_system(
        app.world_mut(),
        "Danger-Zone",
        [10.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    let player_entity = spawn_player(app.world_mut(), remote);

    let ship_entity = app
        .world_mut()
        .spawn((
            Ship {
                name: "Doomed-Flagship".to_string(),
                design_id: "explorer_mk1".to_string(),
                hull_id: "corvette".to_string(),
                modules: Vec::new(),
                owner: Owner::Neutral,
                sublight_speed: 0.75,
                ftl_range: 10.0,
                ruler_aboard: true,
                home_port: capital,
                design_revision: 0,
                fleet: None,
            },
            ShipState::InSystem { system: remote },
            Position::from([10.0, 0.0, 0.0]),
            ShipHitpoints {
                hull: 0.01,
                hull_max: 50.0,
                armor: 0.0,
                armor_max: 0.0,
                shield: 0.0,
                shield_max: 0.0,
                shield_regen: 0.0,
            },
            ShipModifiers::default(),
            CommandQueue::default(),
            Cargo::default(),
        ))
        .id();

    app.world_mut()
        .entity_mut(player_entity)
        .insert(AboardShip { ship: ship_entity });

    let _ = common::spawn_raw_hostile(
        app.world_mut(),
        remote,
        1000.0,
        1000.0,
        100.0,
        0.0,
        "space_creature",
    );

    advance_time(&mut app, 1);

    // Messages are delivered next frame, run one more update
    app.update();

    // Check EventLog for PlayerRespawn
    let event_log = app.world().resource::<EventLog>();
    let respawn_event = event_log
        .entries
        .iter()
        .find(|e| e.kind == GameEventKind::PlayerRespawn);
    assert!(
        respawn_event.is_some(),
        "PlayerRespawn event should be fired when player's ship is destroyed"
    );
    assert!(
        respawn_event
            .unwrap()
            .description
            .contains("Flagship destroyed"),
        "Respawn event should contain appropriate description"
    );
}
