mod common;

use bevy::prelude::*;
use macrocosmo::colony::*;
use macrocosmo::components::Position;
use macrocosmo::galaxy::{Habitability, HostilePresence, HostileType, ResourceLevel, Sovereignty, StarSystem, SystemAttributes};
use macrocosmo::knowledge::*;
use macrocosmo::physics::sublight_travel_hexadies;
use macrocosmo::player::*;
use macrocosmo::ship::*;
use macrocosmo::technology;
use macrocosmo::time_system::{GameClock, HEXADIES_PER_YEAR};

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
    let travel_time = sublight_travel_hexadies(1.0, 0.75);
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
            home_port: Entity::PLACEHOLDER,
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
            home_port: Entity::PLACEHOLDER,
        },
        ShipState::Surveying {
            target_system: sys_b,
            started_at: 0,
            completes_at: SURVEY_DURATION_HEXADIES,
        },
        Position::from([0.0, 0.0, 0.0]),
    )).id();

    // Advance by survey duration
    advance_time(&mut app, SURVEY_DURATION_HEXADIES);

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
            home_port: Entity::PLACEHOLDER,
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
            food: 0.0,
            authority: 0.0,
        },
        Production {
            minerals_per_hexadies: 5.0,
            energy_per_hexadies: 3.0,
            research_per_hexadies: 1.0,
            food_per_hexadies: 0.0,
        },
        BuildQueue {
            queue: Vec::new(),
        },
    ));

    // Advance 10 hexadies
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
    // Research is no longer accumulated in the stockpile; it is emitted
    // as PendingResearch entities via emit_research instead.
    assert!(
        stockpile.research.abs() < 1e-6,
        "Expected 0 research in stockpile (emitted as PendingResearch), got {}",
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
            food: 0.0,
            authority: 0.0,
        },
        Production {
            minerals_per_hexadies: 0.0,
            energy_per_hexadies: 0.0,
            research_per_hexadies: 0.0,
            food_per_hexadies: 0.0,
        },
        BuildQueue {
            queue: vec![BuildOrder {
                ship_type_name: "Explorer".to_string(),
                minerals_cost: 50.0,
                minerals_invested: 0.0,
                energy_cost: 30.0,
                energy_invested: 0.0,
                build_time_total: 60,
                build_time_remaining: 0, // set to 0 so it completes with resources
            }],
        },
        Buildings {
            slots: vec![Some(BuildingType::Shipyard), None, None, None],
        },
    ));

    // Count ships before
    let mut ship_query = app.world_mut().query::<&Ship>();
    let ships_before = ship_query.iter(app.world()).count();

    // Advance 1 hexadies (enough resources to complete the order in one tick)
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

// =========================================================================
// Query conflict detection (B0001)
// =========================================================================

