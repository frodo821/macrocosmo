mod common;

use bevy::prelude::*;
use macrocosmo::amount::{Amt, SignedAmt};
use macrocosmo::colony::*;
use macrocosmo::components::Position;
use macrocosmo::event_system::{EventDefinition, EventSystem, EventTrigger};
use macrocosmo::galaxy::{Habitability, HostilePresence, HostileType, Planet, ResourceLevel, Sovereignty, StarSystem, SystemAttributes, SystemModifiers};
use macrocosmo::knowledge::*;
use macrocosmo::modifier::{ModifiedValue, Modifier};
use macrocosmo::physics::{light_delay_hexadies, sublight_travel_hexadies};
use macrocosmo::player::*;
use macrocosmo::ship::*;
use macrocosmo::technology;
use macrocosmo::time_system::{GameClock, HEXADIES_PER_YEAR};

use macrocosmo::events::{EventLog, GameEventKind};

use common::{advance_time, find_planet, full_test_app, spawn_test_colony, spawn_test_system, test_app, test_app_with_event_log};

/// Find the player empire entity in the world.
fn empire_entity(world: &mut World) -> Entity {
    let mut query = world.query_filtered::<Entity, With<PlayerEmpire>>();
    query.single(world).expect("No player empire found in test world")
}

// Exploration flow

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
            owner: Owner::Neutral,
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
            owner: Owner::Neutral,
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

// Colonization flow

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
            owner: Owner::Neutral,
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

// Production

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
    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 50.0,
            growth_rate: 0.005,
        },
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::units(5)),
            energy_per_hexadies: ModifiedValue::new(Amt::units(3)),
            research_per_hexadies: ModifiedValue::new(Amt::units(1)),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
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
        (stockpile.minerals.to_f64() - 50.0).abs() < 1.0,
        "Expected ~50 minerals, got {}",
        stockpile.minerals
    );
    assert!(
        (stockpile.energy.to_f64() - 30.0).abs() < 1.0,
        "Expected ~30 energy, got {}",
        stockpile.energy
    );
    // Research is no longer accumulated in the stockpile; it is emitted
    // as PendingResearch entities via emit_research instead.
    assert_eq!(
        stockpile.research, Amt::ZERO,
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
    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 100.0,
            growth_rate: 0.01,
        },
        ResourceStockpile {
            minerals: Amt::units(1000),
            energy: Amt::units(1000),
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue {
            queue: vec![BuildOrder {
                ship_type_name: "Explorer".to_string(),
                minerals_cost: Amt::units(50),
                minerals_invested: Amt::ZERO,
                energy_cost: Amt::units(30),
                energy_invested: Amt::ZERO,
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

// Knowledge propagation

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
        let empire = empire_entity(app.world_mut());
        let store = app.world().get::<KnowledgeStore>(empire).unwrap();
        assert!(
            store.get(sys_b).is_none(),
            "Should have no knowledge of distant system at time 0"
        );
    }

    // Light delay for 10 LY = 10 * 60 = 600 sd
    advance_time(&mut app, 600);

    {
        let empire = empire_entity(app.world_mut());
        let store = app.world().get::<KnowledgeStore>(empire).unwrap();
        let knowledge = store.get(sys_b);
        assert!(
            knowledge.is_some(),
            "Should have knowledge of distant system after light delay"
        );
        let k = knowledge.unwrap();
        assert_eq!(k.data.name, "Distant");
    }
}

// Query conflict detection (B0001)

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

    // Capital star system with all components
    let capital = app.world_mut()
        .spawn((
            StarSystem {
                name: "Capital".into(),
                surveyed: true,
                is_capital: true,
                star_type: "default".to_string(),
            },
            Position::from([0.0, 0.0, 0.0]),
            Sovereignty {
                owner: None,
                control_score: 100.0,
            },
        ))
        .id();
    let capital_planet = app.world_mut()
        .spawn((
            Planet {
                name: "Capital I".into(),
                system: capital,
                planet_type: "default".to_string(),
            },
            SystemAttributes {
                habitability: Habitability::Ideal,
                mineral_richness: ResourceLevel::Moderate,
                energy_potential: ResourceLevel::Moderate,
                research_potential: ResourceLevel::Moderate,
                max_building_slots: 6,
            },
            Position::from([0.0, 0.0, 0.0]),
        ))
        .id();

    // Second star system (unsurveyed target)
    let _target = app.world_mut()
        .spawn((
            StarSystem {
                name: "Target".into(),
                surveyed: false,
                is_capital: false,
                star_type: "default".to_string(),
            },
            Position::from([5.0, 0.0, 0.0]),
            Sovereignty::default(),
        ))
        .id();
    app.world_mut().spawn((
        Planet {
            name: "Target I".into(),
            system: _target,
            planet_type: "default".to_string(),
        },
        SystemAttributes {
            habitability: Habitability::Adequate,
            mineral_richness: ResourceLevel::Rich,
            energy_potential: ResourceLevel::Poor,
            research_potential: ResourceLevel::Moderate,
            max_building_slots: 4,
        },
        Position::from([5.0, 0.0, 0.0]),
    ));

    // Third star system (surveyed, not colonized)
    let _surveyed = app.world_mut()
        .spawn((
            StarSystem {
                name: "Surveyed".into(),
                surveyed: true,
                is_capital: false,
                star_type: "default".to_string(),
            },
            Position::from([10.0, 3.0, 0.0]),
            Sovereignty::default(),
        ))
        .id();
    app.world_mut().spawn((
        Planet {
            name: "Surveyed I".into(),
            system: _surveyed,
            planet_type: "default".to_string(),
        },
        SystemAttributes {
            habitability: Habitability::Marginal,
            mineral_richness: ResourceLevel::Poor,
            energy_potential: ResourceLevel::Rich,
            research_potential: ResourceLevel::None,
            max_building_slots: 3,
        },
        Position::from([10.0, 3.0, 0.0]),
    ));

    // Player stationed at capital
    app.world_mut().spawn((Player, StationedAt { system: capital }));

    // Colony at capital
    app.world_mut().spawn((
        Colony {
            planet: capital_planet,
            population: 100.0,
            growth_rate: 0.01,
        },
        ResourceStockpile {
            minerals: Amt::units(500),
            energy: Amt::units(500),
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::units(5)),
            energy_per_hexadies: ModifiedValue::new(Amt::units(5)),
            research_per_hexadies: ModifiedValue::new(Amt::units(1)),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
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
        MaintenanceCost::default(),
        FoodConsumption::default(),
    ));

    // Explorer docked at capital
    app.world_mut().spawn((
        Ship {
            name: "Explorer-1".into(),
            ship_type: ShipType::Explorer,
            owner: Owner::Neutral,
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
    app.world_mut().spawn((
        Ship {
            name: "Colony Ship-1".into(),
            ship_type: ShipType::ColonyShip,
            owner: Owner::Neutral,
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
    app.world_mut().spawn((
        Ship {
            name: "Courier-1".into(),
            ship_type: ShipType::Courier,
            owner: Owner::Neutral,
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

// Combat resolution (#55)

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
            owner: Owner::Neutral,
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
            owner: Owner::Neutral,
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
            owner: Owner::Neutral,
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

// Authority production and consumption (#73)

/// Helper: spawn a star system marked as capital with a planet
fn spawn_capital_system(world: &mut World, name: &str, pos: [f64; 3]) -> Entity {
    let sys = world
        .spawn((
            StarSystem {
                name: name.to_string(),
                surveyed: true,
                is_capital: true,
                star_type: "default".to_string(),
            },
            Position::from(pos),
            Sovereignty::default(),
        ))
        .id();
    world.spawn((
        Planet {
            name: format!("{} I", name),
            system: sys,
            planet_type: "default".to_string(),
        },
        SystemAttributes {
            habitability: Habitability::Ideal,
            mineral_richness: ResourceLevel::Moderate,
            energy_potential: ResourceLevel::Moderate,
            research_potential: ResourceLevel::Moderate,
            max_building_slots: 4,
        },
        Position::from(pos),
    ));
    sys
}

#[test]
fn test_capital_produces_authority() {
    let mut app = test_app();

    let cap_sys = spawn_capital_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0]);

    // Spawn capital colony with zero authority
    let planet_cap_sys = find_planet(app.world_mut(), cap_sys);
    let colony_entity = app.world_mut().spawn((
        Colony {
            planet: planet_cap_sys,
            population: 100.0,
            growth_rate: 0.01,
        },
        ResourceStockpile {
            minerals: Amt::units(500),
            energy: Amt::units(500),
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::units(5)),
            energy_per_hexadies: ModifiedValue::new(Amt::units(5)),
            research_per_hexadies: ModifiedValue::new(Amt::units(1)),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![None; 4] },
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    )).id();

    // Advance 10 hexadies
    advance_time(&mut app, 10);

    let stockpile = app.world().get::<ResourceStockpile>(colony_entity).unwrap();
    // Capital produces BASE_AUTHORITY_PER_HEXADIES (1.0) per hexady, no colonies to drain it
    // Expected: 1.0 * 10 = 10.0
    assert!(
        (stockpile.authority.to_f64() - 10.0).abs() < 1e-6,
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
    let planet_cap_sys = find_planet(app.world_mut(), cap_sys);
    let capital_colony = app.world_mut().spawn((
        Colony {
            planet: planet_cap_sys,
            population: 100.0,
            growth_rate: 0.01,
        },
        ResourceStockpile {
            minerals: Amt::units(500),
            energy: Amt::units(500),
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::units(5), // start with 5
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::units(5)),
            energy_per_hexadies: ModifiedValue::new(Amt::units(5)),
            research_per_hexadies: ModifiedValue::new(Amt::units(1)),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![None; 4] },
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    )).id();

    // Remote colony (non-capital)
    let planet_remote_sys = find_planet(app.world_mut(), remote_sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_remote_sys,
            population: 50.0,
            growth_rate: 0.005,
        },
        ResourceStockpile {
            minerals: Amt::units(100),
            energy: Amt::units(100),
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::units(3)),
            energy_per_hexadies: ModifiedValue::new(Amt::units(3)),
            research_per_hexadies: ModifiedValue::new(Amt::new(0, 500)),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![None; 4] },
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    ));

    // Advance 10 hexadies
    advance_time(&mut app, 10);

    let stockpile = app.world().get::<ResourceStockpile>(capital_colony).unwrap();
    // Production: 1.0 * 10 = 10.0
    // Starting: 5.0
    // Cost: 0.5 * 1 colony * 10 = 5.0
    // Expected: 5.0 + 10.0 - 5.0 = 10.0
    assert!(
        (stockpile.authority.to_f64() - 10.0).abs() < 1e-6,
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
    let planet_cap_sys = find_planet(app.world_mut(), cap_sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_cap_sys,
            population: 100.0,
            growth_rate: 0.01,
        },
        ResourceStockpile {
            minerals: Amt::units(500),
            energy: Amt::units(500),
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::units(5)),
            energy_per_hexadies: ModifiedValue::new(Amt::units(5)),
            research_per_hexadies: ModifiedValue::new(Amt::units(1)),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![None; 4] },
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    ));

    // Three remote colonies with known production rates
    let planet_remote_sys = find_planet(app.world_mut(), remote_sys);
    let remote_colony = app.world_mut().spawn((
        Colony {
            planet: planet_remote_sys,
            population: 50.0,
            growth_rate: 0.005,
        },
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::units(10)),
            energy_per_hexadies: ModifiedValue::new(Amt::units(10)),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![None; 4] },
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    )).id();

    let planet_remote_sys2 = find_planet(app.world_mut(), remote_sys2);
    app.world_mut().spawn((
        Colony {
            planet: planet_remote_sys2,
            population: 50.0,
            growth_rate: 0.005,
        },
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::units(1)),
            energy_per_hexadies: ModifiedValue::new(Amt::units(1)),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![None; 4] },
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    ));

    let planet_remote_sys3 = find_planet(app.world_mut(), remote_sys3);
    app.world_mut().spawn((
        Colony {
            planet: planet_remote_sys3,
            population: 50.0,
            growth_rate: 0.005,
        },
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::units(1)),
            energy_per_hexadies: ModifiedValue::new(Amt::units(1)),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![None; 4] },
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    ));

    // Advance 10 hexadies
    advance_time(&mut app, 10);

    let stockpile = app.world().get::<ResourceStockpile>(remote_colony).unwrap();
    // With authority deficit, production is multiplied by AUTHORITY_DEFICIT_PENALTY (0.5)
    // Normal: 10.0 * 10 = 100.0
    // With penalty: 10.0 * 10 * 0.5 = 50.0
    assert!(
        (stockpile.minerals.to_f64() - 50.0).abs() < 1e-6,
        "Expected 50.0 minerals (penalized), got {}",
        stockpile.minerals
    );
    assert!(
        (stockpile.energy.to_f64() - 50.0).abs() < 1e-6,
        "Expected 50.0 energy (penalized), got {}",
        stockpile.energy
    );
}

