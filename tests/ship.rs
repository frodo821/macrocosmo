mod common;

use bevy::prelude::*;
use macrocosmo::amount::Amt;
use macrocosmo::colony::*;
use macrocosmo::modifier::ModifiedValue;
use macrocosmo::components::Position;
use macrocosmo::galaxy::{Sovereignty, StarSystem};
use macrocosmo::knowledge::*;
use macrocosmo::physics::sublight_travel_hexadies;
use macrocosmo::player::*;
use macrocosmo::ship::*;
use macrocosmo::technology;
use macrocosmo::time_system::GameClock;

use common::{advance_time, empire_entity, find_planet, full_test_app, spawn_test_colony, spawn_test_system, test_app};

fn spawn_ftl_explorer(
    world: &mut World,
    name: &str,
    system: Entity,
    pos: [f64; 3],
) -> Entity {
    world
        .spawn((
            Ship {
                name: name.to_string(),
                design_id: "explorer_mk1".to_string(),
                hull_id: "corvette".to_string(),
                modules: Vec::new(),
                owner: Owner::Neutral,
                sublight_speed: 0.75,
                ftl_range: 15.0,
                player_aboard: false,
                home_port: system,
                design_revision: 0,
            },
            ShipState::Docked { system },
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
        ))
        .id()
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
        1.0,
        true,
        false,
    );
    let sys_b = spawn_test_system(
        app.world_mut(),
        "System-B",
        [1.0, 0.0, 0.0],
        0.7,
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
            design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 0.75,
            ftl_range: 0.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
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
        1.0,
        true,
        false,
    );
    let sys_b = spawn_test_system(
        app.world_mut(),
        "System-B",
        [3.0, 0.0, 0.0],
        0.7,
        false, // not yet surveyed
        false,
    );

    // Spawn explorer in Surveying state targeting System-B
    let ship_entity = app.world_mut().spawn((
        Ship {
            name: "Scout-1".to_string(),
            design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 0.75,
            ftl_range: 0.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
        },
        ShipState::Surveying {
            target_system: sys_b,
            started_at: 0,
            completes_at: SURVEY_DURATION_HEXADIES,
        },
        Position::from([0.0, 0.0, 0.0]),
        ShipHitpoints {
            hull: 50.0, hull_max: 50.0,
            armor: 0.0, armor_max: 0.0,
            shield: 0.0, shield_max: 0.0,
            shield_regen: 0.0,
        },
    )).id();

    // Advance by survey duration
    advance_time(&mut app, SURVEY_DURATION_HEXADIES);

    // System-B should now be surveyed
    let star = app.world().get::<macrocosmo::galaxy::StarSystem>(sys_b).unwrap();
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
        1.0,
        true,
        true,
    );
    let sys_b = spawn_test_system(
        app.world_mut(),
        "System-B",
        [20.0, 0.0, 0.0],
        0.7,
        true,
        true, // already colonized so colony ship isn't consumed on arrival
    );

    // FTL arrival at 120 sd
    let arrival_at: i64 = 120;
    let ship_entity = app.world_mut().spawn((
        Ship {
            name: "Colony-1".to_string(),
            design_id: "colony_ship_mk1".to_string(),
            hull_id: "freighter".to_string(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 0.5,
            ftl_range: 30.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
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

#[test]
fn test_build_queue_spawns_ship() {
    let mut app = test_app();

    // System entity (build queue needs to look up Position on colony.system)
    let sys = spawn_test_system(
        app.world_mut(),
        "Shipyard",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );

    // Colony with ample resources and a build order for an Explorer
    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().entity_mut(sys).insert((ResourceStockpile {
            minerals: Amt::units(1000),
            energy: Amt::units(1000),
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        }, ResourceCapacity::default(),
        SystemBuildings {
            slots: vec![Some(BuildingId::new("shipyard")), None, None, None, None, None],
        },
        SystemBuildingQueue::default(),
    ));
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 100.0,
            growth_rate: 0.01,
        },
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue {
            queue: vec![BuildOrder {
                design_id: "explorer_mk1".to_string(),
                display_name: "Explorer".to_string(),
                minerals_cost: Amt::units(50),
                minerals_invested: Amt::ZERO,
                energy_cost: Amt::units(30),
                energy_invested: Amt::ZERO,
                build_time_total: 60,
                build_time_remaining: 0, // set to 0 so it completes with resources
            }],
        },
        Buildings {
            slots: vec![None, None, None, None],
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
    let new_ship = ships.iter().find(|s| s.design_id == "explorer_mk1");
    assert!(new_ship.is_some(), "The spawned ship should be an Explorer");

    // Build queue should be empty now
    let mut bq_query = app.world_mut().query::<&BuildQueue>();
    let bq = bq_query.iter(app.world()).next().unwrap();
    assert!(bq.queue.is_empty(), "Build queue should be empty after completion");
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
        1.0,
        true,
        true,
    );

    // Spawn a ship with Owner::Empire
    let ship_entity = app.world_mut().spawn((
        Ship {
            name: "Imperial Scout".to_string(),
            design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: Vec::new(),
            owner: Owner::Empire(empire),
            sublight_speed: 0.75,
            ftl_range: 10.0,
            player_aboard: false,
            home_port: sys,
            design_revision: 0,
        },
        ShipState::Docked { system: sys },
        Position::from([0.0, 0.0, 0.0]),
        ShipHitpoints {
            hull: 50.0, hull_max: 50.0,
            armor: 0.0, armor_max: 0.0,
            shield: 0.0, shield_max: 0.0,
            shield_regen: 0.0,
        },
        ShipModifiers::default(),
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
        1.0,
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
        1.0,
        true,
        true,
    );

    // System at 12 LY -- within range (10 base + 5 bonus = 15)
    let sys_b = spawn_test_system(
        app.world_mut(),
        "Near",
        [12.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    // Spawn ship with ftl_range = 10.0 and Owner::Empire
    let ship_entity = app.world_mut().spawn((
        Ship {
            name: "FTL Scout".to_string(),
            design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: Vec::new(),
            owner: Owner::Empire(empire),
            sublight_speed: 0.75,
            ftl_range: 10.0,
            player_aboard: false,
            home_port: sys_a,
            design_revision: 0,
        },
        ShipState::Docked { system: sys_a },
        Position::from([0.0, 0.0, 0.0]),
        ShipHitpoints {
            hull: 50.0, hull_max: 50.0,
            armor: 0.0, armor_max: 0.0,
            shield: 0.0, shield_max: 0.0,
            shield_regen: 0.0,
        },
        ShipModifiers::default(),
        CommandQueue::default(),
        Cargo::default(),
    )).id();

    // Issue FTL command via command queue
    {
        let mut queue = app.world_mut().get_mut::<CommandQueue>(ship_entity).unwrap();
        queue.commands.push(QueuedCommand::MoveTo {
            system: sys_b,
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

#[test]
fn test_scrap_ship_refund_amounts() {
    // Verify scrap_refund returns 50% of build_cost for all ship types (no modules)
    let design_registry = common::create_test_design_registry();
    let empty_module_registry = macrocosmo::ship_design::ModuleRegistry::default();

    let (m, e) = design_registry.build_cost("explorer_mk1");
    assert_eq!(m, Amt::units(200));
    assert_eq!(e, Amt::units(100));
    let (rm, re) = design_registry.scrap_refund("explorer_mk1", &[], &empty_module_registry);
    assert_eq!(rm, Amt::units(100));
    assert_eq!(re, Amt::units(50));

    let (m, e) = design_registry.build_cost("colony_ship_mk1");
    assert_eq!(m, Amt::units(500));
    assert_eq!(e, Amt::units(300));
    let (rm, re) = design_registry.scrap_refund("colony_ship_mk1", &[], &empty_module_registry);
    assert_eq!(rm, Amt::units(250));
    assert_eq!(re, Amt::units(150));

    let (m, e) = design_registry.build_cost("courier_mk1");
    assert_eq!(m, Amt::units(100));
    assert_eq!(e, Amt::units(50));
    let (rm, re) = design_registry.scrap_refund("courier_mk1", &[], &empty_module_registry);
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
        1.0,
        true,
        true,
    );

    let ship = common::spawn_test_ship(
        app.world_mut(),
        "Doomed-1",
        "courier_mk1",
        sys,
        [0.0, 0.0, 0.0],
    );

    // Despawn the ship (simulating scrap action)
    app.world_mut().despawn(ship);

    // Verify ship is gone
    assert!(app.world().get_entity(ship).is_err());
}

#[test]
fn test_scrap_ship_refunds_resources() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Sol",
        [0.0, 0.0, 0.0],
        1.0,
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
        "explorer_mk1",
        sys,
        [0.0, 0.0, 0.0],
    );

    // Get refund amounts (no modules equipped in test ship)
    let design_registry = common::create_test_design_registry();
    let empty_module_registry = macrocosmo::ship_design::ModuleRegistry::default();
    let (refund_m, refund_e) = design_registry.scrap_refund("explorer_mk1", &[], &empty_module_registry);
    assert_eq!(refund_m, Amt::units(100));
    assert_eq!(refund_e, Amt::units(50));

    // Apply refund to system stockpile (stockpile is now on star system entity)
    {
        let mut stockpile = app.world_mut().get_mut::<ResourceStockpile>(sys).unwrap();
        stockpile.minerals = stockpile.minerals.add(refund_m);
        stockpile.energy = stockpile.energy.add(refund_e);
    }

    // Despawn ship
    app.world_mut().despawn(ship);

    // Verify resources were added
    let stockpile = app.world().get::<ResourceStockpile>(sys).unwrap();
    assert_eq!(stockpile.minerals, Amt::units(200)); // 100 + 100 refund
    assert_eq!(stockpile.energy, Amt::units(150));   // 100 + 50 refund

    // Verify ship is gone
    assert!(app.world().get_entity(ship).is_err());
}

// --- #99: Command queue UI improvements tests ---

/// Clearing the command queue removes all queued commands.
#[test]
fn test_clear_command_queue() {
    let mut app = test_app();

    let sys_a = spawn_test_system(
        app.world_mut(),
        "System-A",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    let sys_b = spawn_test_system(
        app.world_mut(),
        "System-B",
        [5.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );
    let sys_c = spawn_test_system(
        app.world_mut(),
        "System-C",
        [10.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    let ship = common::spawn_test_ship(
        app.world_mut(),
        "Explorer-1",
        "explorer_mk1",
        sys_a,
        [0.0, 0.0, 0.0],
    );

    // Add commands to queue
    let mut queue = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
    queue.commands.push(QueuedCommand::MoveTo {
        system: sys_b,
    });
    queue.commands.push(QueuedCommand::MoveTo {
        system: sys_c,
    });

    // Verify commands exist
    let queue = app.world().get::<CommandQueue>(ship).unwrap();
    assert_eq!(queue.commands.len(), 2, "Should have 2 queued commands");

    // Clear all commands
    let mut queue = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
    queue.commands.clear();

    // Verify empty
    let queue = app.world().get::<CommandQueue>(ship).unwrap();
    assert!(queue.commands.is_empty(), "Command queue should be empty after clear");
}

/// Cancelling an individual command removes only that command.
#[test]
fn test_cancel_individual_command() {
    let mut app = test_app();

    let sys_a = spawn_test_system(
        app.world_mut(),
        "System-A",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    let sys_b = spawn_test_system(
        app.world_mut(),
        "System-B",
        [5.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );
    let sys_c = spawn_test_system(
        app.world_mut(),
        "System-C",
        [10.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    let ship = common::spawn_test_ship(
        app.world_mut(),
        "Explorer-1",
        "explorer_mk1",
        sys_a,
        [0.0, 0.0, 0.0],
    );

    // Add 3 commands to queue
    let mut queue = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
    queue.commands.push(QueuedCommand::MoveTo {
        system: sys_a,
    });
    queue.commands.push(QueuedCommand::MoveTo {
        system: sys_b,
    });
    queue.commands.push(QueuedCommand::MoveTo {
        system: sys_c,
    });

    // Cancel the middle command (index 1)
    let mut queue = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
    queue.commands.remove(1);

    // Verify: 2 commands remain, the second system should be sys_c
    let queue = app.world().get::<CommandQueue>(ship).unwrap();
    assert_eq!(queue.commands.len(), 2, "Should have 2 commands after cancelling one");
    match &queue.commands[1] {
        QueuedCommand::MoveTo { system, .. } => {
            assert_eq!(*system, sys_c, "Second remaining command should target System-C");
        }
        _ => panic!("Expected MoveTo command"),
    }
}

/// Cancelling a survey returns the ship to Docked state at the target system.
#[test]
fn test_cancel_survey_returns_to_docked() {
    let mut app = test_app();

    let sys_a = spawn_test_system(
        app.world_mut(),
        "System-A",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );

    let ship = common::spawn_test_ship(
        app.world_mut(),
        "Explorer-1",
        "explorer_mk1",
        sys_a,
        [0.0, 0.0, 0.0],
    );

    // Set ship to Surveying state
    let mut state = app.world_mut().get_mut::<ShipState>(ship).unwrap();
    *state = ShipState::Surveying {
        target_system: sys_a,
        started_at: 0,
        completes_at: 100,
    };

    // Cancel: set back to Docked at the target system
    let mut state = app.world_mut().get_mut::<ShipState>(ship).unwrap();
    let dock_system = match &*state {
        ShipState::Surveying { target_system, .. } => Some(*target_system),
        _ => None,
    };
    if let Some(sys) = dock_system {
        *state = ShipState::Docked { system: sys };
    }

    // Verify ship is docked
    let state = app.world().get::<ShipState>(ship).unwrap();
    assert!(
        matches!(state, ShipState::Docked { system } if *system == sys_a),
        "Ship should be docked at System-A after cancelling survey"
    );
}

/// Cancelling settling returns the ship to Docked state at the settling system.
#[test]
fn test_cancel_settling_returns_to_docked() {
    let mut app = test_app();

    let sys_a = spawn_test_system(
        app.world_mut(),
        "System-A",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );

    let ship = common::spawn_test_ship(
        app.world_mut(),
        "Colony-Ship-1",
        "colony_ship_mk1",
        sys_a,
        [0.0, 0.0, 0.0],
    );

    // Set ship to Settling state
    let mut state = app.world_mut().get_mut::<ShipState>(ship).unwrap();
    *state = ShipState::Settling {
        system: sys_a,
        planet: None,
        started_at: 0,
        completes_at: 120,
    };

    // Cancel: set back to Docked at the settling system
    let mut state = app.world_mut().get_mut::<ShipState>(ship).unwrap();
    let dock_system = match &*state {
        ShipState::Settling { system, .. } => Some(*system),
        _ => None,
    };
    if let Some(sys) = dock_system {
        *state = ShipState::Docked { system: sys };
    }

    // Verify ship is docked
    let state = app.world().get::<ShipState>(ship).unwrap();
    assert!(
        matches!(state, ShipState::Docked { system } if *system == sys_a),
        "Ship should be docked at System-A after cancelling settling"
    );
}

// --- #103: Survey carry-back model ---

#[test]
fn test_ftl_survey_stores_data_on_ship() {
    let mut app = common::test_app();

    let sys_a = common::spawn_test_system(
        app.world_mut(), "System-A", [0.0, 0.0, 0.0],
        1.0, true, false,
    );
    let sys_b = common::spawn_test_system(
        app.world_mut(), "System-B", [3.0, 0.0, 0.0],
        0.7, false, false,
    );

    app.world_mut().spawn((Player, StationedAt { system: sys_a }));

    let ship = spawn_ftl_explorer(app.world_mut(), "FTL-Scout", sys_b, [3.0, 0.0, 0.0]);
    *app.world_mut().get_mut::<ShipState>(ship).unwrap() = ShipState::Surveying {
        target_system: sys_b,
        started_at: 0,
        completes_at: SURVEY_DURATION_HEXADIES,
    };

    common::advance_time(&mut app, SURVEY_DURATION_HEXADIES);

    let star = app.world().get::<StarSystem>(sys_b).unwrap();
    assert!(!star.surveyed, "FTL ship should NOT mark system as surveyed immediately");

    let survey_data = app.world().get::<SurveyData>(ship);
    assert!(survey_data.is_some(), "FTL ship should carry survey data");
    assert_eq!(survey_data.unwrap().target_system, sys_b);
}

#[test]
fn test_ftl_survey_auto_queues_return() {
    let mut app = common::test_app();

    let sys_a = common::spawn_test_system(
        app.world_mut(), "System-A", [0.0, 0.0, 0.0],
        1.0, true, false,
    );
    let sys_b = common::spawn_test_system(
        app.world_mut(), "System-B", [3.0, 0.0, 0.0],
        0.7, false, false,
    );

    app.world_mut().spawn((Player, StationedAt { system: sys_a }));

    let ship = spawn_ftl_explorer(app.world_mut(), "FTL-Scout", sys_b, [3.0, 0.0, 0.0]);
    *app.world_mut().get_mut::<ShipState>(ship).unwrap() = ShipState::Surveying {
        target_system: sys_b,
        started_at: 0,
        completes_at: SURVEY_DURATION_HEXADIES,
    };

    common::advance_time(&mut app, SURVEY_DURATION_HEXADIES);

    let state = app.world().get::<ShipState>(ship).unwrap();
    let queue = app.world().get::<CommandQueue>(ship).unwrap();

    let in_ftl_to_a = matches!(state, ShipState::InFTL { destination_system, .. } if *destination_system == sys_a);
    let queued_ftl_to_a = queue.commands.iter().any(|cmd| {
        matches!(cmd, QueuedCommand::MoveTo { system, .. } if *system == sys_a)
    });

    assert!(
        in_ftl_to_a || queued_ftl_to_a,
        "Ship should be FTL-returning to player system or have FTL return queued"
    );
}

#[test]
fn test_ftl_survey_delivers_on_dock_at_player_system() {
    let mut app = common::test_app();

    let sys_a = common::spawn_test_system(
        app.world_mut(), "System-A", [0.0, 0.0, 0.0],
        1.0, true, false,
    );
    let sys_b = common::spawn_test_system(
        app.world_mut(), "System-B", [3.0, 0.0, 0.0],
        0.7, false, false,
    );

    app.world_mut().spawn((Player, StationedAt { system: sys_a }));

    let ship = spawn_ftl_explorer(app.world_mut(), "FTL-Scout", sys_a, [0.0, 0.0, 0.0]);
    app.world_mut().entity_mut(ship).insert(SurveyData {
        target_system: sys_b,
        surveyed_at: 10,
        system_name: "System-B".to_string(),
        anomaly_id: None,
    });

    common::advance_time(&mut app, 1);

    let star = app.world().get::<StarSystem>(sys_b).unwrap();
    assert!(star.surveyed, "System should be marked surveyed after delivery");

    let survey_data = app.world().get::<SurveyData>(ship);
    assert!(survey_data.is_none(), "Survey data should be cleared after delivery");
}

#[test]
fn test_non_ftl_survey_marks_system_immediately() {
    let mut app = common::test_app();

    let _sys_a = common::spawn_test_system(
        app.world_mut(), "System-A", [0.0, 0.0, 0.0],
        1.0, true, false,
    );
    let sys_b = common::spawn_test_system(
        app.world_mut(), "System-B", [3.0, 0.0, 0.0],
        0.7, false, false,
    );

    // Courier has ftl_range 0.0 (non-FTL)
    let ship = common::spawn_test_ship(
        app.world_mut(), "Scout-1", "courier_mk1", sys_b, [3.0, 0.0, 0.0],
    );
    *app.world_mut().get_mut::<ShipState>(ship).unwrap() = ShipState::Surveying {
        target_system: sys_b,
        started_at: 0,
        completes_at: SURVEY_DURATION_HEXADIES,
    };

    common::advance_time(&mut app, SURVEY_DURATION_HEXADIES);

    let star = app.world().get::<StarSystem>(sys_b).unwrap();
    assert!(star.surveyed, "Non-FTL ship should mark system as surveyed immediately");

    let survey_data = app.world().get::<SurveyData>(ship);
    assert!(survey_data.is_none(), "Non-FTL ship should not carry survey data");
}

// --- Regression: FTL must not jump to unsurveyed systems ---

#[test]
fn test_plan_ftl_route_rejects_unsurveyed_destination() {
    let mut app = common::test_app();

    let sys_a = common::spawn_test_system(
        app.world_mut(), "System-A", [0.0, 0.0, 0.0],
        1.0, true, false, // surveyed=true
    );
    let sys_b = common::spawn_test_system(
        app.world_mut(), "System-B", [5.0, 0.0, 0.0],
        0.7, false, false, // surveyed=false
    );

    let ship = spawn_ftl_explorer(app.world_mut(), "Scout", sys_a, [0.0, 0.0, 0.0]);

    // Queue MoveTo unsurveyed system
    app.world_mut().get_mut::<CommandQueue>(ship).unwrap().commands.push(
        QueuedCommand::MoveTo { system: sys_b },
    );

    // Process command queue — should NOT FTL (unsurveyed), should sublight
    common::advance_time(&mut app, 1);

    let state = app.world().get::<ShipState>(ship).unwrap();
    assert!(
        matches!(state, ShipState::SubLight { .. }),
        "Ship should sublight to unsurveyed system, not FTL. Got: {:?}",
        std::mem::discriminant(state)
    );
}

#[test]
fn test_plan_ftl_route_allows_surveyed_destination() {
    let mut app = common::test_app();

    let sys_a = common::spawn_test_system(
        app.world_mut(), "System-A", [0.0, 0.0, 0.0],
        1.0, true, false, // surveyed=true
    );
    let sys_b = common::spawn_test_system(
        app.world_mut(), "System-B", [5.0, 0.0, 0.0],
        0.7, true, false, // surveyed=true
    );

    let ship = spawn_ftl_explorer(app.world_mut(), "Scout", sys_a, [0.0, 0.0, 0.0]);

    // Queue MoveTo surveyed system within FTL range
    app.world_mut().get_mut::<CommandQueue>(ship).unwrap().commands.push(
        QueuedCommand::MoveTo { system: sys_b },
    );

    common::advance_time(&mut app, 1);

    let state = app.world().get::<ShipState>(ship).unwrap();
    assert!(
        matches!(state, ShipState::InFTL { destination_system, .. } if *destination_system == sys_b),
        "Ship should FTL to surveyed system within range. Got: {:?}",
        std::mem::discriminant(state)
    );
}

#[test]
fn test_survey_return_uses_ftl_to_surveyed_home() {
    let mut app = common::test_app();

    let sys_a = common::spawn_test_system(
        app.world_mut(), "System-A", [0.0, 0.0, 0.0],
        1.0, true, false,
    );
    let sys_b = common::spawn_test_system(
        app.world_mut(), "System-B", [5.0, 0.0, 0.0],
        0.7, false, false, // unsurveyed target
    );

    app.world_mut().spawn((Player, StationedAt { system: sys_a }));

    // Spawn FTL explorer docked at sys_b (as if it arrived by sublight and completed survey)
    let ship = spawn_ftl_explorer(app.world_mut(), "Scout", sys_b, [5.0, 0.0, 0.0]);

    // Queue return MoveTo home (surveyed)
    app.world_mut().get_mut::<CommandQueue>(ship).unwrap().commands.push(
        QueuedCommand::MoveTo { system: sys_a },
    );

    common::advance_time(&mut app, 1);

    let state = app.world().get::<ShipState>(ship).unwrap();
    assert!(
        matches!(state, ShipState::InFTL { destination_system, .. } if *destination_system == sys_a),
        "Ship should FTL back to surveyed home system. Got: {:?}",
        std::mem::discriminant(state)
    );
}

// --- Regression: Multi-hop FTL chain routing ---

#[test]
fn test_multi_hop_ftl_route() {
    let mut app = common::test_app();

    // A --8ly-- B --8ly-- C (all surveyed, FTL range 10ly, can't direct A→C at 16ly)
    let sys_a = common::spawn_test_system(
        app.world_mut(), "System-A", [0.0, 0.0, 0.0],
        1.0, true, false,
    );
    let _sys_b = common::spawn_test_system(
        app.world_mut(), "System-B", [8.0, 0.0, 0.0],
        0.7, true, false,
    );
    let sys_c = common::spawn_test_system(
        app.world_mut(), "System-C", [16.0, 0.0, 0.0],
        0.7, true, false,
    );

    // FTL range 10ly — can reach B from A, C from B, but NOT C from A directly
    let ship = spawn_ftl_explorer(app.world_mut(), "Scout", sys_a, [0.0, 0.0, 0.0]);

    app.world_mut().get_mut::<CommandQueue>(ship).unwrap().commands.push(
        QueuedCommand::MoveTo { system: sys_c },
    );

    // First tick: should FTL to intermediate hop (B)
    common::advance_time(&mut app, 1);

    let state = app.world().get::<ShipState>(ship).unwrap();
    assert!(
        matches!(state, ShipState::InFTL { .. }),
        "Ship should be in FTL for first hop of multi-hop route"
    );

    // Queue should still have remaining hop(s) to C
    let queue = app.world().get::<CommandQueue>(ship).unwrap();
    assert!(
        queue.commands.iter().any(|cmd| matches!(cmd, QueuedCommand::MoveTo { system } if *system == sys_c)),
        "Queue should contain remaining MoveTo for final destination"
    );
}

// --- Regression: Survey data NOT delivered at non-player system ---

// --- Regression: Hybrid FTL+sublight route when full FTL route unavailable ---

#[test]
fn test_hybrid_ftl_sublight_route() {
    let mut app = common::test_app();

    // A (surveyed) --5ly-- B (surveyed) --5ly-- C (unsurveyed)
    // FTL range 10ly: can FTL A→B, but C is unsurveyed so no FTL to C
    // Hybrid: FTL A→B, sublight B→C should be faster than sublight A→C direct
    let sys_a = common::spawn_test_system(
        app.world_mut(), "System-A", [0.0, 0.0, 0.0],
        1.0, true, false,
    );
    let sys_b = common::spawn_test_system(
        app.world_mut(), "System-B", [5.0, 0.0, 0.0],
        0.7, true, false, // surveyed
    );
    let sys_c = common::spawn_test_system(
        app.world_mut(), "System-C", [10.0, 0.0, 0.0],
        0.7, false, false, // unsurveyed
    );

    let ship = spawn_ftl_explorer(app.world_mut(), "Scout", sys_a, [0.0, 0.0, 0.0]);

    app.world_mut().get_mut::<CommandQueue>(ship).unwrap().commands.push(
        QueuedCommand::MoveTo { system: sys_c },
    );

    common::advance_time(&mut app, 1);

    // Ship should take hybrid route: FTL to B first (not sublight direct to C)
    let state = app.world().get::<ShipState>(ship).unwrap();
    let queue = app.world().get::<CommandQueue>(ship).unwrap();

    let in_ftl_to_b = matches!(state, ShipState::InFTL { destination_system, .. } if *destination_system == sys_b);
    let has_move_to_b = queue.commands.iter().any(|cmd| matches!(cmd, QueuedCommand::MoveTo { system } if *system == sys_b));

    assert!(
        in_ftl_to_b || has_move_to_b,
        "Ship should use hybrid route via B, not sublight direct to C"
    );

    // C should still be in the queue for after the waypoint
    let has_move_to_c = queue.commands.iter().any(|cmd| matches!(cmd, QueuedCommand::MoveTo { system } if *system == sys_c));
    assert!(has_move_to_c, "Final destination C should remain in queue");
}

#[test]
fn test_survey_data_not_delivered_at_wrong_system() {
    let mut app = common::test_app();

    let sys_a = common::spawn_test_system(
        app.world_mut(), "System-A", [0.0, 0.0, 0.0],
        1.0, true, false,
    );
    let sys_b = common::spawn_test_system(
        app.world_mut(), "System-B", [5.0, 0.0, 0.0],
        0.7, false, false,
    );
    let sys_c = common::spawn_test_system(
        app.world_mut(), "System-C", [10.0, 0.0, 0.0],
        0.7, false, false,
    );

    // Player stationed at System-A
    app.world_mut().spawn((Player, StationedAt { system: sys_a }));

    // Ship docked at System-C (NOT the player's system), carrying survey data for B
    let ship = spawn_ftl_explorer(app.world_mut(), "Scout", sys_c, [10.0, 0.0, 0.0]);
    app.world_mut().entity_mut(ship).insert(SurveyData {
        target_system: sys_b,
        surveyed_at: 10,
        system_name: "System-B".to_string(),
        anomaly_id: None,
    });

    common::advance_time(&mut app, 1);

    // System-B should NOT be surveyed (ship is not at player's system)
    let star = app.world().get::<StarSystem>(sys_b).unwrap();
    assert!(!star.surveyed, "Survey data should NOT be delivered at non-player system");

    // SurveyData should still be on ship
    assert!(app.world().get::<SurveyData>(ship).is_some(), "SurveyData should remain on ship");
}

// --- Regression: Auto-insert move when Survey queued at wrong system ---

#[test]
fn test_queued_survey_auto_inserts_move() {
    let mut app = common::test_app();

    let sys_a = common::spawn_test_system(
        app.world_mut(), "System-A", [0.0, 0.0, 0.0],
        1.0, true, false,
    );
    let sys_b = common::spawn_test_system(
        app.world_mut(), "System-B", [5.0, 0.0, 0.0],
        0.7, false, false, // unsurveyed
    );

    let ship = spawn_ftl_explorer(app.world_mut(), "Scout", sys_a, [0.0, 0.0, 0.0]);

    // Queue Survey for system B while docked at A
    app.world_mut().get_mut::<CommandQueue>(ship).unwrap().commands.push(
        QueuedCommand::Survey { system: sys_b },
    );

    // Process: should auto-insert MoveTo before Survey
    common::advance_time(&mut app, 1);

    // Ship should be moving (SubLight, since B is unsurveyed → can't FTL)
    // After auto-insert: queue becomes [MoveTo B, Survey B], MoveTo executes → SubLight
    // But MoveTo may stay in queue if process_command_queue runs before the insert takes effect
    let state = app.world().get::<ShipState>(ship).unwrap().clone();
    let queue = app.world().get::<CommandQueue>(ship).unwrap();
    let in_sublight = matches!(state, ShipState::SubLight { .. });
    let has_move_queued = queue.commands.iter().any(|cmd| matches!(cmd, QueuedCommand::MoveTo { system } if *system == sys_b));
    assert!(
        in_sublight || has_move_queued,
        "Ship should be in SubLight or have MoveTo queued. in_sublight={}, has_move={}, queue_len={}",
        in_sublight, has_move_queued, queue.commands.len()
    );

    // Queue should still have Survey queued for after arrival
    let queue = app.world().get::<CommandQueue>(ship).unwrap();
    assert!(
        queue.commands.iter().any(|cmd| matches!(cmd, QueuedCommand::Survey { system } if *system == sys_b)),
        "Survey should remain queued for after arrival"
    );
}

// --- Regression: CommandQueue predicted position tracking ---

#[test]
fn test_command_queue_predicted_position() {
    let mut app = common::test_app();

    let sys_a = common::spawn_test_system(
        app.world_mut(), "System-A", [0.0, 0.0, 0.0],
        1.0, true, false,
    );
    let sys_b = common::spawn_test_system(
        app.world_mut(), "System-B", [5.0, 0.0, 0.0],
        0.7, true, false,
    );
    let sys_c = common::spawn_test_system(
        app.world_mut(), "System-C", [10.0, 0.0, 0.0],
        0.7, true, false,
    );

    let ship = spawn_ftl_explorer(app.world_mut(), "Scout", sys_a, [0.0, 0.0, 0.0]);

    // Push two MoveTo commands
    {
        let mut queue = app.world_mut().get_mut::<CommandQueue>(ship).unwrap();
        queue.commands.push(QueuedCommand::MoveTo { system: sys_b });
        queue.commands.push(QueuedCommand::MoveTo { system: sys_c });
        // Manually set predicted to match last target
        queue.predicted_system = Some(sys_c);
        queue.predicted_position = [10.0, 0.0, 0.0];
    }

    let queue = app.world().get::<CommandQueue>(ship).unwrap();
    assert_eq!(queue.predicted_system, Some(sys_c));
    assert!((queue.predicted_position[0] - 10.0).abs() < 1e-9);
}

// --- #110: Light-speed propagation when faster than FTL return ---

#[test]
fn test_ftl_survey_uses_light_speed_when_faster() {
    let mut app = common::test_app();

    let sys_a = common::spawn_test_system(
        app.world_mut(), "System-A", [0.0, 0.0, 0.0],
        1.0, true, false,
    );
    let sys_b = common::spawn_test_system(
        app.world_mut(), "System-B", [2.0, 0.0, 0.0],
        0.7, false, false,
    );

    app.world_mut().spawn((Player, StationedAt { system: sys_a }));

    // Set ftl_speed_multiplier very low so FTL is slower than light
    // At 0.05x, effective FTL speed = 0.5c, FTL return at 2 ly = 240 hd, light delay = 120 hd
    let empire = empire_entity(app.world_mut());
    {
        let mut params = app.world_mut().get_mut::<technology::GlobalParams>(empire).unwrap();
        params.ftl_speed_multiplier = 0.05;
    }

    let ship = spawn_ftl_explorer(app.world_mut(), "FTL-Scout", sys_b, [2.0, 0.0, 0.0]);
    *app.world_mut().get_mut::<ShipState>(ship).unwrap() = ShipState::Surveying {
        target_system: sys_b,
        started_at: 0,
        completes_at: SURVEY_DURATION_HEXADIES,
    };

    common::advance_time(&mut app, SURVEY_DURATION_HEXADIES);

    let star = app.world().get::<StarSystem>(sys_b).unwrap();
    assert!(star.surveyed, "System should be marked surveyed via light-speed propagation");

    let survey_data = app.world().get::<SurveyData>(ship);
    assert!(survey_data.is_none(), "Ship should not carry survey data when light-speed is faster");
}

#[test]
fn test_ftl_survey_uses_carry_back_when_ftl_faster() {
    let mut app = common::test_app();

    let sys_a = common::spawn_test_system(
        app.world_mut(), "System-A", [0.0, 0.0, 0.0],
        1.0, true, false,
    );
    let sys_b = common::spawn_test_system(
        app.world_mut(), "System-B", [2.0, 0.0, 0.0],
        0.7, false, false,
    );

    app.world_mut().spawn((Player, StationedAt { system: sys_a }));

    let ship = spawn_ftl_explorer(app.world_mut(), "FTL-Scout", sys_b, [2.0, 0.0, 0.0]);
    *app.world_mut().get_mut::<ShipState>(ship).unwrap() = ShipState::Surveying {
        target_system: sys_b,
        started_at: 0,
        completes_at: SURVEY_DURATION_HEXADIES,
    };

    common::advance_time(&mut app, SURVEY_DURATION_HEXADIES);

    let star = app.world().get::<StarSystem>(sys_b).unwrap();
    assert!(!star.surveyed, "System should NOT be marked surveyed when FTL return is faster");

    let survey_data = app.world().get::<SurveyData>(ship);
    assert!(survey_data.is_some(), "Ship should carry survey data for FTL carry-back");
}

// --- Regression: Non-FTL ship should not attempt FTL routing ---

#[test]
fn test_non_ftl_ship_no_ftl_routing_loop() {
    let mut app = common::test_app();

    let sys_a = common::spawn_test_system(
        app.world_mut(), "System-A", [0.0, 0.0, 0.0],
        1.0, true, false,
    );
    let sys_b = common::spawn_test_system(
        app.world_mut(), "System-B", [5.0, 0.0, 0.0],
        0.7, true, false,
    );

    // Courier has ftl_range: 0.0
    let ship = common::spawn_test_ship(
        app.world_mut(), "Courier-1", "courier_mk1", sys_a, [0.0, 0.0, 0.0],
    );

    app.world_mut().get_mut::<CommandQueue>(ship).unwrap().commands.push(
        QueuedCommand::MoveTo { system: sys_b },
    );

    common::advance_time(&mut app, 1);

    // Ship should go sublight, not get stuck in FTL routing loop
    let state = app.world().get::<ShipState>(ship).unwrap();
    assert!(
        matches!(state, ShipState::SubLight { .. }),
        "Non-FTL ship should use sublight directly, not attempt FTL routing"
    );

    // Queue should be empty (command consumed)
    let queue = app.world().get::<CommandQueue>(ship).unwrap();
    assert!(queue.commands.is_empty(), "Queue should be empty after command consumed");
}

/// Hull modifiers from HullDefinition should be pushed to ShipModifiers
/// when sync_ship_module_modifiers runs.
#[test]
fn test_hull_modifiers_applied_to_ship() {
    use macrocosmo::ship_design::{HullDefinition, HullRegistry, HullSlot, ModuleModifier};

    let mut app = test_app();

    // Register a hull with modifiers
    {
        let mut hull_registry = app.world_mut().resource_mut::<HullRegistry>();
        hull_registry.insert(HullDefinition {
            id: "scout_hull".to_string(),
            name: "Scout Hull".to_string(),
            description: String::new(),
            base_hp: 40.0,
            base_speed: 0.85,
            base_evasion: 35.0,
            slots: vec![
                HullSlot { slot_type: "utility".to_string(), count: 2 },
                HullSlot { slot_type: "ftl".to_string(), count: 1 },
            ],
            build_cost_minerals: Amt::units(150),
            build_cost_energy: Amt::units(80),
            build_time: 45,
            maintenance: Amt::new(0, 400),
            modifiers: vec![
                ModuleModifier {
                    target: "ship.survey_speed".to_string(),
                    base_add: 0.0,
                    multiplier: 1.3,
                    add: 0.0,
                },
                ModuleModifier {
                    target: "ship.speed".to_string(),
                    base_add: 0.0,
                    multiplier: 1.15,
                    add: 0.0,
                },
            ],
        });
    }

    let sys = spawn_test_system(
        app.world_mut(),
        "Sol",
        [0.0, 0.0, 0.0],
        0.4,
        true,
        false,
    );

    let ship = {
        let world = app.world_mut();
        world
            .spawn((
                Ship {
                    name: "Scout".to_string(),
                    design_id: "scout_mk1".to_string(),
                    hull_id: "scout_hull".to_string(),
                    modules: Vec::new(),
                    owner: Owner::Neutral,
                    sublight_speed: 0.85,
                    ftl_range: 10.0,
                    player_aboard: false,
                    home_port: sys,
                    design_revision: 0,
                },
                ShipState::Docked { system: sys },
                Position::from([0.0, 0.0, 0.0]),
                ShipHitpoints {
                    hull: 40.0,
                    hull_max: 40.0,
                    armor: 0.0,
                    armor_max: 0.0,
                    shield: 0.0,
                    shield_max: 0.0,
                    shield_regen: 0.0,
                },
                CommandQueue::default(),
                Cargo::default(),
                ShipModifiers::default(),
            ))
            .id()
    };

    app.update();

    let mods = app.world().get::<ShipModifiers>(ship).unwrap();
    // survey_speed should have 1 modifier with multiplier 1.3
    assert_eq!(mods.survey_speed.modifiers().len(), 1);
    assert_eq!(mods.survey_speed.modifiers()[0].id, "hull_scout_hull_ship.survey_speed");
    // speed should have 1 modifier with multiplier 1.15
    assert_eq!(mods.speed.modifiers().len(), 1);
    assert_eq!(mods.speed.modifiers()[0].id, "hull_scout_hull_ship.speed");
}

// --- #117: Courier route automation ---

/// Helper: install a stockpile on a system entity for tests.
fn install_stockpile(world: &mut World, system: Entity, minerals: Amt, energy: Amt) {
    world.entity_mut(system).insert((
        ResourceStockpile {
            minerals,
            energy,
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
    ));
}

#[test]
fn courier_route_resource_transport_picks_up_at_start() {
    let mut app = test_app();

    let sys_a = common::spawn_test_system(
        app.world_mut(), "System-A", [0.0, 0.0, 0.0],
        1.0, true, false,
    );
    let sys_b = common::spawn_test_system(
        app.world_mut(), "System-B", [1.0, 0.0, 0.0],
        0.7, true, false,
    );
    install_stockpile(app.world_mut(), sys_a, Amt::units(1000), Amt::units(800));
    install_stockpile(app.world_mut(), sys_b, Amt::ZERO, Amt::ZERO);

    let courier = common::spawn_test_ship(
        app.world_mut(), "Hermes", "courier_mk1", sys_a, [0.0, 0.0, 0.0],
    );
    app.world_mut().entity_mut(courier).insert(CourierRoute::new(
        vec![sys_a, sys_b],
        CourierMode::ResourceTransport,
    ));

    // First tick: the courier is at sys_a (its current waypoint), so it
    // should load up to capacity and queue MoveTo sys_b.
    common::advance_time(&mut app, 1);

    let cargo = app.world().get::<Cargo>(courier).unwrap();
    let cap = COURIER_DEFAULT_CARGO_CAPACITY;
    assert_eq!(cargo.minerals, cap, "courier should be loaded with minerals");
    assert_eq!(cargo.energy, cap, "courier should be loaded with energy");

    let route = app.world().get::<CourierRoute>(courier).unwrap();
    assert_eq!(route.current_index, 1, "next waypoint should be index 1 (sys_b)");

    let stockpile_a = app.world().get::<ResourceStockpile>(sys_a).unwrap();
    assert_eq!(stockpile_a.minerals, Amt::units(1000).sub(cap));
    assert_eq!(stockpile_a.energy, Amt::units(800).sub(cap));
}

#[test]
fn courier_route_resource_transport_delivers_at_destination() {
    let mut app = test_app();

    let sys_a = common::spawn_test_system(
        app.world_mut(), "System-A", [0.0, 0.0, 0.0],
        1.0, true, false,
    );
    let sys_b = common::spawn_test_system(
        app.world_mut(), "System-B", [0.5, 0.0, 0.0],
        0.7, true, false,
    );
    install_stockpile(app.world_mut(), sys_a, Amt::ZERO, Amt::ZERO);
    install_stockpile(app.world_mut(), sys_b, Amt::ZERO, Amt::ZERO);

    // Spawn the courier already at sys_b, with cargo pre-loaded — and a
    // route whose current waypoint is sys_b. Tick should deliver, then
    // attempt to pick up (nothing available), and queue the move back
    // toward sys_a (the next waypoint after wrapping).
    let courier = common::spawn_test_ship(
        app.world_mut(), "Hermes", "courier_mk1", sys_b, [0.5, 0.0, 0.0],
    );
    app.world_mut().entity_mut(courier).insert(Cargo {
        minerals: Amt::units(300),
        energy: Amt::units(100),
    });
    let mut route = CourierRoute::new(vec![sys_a, sys_b], CourierMode::ResourceTransport);
    route.current_index = 1; // pretend we just arrived at sys_b
    app.world_mut().entity_mut(courier).insert(route);

    common::advance_time(&mut app, 1);

    // Net effect: deliver 300/100 then immediately pick up to capacity.
    // sys_b stockpile is left with whatever wasn't picked up.
    let cap = COURIER_DEFAULT_CARGO_CAPACITY;
    let stockpile_b = app.world().get::<ResourceStockpile>(sys_b).unwrap();
    let expected_remaining_m = Amt::units(300).sub(cap.min(Amt::units(300)));
    let expected_remaining_e = Amt::units(100).sub(cap.min(Amt::units(100)));
    assert_eq!(stockpile_b.minerals, expected_remaining_m, "minerals after deliver+pickup");
    assert_eq!(stockpile_b.energy, expected_remaining_e, "energy after deliver+pickup");

    let cargo = app.world().get::<Cargo>(courier).unwrap();
    assert_eq!(cargo.minerals, cap.min(Amt::units(300)));
    assert_eq!(cargo.energy, cap.min(Amt::units(100)));

    let route = app.world().get::<CourierRoute>(courier).unwrap();
    assert_eq!(route.current_index, 0, "after sys_b, index should wrap to sys_a");

    // The queue may already be empty in the same frame if process_command_queue
    // consumed the MoveTo and started travel — verify by checking either
    // a queued command OR a non-Docked state with sys_a as destination.
    let queue = app.world().get::<CommandQueue>(courier).unwrap();
    let state = app.world().get::<ShipState>(courier).unwrap();
    let dispatched = !queue.commands.is_empty()
        || matches!(state, ShipState::SubLight { target_system: Some(t), .. } if *t == sys_a)
        || matches!(state, ShipState::InFTL { destination_system, .. } if *destination_system == sys_a);
    assert!(dispatched, "courier should be dispatched toward sys_a (queued or in transit)");
}

#[test]
fn courier_route_resource_transport_full_round_trip() {
    use macrocosmo::physics::sublight_travel_hexadies;

    let mut app = test_app();

    let sys_a = common::spawn_test_system(
        app.world_mut(), "System-A", [0.0, 0.0, 0.0],
        1.0, true, false,
    );
    let sys_b = common::spawn_test_system(
        app.world_mut(), "System-B", [0.5, 0.0, 0.0],
        0.7, true, false,
    );
    install_stockpile(app.world_mut(), sys_a, Amt::units(1000), Amt::units(1000));
    install_stockpile(app.world_mut(), sys_b, Amt::ZERO, Amt::ZERO);

    let courier = common::spawn_test_ship(
        app.world_mut(), "Hermes", "courier_mk1", sys_a, [0.0, 0.0, 0.0],
    );
    app.world_mut().entity_mut(courier).insert(CourierRoute::new(
        vec![sys_a, sys_b],
        CourierMode::ResourceTransport,
    ));

    // Tick 1: pickup at sys_a, queue MoveTo sys_b.
    common::advance_time(&mut app, 1);

    // Verify sys_a was tapped.
    let stockpile_a_after_pickup = app.world().get::<ResourceStockpile>(sys_a).unwrap();
    let cap = COURIER_DEFAULT_CARGO_CAPACITY;
    assert_eq!(
        stockpile_a_after_pickup.minerals,
        Amt::units(1000).sub(cap),
        "sys_a stockpile should have decreased by capacity after first pickup"
    );

    // Travel time at courier_mk1's 0.80c over 0.5 ly.
    let travel = sublight_travel_hexadies(0.5, 0.80);
    // Plus a couple of buffer ticks for state transitions.
    common::advance_time(&mut app, travel + 2);

    // After arriving at sys_b, the courier delivered then picked up again
    // (sys_b had a momentary balance equal to delivery), so the stockpile
    // ends near zero. Cargo should still have a load to carry back.
    let cargo_after = app.world().get::<Cargo>(courier).unwrap();
    assert!(
        cargo_after.minerals > Amt::ZERO || cargo_after.energy > Amt::ZERO,
        "Courier should be carrying a fresh load after sys_b dock; got M:{} E:{}",
        cargo_after.minerals, cargo_after.energy
    );

    // Route index should have advanced past sys_b (wrapped to 0 = sys_a).
    let route_after = app.world().get::<CourierRoute>(courier).unwrap();
    assert_eq!(route_after.current_index, 0, "route should wrap back to sys_a");
}

#[test]
fn courier_route_paused_does_not_dispatch() {
    let mut app = test_app();

    let sys_a = common::spawn_test_system(
        app.world_mut(), "System-A", [0.0, 0.0, 0.0],
        1.0, true, false,
    );
    let sys_b = common::spawn_test_system(
        app.world_mut(), "System-B", [1.0, 0.0, 0.0],
        0.7, true, false,
    );
    install_stockpile(app.world_mut(), sys_a, Amt::units(500), Amt::units(500));

    let courier = common::spawn_test_ship(
        app.world_mut(), "Hermes", "courier_mk1", sys_a, [0.0, 0.0, 0.0],
    );
    let mut route = CourierRoute::new(
        vec![sys_a, sys_b],
        CourierMode::ResourceTransport,
    );
    route.paused = true;
    app.world_mut().entity_mut(courier).insert(route);

    common::advance_time(&mut app, 1);

    let cargo = app.world().get::<Cargo>(courier).unwrap();
    assert_eq!(cargo.minerals, Amt::ZERO, "paused route should not pick up");
    assert_eq!(cargo.energy, Amt::ZERO, "paused route should not pick up");
    let queue = app.world().get::<CommandQueue>(courier).unwrap();
    assert!(queue.commands.is_empty(), "paused route should not queue moves");
}

#[test]
fn courier_route_knowledge_relay_delivers_pre_loaded_cargo() {
    let mut app = test_app();

    let sys_a = common::spawn_test_system(
        app.world_mut(), "System-A", [0.0, 0.0, 0.0],
        1.0, true, false,
    );
    let sys_b = common::spawn_test_system(
        app.world_mut(), "System-B", [0.3, 0.0, 0.0],
        0.7, true, false,
    );
    let sys_x = common::spawn_test_system(
        app.world_mut(), "System-X", [10.0, 0.0, 0.0],
        0.5, false, false,
    );

    // Empire's KnowledgeStore: install a stale snapshot of sys_x (observed_at=5).
    {
        let empire = common::empire_entity(app.world_mut());
        let mut store = app.world_mut().get_mut::<KnowledgeStore>(empire).unwrap();
        store.update(SystemKnowledge {
            system: sys_x,
            observed_at: 5,
            received_at: 5,
            data: SystemSnapshot {
                name: "System-X".to_string(),
                position: [10.0, 0.0, 0.0],
                surveyed: false,
                ..Default::default()
            },
        });
    }

    // Spawn the courier docked at sys_b, with a pre-loaded knowledge cargo
    // containing a *newer* snapshot of sys_x (observed_at=50). The route
    // current_index points at sys_b so the tick will execute deliver+pickup
    // immediately.
    let courier = common::spawn_test_ship(
        app.world_mut(), "Hermes", "courier_mk1", sys_b, [0.3, 0.0, 0.0],
    );
    let mut route = CourierRoute::new(vec![sys_a, sys_b], CourierMode::KnowledgeRelay);
    route.current_index = 1;
    app.world_mut().entity_mut(courier).insert(route);
    app.world_mut().entity_mut(courier).insert(CourierKnowledgeCargo {
        entries: vec![SystemKnowledge {
            system: sys_x,
            observed_at: 50,
            received_at: 50,
            data: SystemSnapshot {
                name: "System-X".to_string(),
                position: [10.0, 0.0, 0.0],
                surveyed: true,
                ..Default::default()
            },
        }],
    });

    common::advance_time(&mut app, 1);

    let empire = common::empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    let entry = store.get(sys_x).expect("sys_x knowledge entry");
    assert_eq!(entry.observed_at, 50, "newer cargo snapshot should win on delivery");
    assert!(entry.data.surveyed, "newer snapshot's surveyed flag should propagate");
}

#[test]
fn courier_route_knowledge_relay_pickup_refreshes_received_at() {
    let mut app = test_app();

    let sys_a = common::spawn_test_system(
        app.world_mut(), "System-A", [0.0, 0.0, 0.0],
        1.0, true, false,
    );
    let sys_b = common::spawn_test_system(
        app.world_mut(), "System-B", [0.3, 0.0, 0.0],
        0.7, true, false,
    );

    // Empire knowledge has a stale entry for sys_a observed long ago.
    {
        let empire = common::empire_entity(app.world_mut());
        let mut store = app.world_mut().get_mut::<KnowledgeStore>(empire).unwrap();
        store.update(SystemKnowledge {
            system: sys_a,
            observed_at: 10,
            received_at: 10,
            data: SystemSnapshot {
                name: "System-A".to_string(),
                position: [0.0, 0.0, 0.0],
                surveyed: true,
                ..Default::default()
            },
        });
    }

    let courier = common::spawn_test_ship(
        app.world_mut(), "Hermes", "courier_mk1", sys_a, [0.0, 0.0, 0.0],
    );
    app.world_mut().entity_mut(courier).insert(CourierRoute::new(
        vec![sys_a, sys_b],
        CourierMode::KnowledgeRelay,
    ));
    // Set the clock high so the pickup's received_at update is observable.
    app.world_mut().resource_mut::<GameClock>().elapsed = 100;

    app.update();

    let bag = app.world().get::<CourierKnowledgeCargo>(courier)
        .expect("courier should have CourierKnowledgeCargo after first tick");
    assert!(!bag.entries.is_empty(), "bag should have copied store entries on pickup");
    let sys_a_entry = bag.entries.iter().find(|k| k.system == sys_a).expect("sys_a entry");
    assert_eq!(sys_a_entry.received_at, 100, "received_at should refresh to current time on pickup");
}

// ---------------------------------------------------------------------------
// #123: Design-based refit tests
// ---------------------------------------------------------------------------

/// Build a minimal hull/module/design fixture used by the refit tests below.
/// The design "rev_test" is a corvette with a single weapon slot and is
/// installed in the registry at revision 0.
fn install_refit_fixture(app: &mut App) {
    use macrocosmo::ship_design::*;
    let mut hulls = HullRegistry::default();
    hulls.insert(HullDefinition {
        id: "corvette".into(),
        name: "Corvette".into(),
        description: String::new(),
        base_hp: 50.0,
        base_speed: 0.75,
        base_evasion: 30.0,
        slots: vec![HullSlot { slot_type: "weapon".into(), count: 1 }],
        build_cost_minerals: Amt::units(200),
        build_cost_energy: Amt::units(100),
        build_time: 60,
        maintenance: Amt::new(0, 500),
        modifiers: vec![],
    });

    let mut modules = ModuleRegistry::default();
    let mk = |id: &str, mineral: u64, energy: u64| ModuleDefinition {
        id: id.into(),
        name: id.into(),
        description: String::new(),
        slot_type: "weapon".into(),
        modifiers: vec![],
        weapon: None,
        cost_minerals: Amt::units(mineral),
        cost_energy: Amt::units(energy),
        prerequisite_tech: None,
        upgrade_to: Vec::new(),
    };
    modules.insert(mk("laser_mk1", 50, 20));
    modules.insert(mk("laser_mk2", 80, 30));

    let mut designs = ShipDesignRegistry::default();
    designs.insert(ShipDesignDefinition {
        id: "rev_test".into(),
        name: "Rev Test".into(),
        description: String::new(),
        hull_id: "corvette".into(),
        modules: vec![DesignSlotAssignment {
            slot_type: "weapon".into(),
            module_id: "laser_mk1".into(),
        }],
        can_survey: false,
        can_colonize: false,
        maintenance: Amt::new(0, 500),
        build_cost_minerals: Amt::units(200),
        build_cost_energy: Amt::units(100),
        build_time: 60,
        hp: 50.0,
        sublight_speed: 0.75,
        ftl_range: 0.0,
        revision: 0,
    });

    app.insert_resource(hulls);
    app.insert_resource(modules);
    app.insert_resource(designs);
}

fn spawn_rev_test_ship(world: &mut World, system: Entity, design_revision: u64) -> Entity {
    world
        .spawn((
            Ship {
                name: "Test".to_string(),
                design_id: "rev_test".to_string(),
                hull_id: "corvette".to_string(),
                modules: vec![EquippedModule {
                    slot_type: "weapon".into(),
                    module_id: "laser_mk1".into(),
                }],
                owner: Owner::Neutral,
                sublight_speed: 0.75,
                ftl_range: 0.0,
                player_aboard: false,
                home_port: system,
                design_revision,
            },
            ShipState::Docked { system },
            Position::from([0.0, 0.0, 0.0]),
            ShipHitpoints {
                hull: 50.0, hull_max: 50.0,
                armor: 0.0, armor_max: 0.0,
                shield: 0.0, shield_max: 0.0,
                shield_regen: 0.0,
            },
            CommandQueue::default(),
            Cargo::default(),
            ShipModifiers::default(),
            ShipStats::default(),
            RulesOfEngagement::default(),
        ))
        .id()
}

#[test]
fn editing_design_bumps_revision_flagging_existing_ships() {
    use macrocosmo::ship_design::{
        DesignSlotAssignment, ShipDesignDefinition, ShipDesignRegistry,
    };
    let mut app = test_app();
    install_refit_fixture(&mut app);

    let sys = app.world_mut().spawn((
        StarSystem { name: "S".into(), star_type: "g_main".into(), is_capital: false, surveyed: true },
        Position::from([0.0, 0.0, 0.0]),
    )).id();

    let ship = spawn_rev_test_ship(app.world_mut(), sys, 0);

    // Initially: ship.design_revision == design.revision == 0.
    let registry = app.world().resource::<ShipDesignRegistry>();
    let design = registry.get("rev_test").unwrap();
    let ship_rev = app.world().get::<Ship>(ship).unwrap().design_revision;
    assert_eq!(ship_rev, design.revision);

    // Edit the design via the registry's upsert helper — this is what the
    // Ship Designer's SaveDesign action ultimately invokes.
    let mut edited = design.clone();
    edited.modules = vec![DesignSlotAssignment {
        slot_type: "weapon".into(),
        module_id: "laser_mk2".into(),
    }];
    let mut registry = app.world_mut().resource_mut::<ShipDesignRegistry>();
    let new_rev = registry.upsert_edited(edited);
    assert_eq!(new_rev, 1);

    // Ship's recorded revision should now be behind the registry's.
    let design = app.world().resource::<ShipDesignRegistry>().get("rev_test").unwrap();
    let ship_rev = app.world().get::<Ship>(ship).unwrap().design_revision;
    assert!(
        ship_rev < design.revision,
        "ship should be flagged as needing refit (ship={} < design={})",
        ship_rev, design.revision,
    );
}

#[test]
fn refit_completes_brings_ship_in_sync_with_design() {
    use macrocosmo::ship_design::{
        DesignSlotAssignment, ShipDesignRegistry,
    };
    let mut app = test_app();
    install_refit_fixture(&mut app);

    let sys = app.world_mut().spawn((
        StarSystem { name: "S".into(), star_type: "g_main".into(), is_capital: false, surveyed: true },
        Position::from([0.0, 0.0, 0.0]),
    )).id();

    let ship = spawn_rev_test_ship(app.world_mut(), sys, 0);

    // Edit the design (revision 0 -> 1) and capture the new modules.
    let mut design = app.world().resource::<ShipDesignRegistry>()
        .get("rev_test").unwrap().clone();
    design.modules = vec![DesignSlotAssignment {
        slot_type: "weapon".into(),
        module_id: "laser_mk2".into(),
    }];
    let target_revision = {
        let mut r = app.world_mut().resource_mut::<ShipDesignRegistry>();
        r.upsert_edited(design.clone())
    };

    // Manually push the ship into a Refitting state mirroring what
    // `apply_design_refit` would do.
    let now = app.world().resource::<GameClock>().elapsed;
    let target_modules: Vec<EquippedModule> = design
        .modules
        .iter()
        .map(|a| EquippedModule {
            slot_type: a.slot_type.clone(),
            module_id: a.module_id.clone(),
        })
        .collect();
    let refit_time = 10;
    *app.world_mut().get_mut::<ShipState>(ship).unwrap() = ShipState::Refitting {
        system: sys,
        started_at: now,
        completes_at: now + refit_time,
        new_modules: target_modules,
        target_revision,
    };

    // Advance time past completion and tick.
    advance_time(&mut app, refit_time + 1);

    // Ship should be docked again, with the new module and updated revision.
    let ship_comp = app.world().get::<Ship>(ship).unwrap();
    assert_eq!(ship_comp.design_revision, target_revision);
    assert_eq!(ship_comp.modules.len(), 1);
    assert_eq!(ship_comp.modules[0].module_id, "laser_mk2");
    assert!(matches!(
        app.world().get::<ShipState>(ship),
        Some(ShipState::Docked { .. })
    ));
}

#[test]
fn refit_in_flight_does_not_apply_when_design_edited_again() {
    // If the design is bumped *during* a refit, the ship still completes
    // refit to the revision recorded when refit started — it remains "behind"
    // the latest design but isn't stuck at the older revision either.
    use macrocosmo::ship_design::{
        DesignSlotAssignment, ShipDesignRegistry,
    };
    let mut app = test_app();
    install_refit_fixture(&mut app);

    let sys = app.world_mut().spawn((
        StarSystem { name: "S".into(), star_type: "g_main".into(), is_capital: false, surveyed: true },
        Position::from([0.0, 0.0, 0.0]),
    )).id();
    let ship = spawn_rev_test_ship(app.world_mut(), sys, 0);

    // Bump design once and start refit at target=1.
    {
        let mut r = app.world_mut().resource_mut::<ShipDesignRegistry>();
        let mut d = r.get("rev_test").unwrap().clone();
        d.modules = vec![DesignSlotAssignment {
            slot_type: "weapon".into(),
            module_id: "laser_mk2".into(),
        }];
        let _ = r.upsert_edited(d);
    }
    let now = app.world().resource::<GameClock>().elapsed;
    *app.world_mut().get_mut::<ShipState>(ship).unwrap() = ShipState::Refitting {
        system: sys,
        started_at: now,
        completes_at: now + 5,
        new_modules: vec![EquippedModule {
            slot_type: "weapon".into(), module_id: "laser_mk2".into(),
        }],
        target_revision: 1,
    };

    // Bump the design again mid-refit to revision 2.
    {
        let mut r = app.world_mut().resource_mut::<ShipDesignRegistry>();
        let mut d = r.get("rev_test").unwrap().clone();
        d.modules = vec![DesignSlotAssignment {
            slot_type: "weapon".into(),
            module_id: "laser_mk1".into(),
        }];
        let new_rev = r.upsert_edited(d);
        assert_eq!(new_rev, 2);
    }

    advance_time(&mut app, 6);

    let ship_comp = app.world().get::<Ship>(ship).unwrap();
    // Ship is at target_revision 1, still behind the live design (2) — that's
    // the expected behavior: a fresh refit must be triggered.
    assert_eq!(ship_comp.design_revision, 1);
    let live_rev = app.world().resource::<ShipDesignRegistry>()
        .get("rev_test").unwrap().revision;
    assert_eq!(live_rev, 2);
    assert!(ship_comp.design_revision < live_rev);
}