/// Runs ALL game systems together (ship, colony, knowledge, communication,
/// technology, events, time, player, and visualization) and verifies that no
/// Bevy Query conflicts (B0001) cause a panic.
///
/// If a Query conflict exists, Bevy panics during schedule initialization or
/// the first update. This test catches that by simply running several frames
/// with a realistic world state.
#[test]
fn all_systems_no_query_conflict() {
    let mut app = common::full_test_app();

    let world = app.world_mut();

    // Capital star system with all components
    let capital = world
        .spawn((
            StarSystem {
                name: "Capital".into(),
                surveyed: true,
                colonized: true,
                is_capital: true,
            },
            Position::from([0.0, 0.0, 0.0]),
            SystemAttributes {
                habitability: Habitability::Ideal,
                mineral_richness: ResourceLevel::Moderate,
                energy_potential: ResourceLevel::Moderate,
                research_potential: ResourceLevel::Moderate,
                max_building_slots: 6,
            },
            Sovereignty {
                owner: Some(Owner::Player),
                control_score: 100.0,
            },
        ))
        .id();

    // Second star system (unsurveyed target)
    let _target = world
        .spawn((
            StarSystem {
                name: "Target".into(),
                surveyed: false,
                colonized: false,
                is_capital: false,
            },
            Position::from([5.0, 0.0, 0.0]),
            SystemAttributes {
                habitability: Habitability::Adequate,
                mineral_richness: ResourceLevel::Rich,
                energy_potential: ResourceLevel::Poor,
                research_potential: ResourceLevel::Moderate,
                max_building_slots: 4,
            },
            Sovereignty::default(),
        ))
        .id();

    // Third star system (surveyed, not colonized)
    let _surveyed = world
        .spawn((
            StarSystem {
                name: "Surveyed".into(),
                surveyed: true,
                colonized: false,
                is_capital: false,
            },
            Position::from([10.0, 3.0, 0.0]),
            SystemAttributes {
                habitability: Habitability::Marginal,
                mineral_richness: ResourceLevel::Poor,
                energy_potential: ResourceLevel::Rich,
                research_potential: ResourceLevel::None,
                max_building_slots: 3,
            },
            Sovereignty::default(),
        ))
        .id();

    // Player stationed at capital
    world.spawn((Player, StationedAt { system: capital }));

    // Colony at capital
    world.spawn((
        Colony {
            system: capital,
            population: 100.0,
            growth_rate: 0.01,
        },
        ResourceStockpile {
            minerals: 500.0,
            energy: 500.0,
            research: 0.0,
            food: 0.0,
            authority: 0.0,
        },
        Production {
            minerals_per_hexadies: 5.0,
            energy_per_hexadies: 5.0,
            research_per_hexadies: 1.0,
            food_per_hexadies: 0.0,
        },
        BuildQueue {
            queue: vec![],
        },
        Buildings {
            slots: vec![
                Some(BuildingType::Mine),
                Some(BuildingType::Shipyard),
                None,
                None,
                None,
                None,
            ],
        },
        BuildingQueue::default(),
        ProductionFocus::default(),
    ));

    // Explorer docked at capital
    world.spawn((
        Ship {
            name: "Explorer-1".into(),
            ship_type: ShipType::Explorer,
            owner: Owner::Player,
            sublight_speed: 0.75,
            ftl_range: 0.0,
            hp: 50.0,
            max_hp: 50.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
        },
        ShipState::Docked { system: capital },
        Position::from([0.0, 0.0, 0.0]),
        CommandQueue::default(),
        Cargo::default(),
    ));

    // Colony ship docked at capital
    world.spawn((
        Ship {
            name: "Colony Ship-1".into(),
            ship_type: ShipType::ColonyShip,
            owner: Owner::Player,
            sublight_speed: 0.5,
            ftl_range: 30.0,
            hp: 100.0,
            max_hp: 100.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
        },
        ShipState::Docked { system: capital },
        Position::from([0.0, 0.0, 0.0]),
        CommandQueue::default(),
        Cargo::default(),
    ));

    // Courier docked at capital
    world.spawn((
        Ship {
            name: "Courier-1".into(),
            ship_type: ShipType::Courier,
            owner: Owner::Player,
            sublight_speed: 0.85,
            ftl_range: 0.0,
            hp: 20.0,
            max_hp: 20.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
        },
        ShipState::Docked { system: capital },
        Position::from([0.0, 0.0, 0.0]),
        CommandQueue::default(),
        Cargo::default(),
    ));

    // Run several frames. If any Query conflicts exist, Bevy will panic here.
    for _ in 0..5 {
        app.update();
    }

    // Advance game clock and run again to exercise systems that check time deltas
    app.world_mut().resource_mut::<GameClock>().elapsed += 10;
    for _ in 0..3 {
        app.update();
    }
}

// =========================================================================
// Combat resolution (#55)
// =========================================================================

#[test]
fn test_hostile_destroyed_when_hp_zero() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Battle-System",
        [0.0, 0.0, 0.0],
        Habitability::Adequate,
        true,
        false,
    );

    // Spawn a hostile with low HP so it gets destroyed quickly
    let hostile_entity = app.world_mut().spawn(HostilePresence {
        system: sys,
        strength: 0.0,  // no attack
        hp: 0.05,       // very low HP
        max_hp: 10.0,
        hostile_type: HostileType::SpaceCreature,
    }).id();

    // Spawn a strong explorer docked at that system
    app.world_mut().spawn((
        Ship {
            name: "Warship-1".to_string(),
            ship_type: ShipType::Explorer,
            owner: Owner::Player,
            sublight_speed: 0.75,
            ftl_range: 0.0,
            hp: 50.0,
            max_hp: 50.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
        },
        ShipState::Docked { system: sys },
        Position::from([0.0, 0.0, 0.0]),
        CombatStats { attack: 5.0, defense: 2.0 },
        CommandQueue::default(),
        Cargo::default(),
    ));

    // Run one tick of combat
    advance_time(&mut app, 1);

    // Hostile should be destroyed (despawned)
    assert!(
        app.world().get_entity(hostile_entity).is_err(),
        "Hostile entity should be despawned after HP reaches 0"
    );
}