// Farm food production (#72)

#[test]
fn test_farm_produces_food() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Farm-System",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    // Colony with food_per_hexadies=5.0, a Farm building (+5.0 food bonus), starting food=0
    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
        },
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::units(100),
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::units(5)),
        },
        BuildQueue { queue: Vec::new() },
        Buildings {
            slots: vec![Some(BuildingType::Farm)],
        },
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    ));

    // Advance 10 hexadies
    advance_time(&mut app, 10);

    let mut q = app.world_mut().query::<&ResourceStockpile>();
    let stockpile = q.iter(app.world()).next().unwrap();

    // food_per_hexadies=5.0 (base) + 5.0 (Farm bonus) = 10.0/hd
    // Over 10 hd: 100.0 produced, minus consumption (pop 10 * 0.1 * 10 = 10.0)
    // Net food should be ~90.0
    let expected_food = 90.0;
    assert!(
        (stockpile.food.to_f64() - expected_food).abs() < 5.0,
        "Expected ~{} food, got {}",
        expected_food,
        stockpile.food
    );
    assert!(
        stockpile.food.to_f64() > 0.0,
        "Food should be positive with Farm producing"
    );
}

// Food + Authority deficit interaction (#72 + #73)

#[test]
fn test_authority_deficit_penalizes_food_production() {
    let mut app = test_app();

    // Capital system (provides authority context)
    let cap_sys = spawn_test_system(
        app.world_mut(),
        "Capital",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    // Non-capital system
    let remote_sys = spawn_test_system(
        app.world_mut(),
        "Remote",
        [10.0, 0.0, 0.0],
        Habitability::Adequate,
        false,
        true,
    );

    // Mark as capital
    app.world_mut().entity_mut(cap_sys).get_mut::<StarSystem>().unwrap().is_capital = true;

    // Capital colony with 0 authority (deficit)
    let planet_cap_sys = find_planet(app.world_mut(), cap_sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_cap_sys,
            population: 1.0,
            growth_rate: 0.0,
        },
        ResourceStockpile {
            minerals: Amt::units(1000),
            energy: Amt::units(1000),
            research: Amt::ZERO,
            food: Amt::units(1000),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    ));

    // Spawn 3 remote colonies so authority cost (0.5*3=1.5/hd) > production (1.0/hd),
    // ensuring the capital stays in deficit.
    let remote_systems: Vec<Entity> = (0..3)
        .map(|i| {
            spawn_test_system(
                app.world_mut(),
                &format!("Remote-{}", i),
                [(i + 1) as f64 * 10.0, 0.0, 0.0],
                Habitability::Adequate,
                false,
                true,
            )
        })
        .collect();

    for &sys in &remote_systems {
        let planet_sys = find_planet(app.world_mut(), sys);
        app.world_mut().spawn((
            Colony {
                planet: planet_sys,
                population: 1.0,
                growth_rate: 0.0,
            },
            ResourceStockpile {
                minerals: Amt::ZERO,
                energy: Amt::ZERO,
                research: Amt::ZERO,
                food: Amt::ZERO,
                authority: Amt::ZERO,
            },
            ResourceCapacity::default(),
            Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::units(10)),
        },
            BuildQueue { queue: Vec::new() },
            ProductionFocus::default(),
            MaintenanceCost::default(),
            FoodConsumption::default(),
        ));
    }

    advance_time(&mut app, 10);

    // Check a remote colony's food: 10.0/hd * 0.5 (penalty) * 10 hd = 50.0, minus consumption
    let remote_planet_0 = find_planet(app.world_mut(), remote_systems[0]);
    let mut q = app.world_mut().query::<(&Colony, &ResourceStockpile)>();
    for (colony, stockpile) in q.iter(app.world()) {
        if colony.planet == remote_planet_0 {
            // Without penalty: 100.0 food. With 0.5 penalty: ~50.0 food (minus small consumption)
            assert!(
                stockpile.food.to_f64() < 60.0,
                "Food production should be penalized by authority deficit, got {}",
                stockpile.food
            );
            assert!(
                stockpile.food.to_f64() > 0.0,
                "Food should still be positive, got {}",
                stockpile.food
            );
        }
    }
}

// Maintenance system (#68)

#[test]
fn test_maintenance_deducts_energy_integration() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Maint-System",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    // Colony with Mine (0.2 E/hd) and Shipyard (1.0 E/hd) = 1.2 E/hd total maintenance
    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
        },
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::units(100),
            research: Amt::ZERO,
            food: Amt::units(10000),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::units(10)),
        },
        BuildQueue { queue: Vec::new() },
        Buildings {
            slots: vec![Some(BuildingType::Mine), Some(BuildingType::Shipyard)],
        },
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    ));

    // Advance 10 hexadies — maintenance should deduct 1.2 * 10 = 12 energy
    advance_time(&mut app, 10);

    let mut q = app.world_mut().query::<&ResourceStockpile>();
    let stockpile = q.iter(app.world()).next().unwrap();

    assert!(
        stockpile.energy.to_f64() < 100.0,
        "Maintenance should have deducted energy, got {}",
        stockpile.energy
    );
    assert!(
        (stockpile.energy.to_f64() - 88.0).abs() < 1.0,
        "Expected ~88 energy (100 - 12), got {}",
        stockpile.energy
    );
}

// Logistic population growth (#69)

#[test]
fn test_population_capped_by_carrying_capacity() {
    let mut app = test_app();

    // Marginal habitability: base_score=0.4, K_habitat = 200 * 0.4 = 80
    // food_per_hd=10 (base) + 0 (no farm) = 10 → K_food = 10/0.1 = 100
    // effective K = min(80, 100) = 80
    let sys = spawn_test_system(
        app.world_mut(),
        "Marginal-World",
        [0.0, 0.0, 0.0],
        Habitability::Marginal,
        true,
        true,
    );

    let planet_sys = find_planet(app.world_mut(), sys);
    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 70.0,
            growth_rate: 0.05,
        },
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::units(10000),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::units(10)),
        },
        BuildQueue { queue: Vec::new() },
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    ));

    // Advance in 1-hexady steps for stable Euler integration
    for _ in 0..600 {
        advance_time(&mut app, 1);
    }

    let mut q = app.world_mut().query::<&Colony>();
    let colony = q.iter(app.world()).next().unwrap();

    assert!(
        colony.population <= 81.0,
        "Population should not exceed carrying capacity ~80, got {}",
        colony.population
    );
    assert!(
        colony.population > 60.0,
        "Population should have grown toward K, got {}",
        colony.population
    );
}

