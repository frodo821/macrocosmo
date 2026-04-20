//! #386: Integration tests for SystemBuilding → Ship migration.
//!
//! Verifies that:
//! - System building construction spawns a station Ship entity
//! - System building demolition despawns the station Ship entity
//! - `sync_system_buildings_from_ships` derives SystemBuildings from station ships
//! - Station ships carry the correct FactionOwner

mod common;

use macrocosmo::amount::Amt;
use macrocosmo::colony::{
    BuildingOrder, DEFAULT_SYSTEM_BUILDING_SLOTS, DemolitionOrder, ResourceStockpile,
    SystemBuildingQueue, SystemBuildings,
};
use macrocosmo::components::Position;
use macrocosmo::faction::FactionOwner;
use macrocosmo::galaxy::StarSystem;
use macrocosmo::scripting::building_api::BuildingId;
use macrocosmo::ship::{Ship, ShipState};

use common::{advance_time, test_app};

/// Helper: spawn a star system with SystemBuildings, SystemBuildingQueue,
/// ResourceStockpile, Position, and optionally a FactionOwner.
fn spawn_test_system(
    world: &mut bevy::prelude::World,
    owner: Option<bevy::prelude::Entity>,
) -> bevy::prelude::Entity {
    let mut entity = world.spawn((
        StarSystem {
            name: "Test System".into(),
            star_type: "yellow".into(),
            surveyed: true,
            is_capital: false,
        },
        Position::from([0.0, 0.0, 0.0]),
        SystemBuildings {
            slots: vec![None; DEFAULT_SYSTEM_BUILDING_SLOTS],
        },
        SystemBuildingQueue::default(),
        ResourceStockpile {
            minerals: Amt::units(10000),
            energy: Amt::units(10000),
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        },
        macrocosmo::colony::ResourceCapacity::default(),
    ));
    if let Some(empire) = owner {
        entity.insert(FactionOwner(empire));
    }
    entity.id()
}

/// Count station ships with the given design_id at the given system.
fn count_station_ships(
    world: &mut bevy::prelude::World,
    design_id: &str,
    system: bevy::prelude::Entity,
) -> usize {
    let mut q = world.query::<(&Ship, &ShipState)>();
    q.iter(world)
        .filter(|(ship, state)| {
            ship.design_id == design_id
                && matches!(state, ShipState::InSystem { system: s } if *s == system)
        })
        .count()
}

#[test]
fn test_system_building_completion_spawns_station_ship() {
    let mut app = test_app();

    let empire = {
        let mut q = app
            .world_mut()
            .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<macrocosmo::player::PlayerEmpire>>();
        q.iter(app.world()).next().unwrap()
    };

    let system = spawn_test_system(app.world_mut(), Some(empire));

    // Queue a shipyard build order with zero cost and zero build time so it
    // completes on the next tick.
    {
        let mut sbq = app
            .world_mut()
            .get_mut::<SystemBuildingQueue>(system)
            .unwrap();
        sbq.push_build_order(BuildingOrder {
            order_id: 0,
            building_id: BuildingId::new("shipyard"),
            target_slot: 0,
            minerals_remaining: Amt::ZERO,
            energy_remaining: Amt::ZERO,
            build_time_remaining: 0,
        });
    }

    // Advance one tick
    advance_time(&mut app, 1);

    // Verify a station ship was spawned
    let ship_count = count_station_ships(app.world_mut(), "station_shipyard_v1", system);
    assert_eq!(
        ship_count, 1,
        "Expected one station_shipyard_v1 Ship entity after building completion"
    );
}

#[test]
fn test_system_building_demolish_despawns_station_ship() {
    let mut app = test_app();

    let empire = {
        let mut q = app
            .world_mut()
            .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<macrocosmo::player::PlayerEmpire>>();
        q.iter(app.world()).next().unwrap()
    };

    let system = spawn_test_system(app.world_mut(), Some(empire));

    // Build a shipyard (instant completion)
    {
        let mut sbq = app
            .world_mut()
            .get_mut::<SystemBuildingQueue>(system)
            .unwrap();
        sbq.push_build_order(BuildingOrder {
            order_id: 0,
            building_id: BuildingId::new("shipyard"),
            target_slot: 0,
            minerals_remaining: Amt::ZERO,
            energy_remaining: Amt::ZERO,
            build_time_remaining: 0,
        });
    }
    advance_time(&mut app, 1);

    // Verify the ship exists
    let ship_count = count_station_ships(app.world_mut(), "station_shipyard_v1", system);
    assert!(ship_count > 0, "Station ship should exist after build");

    // Queue demolition (instant)
    {
        let mut sbq = app
            .world_mut()
            .get_mut::<SystemBuildingQueue>(system)
            .unwrap();
        sbq.push_demolition_order(DemolitionOrder {
            order_id: 0,
            target_slot: 0,
            building_id: BuildingId::new("shipyard"),
            time_remaining: 0,
            minerals_refund: Amt::ZERO,
            energy_refund: Amt::ZERO,
        });
    }
    advance_time(&mut app, 1);

    // Verify the ship is gone
    let ship_count = count_station_ships(app.world_mut(), "station_shipyard_v1", system);
    assert_eq!(
        ship_count, 0,
        "Station ship should be despawned after demolition"
    );
}