#[test]
fn test_ship_destroyed_when_hp_zero_in_combat() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Danger-System",
        [0.0, 0.0, 0.0],
        Habitability::Adequate,
        true,
        false,
    );

    // Spawn a powerful hostile
    app.world_mut().spawn(HostilePresence {
        system: sys,
        strength: 100.0,  // very strong attack
        hp: 1000.0,
        max_hp: 1000.0,
        hostile_type: HostileType::AncientDefense,
    });

    // Spawn a very weak ship with 1 HP
    let ship_entity = app.world_mut().spawn((
        Ship {
            name: "Doomed-1".to_string(),
            ship_type: ShipType::Courier,
            owner: Owner::Player,
            sublight_speed: 0.85,
            ftl_range: 0.0,
            hp: 0.01,  // nearly dead
            max_hp: 20.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
        },
        ShipState::Docked { system: sys },
        Position::from([0.0, 0.0, 0.0]),
        CombatStats { attack: 0.0, defense: 0.0 },
        CommandQueue::default(),
        Cargo::default(),
    )).id();

    // Run one tick of combat
    advance_time(&mut app, 1);

    // Ship should be destroyed
    assert!(
        app.world().get_entity(ship_entity).is_err(),
        "Ship should be despawned after HP reaches 0 in combat"
    );
}

#[test]
fn test_no_combat_when_no_ships_present() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Empty-System",
        [0.0, 0.0, 0.0],
        Habitability::Adequate,
        true,
        false,
    );

    // Spawn hostile - should not be affected without ships present
    let hostile_entity = app.world_mut().spawn(HostilePresence {
        system: sys,
        strength: 5.0,
        hp: 10.0,
        max_hp: 10.0,
        hostile_type: HostileType::SpaceCreature,
    }).id();

    advance_time(&mut app, 1);

    // Hostile should still exist with full HP
    let hostile = app.world().get::<HostilePresence>(hostile_entity).unwrap();
    assert!((hostile.hp - 10.0).abs() < f64::EPSILON, "Hostile HP should be unchanged");
}

#[test]
fn test_combat_takes_multiple_ticks() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Prolonged-Battle",
        [0.0, 0.0, 0.0],
        Habitability::Adequate,
        true,
        false,
    );

    // Hostile with significant HP
    let hostile_entity = app.world_mut().spawn(HostilePresence {
        system: sys,
        strength: 1.0,
        hp: 10.0,
        max_hp: 10.0,
        hostile_type: HostileType::SpaceCreature,
    }).id();

    // Ship with moderate attack
    app.world_mut().spawn((
        Ship {
            name: "Fighter-1".to_string(),
            ship_type: ShipType::Explorer,
            owner: Owner::Player,
            sublight_speed: 0.75,
            ftl_range: 0.0,
            hp: 50.0,
            max_hp: 50.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
        },
        ShipState::Docked { system: sys },
        Position::from([0.0, 0.0, 0.0]),
        CombatStats { attack: 2.0, defense: 5.0 },
        CommandQueue::default(),
        Cargo::default(),
    ));

    // After 1 tick, hostile should still be alive but damaged
    advance_time(&mut app, 1);

    // Hostile damage = max(2.0 - 1.0*0.5, 0) * 0.1 = 0.15
    let hostile = app.world().get::<HostilePresence>(hostile_entity).unwrap();
    assert!(hostile.hp < 10.0, "Hostile should have taken some damage");
    assert!(hostile.hp > 0.0, "Hostile should still be alive after one tick");
}

// =========================================================================
// Authority production and consumption (#73)
// =========================================================================