#[test]
fn test_habitability_affects_growth_rate() {
    // Same setup, different habitability → different growth speed
    let mut ideal_app = test_app();
    let mut marginal_app = test_app();

    let ideal_sys = spawn_test_system(
        ideal_app.world_mut(),
        "Ideal-World",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );
    let marginal_sys = spawn_test_system(
        marginal_app.world_mut(),
        "Marginal-World",
        [0.0, 0.0, 0.0],
        Habitability::Marginal,
        true,
        true,
    );

    let colony_bundle = |planet_entity: Entity| {
        (
            Colony {
                planet: planet_entity,
                population: 10.0,
                growth_rate: 0.05,
            },
            ResourceStockpile {
                minerals: Amt::ZERO,
                energy: Amt::ZERO,
                research: Amt::ZERO,
                food: Amt::units(10000),
                authority: Amt::ZERO,
            },
            ResourceCapacity::default(),
            Production {
                minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
                energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
                research_per_hexadies: ModifiedValue::new(Amt::ZERO),
                food_per_hexadies: ModifiedValue::new(Amt::units(100)), // abundant food so K isn't food-limited
            },
            BuildQueue { queue: Vec::new() },
            ProductionFocus::default(),
            MaintenanceCost::default(),
            FoodConsumption::default(),
        )
    };

    let ideal_planet = find_planet(ideal_app.world_mut(), ideal_sys);
    ideal_app.world_mut().spawn(colony_bundle(ideal_planet));
    let marginal_planet = find_planet(marginal_app.world_mut(), marginal_sys);
    marginal_app.world_mut().spawn(colony_bundle(marginal_planet));

    for _ in 0..60 {
        advance_time(&mut ideal_app, 1);
        advance_time(&mut marginal_app, 1);
    }

    let ideal_pop = ideal_app
        .world_mut()
        .query::<&Colony>()
        .iter(ideal_app.world())
        .next()
        .unwrap()
        .population;
    let marginal_pop = marginal_app
        .world_mut()
        .query::<&Colony>()
        .iter(marginal_app.world())
        .next()
        .unwrap()
        .population;

    assert!(
        ideal_pop > marginal_pop,
        "Ideal world should grow faster: ideal={}, marginal={}",
        ideal_pop,
        marginal_pop
    );
}

#[test]
fn test_food_limits_carrying_capacity() {
    let mut app = test_app();

    // Ideal habitability: K_habitat = 200 * 1.0 = 200
    // But food_per_hd = 5.0 → K_food = 5.0/0.1 = 50
    // effective K = min(200, 50) = 50
    let sys = spawn_test_system(
        app.world_mut(),
        "Food-Limited",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 40.0,
            growth_rate: 0.05,
        },
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::units(10000),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::units(5)),
        },
        BuildQueue { queue: Vec::new() },
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    ));

    for _ in 0..600 {
        advance_time(&mut app, 1);
    }

    let mut q = app.world_mut().query::<&Colony>();
    let colony = q.iter(app.world()).next().unwrap();

    assert!(
        colony.population <= 51.0,
        "Population should be capped by food K=50, got {}",
        colony.population
    );
}

// ResourceCapacity clamping

#[test]
fn test_resource_capacity_clamps_stockpile() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Cap-System",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    // Colony with very high production but low capacity
    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
        },
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        },
        ResourceCapacity {
            minerals: Amt::units(100),
            energy: Amt::units(100),
            food: Amt::units(500),
            authority: Amt::units(10000),
        },
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::units(50)),
            energy_per_hexadies: ModifiedValue::new(Amt::units(50)),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![] },
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    ));

    // After 10 hd, production would be 500 minerals without cap
    advance_time(&mut app, 10);

    let mut q = app.world_mut().query::<&ResourceStockpile>();
    let stockpile = q.iter(app.world()).next().unwrap();

    assert_eq!(
        stockpile.minerals,
        Amt::units(100),
        "Minerals should be clamped to capacity 100, got {}",
        stockpile.minerals
    );
    assert_eq!(
        stockpile.energy,
        Amt::units(100),
        "Energy should be clamped to capacity 100, got {}",
        stockpile.energy
    );
}

// Modifier affects production output

#[test]
fn test_modifier_affects_production_output() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Mod-System",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    let mut minerals_prod = ModifiedValue::new(Amt::units(5));
    minerals_prod.push_modifier(Modifier {
        id: "tech_boost".to_string(),
        label: "Tech Boost".to_string(),
        base_add: SignedAmt::ZERO,
        multiplier: SignedAmt::new(0, 200), // +20%
        add: SignedAmt::ZERO,
        expires_at: None,
        on_expire_event: None,
    });

    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
        },
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: minerals_prod,
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![] },
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    ));

    advance_time(&mut app, 10);

    let mut q = app.world_mut().query::<&ResourceStockpile>();
    let stockpile = q.iter(app.world()).next().unwrap();

    // 5 * 1.2 * 10 = 60
    assert!(
        stockpile.minerals.to_f64() > 50.0,
        "Expected minerals > 50 with +20% modifier, got {}",
        stockpile.minerals
    );
    assert!(
        (stockpile.minerals.to_f64() - 60.0).abs() < 1.0,
        "Expected ~60 minerals, got {}",
        stockpile.minerals
    );
}

// Building queue completes construction

#[test]
fn test_building_queue_completes_construction() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Build-System",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    // Colony with enough resources and an empty slot; queue a Mine
    let (minerals_cost, energy_cost) = BuildingType::Mine.build_cost();
    let build_time = BuildingType::Mine.build_time();

    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
        },
        ResourceStockpile {
            minerals: Amt::units(500),
            energy: Amt::units(500),
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![None, None, None, None] },
        BuildingQueue {
            queue: vec![BuildingOrder {
                building_type: BuildingType::Mine,
                target_slot: 0,
                minerals_remaining: minerals_cost,
                energy_remaining: energy_cost,
                build_time_remaining: build_time,
            }],
            demolition_queue: Vec::new(),
        },
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    ));

    // Advance enough time for completion
    advance_time(&mut app, build_time + 5);

    let mut q = app.world_mut().query::<&Buildings>();
    let buildings = q.iter(app.world()).next().unwrap();

    assert_eq!(
        buildings.slots[0],
        Some(BuildingType::Mine),
        "Mine should have been built in slot 0"
    );
}

// Building demolition

#[test]
fn test_demolish_building_removes_from_slot() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Demo-System",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    let demo_time = BuildingType::Mine.demolition_time();
    let (m_refund, e_refund) = BuildingType::Mine.demolition_refund();

    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
        },
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings {
            slots: vec![Some(BuildingType::Mine), None, None, None],
        },
        BuildingQueue {
            queue: Vec::new(),
            demolition_queue: vec![DemolitionOrder {
                target_slot: 0,
                building_type: BuildingType::Mine,
                time_remaining: demo_time,
                minerals_refund: m_refund,
                energy_refund: e_refund,
            }],
        },
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    ));

    // Advance enough time for demolition to complete
    advance_time(&mut app, demo_time + 1);

    let mut q = app.world_mut().query::<&Buildings>();
    let buildings = q.iter(app.world()).next().unwrap();
    assert_eq!(
        buildings.slots[0], None,
        "Slot 0 should be empty after demolition completes"
    );
}

#[test]
fn test_demolish_refunds_resources() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Refund-System",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    let demo_time = BuildingType::Mine.demolition_time();
    let (m_refund, e_refund) = BuildingType::Mine.demolition_refund();

    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
        },
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings {
            slots: vec![Some(BuildingType::Mine), None, None, None],
        },
        BuildingQueue {
            queue: Vec::new(),
            demolition_queue: vec![DemolitionOrder {
                target_slot: 0,
                building_type: BuildingType::Mine,
                time_remaining: demo_time,
                minerals_refund: m_refund,
                energy_refund: e_refund,
            }],
        },
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    ));

    advance_time(&mut app, demo_time + 1);

    let mut q = app.world_mut().query::<&ResourceStockpile>();
    let stockpile = q.iter(app.world()).next().unwrap();
    assert!(
        stockpile.minerals >= m_refund,
        "Should have received minerals refund: expected at least {}, got {}",
        m_refund,
        stockpile.minerals
    );
    assert!(
        stockpile.energy >= e_refund,
        "Should have received energy refund: expected at least {}, got {}",
        e_refund,
        stockpile.energy
    );
}

#[test]
fn test_demolish_takes_time() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Time-System",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    let demo_time = BuildingType::Shipyard.demolition_time(); // 30 / 2 = 15
    let (m_refund, e_refund) = BuildingType::Shipyard.demolition_refund();

    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
        },
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings {
            slots: vec![Some(BuildingType::Shipyard), None, None, None],
        },
        BuildingQueue {
            queue: Vec::new(),
            demolition_queue: vec![DemolitionOrder {
                target_slot: 0,
                building_type: BuildingType::Shipyard,
                time_remaining: demo_time,
                minerals_refund: m_refund,
                energy_refund: e_refund,
            }],
        },
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    ));

    // Advance only half the demolition time — building should still be present
    let partial = demo_time / 2;
    assert!(partial > 0, "Partial time should be > 0 for this test");
    advance_time(&mut app, partial);

    {
        let mut q = app.world_mut().query::<&Buildings>();
        let buildings = q.iter(app.world()).next().unwrap();
        assert_eq!(
            buildings.slots[0],
            Some(BuildingType::Shipyard),
            "Building should still be present before demolition completes"
        );
    }

    // Advance the rest of the time + 1 to complete
    advance_time(&mut app, demo_time - partial + 1);

    {
        let mut q = app.world_mut().query::<&Buildings>();
        let buildings = q.iter(app.world()).next().unwrap();
        assert_eq!(
            buildings.slots[0], None,
            "Building should be removed after demolition completes"
        );
    }
}

// ConstructionParams resource exists and can be modified

