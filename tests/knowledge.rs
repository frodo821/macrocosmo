mod common;

use bevy::prelude::*;
use macrocosmo::amount::Amt;
use macrocosmo::colony::*;
use macrocosmo::components::Position;
use macrocosmo::galaxy::{Habitability, Planet, ResourceLevel, Sovereignty, StarSystem, SystemAttributes};
use macrocosmo::knowledge::*;
use macrocosmo::physics::light_delay_hexadies;
use macrocosmo::player::*;
use macrocosmo::ship::*;
use macrocosmo::technology::TechKnowledge;

use common::{advance_time, empire_entity, find_planet, full_test_app, spawn_test_colony, spawn_test_system, test_app};

/// Helper: set up an app with tech research + propagation systems for knowledge tests.
fn tech_knowledge_app() -> App {
    let app = full_test_app();
    app
}

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
        "explorer_mk1",
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
    app.world_mut().resource_mut::<macrocosmo::time_system::GameClock>().elapsed = current_time;

    app.world_mut().spawn(PendingShipCommand {
        ship: ship_entity,
        command: ShipCommand::MoveTo { destination: sys_c },
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
        "explorer_mk1",
        sys_a,
        [0.0, 0.0, 0.0],
    );
    app.world_mut().get_mut::<Ship>(ship_entity).unwrap().ftl_range = 20.0;

    let current_time = 100;
    app.world_mut().resource_mut::<macrocosmo::time_system::GameClock>().elapsed = current_time;

    // Create a PendingShipCommand with arrives_at = now (simulating 0 delay that
    // was routed through the pending system anyway, or a command that has arrived)
    let arrives_at = current_time + 10; // small delay
    app.world_mut().spawn(PendingShipCommand {
        ship: ship_entity,
        command: ShipCommand::MoveTo { destination: sys_b },
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
        "explorer_mk1",
        sys_b,
        [3.0, 0.0, 0.0],
    );

    let current_time = 100;
    app.world_mut().resource_mut::<macrocosmo::time_system::GameClock>().elapsed = current_time;

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

/// EnqueueCommand on a despawned ship should not crash.
#[test]
fn test_enqueue_command_despawned_ship_no_crash() {
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
        [10.0, 0.0, 0.0],
        Habitability::Adequate,
        true,
        false,
    );

    app.world_mut().spawn((Player, StationedAt { system: sys_a }));

    let ship_entity = common::spawn_test_ship(
        app.world_mut(),
        "Doomed-Ship",
        "explorer_mk1",
        sys_a,
        [0.0, 0.0, 0.0],
    );

    let current_time = 100;
    app.world_mut().resource_mut::<macrocosmo::time_system::GameClock>().elapsed = current_time;

    // Queue an EnqueueCommand that arrives after delay
    app.world_mut().spawn(PendingShipCommand {
        ship: ship_entity,
        command: ShipCommand::EnqueueCommand(QueuedCommand::MoveTo { system: sys_b }),
        arrives_at: current_time + 50,
    });

    // Despawn the ship before the command arrives
    app.world_mut().despawn(ship_entity);

    // Advance past arrives_at — should not crash
    advance_time(&mut app, 100);

    // PendingShipCommand should be cleaned up
    let pending_count = app
        .world_mut()
        .query::<&PendingShipCommand>()
        .iter(app.world())
        .count();
    assert_eq!(pending_count, 0, "Pending command should be cleaned up");
}

/// EnqueueCommand should add to CommandQueue when ship is alive and in transit.
#[test]
fn test_enqueue_command_adds_to_queue() {
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
    let sys_c = spawn_test_system(
        app.world_mut(),
        "System-C",
        [10.0, 0.0, 0.0],
        Habitability::Adequate,
        true,
        false,
    );

    app.world_mut().spawn((Player, StationedAt { system: sys_a }));

    let ship_entity = common::spawn_test_ship(
        app.world_mut(),
        "Transit-Ship",
        "explorer_mk1",
        sys_a,
        [0.0, 0.0, 0.0],
    );

    // Put ship in FTL transit to sys_b (arrives well after the test ends)
    *app.world_mut().get_mut::<ShipState>(ship_entity).unwrap() = ShipState::InFTL {
        origin_system: sys_a,
        destination_system: sys_b,
        departed_at: 50,
        arrival_at: 9999,
    };

    let current_time = 100;
    app.world_mut().resource_mut::<macrocosmo::time_system::GameClock>().elapsed = current_time;

    // Queue an EnqueueCommand to move to sys_c after delay
    app.world_mut().spawn(PendingShipCommand {
        ship: ship_entity,
        command: ShipCommand::EnqueueCommand(QueuedCommand::MoveTo { system: sys_c }),
        arrives_at: current_time + 150,
    });

    // Before arrival: command queue should be empty
    advance_time(&mut app, 50);
    let queue = app.world().get::<CommandQueue>(ship_entity).unwrap();
    assert!(queue.commands.is_empty(), "Queue should be empty before command arrives");

    // After arrival: command should be in queue (ship still in FTL, so queue not consumed)
    advance_time(&mut app, 150);
    let queue = app.world().get::<CommandQueue>(ship_entity).unwrap();
    assert_eq!(queue.commands.len(), 1, "Queue should have 1 command after arrival");
    assert!(
        matches!(queue.commands[0], QueuedCommand::MoveTo { system } if system == sys_c),
        "Queued command should be MoveTo sys_c"
    );
}

// Technology knowledge propagation (#88)

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
            .push(TechId("social_xenolinguistics".into()));
    }

    // Run one update
    advance_time(&mut app, 1);

    // Capital should have the tech immediately
    let knowledge = app.world().get::<TechKnowledge>(capital).unwrap();
    assert!(
        knowledge.known_techs.contains(&TechId("social_xenolinguistics".into())),
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
            .push(TechId("physics_sensor_arrays".into()));
    }

    // First tick: propagation entities spawned
    advance_time(&mut app, 1);

    // Capital should have it immediately
    let capital_knowledge = app.world().get::<TechKnowledge>(capital).unwrap();
    assert!(capital_knowledge.known_techs.contains(&TechId("physics_sensor_arrays".into())));

    // Remote should NOT have it yet (need 60 hexadies for 1 LY)
    let remote_knowledge = app.world().get::<TechKnowledge>(remote).unwrap();
    assert!(
        !remote_knowledge.known_techs.contains(&TechId("physics_sensor_arrays".into())),
        "Remote system should not know tech before light delay"
    );

    // Advance to just before arrival (59 more hexadies, total elapsed = 60)
    advance_time(&mut app, 59);
    let remote_knowledge = app.world().get::<TechKnowledge>(remote).unwrap();
    assert!(
        !remote_knowledge.known_techs.contains(&TechId("physics_sensor_arrays".into())),
        "Remote system should not know tech at tick 60 (arrives_at = 60, spawned at tick 1)"
    );

    // Advance one more tick to reach arrival time
    advance_time(&mut app, 1);
    let remote_knowledge = app.world().get::<TechKnowledge>(remote).unwrap();
    assert!(
        remote_knowledge.known_techs.contains(&TechId("physics_sensor_arrays".into())),
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
            .push(TechId("industrial_automated_mining".into()));
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

// #176: Snapshot extended fields tests

#[test]
fn test_knowledge_snapshot_hostile_presence() {
    use macrocosmo::galaxy::HostilePresence;

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

    // Remote system at 1 LY with hostiles
    let sys_hostile = spawn_test_system(
        app.world_mut(),
        "Hostile System",
        [1.0, 0.0, 0.0],
        Habitability::Adequate,
        true,
        false,
    );

    // Spawn player
    app.world_mut().spawn((Player, StationedAt { system: sys_capital }));

    // Spawn hostile presence at remote system
    app.world_mut().spawn(HostilePresence {
        system: sys_hostile,
        strength: 5.0,
        hp: 100.0,
        max_hp: 100.0,
        hostile_type: macrocosmo::galaxy::HostileType::SpaceCreature,
        evasion: 0.0,
    });

    // Advance past light delay (1 LY = 60 hexadies)
    advance_time(&mut app, 61);

    let empire = empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    let knowledge = store.get(sys_hostile).unwrap();

    assert!(knowledge.data.has_hostile, "Should have hostile presence in snapshot");
    assert!((knowledge.data.hostile_strength - 5.0).abs() < 0.01, "Hostile strength should be 5.0");
}

#[test]
fn test_knowledge_snapshot_system_attributes() {
    let mut app = test_app();

    let sys_capital = spawn_test_system(
        app.world_mut(),
        "Capital",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    // Remote system at 1 LY with specific attributes
    let sys_remote = spawn_test_system(
        app.world_mut(),
        "Remote",
        [1.0, 0.0, 0.0],
        Habitability::Adequate,
        true,
        false,
    );

    app.world_mut().spawn((Player, StationedAt { system: sys_capital }));

    // Advance past light delay
    advance_time(&mut app, 61);

    let empire = empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    let knowledge = store.get(sys_remote).unwrap();

    // spawn_test_system creates planets with SystemAttributes containing the given habitability
    assert_eq!(knowledge.data.habitability, Some(Habitability::Adequate));
}

// #175: Ship knowledge tests

#[test]
fn test_ship_knowledge_propagation() {
    use macrocosmo::knowledge::ShipSnapshotState;

    let mut app = test_app();

    let sys_capital = spawn_test_system(
        app.world_mut(),
        "Capital",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    let sys_remote = spawn_test_system(
        app.world_mut(),
        "Remote",
        [1.0, 0.0, 0.0],
        Habitability::Adequate,
        true,
        false,
    );

    app.world_mut().spawn((Player, StationedAt { system: sys_capital }));

    // Spawn ship at remote system
    let ship_entity = common::spawn_test_ship(
        app.world_mut(),
        "Scout-1",
        "explorer_mk1",
        sys_remote,
        [1.0, 0.0, 0.0],
    );

    // Before light delay: no ship knowledge
    app.update();
    {
        let empire = empire_entity(app.world_mut());
        let store = app.world().get::<KnowledgeStore>(empire).unwrap();
        assert!(
            store.get_ship(ship_entity).is_none(),
            "Should have no ship knowledge before light delay"
        );
    }

    // After light delay (1 LY = 60 hexadies)
    advance_time(&mut app, 61);

    {
        let empire = empire_entity(app.world_mut());
        let store = app.world().get::<KnowledgeStore>(empire).unwrap();
        let ship_snap = store.get_ship(ship_entity);
        assert!(ship_snap.is_some(), "Should have ship knowledge after light delay");
        let snap = ship_snap.unwrap();
        assert_eq!(snap.name, "Scout-1");
        assert_eq!(snap.design_id, "explorer_mk1");
        assert_eq!(snap.last_known_state, ShipSnapshotState::Docked);
        assert_eq!(snap.last_known_system, Some(sys_remote));
    }
}

#[test]
fn test_ship_knowledge_local_system_immediate() {
    use macrocosmo::knowledge::ShipSnapshotState;

    let mut app = test_app();

    let sys_capital = spawn_test_system(
        app.world_mut(),
        "Capital",
        [0.0, 0.0, 0.0],
        Habitability::Ideal,
        true,
        true,
    );

    app.world_mut().spawn((Player, StationedAt { system: sys_capital }));

    // Spawn ship at capital (local system — 0 light delay)
    let ship_entity = common::spawn_test_ship(
        app.world_mut(),
        "Local-Ship",
        "explorer_mk1",
        sys_capital,
        [0.0, 0.0, 0.0],
    );

    // Even at time 0, local ships should be known immediately
    app.update();

    let empire = empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    let ship_snap = store.get_ship(ship_entity);
    assert!(ship_snap.is_some(), "Local ship should be known immediately");
    assert_eq!(ship_snap.unwrap().last_known_state, ShipSnapshotState::Docked);
}

#[test]
fn test_knowledge_store_ship_update_newer_replaces() {
    use macrocosmo::knowledge::{ShipSnapshot, ShipSnapshotState};
    use bevy::ecs::world::World;

    let mut world = World::new();
    let entity = world.spawn_empty().id();
    let system_entity = world.spawn_empty().id();

    let mut store = KnowledgeStore::default();

    store.update_ship(ShipSnapshot {
        entity,
        name: "Ship".into(),
        design_id: "test".into(),
        last_known_state: ShipSnapshotState::Docked,
        last_known_system: Some(system_entity),
        observed_at: 10,
        hp: 100.0,
        hp_max: 100.0,
    });

    store.update_ship(ShipSnapshot {
        entity,
        name: "Ship".into(),
        design_id: "test".into(),
        last_known_state: ShipSnapshotState::InTransit,
        last_known_system: None,
        observed_at: 20,
        hp: 80.0,
        hp_max: 100.0,
    });

    let snap = store.get_ship(entity).unwrap();
    assert_eq!(snap.observed_at, 20);
    assert_eq!(snap.last_known_state, ShipSnapshotState::InTransit);
}

#[test]
fn test_knowledge_store_ship_older_does_not_replace() {
    use macrocosmo::knowledge::{ShipSnapshot, ShipSnapshotState};
    use bevy::ecs::world::World;

    let mut world = World::new();
    let entity = world.spawn_empty().id();
    let system_entity = world.spawn_empty().id();

    let mut store = KnowledgeStore::default();

    store.update_ship(ShipSnapshot {
        entity,
        name: "Ship".into(),
        design_id: "test".into(),
        last_known_state: ShipSnapshotState::InTransit,
        last_known_system: None,
        observed_at: 20,
        hp: 80.0,
        hp_max: 100.0,
    });

    store.update_ship(ShipSnapshot {
        entity,
        name: "Ship".into(),
        design_id: "test".into(),
        last_known_state: ShipSnapshotState::Docked,
        last_known_system: Some(system_entity),
        observed_at: 10,
        hp: 100.0,
        hp_max: 100.0,
    });

    let snap = store.get_ship(entity).unwrap();
    assert_eq!(snap.observed_at, 20, "Newer observation should not be replaced by older");
    assert_eq!(snap.last_known_state, ShipSnapshotState::InTransit);
}