/// Helper: spawn a star system marked as capital
fn spawn_capital_system(world: &mut World, name: &str, pos: [f64; 3]) -> Entity {
    world
        .spawn((
            StarSystem {
                name: name.to_string(),
                surveyed: true,
                colonized: true,
                is_capital: true,
            },
            Position::from(pos),
            SystemAttributes {
                habitability: Habitability::Ideal,
                mineral_richness: ResourceLevel::Moderate,
                energy_potential: ResourceLevel::Moderate,
                research_potential: ResourceLevel::Moderate,
                max_building_slots: 4,
            },
            Sovereignty::default(),
        ))
        .id()
}

#[test]
fn test_capital_produces_authority() {
    let mut app = test_app();

    let cap_sys = spawn_capital_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0]);

    // Spawn capital colony with zero authority
    let colony_entity = app.world_mut().spawn((
        Colony {
            system: cap_sys,
            population: 100.0,
            growth_rate: 0.01,
        },
        ResourceStockpile {
            minerals: 500.0,
            energy: 500.0,
            research: 0.0,
            food: 0.0,
            authority: 0.0,
        },
        Production {
            minerals_per_hexadies: 5.0,
            energy_per_hexadies: 5.0,
            research_per_hexadies: 1.0,
            food_per_hexadies: 0.0,
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![None; 4] },
        BuildingQueue::default(),
        ProductionFocus::default(),
    )).id();

    // Advance 10 hexadies
    advance_time(&mut app, 10);

    let stockpile = app.world().get::<ResourceStockpile>(colony_entity).unwrap();
    // Capital produces BASE_AUTHORITY_PER_HEXADIES (1.0) per hexady, no colonies to drain it
    // Expected: 1.0 * 10 = 10.0
    assert!(
        (stockpile.authority - 10.0).abs() < 1e-6,
        "Expected 10.0 authority, got {}",
        stockpile.authority
    );
}

#[test]
fn test_empire_scale_authority_cost() {
    let mut app = test_app();

    let cap_sys = spawn_capital_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0]);
    let remote_sys = spawn_test_system(
        app.world_mut(),
        "Remote",
        [5.0, 0.0, 0.0],
        Habitability::Adequate,
        true,
        true,
    );

    // Capital colony starts with some authority
    let capital_colony = app.world_mut().spawn((
        Colony {
            system: cap_sys,
            population: 100.0,
            growth_rate: 0.01,
        },
        ResourceStockpile {
            minerals: 500.0,
            energy: 500.0,
            research: 0.0,
            food: 0.0,
            authority: 5.0, // start with 5
        },
        Production {
            minerals_per_hexadies: 5.0,
            energy_per_hexadies: 5.0,
            research_per_hexadies: 1.0,
            food_per_hexadies: 0.0,
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![None; 4] },
        BuildingQueue::default(),
        ProductionFocus::default(),
    )).id();

    // Remote colony (non-capital)
    app.world_mut().spawn((
        Colony {
            system: remote_sys,
            population: 50.0,
            growth_rate: 0.005,
        },
        ResourceStockpile {
            minerals: 100.0,
            energy: 100.0,
            research: 0.0,
            food: 0.0,
            authority: 0.0,
        },
        Production {
            minerals_per_hexadies: 3.0,
            energy_per_hexadies: 3.0,
            research_per_hexadies: 0.5,
            food_per_hexadies: 0.0,
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![None; 4] },
        BuildingQueue::default(),
        ProductionFocus::default(),
    ));

    // Advance 10 hexadies
    advance_time(&mut app, 10);

    let stockpile = app.world().get::<ResourceStockpile>(capital_colony).unwrap();
    // Production: 1.0 * 10 = 10.0
    // Starting: 5.0
    // Cost: 0.5 * 1 colony * 10 = 5.0
    // Expected: 5.0 + 10.0 - 5.0 = 10.0
    assert!(
        (stockpile.authority - 10.0).abs() < 1e-6,
        "Expected 10.0 authority, got {}",
        stockpile.authority
    );
}