#[test]
fn test_construction_params_modify_ship_cost() {
    let mut app = test_app();

    // Verify ConstructionParams component on empire exists
    {
        let empire = empire_entity(app.world_mut());
        let params = app.world().get::<ConstructionParams>(empire).unwrap();
        assert_eq!(
            params.ship_cost_modifier.final_value(),
            Amt::units(1),
            "Default ship cost modifier should be 1.0"
        );
    }

    // Modify it
    {
        let empire = empire_entity(app.world_mut());
        let mut params = app.world_mut().get_mut::<ConstructionParams>(empire).unwrap();
        params.ship_cost_modifier.push_modifier(Modifier {
            id: "tech_cheaper_ships".to_string(),
            label: "Cheaper Ships".to_string(),
            base_add: SignedAmt::ZERO,
            multiplier: SignedAmt::new(0, 500), // +50%
            add: SignedAmt::ZERO,
            expires_at: None,
            on_expire_event: None,
        });
    }

    let empire = empire_entity(app.world_mut());
    let params = app.world().get::<ConstructionParams>(empire).unwrap();
    assert_eq!(
        params.ship_cost_modifier.final_value(),
        Amt::new(1, 500),
        "Ship cost modifier should be 1.5 after pushing +50% modifier"
    );
}

// Building bonus via sync_building_modifiers

#[test]
fn test_building_bonus_via_sync_modifiers() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Sync-System",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    // Colony with Mine in slot 0, base minerals=5
    let colony = spawn_test_colony(
        app.world_mut(),
        sys,
        Amt::ZERO,
        Amt::ZERO,
        vec![Some(BuildingType::Mine), None, None, None],
    );

    // Run one update to trigger sync_building_modifiers
    app.update();

    let prod = app.world().get::<Production>(colony).unwrap();
    // Base=5 + Mine base_add=3 -> effective_base=8, no multipliers -> final=8
    assert_eq!(
        prod.minerals_per_hexadies.final_value(),
        Amt::units(8),
        "Expected 8 minerals/hd (5 base + 3 Mine), got {}",
        prod.minerals_per_hexadies.final_value()
    );
}

// Maintenance modifier affects energy

#[test]
fn test_maintenance_modifier_affects_energy() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Maint-System",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    // Mine maintenance = 0.2, Shipyard maintenance = 1.0 => total base = 1.2/hd
    // With +50% multiplier => 1.2 * 1.5 = 1.8/hd
    // Over 10 hd => 18.0 energy deducted from 100 => 82.0 remaining
    let mut maint = MaintenanceCost::default();
    maint.energy_per_hexadies.push_modifier(Modifier {
        id: "tech_expensive".to_string(),
        label: "Expensive Maintenance".to_string(),
        base_add: SignedAmt::ZERO,
        multiplier: SignedAmt::new(0, 500), // +50%
        add: SignedAmt::ZERO,
        expires_at: None,
        on_expire_event: None,
    });

    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
        },
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::units(100),
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings {
            slots: vec![Some(BuildingType::Mine), Some(BuildingType::Shipyard), None, None],
        },
        BuildingQueue::default(),
        ProductionFocus::default(),
        maint,
        FoodConsumption::default(),
    ));

    // First update to sync maintenance modifiers (adds building base_adds)
    app.update();

    // Now advance 10 hd
    for _ in 0..10 {
        advance_time(&mut app, 1);
    }

    let mut q = app.world_mut().query::<&ResourceStockpile>();
    let stockpile = q.iter(app.world()).next().unwrap();

    // Base maintenance from buildings: Mine=0.2 + Shipyard=1.0 = 1.2/hd
    // With +50% multiplier: 1.2 * 1.5 = 1.8/hd
    // Over 10 hd: 18.0 deducted from 100 => 82.0 remaining
    let remaining = stockpile.energy.to_f64();
    assert!(
        (remaining - 82.0).abs() < 2.0,
        "Expected ~82 energy remaining (18 deducted with +50% maint modifier), got {}",
        remaining
    );
}

// Food consumption modifier

#[test]
fn test_food_consumption_modifier() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Food-System",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    // Population=100, FOOD_PER_POP=0.1/hd => base consumption=10/hd
    // With +20% multiplier => 12/hd
    // After 1 hd: 12 food consumed from 100 => 88 remaining
    let mut food_consumption = FoodConsumption::default();
    food_consumption.food_per_hexadies.push_modifier(Modifier {
        id: "tech_food".to_string(),
        label: "Extra Consumption".to_string(),
        base_add: SignedAmt::ZERO,
        multiplier: SignedAmt::new(0, 200), // +20%
        add: SignedAmt::ZERO,
        expires_at: None,
        on_expire_event: None,
    });

    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 100.0,
            growth_rate: 0.0,
        },
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![] },
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        food_consumption,
    ));

    // Run one update so sync_food_consumption sets the base
    app.update();

    // Advance 1 hd
    advance_time(&mut app, 1);

    let mut q = app.world_mut().query::<&ResourceStockpile>();
    let stockpile = q.iter(app.world()).next().unwrap();

    // Base food consumption: 100 pop * 0.1 = 10/hd
    // With +20% multiplier: 10 * 1.2 = 12/hd
    // After 1 hd: 100 - 12 = 88
    let remaining = stockpile.food.to_f64();
    assert!(
        (remaining - 88.0).abs() < 2.0,
        "Expected ~88 food remaining (12 consumed with +20% modifier), got {}",
        remaining
    );
}

// Authority params modifier

#[test]
fn test_authority_params_modifier() {
    let mut app = test_app();

    // Push +50% multiplier to authority production
    {
        let empire = empire_entity(app.world_mut());
        let mut params = app.world_mut().get_mut::<AuthorityParams>(empire).unwrap();
        params.production.push_modifier(Modifier {
            id: "tech_authority".to_string(),
            label: "Authority Boost".to_string(),
            base_add: SignedAmt::ZERO,
            multiplier: SignedAmt::new(0, 500), // +50%
            add: SignedAmt::ZERO,
            expires_at: None,
            on_expire_event: None,
        });
    }

    let sys = spawn_test_system(
        app.world_mut(),
        "Auth-System",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    // Mark as capital
    app.world_mut().get_mut::<StarSystem>(sys).unwrap().is_capital = true;

    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
        },
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![] },
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    ));

    // Advance 10 hd
    advance_time(&mut app, 10);

    let mut q = app.world_mut().query::<&ResourceStockpile>();
    let stockpile = q.iter(app.world()).next().unwrap();

    // Base authority = 1.0/hd, with +50% = 1.5/hd, over 10 hd = 15.0
    assert!(
        (stockpile.authority.to_f64() - 15.0).abs() < 1.0,
        "Expected ~15 authority (1.5/hd * 10), got {}",
        stockpile.authority
    );
}

// Production focus weights

#[test]
fn test_production_focus_weights() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Focus-System",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
        },
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::units(5)),
            energy_per_hexadies: ModifiedValue::new(Amt::units(5)),
            research_per_hexadies: ModifiedValue::new(Amt::units(1)),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![] },
        BuildingQueue::default(),
        ProductionFocus::minerals(), // minerals_weight=2.0, energy_weight=0.5
        MaintenanceCost::default(),
        FoodConsumption::default(),
    ));

    advance_time(&mut app, 10);

    let mut q = app.world_mut().query::<&ResourceStockpile>();
    let stockpile = q.iter(app.world()).next().unwrap();

    // minerals: 5 * 2.0 * 10 = 100, energy: 5 * 0.5 * 10 = 25
    assert!(
        stockpile.minerals > stockpile.energy,
        "Minerals ({}) should exceed energy ({}) with minerals focus",
        stockpile.minerals,
        stockpile.energy
    );
    assert!(
        (stockpile.minerals.to_f64() - 100.0).abs() < 5.0,
        "Expected ~100 minerals, got {}",
        stockpile.minerals
    );
    assert!(
        (stockpile.energy.to_f64() - 25.0).abs() < 5.0,
        "Expected ~25 energy, got {}",
        stockpile.energy
    );
}

// Build queue partial resources

#[test]
fn test_build_queue_partial_resources() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Partial-System",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    // Colony with only 20 minerals, building order costs 150 minerals + 50 energy
    // Mine build_time = 10 hd
    let (minerals_cost, energy_cost) = BuildingType::Mine.build_cost();
    let build_time = BuildingType::Mine.build_time();

    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
        },
        ResourceStockpile {
            minerals: Amt::units(20),
            energy: Amt::units(200),
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::units(20)), // produces 20/hd
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![None, None, None, None] },
        BuildingQueue {
            queue: vec![BuildingOrder {
                building_type: BuildingType::Mine,
                target_slot: 0,
                minerals_remaining: minerals_cost,
                energy_remaining: energy_cost,
                build_time_remaining: build_time,
            }],
            demolition_queue: Vec::new(),
        },
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    ));

    // After 1 hd: only 20 minerals available, not enough to fully pay 150
    advance_time(&mut app, 1);

    let mut q = app.world_mut().query::<&Buildings>();
    let buildings = q.iter(app.world()).next().unwrap();
    assert_eq!(
        buildings.slots[0], None,
        "Mine should NOT be complete after 1 hd (insufficient resources)"
    );

    // Keep advancing -- production adds 20/hd, eventually enough
    for _ in 0..20 {
        advance_time(&mut app, 1);
    }

    let mut q = app.world_mut().query::<&Buildings>();
    let buildings = q.iter(app.world()).next().unwrap();
    assert_eq!(
        buildings.slots[0],
        Some(BuildingType::Mine),
        "Mine should be complete after enough time with ongoing production"
    );
}

// Build queue requires shipyard

