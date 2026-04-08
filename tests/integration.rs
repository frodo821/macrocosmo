mod common;

use bevy::prelude::*;
use macrocosmo::colony::*;
use macrocosmo::components::Position;
use macrocosmo::galaxy::{Habitability, StarSystem};
use macrocosmo::knowledge::*;
use macrocosmo::physics::sublight_travel_sexadies;
use macrocosmo::player::*;
use macrocosmo::ship::*;

use common::{advance_time, spawn_test_system, test_app};

// =========================================================================
// Exploration flow
// =========================================================================

#[test]
fn test_sublight_travel_and_arrival() {
    let mut app = test_app();

    // System-A at origin, System-B at 1 LY along x-axis
    let _sys_a = spawn_test_system(
        app.world_mut(),
        "System-A",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        false,
    );
    let sys_b = spawn_test_system(
        app.world_mut(),
        "System-B",
        [1.0, 0.0, 0.0],
        Habitability::Adequate,
        false,
        false,
    );

    // Explorer speed is 0.75c. Travel time for 1 LY = ceil(1.0 / (1/60 * 0.75)) = 80 sd
    let travel_time = sublight_travel_sexadies(1.0, 0.75);
    assert_eq!(travel_time, 80);

    // Spawn explorer docked at System-A
    let ship_entity = app.world_mut().spawn((
        Ship {
            name: "Scout-1".to_string(),
            ship_type: ShipType::Explorer,
            owner: Owner::Player,
            sublight_speed: 0.75,
            ftl_range: 0.0,
            hp: 50.0,
            max_hp: 50.0,
            player_aboard: false,
        },
        ShipState::SubLight {
            origin: [0.0, 0.0, 0.0],
            destination: [1.0, 0.0, 0.0],
            target_system: Some(sys_b),
            departed_at: 0,
            arrival_at: travel_time,
        },
        Position::from([0.0, 0.0, 0.0]),
    )).id();

    // Advance exactly to arrival time
    advance_time(&mut app, travel_time);

    // Ship should now be docked at System-B
    let state = app.world().get::<ShipState>(ship_entity).unwrap();
    match state {
        ShipState::Docked { system } => {
            assert_eq!(*system, sys_b, "Ship should be docked at System-B");
        }
        _ => panic!("Expected ship to be Docked, got {:?}", std::mem::discriminant(state)),
    }

    // Position should match System-B
    let pos = app.world().get::<Position>(ship_entity).unwrap();
    assert!((pos.x - 1.0).abs() < 1e-9, "Ship x should be 1.0, got {}", pos.x);
    assert!((pos.y).abs() < 1e-9, "Ship y should be 0.0, got {}", pos.y);
}

#[test]
fn test_survey_completes_and_marks_system() {
    let mut app = test_app();

    // System-A at origin (where explorer is docked), System-B within 5 LY survey range
    let _sys_a = spawn_test_system(
        app.world_mut(),
        "System-A",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        false,
    );
    let sys_b = spawn_test_system(
        app.world_mut(),
        "System-B",
        [3.0, 0.0, 0.0],
        Habitability::Adequate,
        false, // not yet surveyed
        false,
    );

    // Spawn explorer in Surveying state targeting System-B
    let ship_entity = app.world_mut().spawn((
        Ship {
            name: "Scout-1".to_string(),
            ship_type: ShipType::Explorer,
            owner: Owner::Player,
            sublight_speed: 0.75,
            ftl_range: 0.0,
            hp: 50.0,
            max_hp: 50.0,
            player_aboard: false,
        },
        ShipState::Surveying {
            target_system: sys_b,
            started_at: 0,
            completes_at: SURVEY_DURATION_SEXADIES, // 5 sd
        },
        Position::from([0.0, 0.0, 0.0]),
    )).id();

    // Advance by survey duration (5 sd)
    advance_time(&mut app, SURVEY_DURATION_SEXADIES);

    // System-B should now be surveyed
    let star = app.world().get::<StarSystem>(sys_b).unwrap();
    assert!(star.surveyed, "System-B should be marked as surveyed");

    // Ship should be back to Docked at the target system
    let state = app.world().get::<ShipState>(ship_entity).unwrap();
    match state {
        ShipState::Docked { system } => {
            assert_eq!(*system, sys_b, "Ship should be docked at survey target");
        }
        _ => panic!("Expected ship to be Docked after survey"),
    }
}

// =========================================================================
// Colonization flow
// =========================================================================

#[test]
fn test_ftl_travel_and_arrival() {
    let mut app = test_app();

    let sys_a = spawn_test_system(
        app.world_mut(),
        "System-A",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );
    let sys_b = spawn_test_system(
        app.world_mut(),
        "System-B",
        [20.0, 0.0, 0.0],
        Habitability::Adequate,
        true,
        true, // already colonized so colony ship isn't consumed on arrival
    );

    // FTL arrival at 120 sd
    let arrival_at: i64 = 120;
    let ship_entity = app.world_mut().spawn((
        Ship {
            name: "Colony-1".to_string(),
            ship_type: ShipType::ColonyShip,
            owner: Owner::Player,
            sublight_speed: 0.5,
            ftl_range: 30.0,
            hp: 100.0,
            max_hp: 100.0,
            player_aboard: false,
        },
        ShipState::InFTL {
            origin_system: sys_a,
            destination_system: sys_b,
            departed_at: 0,
            arrival_at,
        },
        Position::from([0.0, 0.0, 0.0]),
    )).id();

    // Advance to arrival
    advance_time(&mut app, arrival_at);

    // Ship should be docked at System-B
    let state = app.world().get::<ShipState>(ship_entity).unwrap();
    match state {
        ShipState::Docked { system } => {
            assert_eq!(*system, sys_b, "Ship should be docked at System-B after FTL");
        }
        _ => panic!("Expected ship to be Docked after FTL travel"),
    }

    // Position should match System-B (20, 0, 0)
    let pos = app.world().get::<Position>(ship_entity).unwrap();
    assert!((pos.x - 20.0).abs() < 1e-9, "Ship x should be 20.0, got {}", pos.x);
}