#[test]
fn test_authority_deficit_reduces_non_capital_production() {
    let mut app = test_app();

    let cap_sys = spawn_capital_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0]);
    let remote_sys = spawn_test_system(
        app.world_mut(),
        "Remote",
        [5.0, 0.0, 0.0],
        Habitability::Adequate,
        true,
        true,
    );

    // Capital colony with zero authority -- will be in deficit
    // Note: tick_authority runs before tick_production in the chain.
    // With 3 non-capital colonies and 1.0 production per hexady,
    // authority will be produced then immediately consumed.
    // To guarantee deficit, we use 3 remote colonies so cost > production.
    let remote_sys2 = spawn_test_system(
        app.world_mut(),
        "Remote2",
        [10.0, 0.0, 0.0],
        Habitability::Adequate,
        true,
        true,
    );
    let remote_sys3 = spawn_test_system(
        app.world_mut(),
        "Remote3",
        [15.0, 0.0, 0.0],
        Habitability::Adequate,
        true,
        true,
    );

    // Capital colony: authority = 0, so after tick_authority it stays 0
    // because cost (3 * 0.5 = 1.5) > production (1.0), net = -0.5, capped to 0
    app.world_mut().spawn((
        Colony {
            system: cap_sys,
            population: 100.0,
            growth_rate: 0.01,
        },
        ResourceStockpile {
            minerals: 500.0,
            energy: 500.0,
            research: 0.0,
            food: 0.0,
            authority: 0.0,
        },
        Production {
            minerals_per_hexadies: 5.0,
            energy_per_hexadies: 5.0,
            research_per_hexadies: 1.0,
            food_per_hexadies: 0.0,
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![None; 4] },
        BuildingQueue::default(),
        ProductionFocus::default(),
    ));

    // Three remote colonies with known production rates
    let remote_colony = app.world_mut().spawn((
        Colony {
            system: remote_sys,
            population: 50.0,
            growth_rate: 0.005,
        },
        ResourceStockpile {
            minerals: 0.0,
            energy: 0.0,
            research: 0.0,
            food: 0.0,
            authority: 0.0,
        },
        Production {
            minerals_per_hexadies: 10.0,
            energy_per_hexadies: 10.0,
            research_per_hexadies: 0.0,
            food_per_hexadies: 0.0,
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![None; 4] },
        BuildingQueue::default(),
        ProductionFocus::default(),
    )).id();

    app.world_mut().spawn((
        Colony {
            system: remote_sys2,
            population: 50.0,
            growth_rate: 0.005,
        },
        ResourceStockpile {
            minerals: 0.0,
            energy: 0.0,
            research: 0.0,
            food: 0.0,
            authority: 0.0,
        },
        Production {
            minerals_per_hexadies: 1.0,
            energy_per_hexadies: 1.0,
            research_per_hexadies: 0.0,
            food_per_hexadies: 0.0,
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![None; 4] },
        BuildingQueue::default(),
        ProductionFocus::default(),
    ));

    app.world_mut().spawn((
        Colony {
            system: remote_sys3,
            population: 50.0,
            growth_rate: 0.005,
        },
        ResourceStockpile {
            minerals: 0.0,
            energy: 0.0,
            research: 0.0,
            food: 0.0,
            authority: 0.0,
        },
        Production {
            minerals_per_hexadies: 1.0,
            energy_per_hexadies: 1.0,
            research_per_hexadies: 0.0,
            food_per_hexadies: 0.0,
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![None; 4] },
        BuildingQueue::default(),
        ProductionFocus::default(),
    ));

    // Advance 10 hexadies
    advance_time(&mut app, 10);

    let stockpile = app.world().get::<ResourceStockpile>(remote_colony).unwrap();
    // With authority deficit, production is multiplied by AUTHORITY_DEFICIT_PENALTY (0.5)
    // Normal: 10.0 * 10 = 100.0
    // With penalty: 10.0 * 10 * 0.5 = 50.0
    assert!(
        (stockpile.minerals - 50.0).abs() < 1e-6,
        "Expected 50.0 minerals (penalized), got {}",
        stockpile.minerals
    );
    assert!(
        (stockpile.energy - 50.0).abs() < 1e-6,
        "Expected 50.0 energy (penalized), got {}",
        stockpile.energy
    );
}