#[test]
fn test_build_queue_requires_shipyard() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "NoYard-System",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    // Colony WITHOUT Shipyard, but with a ship build order
    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
        },
        ResourceStockpile {
            minerals: Amt::units(500),
            energy: Amt::units(500),
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue {
            queue: vec![BuildOrder {
                ship_type_name: "Explorer".to_string(),
                minerals_cost: Amt::units(100),
                minerals_invested: Amt::ZERO,
                energy_cost: Amt::units(50),
                energy_invested: Amt::ZERO,
                build_time_total: 60,
                build_time_remaining: 60,
            }],
        },
        Buildings { slots: vec![Some(BuildingType::Mine), None, None, None] }, // No Shipyard!
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
        Position::from([0.0, 0.0, 0.0]),
    ));

    advance_time(&mut app, 100);

    // Verify no ship was spawned
    let mut ship_q = app.world_mut().query::<&Ship>();
    let ship_count = ship_q.iter(app.world()).count();
    assert_eq!(
        ship_count, 0,
        "No ship should be spawned without a Shipyard"
    );

    // Build order should still be in queue (not consumed)
    let mut bq_q = app.world_mut().query::<&BuildQueue>();
    let bq = bq_q.iter(app.world()).next().unwrap();
    assert_eq!(
        bq.queue.len(),
        1,
        "Build order should still be in queue without Shipyard"
    );
}

// Starvation reduces population

#[test]
fn test_starvation_reduces_population() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Starve-System",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 100.0,
            growth_rate: 0.01,
        },
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::ZERO, // No food!
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO), // No food production
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![] },
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    ));

    advance_time(&mut app, 1);

    let mut q = app.world_mut().query::<&Colony>();
    let colony = q.iter(app.world()).next().unwrap();

    assert!(
        colony.population < 100.0,
        "Population should decrease during starvation, got {}",
        colony.population
    );
}

// Starvation population floor

#[test]
fn test_starvation_population_floor() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Floor-System",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 1.5,
            growth_rate: 0.01,
        },
        ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![] },
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
    ));

    // Advance many hexadies with starvation
    for _ in 0..500 {
        advance_time(&mut app, 1);
    }

    let mut q = app.world_mut().query::<&Colony>();
    let colony = q.iter(app.world()).next().unwrap();

    assert!(
        colony.population >= 1.0,
        "Population should never drop below 1.0, got {}",
        colony.population
    );
}

#[test]
fn test_timed_modifier_expires_in_game() {
    use common::*;

    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "TimedTest",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    // Spawn colony with base mineral production = 5/hd, no buildings
    let colony_id = spawn_test_colony(
        app.world_mut(),
        sys,
        Amt::ZERO,
        Amt::ZERO,
        vec![],
    );

    // Push a +20% mineral production modifier that expires in 5 hd
    {
        let mut prod = app.world_mut().get_mut::<Production>(colony_id).unwrap();
        prod.minerals_per_hexadies.push_modifier_timed(
            Modifier {
                id: "timed_boost".to_string(),
                label: "Timed Boost".to_string(),
                base_add: SignedAmt::ZERO,
                multiplier: SignedAmt::new(0, 200), // +20%
                add: SignedAmt::ZERO,
                expires_at: None, // will be set by push_modifier_timed
                on_expire_event: None,
            },
            0,
            5,
        );
    }

    // Verify modifier is present and production is boosted: 5 * 1.2 = 6
    {
        let prod = app.world().get::<Production>(colony_id).unwrap();
        assert_eq!(prod.minerals_per_hexadies.final_value(), Amt::units(6));
        assert!(prod.minerals_per_hexadies.has_modifier("timed_boost"));
    }

    // Advance 3 hd — modifier should still be active
    advance_time(&mut app, 3);
    {
        let prod = app.world().get::<Production>(colony_id).unwrap();
        assert!(
            prod.minerals_per_hexadies.has_modifier("timed_boost"),
            "Timed modifier should still be present at clock=3"
        );
        assert_eq!(prod.minerals_per_hexadies.final_value(), Amt::units(6));
    }

    // Advance 3 more hd (total clock=6) — modifier should be expired and removed
    advance_time(&mut app, 3);
    {
        let prod = app.world().get::<Production>(colony_id).unwrap();
        assert!(
            !prod.minerals_per_hexadies.has_modifier("timed_boost"),
            "Timed modifier should be removed at clock=6 (expired at 5)"
        );
        assert_eq!(prod.minerals_per_hexadies.final_value(), Amt::units(5));
    }
}