#[test]
fn test_sync_system_buildings_reflects_station_ships() {
    let mut app = test_app();

    let system = spawn_test_system(app.world_mut(), None);

    // Manually spawn a station ship at the system (simulating external spawn)
    common::spawn_test_ship(
        app.world_mut(),
        "Test Shipyard",
        "station_shipyard_v1",
        system,
        [0.0, 0.0, 0.0],
    );

    // Run the app to trigger sync_system_buildings_from_ships
    advance_time(&mut app, 1);

    // Check that SystemBuildings now reflects the station ship
    let sys_buildings = app.world().get::<SystemBuildings>(system).unwrap();
    let has_shipyard = sys_buildings
        .slots
        .iter()
        .any(|s| s.as_ref().is_some_and(|b| b.0 == "shipyard"));
    assert!(
        has_shipyard,
        "SystemBuildings should contain 'shipyard' after sync from station ship"
    );
}

#[test]
fn test_station_ship_has_correct_owner() {
    let mut app = test_app();

    let empire = {
        let mut q = app
            .world_mut()
            .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<macrocosmo::player::PlayerEmpire>>();
        q.iter(app.world()).next().unwrap()
    };

    let system = spawn_test_system(app.world_mut(), Some(empire));

    // Queue a shipyard build (instant)
    {
        let mut sbq = app
            .world_mut()
            .get_mut::<SystemBuildingQueue>(system)
            .unwrap();
        sbq.push_build_order(BuildingOrder {
            order_id: 0,
            building_id: BuildingId::new("shipyard"),
            target_slot: 0,
            minerals_remaining: Amt::ZERO,
            energy_remaining: Amt::ZERO,
            build_time_remaining: 0,
        });
    }
    advance_time(&mut app, 1);

    // Find the station ship entity
    let ship_entity = {
        let mut q = app.world_mut().query::<(bevy::prelude::Entity, &Ship)>();
        q.iter(app.world())
            .find(|(_, ship)| ship.design_id == "station_shipyard_v1")
            .map(|(e, _)| e)
            .expect("Station ship should exist")
    };

    let faction_owner = app
        .world()
        .get::<FactionOwner>(ship_entity)
        .expect("Station ship should have FactionOwner");
    assert_eq!(
        faction_owner.0, empire,
        "Station ship FactionOwner should match the system's owner empire"
    );
}

/// #413: Regression test — completing a Port build must not destroy an
/// existing Shipyard in the same system. The bug was that
/// `tick_system_building_queue` blindly wrote the completed building into
/// `target_slot` (slot 0), overwriting the Shipyard that had been placed
/// there by `sync_system_buildings_from_ships`. The fix makes completion
/// fall back to the first empty slot when the target is already occupied.
#[test]
fn test_port_completion_does_not_destroy_shipyard() {
    let mut app = test_app();

    let empire = {
        let mut q = app
            .world_mut()
            .query_filtered::<bevy::prelude::Entity, bevy::prelude::With<macrocosmo::player::PlayerEmpire>>();
        q.iter(app.world()).next().unwrap()
    };

    let system = spawn_test_system(app.world_mut(), Some(empire));

    // Step 1: Build a Shipyard (instant completion) — fills slot 0.
    {
        let mut sbq = app
            .world_mut()
            .get_mut::<SystemBuildingQueue>(system)
            .unwrap();
        sbq.push_build_order(BuildingOrder {
            order_id: 0,
            building_id: BuildingId::new("shipyard"),
            target_slot: 0,
            minerals_remaining: Amt::ZERO,
            energy_remaining: Amt::ZERO,
            build_time_remaining: 0,
        });
    }
    advance_time(&mut app, 1);

    // Verify Shipyard is in slot 0.
    {
        let sb = app.world().get::<SystemBuildings>(system).unwrap();
        assert_eq!(
            sb.slots[0].as_ref().map(|b| b.0.as_str()),
            Some("shipyard"),
            "Shipyard should occupy slot 0 after build"
        );
    }

    // Step 2: Queue a Port build that also targets slot 0 (simulating the
    // race where the slot was empty at queue-time but got filled before
    // completion).
    {
        let mut sbq = app
            .world_mut()
            .get_mut::<SystemBuildingQueue>(system)
            .unwrap();
        sbq.push_build_order(BuildingOrder {
            order_id: 0,
            building_id: BuildingId::new("port"),
            target_slot: 0, // same slot as Shipyard — this is the bug trigger
            minerals_remaining: Amt::ZERO,
            energy_remaining: Amt::ZERO,
            build_time_remaining: 0,
        });
    }
    advance_time(&mut app, 1);

    // Step 3: Both Shipyard AND Port must coexist.
    let sb = app.world().get::<SystemBuildings>(system).unwrap();
    let has_shipyard = sb.slots.iter().any(|s| s.as_ref().is_some_and(|b| b.0 == "shipyard"));
    let has_port = sb.slots.iter().any(|s| s.as_ref().is_some_and(|b| b.0 == "port"));
    assert!(
        has_shipyard,
        "Shipyard must still exist after Port completion (slots: {:?})",
        sb.slots
    );
    assert!(
        has_port,
        "Port must exist after completion (slots: {:?})",
        sb.slots
    );

    // Verify both station ships exist.
    let shipyard_count = count_station_ships(app.world_mut(), "station_shipyard_v1", system);
    let port_count = count_station_ships(app.world_mut(), "station_port_v1", system);
    assert_eq!(
        shipyard_count, 1,
        "Shipyard station ship must survive Port construction"
    );
    assert_eq!(
        port_count, 1,
        "Port station ship must be spawned on completion"
    );
}