// NOTE: Colony ship auto-colonization tests removed.
// Colonization is now a manual player command (C key) handled in the visualization layer.

// =========================================================================
// Production
// =========================================================================

#[test]
fn test_production_accumulates_resources() {
    let mut app = test_app();

    // Need a system entity for the colony to reference
    let sys = spawn_test_system(
        app.world_mut(),
        "Prod-System",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    // Spawn colony with production rates 5/3/1 and zero stockpile
    app.world_mut().spawn((
        Colony {
            system: sys,
            population: 50.0,
            growth_rate: 0.005,
        },
        ResourceStockpile {
            minerals: 0.0,
            energy: 0.0,
            research: 0.0,
        },
        Production {
            minerals_per_sexadie: 5.0,
            energy_per_sexadie: 3.0,
            research_per_sexadie: 1.0,
        },
        BuildQueue {
            queue: Vec::new(),
        },
    ));

    // Advance 10 sexadies
    advance_time(&mut app, 10);

    let mut stockpile_query = app.world_mut().query::<&ResourceStockpile>();
    let stockpile = stockpile_query.iter(app.world()).next().unwrap();

    assert!(
        (stockpile.minerals - 50.0).abs() < 1e-6,
        "Expected 50 minerals, got {}",
        stockpile.minerals
    );
    assert!(
        (stockpile.energy - 30.0).abs() < 1e-6,
        "Expected 30 energy, got {}",
        stockpile.energy
    );
    assert!(
        (stockpile.research - 10.0).abs() < 1e-6,
        "Expected 10 research, got {}",
        stockpile.research
    );
}

#[test]
fn test_build_queue_spawns_ship() {
    let mut app = test_app();

    // System entity (build queue needs to look up Position on colony.system)
    let sys = spawn_test_system(
        app.world_mut(),
        "Shipyard",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    // Colony with ample resources and a build order for an Explorer
    app.world_mut().spawn((
        Colony {
            system: sys,
            population: 100.0,
            growth_rate: 0.01,
        },
        ResourceStockpile {
            minerals: 1000.0,
            energy: 1000.0,
            research: 0.0,
        },
        Production {
            minerals_per_sexadie: 0.0,
            energy_per_sexadie: 0.0,
            research_per_sexadie: 0.0,
        },
        BuildQueue {
            queue: vec![BuildOrder {
                ship_type_name: "Explorer".to_string(),
                minerals_cost: 50.0,
                minerals_invested: 0.0,
                energy_cost: 30.0,
                energy_invested: 0.0,
            }],
        },
    ));

    // Count ships before
    let mut ship_query = app.world_mut().query::<&Ship>();
    let ships_before = ship_query.iter(app.world()).count();

    // Advance 1 sexadie (enough resources to complete the order in one tick)
    advance_time(&mut app, 1);
    // Need another update to flush deferred spawn commands
    app.update();

    let mut ship_query = app.world_mut().query::<&Ship>();
    let ships: Vec<_> = ship_query.iter(app.world()).collect();
    assert_eq!(
        ships.len(),
        ships_before + 1,
        "One new ship should have been spawned"
    );

    // Verify it's an Explorer
    let new_ship = ships.iter().find(|s| s.ship_type == ShipType::Explorer);
    assert!(new_ship.is_some(), "The spawned ship should be an Explorer");

    // Build queue should be empty now
    let mut bq_query = app.world_mut().query::<&BuildQueue>();
    let bq = bq_query.iter(app.world()).next().unwrap();
    assert!(bq.queue.is_empty(), "Build queue should be empty after completion");
}

// =========================================================================
// Knowledge propagation
// =========================================================================

#[test]
fn test_knowledge_propagation_light_delay() {
    let mut app = test_app();

    // Player at origin
    let sys_capital = spawn_test_system(
        app.world_mut(),
        "Capital",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    // System-B at 10 LY away
    let sys_b = spawn_test_system(
        app.world_mut(),
        "Distant",
        [10.0, 0.0, 0.0],
        Habitability::Adequate,
        true,
        false,
    );

    // Spawn player stationed at capital
    app.world_mut().spawn((Player, StationedAt { system: sys_capital }));

    // At time 0, no knowledge should exist of System-B (light hasn't arrived)
    app.update();
    {
        let store = app.world().resource::<KnowledgeStore>();
        assert!(
            store.get(sys_b).is_none(),
            "Should have no knowledge of distant system at time 0"
        );
    }

    // Light delay for 10 LY = 10 * 60 = 600 sd
    advance_time(&mut app, 600);

    {
        let store = app.world().resource::<KnowledgeStore>();
        let knowledge = store.get(sys_b);
        assert!(
            knowledge.is_some(),
            "Should have knowledge of distant system after light delay"
        );
        let k = knowledge.unwrap();
        assert_eq!(k.data.name, "Distant");
    }
}