#[test]
fn test_expired_modifier_has_on_expire_event() {
    use common::*;

    let mut app = test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "Expire Event Test",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
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

// Research control (#75)

#[test]
fn test_start_research_sets_queue() {
    use technology::{ResearchQueue, TechId};

    let mut queue = ResearchQueue::default();
    assert!(queue.current.is_none());
    assert_eq!(queue.accumulated, 0.0);
    assert!(!queue.blocked);

    queue.start_research(TechId(100));
    assert_eq!(queue.current, Some(TechId(100)));
    assert_eq!(queue.accumulated, 0.0);
    assert!(!queue.blocked);
}

#[test]
fn test_block_research_stops_progress() {
    use technology::{ResearchQueue, ResearchPool, TechId, TechTree, Technology, TechBranch, TechCost, LastResearchTick};
    use macrocosmo::amount::Amt;

    let mut app = test_app();

    // Add technology systems not included in basic test_app
    app.add_systems(
        Update,
        (
            technology::emit_research,
            technology::receive_research,
            technology::tick_research,
            technology::flush_research,
        )
            .chain()
            .after(macrocosmo::time_system::advance_game_time),
    );

    // Insert tech tree onto empire entity
    let tree = TechTree::from_vec(vec![Technology {
        id: TechId(1),
        name: "Test".into(),
        branch: TechBranch::Physics,
        cost: TechCost::research_only(Amt::units(100)),
        prerequisites: vec![],
        description: String::new(),
    }]);
    {
        let empire = empire_entity(app.world_mut());
        app.world_mut().entity_mut(empire).insert(tree);
    }

    // Start research and block it
    {
        let empire = empire_entity(app.world_mut());
        let mut queue = app.world_mut().get_mut::<ResearchQueue>(empire).unwrap();
        queue.start_research(TechId(1));
        queue.block();
    }

    // Add points to pool
    {
        let empire = empire_entity(app.world_mut());
        app.world_mut().get_mut::<ResearchPool>(empire).unwrap().points = 50.0;
    }

    // Advance time
    advance_time(&mut app, 1);

    // Queue should have no progress because it's blocked
    let empire = empire_entity(app.world_mut());
    let queue = app.world().get::<ResearchQueue>(empire).unwrap();
    assert_eq!(queue.accumulated, 0.0);
    assert!(queue.blocked);
    assert_eq!(queue.current, Some(TechId(1)));
}

#[test]
fn test_add_research_progress() {
    use technology::{ResearchQueue, TechId};

    let mut queue = ResearchQueue::default();
    queue.start_research(TechId(1));
    assert_eq!(queue.accumulated, 0.0);

    queue.add_progress(25.0);
    assert_eq!(queue.accumulated, 25.0);

    queue.add_progress(10.0);
    assert_eq!(queue.accumulated, 35.0);
}

#[test]
fn test_cancel_research_clears_queue() {
    use technology::{ResearchQueue, TechId};

    let mut queue = ResearchQueue::default();
    queue.start_research(TechId(1));
    queue.add_progress(50.0);

    queue.cancel_research();
    assert!(queue.current.is_none());
    assert_eq!(queue.accumulated, 0.0);
}

// Technology knowledge propagation (#88)

/// Helper: set up an app with tech research + propagation systems for knowledge tests.
fn tech_knowledge_app() -> App {
    let app = full_test_app();
    app
}

#[test]
fn test_tech_propagates_to_capital_immediately() {
    use macrocosmo::technology::{
        RecentlyResearched, TechId, TechKnowledge,
    };

    let mut app = tech_knowledge_app();

    // Spawn capital system
    let capital = app.world_mut().spawn((
        StarSystem {
            name: "Capital".into(),
            surveyed: true,
            is_capital: true,
                star_type: "default".to_string(),
        },
        Position::from([0.0, 0.0, 0.0]),
        Sovereignty::default(),
        TechKnowledge::default(),
    )).id();
    app.world_mut().spawn((
        Planet { name: "Capital I".into(), system: capital , planet_type: "default".to_string() },
        SystemAttributes {
            habitability: Habitability::Ideal,
            mineral_richness: ResourceLevel::Moderate,
            energy_potential: ResourceLevel::Moderate,
            research_potential: ResourceLevel::Moderate,
            max_building_slots: 4,
        },
        Position::from([0.0, 0.0, 0.0]),
    ));

    // Spawn a colony at the capital
    spawn_test_colony(
        app.world_mut(),
        capital,
        Amt::units(100),
        Amt::units(100),
        vec![],
    );

    // Simulate a tech being recently researched
    {
        let empire = empire_entity(app.world_mut());
        app.world_mut()
            .get_mut::<RecentlyResearched>(empire)
            .unwrap()
            .techs
            .push(TechId(100));
    }

    // Run one update
    advance_time(&mut app, 1);

    // Capital should have the tech immediately
    let knowledge = app.world().get::<TechKnowledge>(capital).unwrap();
    assert!(
        knowledge.known_techs.contains(&TechId(100)),
        "Capital should know tech immediately after research"
    );
}

#[test]
fn test_tech_propagates_to_remote_with_delay() {
    use macrocosmo::technology::{
        RecentlyResearched, TechId, TechKnowledge,
    };

    let mut app = tech_knowledge_app();

    // Capital at origin
    let capital = app.world_mut().spawn((
        StarSystem {
            name: "Capital".into(),
            surveyed: true,
            is_capital: true,
                star_type: "default".to_string(),
        },
        Position::from([0.0, 0.0, 0.0]),
        Sovereignty::default(),
        TechKnowledge::default(),
    )).id();
    app.world_mut().spawn((
        Planet { name: "Capital I".into(), system: capital , planet_type: "default".to_string() },
        SystemAttributes {
            habitability: Habitability::Ideal,
            mineral_richness: ResourceLevel::Moderate,
            energy_potential: ResourceLevel::Moderate,
            research_potential: ResourceLevel::Moderate,
            max_building_slots: 4,
        },
        Position::from([0.0, 0.0, 0.0]),
    ));

    // Remote system at 1 LY (light delay = 60 hexadies)
    let remote = app.world_mut().spawn((
        StarSystem {
            name: "Remote".into(),
            surveyed: true,
            is_capital: false,
                star_type: "default".to_string(),
        },
        Position::from([1.0, 0.0, 0.0]),
        Sovereignty::default(),
        TechKnowledge::default(),
    )).id();
    app.world_mut().spawn((
        Planet { name: "Remote I".into(), system: remote , planet_type: "default".to_string() },
        SystemAttributes {
            habitability: Habitability::Adequate,
            mineral_richness: ResourceLevel::Moderate,
            energy_potential: ResourceLevel::Moderate,
            research_potential: ResourceLevel::Moderate,
            max_building_slots: 4,
        },
        Position::from([1.0, 0.0, 0.0]),
    ));

    // Colonies at both systems
    spawn_test_colony(
        app.world_mut(),
        capital,
        Amt::units(100),
        Amt::units(100),
        vec![],
    );
    spawn_test_colony(
        app.world_mut(),
        remote,
        Amt::units(100),
        Amt::units(100),
        vec![],
    );

    // Simulate tech researched at tick 0
    {
        let empire = empire_entity(app.world_mut());
        app.world_mut()
            .get_mut::<RecentlyResearched>(empire)
            .unwrap()
            .techs
            .push(TechId(200));
    }

    // First tick: propagation entities spawned
    advance_time(&mut app, 1);

    // Capital should have it immediately
    let capital_knowledge = app.world().get::<TechKnowledge>(capital).unwrap();
    assert!(capital_knowledge.known_techs.contains(&TechId(200)));

    // Remote should NOT have it yet (need 60 hexadies for 1 LY)
    let remote_knowledge = app.world().get::<TechKnowledge>(remote).unwrap();
    assert!(
        !remote_knowledge.known_techs.contains(&TechId(200)),
        "Remote system should not know tech before light delay"
    );

    // Advance to just before arrival (59 more hexadies, total elapsed = 60)
    advance_time(&mut app, 59);
    let remote_knowledge = app.world().get::<TechKnowledge>(remote).unwrap();
    assert!(
        !remote_knowledge.known_techs.contains(&TechId(200)),
        "Remote system should not know tech at tick 60 (arrives_at = 60, spawned at tick 1)"
    );

    // Advance one more tick to reach arrival time
    advance_time(&mut app, 1);
    let remote_knowledge = app.world().get::<TechKnowledge>(remote).unwrap();
    assert!(
        remote_knowledge.known_techs.contains(&TechId(200)),
        "Remote system should know tech after light delay"
    );
}

#[test]
fn test_uncolonized_system_no_propagation() {
    use macrocosmo::technology::{
        PendingKnowledgePropagation, RecentlyResearched, TechId, TechKnowledge,
    };

    let mut app = tech_knowledge_app();

    // Capital at origin
    let capital = app.world_mut().spawn((
        StarSystem {
            name: "Capital".into(),
            surveyed: true,
            is_capital: true,
                star_type: "default".to_string(),
        },
        Position::from([0.0, 0.0, 0.0]),
        Sovereignty::default(),
        TechKnowledge::default(),
    )).id();
    app.world_mut().spawn((
        Planet { name: "Capital I".into(), system: capital , planet_type: "default".to_string() },
        SystemAttributes {
            habitability: Habitability::Ideal,
            mineral_richness: ResourceLevel::Moderate,
            energy_potential: ResourceLevel::Moderate,
            research_potential: ResourceLevel::Moderate,
            max_building_slots: 4,
        },
        Position::from([0.0, 0.0, 0.0]),
    ));

    // Uncolonized system (no colony spawned for it)
    let _uncolonized = app.world_mut().spawn((
        StarSystem {
            name: "Uncolonized".into(),
            surveyed: true,
            is_capital: false,
                star_type: "default".to_string(),
        },
        Position::from([1.0, 0.0, 0.0]),
        Sovereignty::default(),
        TechKnowledge::default(),
    )).id();

    // Colony only at capital
    spawn_test_colony(
        app.world_mut(),
        capital,
        Amt::units(100),
        Amt::units(100),
        vec![],
    );

    // Simulate tech researched
    {
        let empire = empire_entity(app.world_mut());
        app.world_mut()
            .get_mut::<RecentlyResearched>(empire)
            .unwrap()
            .techs
            .push(TechId(300));
    }

    advance_time(&mut app, 1);

    // No PendingKnowledgePropagation entities should exist for uncolonized system
    let pending_count = app
        .world_mut()
        .query::<&PendingKnowledgePropagation>()
        .iter(app.world())
        .count();
    assert_eq!(
        pending_count, 0,
        "No propagation should be created for uncolonized systems"
    );
}

// #76: Light-speed command delay

/// When a PendingShipCommand is created with arrives_at in the future,
/// the ship should NOT change state until the clock reaches arrives_at.
#[test]
fn test_remote_command_has_light_delay() {
    let mut app = test_app();

    // Player at system A (origin), ship at system B (10 ly away)
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
        [10.0, 0.0, 0.0],
        Habitability::Adequate,
        true,
        false,
    );
    let sys_c = spawn_test_system(
        app.world_mut(),
        "System-C",
        [12.0, 0.0, 0.0],
        Habitability::Adequate,
        true,
        false,
    );

    // Spawn player at system A
    app.world_mut().spawn((Player, StationedAt { system: sys_a }));

    // Spawn explorer at system B with FTL range
    let ship_entity = common::spawn_test_ship(
        app.world_mut(),
        "Explorer-1",
        ShipType::Explorer,
        sys_b,
        [10.0, 0.0, 0.0],
    );
    // Give it FTL range to reach system C
    app.world_mut().get_mut::<Ship>(ship_entity).unwrap().ftl_range = 20.0;

    // Calculate expected delay: 10 ly -> 600 hexadies
    let expected_delay = light_delay_hexadies(10.0);
    assert_eq!(expected_delay, 600);

    // Simulate what the UI does: create a PendingShipCommand with light delay
    let current_time = 100;
    app.world_mut().resource_mut::<GameClock>().elapsed = current_time;

    app.world_mut().spawn(PendingShipCommand {
        ship: ship_entity,
        command: ShipCommand::FTLTo { destination: sys_c },
        arrives_at: current_time + expected_delay,
    });

    // Advance time but NOT past arrives_at
    advance_time(&mut app, 100);

    // Ship should still be docked — command hasn't arrived
    let state = app.world().get::<ShipState>(ship_entity).unwrap();
    assert!(
        matches!(state, ShipState::Docked { system } if *system == sys_b),
        "Ship should remain docked before command arrives"
    );

    // PendingShipCommand should still exist
    let pending_count = app
        .world_mut()
        .query::<&PendingShipCommand>()
        .iter(app.world())
        .count();
    assert_eq!(pending_count, 1, "Pending command should still exist");
}

/// When the player and ship are at the same system, command delay is 0
/// and the PendingShipCommand system executes immediately.
#[test]
fn test_pending_command_executes_on_arrival() {
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
        [5.0, 0.0, 0.0],
        Habitability::Adequate,
        true,
        false,
    );

    // Spawn player at system A
    app.world_mut().spawn((Player, StationedAt { system: sys_a }));

    // Spawn colony at sys_a so port check passes
    spawn_test_colony(
        app.world_mut(),
        sys_a,
        Amt::units(1000),
        Amt::units(1000),
        vec![],
    );

    // Spawn explorer at system A with FTL range
    let ship_entity = common::spawn_test_ship(
        app.world_mut(),
        "Explorer-1",
        ShipType::Explorer,
        sys_a,
        [0.0, 0.0, 0.0],
    );
    app.world_mut().get_mut::<Ship>(ship_entity).unwrap().ftl_range = 20.0;

    let current_time = 100;
    app.world_mut().resource_mut::<GameClock>().elapsed = current_time;

    // Create a PendingShipCommand with arrives_at = now (simulating 0 delay that
    // was routed through the pending system anyway, or a command that has arrived)
    let arrives_at = current_time + 10; // small delay
    app.world_mut().spawn(PendingShipCommand {
        ship: ship_entity,
        command: ShipCommand::FTLTo { destination: sys_b },
        arrives_at,
    });

    // Advance time past arrives_at
    advance_time(&mut app, 15);

    // Ship should now be in FTL
    let state = app.world().get::<ShipState>(ship_entity).unwrap();
    assert!(
        matches!(state, ShipState::InFTL { destination_system, .. } if *destination_system == sys_b),
        "Ship should be in FTL after pending command executes",
    );

    // PendingShipCommand should be despawned
    let pending_count = app
        .world_mut()
        .query::<&PendingShipCommand>()
        .iter(app.world())
        .count();
    assert_eq!(pending_count, 0, "Pending command should be consumed");
}

/// Verify that a PendingShipCommand for survey executes properly after delay.
#[test]
fn test_pending_survey_command_executes_after_delay() {
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
        [3.0, 0.0, 0.0],
        Habitability::Adequate,
        false, // unsurveyed
        false,
    );

    app.world_mut().spawn((Player, StationedAt { system: sys_a }));

    let ship_entity = common::spawn_test_ship(
        app.world_mut(),
        "Explorer-1",
        ShipType::Explorer,
        sys_b,
        [3.0, 0.0, 0.0],
    );

    let current_time = 100;
    app.world_mut().resource_mut::<GameClock>().elapsed = current_time;

    // 3 ly delay = 180 hexadies
    let delay = light_delay_hexadies(3.0);
    assert_eq!(delay, 180);

    app.world_mut().spawn(PendingShipCommand {
        ship: ship_entity,
        command: ShipCommand::Survey { target: sys_b },
        arrives_at: current_time + delay,
    });

    // Before arrival: ship still docked
    advance_time(&mut app, 100);
    let state = app.world().get::<ShipState>(ship_entity).unwrap();
    assert!(
        matches!(state, ShipState::Docked { .. }),
        "Ship should still be docked before command arrives"
    );

    // After arrival: ship surveying
    advance_time(&mut app, 100); // now at 300, arrives_at = 280
    let state = app.world().get::<ShipState>(ship_entity).unwrap();
    assert!(
        matches!(state, ShipState::Surveying { target_system, .. } if *target_system == sys_b),
        "Ship should be surveying after command arrives",
    );
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
        Habitability::Ideal,
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

// CRITICAL: Owner::Empire ships (#3)

#[test]
fn test_empire_owned_ships() {
    let mut app = test_app();

    let empire = empire_entity(app.world_mut());

    let sys = spawn_test_system(
        app.world_mut(),
        "Home System",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    // Spawn a ship with Owner::Empire
    let ship_entity = app.world_mut().spawn((
        Ship {
            name: "Imperial Scout".to_string(),
            ship_type: ShipType::Explorer,
            owner: Owner::Empire(empire),
            sublight_speed: 0.75,
            ftl_range: 10.0,
            hp: 50.0,
            max_hp: 50.0,
            player_aboard: false,
            home_port: sys,
        },
        ShipState::Docked { system: sys },
        Position::from([0.0, 0.0, 0.0]),
        CombatStats { attack: 5.0, defense: 5.0 },
        CommandQueue::default(),
        Cargo::default(),
    )).id();

    // Verify the owner is correctly set
    let ship = app.world().get::<Ship>(ship_entity).unwrap();
    assert_eq!(ship.owner, Owner::Empire(empire));
    assert!(ship.owner.is_empire());

    // Verify update_sovereignty works with empire-owned ships using full_test_app
    // which includes the update_sovereignty system
    let mut app2 = full_test_app();
    let empire2 = empire_entity(app2.world_mut());

    let sys2 = spawn_test_system(
        app2.world_mut(),
        "Sov System",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    // Spawn a colony to trigger sovereignty update
    spawn_test_colony(
        app2.world_mut(),
        sys2,
        Amt::units(100),
        Amt::units(100),
        vec![],
    );

    advance_time(&mut app2, 1);

    // Sovereignty should be set to the empire owner
    let sov = app2.world().get::<Sovereignty>(sys2).unwrap();
    assert_eq!(sov.owner, Some(Owner::Empire(empire2)));
}

// CRITICAL: GlobalParams on empire entity (#4)

#[test]
fn test_global_params_on_empire_entity() {
    let mut app = test_app();

    let empire = empire_entity(app.world_mut());
    let params = app.world().get::<technology::GlobalParams>(empire).unwrap();

    // Verify defaults
    assert_eq!(params.sublight_speed_bonus, 0.0);
    assert_eq!(params.ftl_speed_multiplier, 1.0);
    assert_eq!(params.ftl_range_bonus, 0.0);
    assert_eq!(params.survey_range_bonus, 0.0);
    assert_eq!(params.build_speed_multiplier, 1.0);
}

#[test]
fn test_ftl_range_bonus_extends_range() {
    let mut app = test_app();

    let empire = empire_entity(app.world_mut());

    // Set ftl_range_bonus to 5.0
    {
        let mut params = app.world_mut().get_mut::<technology::GlobalParams>(empire).unwrap();
        params.ftl_range_bonus = 5.0;
    }

    let sys_a = spawn_test_system(
        app.world_mut(),
        "Origin",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    // System at 12 LY -- within range (10 base + 5 bonus = 15)
    let sys_b = spawn_test_system(
        app.world_mut(),
        "Near",
        [12.0, 0.0, 0.0],
        Habitability::Adequate,
        true,
        false,
    );

    // Spawn ship with ftl_range = 10.0 and Owner::Empire
    let ship_entity = app.world_mut().spawn((
        Ship {
            name: "FTL Scout".to_string(),
            ship_type: ShipType::Explorer,
            owner: Owner::Empire(empire),
            sublight_speed: 0.75,
            ftl_range: 10.0,
            hp: 50.0,
            max_hp: 50.0,
            player_aboard: false,
            home_port: sys_a,
        },
        ShipState::Docked { system: sys_a },
        Position::from([0.0, 0.0, 0.0]),
        CombatStats { attack: 5.0, defense: 5.0 },
        CommandQueue::default(),
        Cargo::default(),
    )).id();

    // Issue FTL command via command queue
    {
        let mut queue = app.world_mut().get_mut::<CommandQueue>(ship_entity).unwrap();
        queue.commands.push(QueuedCommand::FTLTo {
            system: sys_b,
            expected_position: [0.0, 0.0, 0.0],
        });
    }

    advance_time(&mut app, 1);

    // Ship should be in FTL travel (InFTL state)
    let state = app.world().get::<ShipState>(ship_entity).unwrap();
    assert!(
        matches!(state, ShipState::InFTL { .. }),
        "Ship should be in FTL state when destination is within base + bonus range, got {:?}",
        std::mem::discriminant(state)
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
        Habitability::Ideal,
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

// MAJOR: sync_maintenance_modifiers ship maintenance (#7)

#[test]
fn test_ship_maintenance_synced_via_modifiers() {
    use common::spawn_test_ship;

    let mut app = test_app();

    let empire = empire_entity(app.world_mut());

    let sys = spawn_test_system(
        app.world_mut(),
        "Home System",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    // Spawn colony at the system
    let colony = spawn_test_colony(
        app.world_mut(),
        sys,
        Amt::units(500),
        Amt::units(500),
        vec![],
    );

    // Spawn an explorer ship docked at the colony system (home_port = sys)
    let ship_entity = spawn_test_ship(
        app.world_mut(),
        "Explorer-1",
        ShipType::Explorer,
        sys,
        [0.0, 0.0, 0.0],
    );
    // Set owner to empire
    {
        let mut ship = app.world_mut().get_mut::<Ship>(ship_entity).unwrap();
        ship.owner = Owner::Empire(empire);
    }

    // Advance 1 tick to run sync_maintenance_modifiers
    advance_time(&mut app, 1);

    // Check MaintenanceCost on colony has a ship maintenance modifier
    let maint = app.world().get::<MaintenanceCost>(colony).unwrap();
    let ship_maint_modifier = maint
        .energy_per_hexadies
        .modifiers()
        .iter()
        .find(|m| m.id.starts_with("ship_maint_"));
    assert!(
        ship_maint_modifier.is_some(),
        "Colony MaintenanceCost should have a ship maintenance modifier"
    );

    // Explorer maintenance is 0.5 E/hd = Amt(500)
    let modifier = ship_maint_modifier.unwrap();
    assert_eq!(
        modifier.base_add,
        macrocosmo::amount::SignedAmt::from_amt(Amt::new(0, 500)),
        "Ship maintenance modifier should match Explorer maintenance cost (0.5 E/hd)"
    );
}

// Job auto-assignment (#87)

#[test]
fn test_job_auto_assignment() {
    use macrocosmo::species::*;

    let mut app = test_app();

    let sys = common::spawn_test_system(
        app.world_mut(),
        "Job Test",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    // Spawn a colony with population 10, job slots [miner:5, farmer:5]
    let planet_sys = find_planet(app.world_mut(), sys);
    let colony = app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.01,
        },
        ResourceStockpile {
            minerals: Amt::units(100),
            energy: Amt::units(100),
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::units(5)),
            energy_per_hexadies: ModifiedValue::new(Amt::units(5)),
            research_per_hexadies: ModifiedValue::new(Amt::units(1)),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![None; 4] },
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
        ColonyPopulation {
            species: vec![ColonySpecies {
                species_id: "human".to_string(),
                population: 10,
            }],
        },
        ColonyJobs {
            slots: vec![
                JobSlot {
                    job_id: "miner".to_string(),
                    capacity: 5,
                    assigned: 0,
                },
                JobSlot {
                    job_id: "farmer".to_string(),
                    capacity: 5,
                    assigned: 0,
                },
            ],
        },
    )).id();

    // Run one update to trigger sync_job_assignment
    advance_time(&mut app, 1);

    // Verify all 10 pops are assigned
    let jobs = app.world().get::<ColonyJobs>(colony).unwrap();
    assert_eq!(jobs.total_employed(), 10);
    assert_eq!(jobs.slots[0].assigned, 5); // miner full
    assert_eq!(jobs.slots[1].assigned, 5); // farmer full

    // Now reduce population to 7
    app.world_mut().get_mut::<ColonyPopulation>(colony).unwrap().species[0].population = 7;

    advance_time(&mut app, 1);

    // Verify assignment adjusts: miner=5, farmer=2
    let jobs = app.world().get::<ColonyJobs>(colony).unwrap();
    assert_eq!(jobs.total_employed(), 7);
    assert_eq!(jobs.slots[0].assigned, 5); // miner still full
    assert_eq!(jobs.slots[1].assigned, 2); // farmer reduced

    // Reduce population to 3
    app.world_mut().get_mut::<ColonyPopulation>(colony).unwrap().species[0].population = 3;

    advance_time(&mut app, 1);

    // Verify: miner=3, farmer=0
    let jobs = app.world().get::<ColonyJobs>(colony).unwrap();
    assert_eq!(jobs.total_employed(), 3);
    assert_eq!(jobs.slots[0].assigned, 3);
    assert_eq!(jobs.slots[1].assigned, 0);
}

#[test]
fn test_job_auto_assignment_excess_population() {
    use macrocosmo::species::*;

    let mut app = test_app();

    let sys = common::spawn_test_system(
        app.world_mut(),
        "Excess Pop",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    let planet_sys = find_planet(app.world_mut(), sys);
    let colony = app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 15.0,
            growth_rate: 0.01,
        },
        ResourceStockpile {
            minerals: Amt::units(100),
            energy: Amt::units(100),
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::units(5)),
            energy_per_hexadies: ModifiedValue::new(Amt::units(5)),
            research_per_hexadies: ModifiedValue::new(Amt::units(1)),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings { slots: vec![None; 4] },
        BuildingQueue::default(),
        ProductionFocus::default(),
        MaintenanceCost::default(),
        FoodConsumption::default(),
        ColonyPopulation {
            species: vec![ColonySpecies {
                species_id: "human".to_string(),
                population: 15,
            }],
        },
        ColonyJobs {
            slots: vec![
                JobSlot {
                    job_id: "miner".to_string(),
                    capacity: 5,
                    assigned: 0,
                },
                JobSlot {
                    job_id: "farmer".to_string(),
                    capacity: 5,
                    assigned: 0,
                },
            ],
        },
    )).id();

    advance_time(&mut app, 1);

    // 15 pop but only 10 capacity -> 10 employed, 5 unemployed
    let jobs = app.world().get::<ColonyJobs>(colony).unwrap();
    let pop = app.world().get::<ColonyPopulation>(colony).unwrap();
    assert_eq!(jobs.total_employed(), 10);
    assert_eq!(pop.total() - jobs.total_employed(), 5); // 5 unemployed
}

// #79: Ship scrapping (recycling)

#[test]
fn test_scrap_ship_refund_amounts() {
    // Verify scrap_refund returns 50% of build_cost for all ship types
    let (m, e) = ShipType::Explorer.build_cost();
    assert_eq!(m, Amt::units(200));
    assert_eq!(e, Amt::units(100));
    let (rm, re) = ShipType::Explorer.scrap_refund();
    assert_eq!(rm, Amt::units(100));
    assert_eq!(re, Amt::units(50));

    let (m, e) = ShipType::ColonyShip.build_cost();
    assert_eq!(m, Amt::units(500));
    assert_eq!(e, Amt::units(300));
    let (rm, re) = ShipType::ColonyShip.scrap_refund();
    assert_eq!(rm, Amt::units(250));
    assert_eq!(re, Amt::units(150));

    let (m, e) = ShipType::Courier.build_cost();
    assert_eq!(m, Amt::units(100));
    assert_eq!(e, Amt::units(50));
    let (rm, re) = ShipType::Courier.scrap_refund();
    assert_eq!(rm, Amt::units(50));
    assert_eq!(re, Amt::units(25));
}

#[test]
fn test_scrap_ship_despawns_entity() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Sol",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    let ship = common::spawn_test_ship(
        app.world_mut(),
        "Doomed-1",
        ShipType::Courier,
        sys,
        [0.0, 0.0, 0.0],
    );

    // Despawn the ship (simulating scrap action)
    app.world_mut().despawn(ship);

    // Verify ship is gone
    assert!(app.world().get_entity(ship).is_err());
}

// =========================================================================
// Resource depletion alerts (#80)
// =========================================================================

#[test]
fn test_scrap_ship_refunds_resources() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Sol",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    let colony = spawn_test_colony(
        app.world_mut(),
        sys,
        Amt::units(100),
        Amt::units(100),
        vec![None; 4],
    );

    let ship = common::spawn_test_ship(
        app.world_mut(),
        "Explorer-1",
        ShipType::Explorer,
        sys,
        [0.0, 0.0, 0.0],
    );

    // Get refund amounts
    let (refund_m, refund_e) = ShipType::Explorer.scrap_refund();
    assert_eq!(refund_m, Amt::units(100));
    assert_eq!(refund_e, Amt::units(50));

    // Apply refund to colony stockpile
    {
        let mut stockpile = app.world_mut().get_mut::<ResourceStockpile>(colony).unwrap();
        stockpile.minerals = stockpile.minerals.add(refund_m);
        stockpile.energy = stockpile.energy.add(refund_e);
    }

    // Despawn ship
    app.world_mut().despawn(ship);

    // Verify resources were added
    let stockpile = app.world().get::<ResourceStockpile>(colony).unwrap();
    assert_eq!(stockpile.minerals, Amt::units(200)); // 100 + 100 refund
    assert_eq!(stockpile.energy, Amt::units(150));   // 100 + 50 refund

    // Verify ship is gone
    assert!(app.world().get_entity(ship).is_err());
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
        Habitability::Ideal,
        true,
        true,
    );

    // Colony with food = 0
    let planet_sys = find_planet(app.world_mut(), sys);
    let _colony = app.world_mut().spawn((
        Colony { planet: planet_sys, population: 100.0, growth_rate: 0.01 },
        ResourceStockpile {
            minerals: Amt::units(500),
            energy: Amt::units(500),
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
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
        Habitability::Ideal,
        true,
        true,
    );

    // Colony with energy = 0
    let planet_sys = find_planet(app.world_mut(), sys);
    let _colony = app.world_mut().spawn((
        Colony { planet: planet_sys, population: 100.0, growth_rate: 0.01 },
        ResourceStockpile {
            minerals: Amt::units(500),
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
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
        Habitability::Ideal,
        true,
        true,
    );
    // Colony with food = 0 and no food production
    let planet_sys = find_planet(app.world_mut(), sys);
    let _colony = app.world_mut().spawn((
        Colony { planet: planet_sys, population: 100.0, growth_rate: 0.01 },
        ResourceStockpile {
            minerals: Amt::units(500),
            energy: Amt::units(500),
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
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

// #93: Lua-defined star and planet types

/// Generate a galaxy with star/planet type registries and verify that all stars
/// and planets have their type fields set.
#[test]
fn test_galaxy_generation_uses_types() {
    use macrocosmo::scripting::galaxy_api::{
        PlanetTypeDefinition, PlanetTypeRegistry, ResourceBias, StarTypeDefinition,
        StarTypeRegistry,
    };

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);

    // Insert registries with test data
    let mut star_reg = StarTypeRegistry::default();
    star_reg.types.push(StarTypeDefinition {
        id: "test_star".to_string(),
        name: "Test Star".to_string(),
        color: [1.0, 1.0, 1.0],
        planet_lambda: 2.0,
        max_planets: 5,
        habitability_bonus: 0.0,
        weight: 1.0,
    });
    app.insert_resource(star_reg);

    let mut planet_reg = PlanetTypeRegistry::default();
    planet_reg.types.push(PlanetTypeDefinition {
        id: "test_planet".to_string(),
        name: "Test Planet".to_string(),
        base_habitability: 0.7,
        base_slots: 4,
        resource_bias: ResourceBias {
            minerals: 1.0,
            energy: 1.0,
            research: 1.0,
        },
        weight: 1.0,
    });
    app.insert_resource(planet_reg);

    // Run generate_galaxy as a one-shot system
    app.add_systems(Startup, macrocosmo::galaxy::generate_galaxy);
    app.update();

    // Verify all stars have star_type set
    let star_count = app
        .world_mut()
        .query::<&StarSystem>()
        .iter(app.world())
        .count();
    assert!(star_count > 0, "Should have generated star systems");

    for star in app.world_mut().query::<&StarSystem>().iter(app.world()) {
        assert_eq!(
            star.star_type, "test_star",
            "All stars should have star_type 'test_star'"
        );
    }

    // Verify all planets have planet_type set
    let planet_count = app
        .world_mut()
        .query::<&Planet>()
        .iter(app.world())
        .count();
    assert!(planet_count > 0, "Should have generated planets");

    for planet in app.world_mut().query::<&Planet>().iter(app.world()) {
        assert_eq!(
            planet.planet_type, "test_planet",
            "All planets should have planet_type 'test_planet'"
        );
    }
}

#[test]
fn test_system_modifiers_on_star_systems() {
    use macrocosmo::scripting::galaxy_api::{
        PlanetTypeDefinition, PlanetTypeRegistry, ResourceBias, StarTypeDefinition,
        StarTypeRegistry,
    };

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);

    let mut star_reg = StarTypeRegistry::default();
    star_reg.types.push(StarTypeDefinition {
        id: "test_star".to_string(),
        name: "Test Star".to_string(),
        color: [1.0, 1.0, 1.0],
        planet_lambda: 2.0,
        max_planets: 3,
        habitability_bonus: 0.0,
        weight: 1.0,
    });
    app.insert_resource(star_reg);

    let mut planet_reg = PlanetTypeRegistry::default();
    planet_reg.types.push(PlanetTypeDefinition {
        id: "test_planet".to_string(),
        name: "Test Planet".to_string(),
        base_habitability: 0.7,
        base_slots: 4,
        resource_bias: ResourceBias {
            minerals: 1.0,
            energy: 1.0,
            research: 1.0,
        },
        weight: 1.0,
    });
    app.insert_resource(planet_reg);

    app.add_systems(Startup, macrocosmo::galaxy::generate_galaxy);
    app.update();

    // Every star system should have a SystemModifiers component
    let star_count = app
        .world_mut()
        .query::<&StarSystem>()
        .iter(app.world())
        .count();
    assert!(star_count > 0, "Should have generated star systems");

    let modifiers_count = app
        .world_mut()
        .query::<(&StarSystem, &SystemModifiers)>()
        .iter(app.world())
        .count();
    assert_eq!(
        star_count, modifiers_count,
        "Every star system should have a SystemModifiers component"
    );
}
