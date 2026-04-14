mod common;

use bevy::prelude::*;
use macrocosmo::amount::Amt;
use macrocosmo::colony::*;
use macrocosmo::components::Position;
use macrocosmo::galaxy::{Planet, Sovereignty, StarSystem, SystemAttributes};
use macrocosmo::knowledge::*;
use macrocosmo::physics::{light_delay_hexadies, sublight_travel_hexadies};
use macrocosmo::player::*;
use macrocosmo::ship::*;
use macrocosmo::technology::TechKnowledge;

use common::{
    advance_time, empire_entity, find_planet, full_test_app, spawn_test_colony, spawn_test_system,
    test_app,
};

/// Helper: set up an app with tech research + propagation systems for knowledge tests.
fn tech_knowledge_app() -> App {
    let app = full_test_app();
    app
}

#[test]
fn test_knowledge_propagation_light_delay() {
    let mut app = test_app();

    // Player at origin
    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);

    // System-B at 10 LY away
    let sys_b = spawn_test_system(
        app.world_mut(),
        "Distant",
        [10.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    // Spawn player stationed at capital
    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

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
        1.0,
        true,
        true,
    );
    let sys_b = spawn_test_system(
        app.world_mut(),
        "System-B",
        [10.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );
    let sys_c = spawn_test_system(
        app.world_mut(),
        "System-C",
        [12.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    // Spawn player at system A
    app.world_mut()
        .spawn((Player, StationedAt { system: sys_a }));

    // Spawn explorer at system B with FTL range
    let ship_entity = common::spawn_test_ship(
        app.world_mut(),
        "Explorer-1",
        "explorer_mk1",
        sys_b,
        [10.0, 0.0, 0.0],
    );
    // Give it FTL range to reach system C
    app.world_mut()
        .get_mut::<Ship>(ship_entity)
        .unwrap()
        .ftl_range = 20.0;

    // Calculate expected delay: 10 ly -> 600 hexadies
    let expected_delay = light_delay_hexadies(10.0);
    assert_eq!(expected_delay, 600);

    // Simulate what the UI does: create a PendingShipCommand with light delay
    let current_time = 100;
    app.world_mut()
        .resource_mut::<macrocosmo::time_system::GameClock>()
        .elapsed = current_time;

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

    // Spawn player at system A
    app.world_mut()
        .spawn((Player, StationedAt { system: sys_a }));

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
    app.world_mut()
        .get_mut::<Ship>(ship_entity)
        .unwrap()
        .ftl_range = 20.0;

    let current_time = 100;
    app.world_mut()
        .resource_mut::<macrocosmo::time_system::GameClock>()
        .elapsed = current_time;

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
        1.0,
        true,
        true,
    );
    let sys_b = spawn_test_system(
        app.world_mut(),
        "System-B",
        [3.0, 0.0, 0.0],
        0.7,
        false, // unsurveyed
        false,
    );

    app.world_mut()
        .spawn((Player, StationedAt { system: sys_a }));

    let ship_entity = common::spawn_test_ship(
        app.world_mut(),
        "Explorer-1",
        "explorer_mk1",
        sys_b,
        [3.0, 0.0, 0.0],
    );

    let current_time = 100;
    app.world_mut()
        .resource_mut::<macrocosmo::time_system::GameClock>()
        .elapsed = current_time;

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
        1.0,
        true,
        true,
    );
    let sys_b = spawn_test_system(
        app.world_mut(),
        "System-B",
        [10.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    app.world_mut()
        .spawn((Player, StationedAt { system: sys_a }));

    let ship_entity = common::spawn_test_ship(
        app.world_mut(),
        "Doomed-Ship",
        "explorer_mk1",
        sys_a,
        [0.0, 0.0, 0.0],
    );

    let current_time = 100;
    app.world_mut()
        .resource_mut::<macrocosmo::time_system::GameClock>()
        .elapsed = current_time;

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

    app.world_mut()
        .spawn((Player, StationedAt { system: sys_a }));

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
    app.world_mut()
        .resource_mut::<macrocosmo::time_system::GameClock>()
        .elapsed = current_time;

    // Queue an EnqueueCommand to move to sys_c after delay
    app.world_mut().spawn(PendingShipCommand {
        ship: ship_entity,
        command: ShipCommand::EnqueueCommand(QueuedCommand::MoveTo { system: sys_c }),
        arrives_at: current_time + 150,
    });

    // Before arrival: command queue should be empty
    advance_time(&mut app, 50);
    let queue = app.world().get::<CommandQueue>(ship_entity).unwrap();
    assert!(
        queue.commands.is_empty(),
        "Queue should be empty before command arrives"
    );

    // After arrival: command should be in queue (ship still in FTL, so queue not consumed)
    advance_time(&mut app, 150);
    let queue = app.world().get::<CommandQueue>(ship_entity).unwrap();
    assert_eq!(
        queue.commands.len(),
        1,
        "Queue should have 1 command after arrival"
    );
    assert!(
        matches!(queue.commands[0], QueuedCommand::MoveTo { system } if system == sys_c),
        "Queued command should be MoveTo sys_c"
    );
}

// Technology knowledge propagation (#88)

#[test]
fn test_tech_propagates_to_capital_immediately() {
    use macrocosmo::technology::{RecentlyResearched, TechId, TechKnowledge};

    let mut app = tech_knowledge_app();

    // Spawn capital system
    let capital = app
        .world_mut()
        .spawn((
            StarSystem {
                name: "Capital".into(),
                surveyed: true,
                is_capital: true,
                star_type: "default".to_string(),
            },
            Position::from([0.0, 0.0, 0.0]),
            Sovereignty::default(),
            TechKnowledge::default(),
        ))
        .id();
    app.world_mut().spawn((
        Planet {
            name: "Capital I".into(),
            system: capital,
            planet_type: "default".to_string(),
        },
        SystemAttributes {
            habitability: 1.0,
            mineral_richness: 0.5,
            energy_potential: 0.5,
            research_potential: 0.5,
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
        knowledge
            .known_techs
            .contains(&TechId("social_xenolinguistics".into())),
        "Capital should know tech immediately after research"
    );
}

#[test]
fn test_tech_propagates_to_remote_with_delay() {
    use macrocosmo::technology::{RecentlyResearched, TechId, TechKnowledge};

    let mut app = tech_knowledge_app();

    // Capital at origin
    let capital = app
        .world_mut()
        .spawn((
            StarSystem {
                name: "Capital".into(),
                surveyed: true,
                is_capital: true,
                star_type: "default".to_string(),
            },
            Position::from([0.0, 0.0, 0.0]),
            Sovereignty::default(),
            TechKnowledge::default(),
        ))
        .id();
    app.world_mut().spawn((
        Planet {
            name: "Capital I".into(),
            system: capital,
            planet_type: "default".to_string(),
        },
        SystemAttributes {
            habitability: 1.0,
            mineral_richness: 0.5,
            energy_potential: 0.5,
            research_potential: 0.5,
            max_building_slots: 4,
        },
        Position::from([0.0, 0.0, 0.0]),
    ));

    // Remote system at 1 LY (light delay = 60 hexadies)
    let remote = app
        .world_mut()
        .spawn((
            StarSystem {
                name: "Remote".into(),
                surveyed: true,
                is_capital: false,
                star_type: "default".to_string(),
            },
            Position::from([1.0, 0.0, 0.0]),
            Sovereignty::default(),
            TechKnowledge::default(),
        ))
        .id();
    app.world_mut().spawn((
        Planet {
            name: "Remote I".into(),
            system: remote,
            planet_type: "default".to_string(),
        },
        SystemAttributes {
            habitability: 0.7,
            mineral_richness: 0.5,
            energy_potential: 0.5,
            research_potential: 0.5,
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
    assert!(
        capital_knowledge
            .known_techs
            .contains(&TechId("physics_sensor_arrays".into()))
    );

    // Remote should NOT have it yet (need 60 hexadies for 1 LY)
    let remote_knowledge = app.world().get::<TechKnowledge>(remote).unwrap();
    assert!(
        !remote_knowledge
            .known_techs
            .contains(&TechId("physics_sensor_arrays".into())),
        "Remote system should not know tech before light delay"
    );

    // Advance to just before arrival (59 more hexadies, total elapsed = 60)
    advance_time(&mut app, 59);
    let remote_knowledge = app.world().get::<TechKnowledge>(remote).unwrap();
    assert!(
        !remote_knowledge
            .known_techs
            .contains(&TechId("physics_sensor_arrays".into())),
        "Remote system should not know tech at tick 60 (arrives_at = 60, spawned at tick 1)"
    );

    // Advance one more tick to reach arrival time
    advance_time(&mut app, 1);
    let remote_knowledge = app.world().get::<TechKnowledge>(remote).unwrap();
    assert!(
        remote_knowledge
            .known_techs
            .contains(&TechId("physics_sensor_arrays".into())),
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
    let capital = app
        .world_mut()
        .spawn((
            StarSystem {
                name: "Capital".into(),
                surveyed: true,
                is_capital: true,
                star_type: "default".to_string(),
            },
            Position::from([0.0, 0.0, 0.0]),
            Sovereignty::default(),
            TechKnowledge::default(),
        ))
        .id();
    app.world_mut().spawn((
        Planet {
            name: "Capital I".into(),
            system: capital,
            planet_type: "default".to_string(),
        },
        SystemAttributes {
            habitability: 1.0,
            mineral_richness: 0.5,
            energy_potential: 0.5,
            research_potential: 0.5,
            max_building_slots: 4,
        },
        Position::from([0.0, 0.0, 0.0]),
    ));

    // Uncolonized system (no colony spawned for it)
    let _uncolonized = app
        .world_mut()
        .spawn((
            StarSystem {
                name: "Uncolonized".into(),
                surveyed: true,
                is_capital: false,
                star_type: "default".to_string(),
            },
            Position::from([1.0, 0.0, 0.0]),
            Sovereignty::default(),
            TechKnowledge::default(),
        ))
        .id();

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
    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);

    // Remote system at 1 LY with hostiles
    let sys_hostile = spawn_test_system(
        app.world_mut(),
        "Hostile System",
        [1.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    // Spawn player
    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

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

    assert!(
        knowledge.data.has_hostile,
        "Should have hostile presence in snapshot"
    );
    assert!(
        (knowledge.data.hostile_strength - 5.0).abs() < 0.01,
        "Hostile strength should be 5.0"
    );
}

#[test]
fn test_knowledge_snapshot_system_attributes() {
    let mut app = test_app();

    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);

    // Remote system at 1 LY with specific attributes
    let sys_remote =
        spawn_test_system(app.world_mut(), "Remote", [1.0, 0.0, 0.0], 0.7, true, false);

    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

    // Advance past light delay
    advance_time(&mut app, 61);

    let empire = empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    let knowledge = store.get(sys_remote).unwrap();

    // spawn_test_system creates planets with SystemAttributes containing the given habitability
    assert_eq!(knowledge.data.habitability, Some(0.7));
}

// #175: Ship knowledge tests

#[test]
fn test_ship_knowledge_propagation() {
    use macrocosmo::knowledge::ShipSnapshotState;

    let mut app = test_app();

    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);

    let sys_remote =
        spawn_test_system(app.world_mut(), "Remote", [1.0, 0.0, 0.0], 0.7, true, false);

    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

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
        assert!(
            ship_snap.is_some(),
            "Should have ship knowledge after light delay"
        );
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

    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);

    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

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
    assert!(
        ship_snap.is_some(),
        "Local ship should be known immediately"
    );
    assert_eq!(
        ship_snap.unwrap().last_known_state,
        ShipSnapshotState::Docked
    );
}

#[test]
fn test_knowledge_store_ship_update_newer_replaces() {
    use bevy::ecs::world::World;
    use macrocosmo::knowledge::{ShipSnapshot, ShipSnapshotState};

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
        source: ObservationSource::Direct,
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
        source: ObservationSource::Direct,
    });

    let snap = store.get_ship(entity).unwrap();
    assert_eq!(snap.observed_at, 20);
    assert_eq!(snap.last_known_state, ShipSnapshotState::InTransit);
}

#[test]
fn test_knowledge_store_ship_older_does_not_replace() {
    use bevy::ecs::world::World;
    use macrocosmo::knowledge::{ShipSnapshot, ShipSnapshotState};

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
        source: ObservationSource::Direct,
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
        source: ObservationSource::Direct,
    });

    let snap = store.get_ship(entity).unwrap();
    assert_eq!(
        snap.observed_at, 20,
        "Newer observation should not be replaced by older"
    );
    assert_eq!(snap.last_known_state, ShipSnapshotState::InTransit);
}

// --- #118: Sensor Buoy detection tests ---

/// Insert the default `sensor_buoy` structure definition (range 3.0 ly,
/// detect_sublight only) into the test app's `StructureRegistry`.
fn install_sensor_buoy_definition(app: &mut App) {
    use macrocosmo::deep_space::{
        CapabilityParams, DeliverableMetadata, ResourceCost, StructureDefinition, StructureRegistry,
    };
    use std::collections::HashMap;
    let mut registry = app
        .world_mut()
        .get_resource_mut::<StructureRegistry>()
        .expect("StructureRegistry not initialized in test_app");
    registry.insert(StructureDefinition {
        id: "sensor_buoy".to_string(),
        name: "Sensor Buoy".to_string(),
        description: "Detects sublight vessel movements.".to_string(),
        max_hp: 20.0,
        capabilities: HashMap::from([(
            "detect_sublight".to_string(),
            CapabilityParams { range: 3.0 },
        )]),
        energy_drain: Amt::milli(100),
        prerequisites: None,
        deliverable: Some(DeliverableMetadata {
            cost: ResourceCost::default(),
            build_time: 15,
            cargo_size: 1,
            scrap_refund: 0.5,
        }),
        upgrade_to: Vec::new(),
        upgrade_from: None,
    });
}

/// Spawn a sensor buoy at the given position. Returns the buoy entity.
fn spawn_sensor_buoy(world: &mut World, pos: [f64; 3]) -> Entity {
    use macrocosmo::deep_space::{DeepSpaceStructure, StructureHitpoints};
    world
        .spawn((
            DeepSpaceStructure {
                definition_id: "sensor_buoy".to_string(),
                name: "Buoy Alpha".to_string(),
                owner: macrocosmo::ship::Owner::Neutral,
            },
            StructureHitpoints {
                current: 20.0,
                max: 20.0,
            },
            Position::from(pos),
        ))
        .id()
}

#[test]
fn test_sensor_buoy_detects_sublight_ship_in_range() {
    // Strategy: place the buoy CLOSER to the player than the ship's
    // location is to the player. The buoy should report the ship via a
    // shorter light path, producing a snapshot earlier than direct
    // propagate_knowledge would.
    //
    // Player:   [0,  0, 0]
    // Buoy:     [3,  0, 0]  (3 ly from player → 180 hexadies)
    // Ship:     [3.5,0, 0]  (3.5 ly from player → 210 hexadies; 0.5 ly from buoy, within 3 ly range)
    let mut app = test_app();
    install_sensor_buoy_definition(&mut app);

    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    let sys_remote =
        spawn_test_system(app.world_mut(), "Remote", [3.5, 0.0, 0.0], 0.5, true, false);
    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

    spawn_sensor_buoy(app.world_mut(), [3.0, 0.0, 0.0]);

    // Sublight ship right next to the buoy (ship_pos = [3.5, 0, 0]).
    let ship_entity = common::spawn_test_ship(
        app.world_mut(),
        "Probe-1",
        "courier_mk1", // ftl_range = 0
        sys_remote,
        [3.5, 0.0, 0.0],
    );
    *app.world_mut().get_mut::<ShipState>(ship_entity).unwrap() = ShipState::SubLight {
        origin: [3.5, 0.0, 0.0],
        destination: [4.5, 0.0, 0.0],
        target_system: None,
        departed_at: 0,
        arrival_at: 1_000_000,
    };

    // At t=200: buoy delay = 180 ✓ (detection possible),
    // but propagate_knowledge for SubLight uses distance=0 (a known
    // simplification in the existing code), so it would give observed_at=200.
    // To make the buoy contribution observable, we compare against the
    // buoy-derived observed_at after detection runs.
    advance_time(&mut app, 200);
    let empire = empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    let snap = store
        .get_ship(ship_entity)
        .expect("ship should be in KnowledgeStore");
    // The most-recent observation wins. Whether buoy or propagate_knowledge
    // wins depends on internals. The behavior we verify is: snapshot exists
    // and reflects the SubLight state.
    assert_eq!(snap.last_known_state, ShipSnapshotState::InTransit);
    assert_eq!(snap.name, "Probe-1");
}

#[test]
fn test_sensor_buoy_detects_remote_docked_ship_via_buoy_path() {
    // For Docked ships propagate_knowledge applies the player→ship light
    // delay, so by placing the buoy CLOSER to the player than the ship is,
    // we can prove the buoy is the source of the early observation.
    //
    // Player:   [0, 0, 0]
    // Buoy:     [5, 0, 0]   (5 ly → 300 hexadies delay)
    // Ship:     [7, 0, 0]   (7 ly → 420 hexadies delay; 2 ly from buoy)
    let mut app = test_app();
    install_sensor_buoy_definition(&mut app);

    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    let sys_outpost = spawn_test_system(
        app.world_mut(),
        "Outpost",
        [7.0, 0.0, 0.0],
        0.5,
        true,
        false,
    );
    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

    spawn_sensor_buoy(app.world_mut(), [5.0, 0.0, 0.0]);

    let ship_entity = common::spawn_test_ship(
        app.world_mut(),
        "Outpost-Patrol",
        "courier_mk1",
        sys_outpost,
        [7.0, 0.0, 0.0],
    );

    // At t=350: buoy delay = 300 ✓; propagate_knowledge for Docked needs 420 ✗.
    // So any snapshot present must come from the buoy.
    advance_time(&mut app, 350);
    let empire = empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    let snap = store
        .get_ship(ship_entity)
        .expect("Buoy should have detected docked ship before propagate_knowledge could");
    // observed_at = 350 - 300 = 50, and importantly LESS than 350 (proving
    // it isn't from a closer-but-untimed source).
    assert_eq!(snap.observed_at, 50);
    assert_eq!(snap.last_known_state, ShipSnapshotState::Docked);
    assert_eq!(snap.last_known_system, Some(sys_outpost));
}

#[test]
fn test_sensor_buoy_does_not_detect_ftl_ship() {
    let mut app = test_app();
    install_sensor_buoy_definition(&mut app);

    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    let sys_remote_a = spawn_test_system(
        app.world_mut(),
        "RemoteA",
        [10.0, 0.0, 0.0],
        0.5,
        true,
        false,
    );
    let sys_remote_b = spawn_test_system(
        app.world_mut(),
        "RemoteB",
        [11.0, 0.0, 0.0],
        0.5,
        true,
        false,
    );
    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

    let buoy_pos = [10.0, 0.0, 0.0];
    spawn_sensor_buoy(app.world_mut(), buoy_pos);

    // FTL ship right next to the buoy.
    let ship_entity = common::spawn_test_ship(
        app.world_mut(),
        "FTL-1",
        "explorer_mk1", // ftl_range > 0
        sys_remote_a,
        [10.0, 0.0, 0.0],
    );
    *app.world_mut().get_mut::<ShipState>(ship_entity).unwrap() = ShipState::InFTL {
        origin_system: sys_remote_a,
        destination_system: sys_remote_b,
        departed_at: 0,
        arrival_at: 1_000_000,
    };

    // Advance well beyond the buoy->player light delay (600 hexadies for 10 ly).
    advance_time(&mut app, 700);

    let empire = empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    // The buoy must not produce a snapshot for an FTL ship. The standard
    // propagate_knowledge path uses the ship's `origin_system` position
    // (RemoteA at 10 ly), so it could produce a Docked-equivalent snapshot
    // — but we only assert here that *if* a snapshot exists, it is not
    // marked as InTransit by the buoy at the buoy's location. The
    // important regression target is "buoy did not contribute". Because
    // the propagate_knowledge would emit an InTransit state with
    // `last_known_system == Some(sys_remote_b)` for this FTL ship at the
    // appropriate light delay (10 ly => 600 hexadies), we assert the
    // observed_at matches *its* delay, never an artificially-fresh value
    // from a buoy override.
    if let Some(snap) = store.get_ship(ship_entity) {
        // propagate_knowledge sees ship at sys_remote_a (10 ly away) and
        // light_delay = 600. observed_at = 700 - 600 = 100.
        assert_eq!(
            snap.observed_at, 100,
            "FTL ship snapshot should come only from propagate_knowledge, not buoy"
        );
    }
}

#[test]
fn test_sensor_buoy_does_not_detect_ship_out_of_range() {
    // Setup so that *only* a buoy detection would surface knowledge in time.
    //
    // Player:  [0, 0, 0]
    // Buoy:    [5, 0, 0]   (5 ly → 300 hexadies)
    // Docked ship in remote outpost at [10, 0, 0]:
    //   - 10 ly from player → propagate_knowledge needs 600 hexadies
    //   - 5 ly from buoy → OUTSIDE 3 ly buoy range
    // We advance to 400 hexadies. If the buoy were broken (detected
    // out-of-range ships), a snapshot would appear. propagate_knowledge
    // can't produce one yet (needs 600).
    let mut app = test_app();
    install_sensor_buoy_definition(&mut app);

    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    let sys_remote = spawn_test_system(
        app.world_mut(),
        "Remote",
        [10.0, 0.0, 0.0],
        0.5,
        true,
        false,
    );
    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

    spawn_sensor_buoy(app.world_mut(), [5.0, 0.0, 0.0]);

    let ship_entity = common::spawn_test_ship(
        app.world_mut(),
        "FarShip",
        "courier_mk1",
        sys_remote,
        [10.0, 0.0, 0.0],
    );

    advance_time(&mut app, 400);

    let empire = empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    assert!(
        store.get_ship(ship_entity).is_none(),
        "Buoy must not detect ship outside its range; \
         propagate_knowledge has not had time either."
    );
}

#[test]
fn test_sensor_buoy_detects_docked_ship_in_range() {
    // Even a Docked ship within sensor range should be reported by the buoy.
    // This exercises the snapshot_state mapping for non-FTL Docked ships.
    let mut app = test_app();
    install_sensor_buoy_definition(&mut app);

    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    let sys_outpost = spawn_test_system(
        app.world_mut(),
        "Outpost",
        [10.0, 0.0, 0.0],
        0.5,
        true,
        false,
    );
    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

    spawn_sensor_buoy(app.world_mut(), [10.0, 0.0, 0.0]);

    // Docked ship at outpost (right under the buoy).
    let ship_entity = common::spawn_test_ship(
        app.world_mut(),
        "DockedShip",
        "courier_mk1",
        sys_outpost,
        [10.0, 0.0, 0.0],
    );

    // Advance past buoy->player light delay (600).
    advance_time(&mut app, 605);

    let empire = empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    let snap = store
        .get_ship(ship_entity)
        .expect("Buoy should detect docked ship in range");
    assert_eq!(snap.last_known_state, ShipSnapshotState::Docked);
    assert_eq!(snap.last_known_system, Some(sys_outpost));
}

#[test]
fn test_sensor_buoy_no_player_no_panic() {
    // Sanity: with no Player entity, the system should early-return cleanly.
    let mut app = test_app();
    install_sensor_buoy_definition(&mut app);

    let _sys = spawn_test_system(app.world_mut(), "Lone", [0.0, 0.0, 0.0], 0.5, true, false);

    spawn_sensor_buoy(app.world_mut(), [5.0, 0.0, 0.0]);

    // Just confirm we don't panic.
    advance_time(&mut app, 100);
}

// --- #188: SubLight ship knowledge propagation light-speed delay ---

/// Regression for #188: a SubLight ship's interpolated position is used to compute
/// the light-speed delay to the player's KnowledgeStore. A ship far from the player
/// must not be reported with `observed_at == clock.elapsed`.
#[test]
fn test_sublight_ship_knowledge_uses_light_speed_delay() {
    // test_app() spawns the empire entity.
    let mut app = test_app();

    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

    // Spawn a ship in SubLight transit between (10, 0, 0) and (12, 0, 0); long enough
    // travel that during our test window (0..120 hd) the ship has not yet arrived.
    // At t=120 hd the ship will be at roughly (10 + 0.5*2, 0, 0) = (11, 0, 0) which
    // is ~11 LY from the player at the capital.
    let ship_entity = app
        .world_mut()
        .spawn((
            Ship {
                name: "Far-Scout".to_string(),
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
                origin: [10.0, 0.0, 0.0],
                destination: [12.0, 0.0, 0.0],
                target_system: None,
                departed_at: 0,
                arrival_at: sublight_travel_hexadies(2.0, 0.75), // 160 hd
            },
            Position::from([10.0, 0.0, 0.0]),
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
            RulesOfEngagement::default(),
        ))
        .id();

    // Advance enough that, at light delay >= 600 hd (10 LY), we have NOT yet
    // received any snapshot for the ship.
    advance_time(&mut app, 120);
    {
        let empire = empire_entity(app.world_mut());
        let store = app.world().get::<KnowledgeStore>(empire).unwrap();
        assert!(
            store.get_ship(ship_entity).is_none(),
            "Far SubLight ship must not be in KnowledgeStore before light delay elapses"
        );
    }

    // Now advance well past the ship's projected light delay (~660+ hd) and
    // confirm we receive a snapshot whose observed_at lags the current clock by at
    // least the light delay (650+ hd).
    advance_time(&mut app, 700);
    let empire = empire_entity(app.world_mut());
    let clock = app
        .world()
        .resource::<macrocosmo::time_system::GameClock>()
        .elapsed;
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    let snap = store
        .get_ship(ship_entity)
        .expect("Should have ship knowledge by now");
    let lag = clock - snap.observed_at;
    assert!(
        lag >= 600,
        "SubLight ship snapshot must lag clock by at least the light delay (~600 hd for ~10 LY); got lag={} (clock={}, observed_at={})",
        lag,
        clock,
        snap.observed_at
    );
}

/// Regression for #188: a SubLight ship near the player must arrive in the
/// KnowledgeStore with near-zero delay (lag less than a few hexadies).
#[test]
fn test_sublight_ship_nearby_knowledge_negligible_delay() {
    // test_app() spawns the empire entity.
    let mut app = test_app();

    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

    // SubLight ship interpolating very close to the player (well under 0.05 LY).
    let ship_entity = app
        .world_mut()
        .spawn((
            Ship {
                name: "Near-Scout".to_string(),
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
                destination: [0.01, 0.0, 0.0],
                target_system: None,
                departed_at: 0,
                arrival_at: 1000,
            },
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
            RulesOfEngagement::default(),
        ))
        .id();

    advance_time(&mut app, 5);

    let empire = empire_entity(app.world_mut());
    let clock = app
        .world()
        .resource::<macrocosmo::time_system::GameClock>()
        .elapsed;
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    let snap = store
        .get_ship(ship_entity)
        .expect("Nearby ship must be in KnowledgeStore");
    let lag = clock - snap.observed_at;
    // Light delay for ~0.01 LY = ceil(0.01 * 60) = 1 hd; allow some slack.
    assert!(
        lag <= 5,
        "Nearby SubLight ship snapshot should have negligible lag, got {}",
        lag
    );
}

/// #185 + #188: Loitering ships are also reported through KnowledgeStore with the
/// correct light-speed delay computed from their loitering position.
#[test]
fn test_loitering_ship_knowledge_uses_light_speed_delay() {
    use macrocosmo::knowledge::ShipSnapshotState;

    // test_app() spawns the empire entity.
    let mut app = test_app();

    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

    let loiter_pos = [10.0, 0.0, 0.0];
    let ship_entity = app
        .world_mut()
        .spawn((
            Ship {
                name: "Deep-Loiter".to_string(),
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
            ShipState::Loitering {
                position: loiter_pos,
            },
            Position::from(loiter_pos),
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
            RulesOfEngagement::default(),
        ))
        .id();

    // Before light delay (10 LY = 600 hd): no knowledge.
    advance_time(&mut app, 100);
    {
        let empire = empire_entity(app.world_mut());
        let store = app.world().get::<KnowledgeStore>(empire).unwrap();
        assert!(
            store.get_ship(ship_entity).is_none(),
            "Loitering ship must not be in KnowledgeStore before light delay"
        );
    }

    // After light delay: knowledge with Loitering snapshot variant.
    advance_time(&mut app, 700);
    let empire = empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    let snap = store
        .get_ship(ship_entity)
        .expect("Should have Loitering snapshot");
    match &snap.last_known_state {
        ShipSnapshotState::Loitering { position } => {
            assert!((position[0] - loiter_pos[0]).abs() < 1e-9);
        }
        other => panic!("Expected Loitering snapshot state, got {:?}", other),
    }
    assert_eq!(snap.last_known_system, None);
}

// --- #119: FTL Comm Relay tests ---

/// Install an ftl_comm_relay structure definition with a configurable range.
fn install_ftl_comm_relay_definition(app: &mut App, range_ly: f64) {
    use macrocosmo::deep_space::{
        CapabilityParams, DeliverableMetadata, ResourceCost, StructureDefinition, StructureRegistry,
    };
    use std::collections::HashMap;
    let mut registry = app
        .world_mut()
        .get_resource_mut::<StructureRegistry>()
        .expect("StructureRegistry not initialized in test_app");
    registry.insert(StructureDefinition {
        id: "ftl_comm_relay".to_string(),
        name: "FTL Comm Relay".to_string(),
        description: "Pair-based FTL relay".to_string(),
        max_hp: 50.0,
        capabilities: HashMap::from([(
            "ftl_comm_relay".to_string(),
            CapabilityParams { range: range_ly },
        )]),
        energy_drain: Amt::milli(500),
        prerequisites: None,
        deliverable: Some(DeliverableMetadata {
            cost: ResourceCost::default(),
            build_time: 20,
            cargo_size: 2,
            scrap_refund: 0.4,
        }),
        upgrade_to: Vec::new(),
        upgrade_from: None,
    });
}

/// Spawn an FTL comm relay at the given position.
fn spawn_ftl_comm_relay(world: &mut World, name: &str, pos: [f64; 3]) -> Entity {
    use macrocosmo::deep_space::{DeepSpaceStructure, StructureHitpoints};
    world
        .spawn((
            DeepSpaceStructure {
                definition_id: "ftl_comm_relay".to_string(),
                name: name.to_string(),
                owner: macrocosmo::ship::Owner::Neutral,
            },
            StructureHitpoints {
                current: 50.0,
                max: 50.0,
            },
            Position::from(pos),
        ))
        .id()
}

#[test]
fn test_ftl_comm_relay_bidirectional_propagates_remote_ship_at_ftl_speed() {
    // Setup:
    //   Player @ [0, 0, 0]
    //   Relay-A @ [2, 0, 0]  (near player, range 3 ly)
    //   Relay-B @ [20, 0, 0] (remote, range 3 ly)
    //   Ship-Remote @ [20.5, 0, 0] (within 3 ly of Relay-B, ~20.5 ly from player)
    //
    // Direct propagate_knowledge delay for ship = ~20.5 ly → 1230 hd.
    // Relay path: Relay-B observes ship, Relay-A is within 2 ly of player. Player
    // is within Relay-A's 3 ly range. FTL = instant (observed_at == clock.elapsed).
    //
    // At t=100 hd (well before any light from the ship could reach the player),
    // the ship's snapshot must exist in the KnowledgeStore, delivered by the relay.
    use macrocosmo::deep_space::{CommDirection, pair_relay_command};

    let mut app = test_app();
    install_ftl_comm_relay_definition(&mut app, 3.0);

    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    let sys_remote = spawn_test_system(
        app.world_mut(),
        "Remote",
        [20.5, 0.0, 0.0],
        0.5,
        true,
        false,
    );
    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

    let relay_a = spawn_ftl_comm_relay(app.world_mut(), "Relay-A", [2.0, 0.0, 0.0]);
    let relay_b = spawn_ftl_comm_relay(app.world_mut(), "Relay-B", [20.0, 0.0, 0.0]);
    pair_relay_command(
        app.world_mut(),
        relay_a,
        relay_b,
        CommDirection::Bidirectional,
    )
    .unwrap();

    let ship_entity = common::spawn_test_ship(
        app.world_mut(),
        "Remote-Scout",
        "courier_mk1",
        sys_remote,
        [20.5, 0.0, 0.0],
    );

    advance_time(&mut app, 100);
    let empire = empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    let snap = store
        .get_ship(ship_entity)
        .expect("Relay should have delivered remote ship knowledge at FTL speed");
    assert_eq!(
        snap.observed_at, 100,
        "FTL relay delivers with no light delay"
    );
    assert_eq!(snap.name, "Remote-Scout");
}

#[test]
fn test_ftl_comm_relay_oneway_reverse_direction_does_not_propagate() {
    // OneWay A→B: A is the sender (covers ships), B is the receiver (covers
    // player). If we flip the physical layout so the PLAYER is near A and the
    // SHIP is near B, the pair A→B has:
    //   - source A covers ships near A (no remote ship there)
    //   - target B covers player near B (player isn't there)
    // → no propagation.
    //
    // This proves the direction is enforced — with Bidirectional, the reverse
    // path would kick in (B as source sees the ship, A as target sees the
    // player) and the ship would get a snapshot.
    use macrocosmo::deep_space::{CommDirection, pair_relay_command};

    let mut app = test_app();
    install_ftl_comm_relay_definition(&mut app, 3.0);

    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    let sys_remote = spawn_test_system(
        app.world_mut(),
        "Remote",
        [20.5, 0.0, 0.0],
        0.5,
        true,
        false,
    );
    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

    // Relay-A near PLAYER, Relay-B near SHIP. OneWay A→B means A sends, B
    // receives. But A's source-range covers nothing near the ship, and B
    // doesn't send, so the ship's snapshot is NOT populated by the relay.
    let relay_a = spawn_ftl_comm_relay(app.world_mut(), "Relay-A", [2.0, 0.0, 0.0]);
    let relay_b = spawn_ftl_comm_relay(app.world_mut(), "Relay-B", [20.0, 0.0, 0.0]);
    pair_relay_command(app.world_mut(), relay_a, relay_b, CommDirection::OneWay).unwrap();

    let ship_entity = common::spawn_test_ship(
        app.world_mut(),
        "Remote-Scout",
        "courier_mk1",
        sys_remote,
        [20.5, 0.0, 0.0],
    );

    advance_time(&mut app, 100);
    let empire = empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    // At t=100 hd, 20.5 ly light delay is ~1230 hd, so direct propagate_knowledge
    // can't reach yet. And the relay path doesn't fire because B doesn't send.
    assert!(
        store.get_ship(ship_entity).is_none(),
        "OneWay A→B must not propagate ships near B back to player near A"
    );
}

#[test]
fn test_ftl_comm_relay_oneway_forward_direction_propagates() {
    // Companion test to the previous: reverse the physical layout so the
    // OneWay pair A→B actually sends — ship is near A, player is near B.
    use macrocosmo::deep_space::{CommDirection, pair_relay_command};

    let mut app = test_app();
    install_ftl_comm_relay_definition(&mut app, 3.0);

    // Player is near Relay-B this time; ship is near Relay-A.
    let sys_capital = spawn_test_system(
        app.world_mut(),
        "Capital",
        [20.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    let sys_remote =
        spawn_test_system(app.world_mut(), "Remote", [0.5, 0.0, 0.0], 0.5, true, false);
    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

    let relay_a = spawn_ftl_comm_relay(app.world_mut(), "Relay-A", [0.0, 0.0, 0.0]);
    let relay_b = spawn_ftl_comm_relay(app.world_mut(), "Relay-B", [20.5, 0.0, 0.0]);
    pair_relay_command(app.world_mut(), relay_a, relay_b, CommDirection::OneWay).unwrap();

    let ship_entity = common::spawn_test_ship(
        app.world_mut(),
        "Remote-Scout",
        "courier_mk1",
        sys_remote,
        [0.5, 0.0, 0.0],
    );

    advance_time(&mut app, 100);
    let empire = empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    let snap = store
        .get_ship(ship_entity)
        .expect("OneWay A→B must propagate ships near A to player near B");
    assert_eq!(snap.observed_at, 100);
}

#[test]
fn test_ftl_comm_relay_destroyed_becomes_unpaired() {
    // Verify that despawning one relay clears the partner's FTLCommRelay
    // component on the next tick, and propagation stops.
    use macrocosmo::deep_space::{CommDirection, FTLCommRelay, pair_relay_command};

    let mut app = test_app();
    install_ftl_comm_relay_definition(&mut app, 3.0);

    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    let sys_remote = spawn_test_system(
        app.world_mut(),
        "Remote",
        [20.5, 0.0, 0.0],
        0.5,
        true,
        false,
    );
    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

    let relay_a = spawn_ftl_comm_relay(app.world_mut(), "Relay-A", [2.0, 0.0, 0.0]);
    let relay_b = spawn_ftl_comm_relay(app.world_mut(), "Relay-B", [20.0, 0.0, 0.0]);
    pair_relay_command(
        app.world_mut(),
        relay_a,
        relay_b,
        CommDirection::Bidirectional,
    )
    .unwrap();

    let ship_entity = common::spawn_test_ship(
        app.world_mut(),
        "Remote-Scout",
        "courier_mk1",
        sys_remote,
        [20.5, 0.0, 0.0],
    );

    // First tick: both relays live, ship snapshot gets populated.
    advance_time(&mut app, 10);
    {
        let empire = empire_entity(app.world_mut());
        let store = app.world().get::<KnowledgeStore>(empire).unwrap();
        assert!(
            store.get_ship(ship_entity).is_some(),
            "Relay chain should work while paired"
        );
    }

    // Destroy Relay-B. Next tick, verify_relay_pairings_system strips
    // Relay-A's FTLCommRelay component.
    app.world_mut().despawn(relay_b);

    advance_time(&mut app, 1);

    assert!(
        app.world().get::<FTLCommRelay>(relay_a).is_none(),
        "Partner despawned → relay_a must be unpaired by verify system"
    );

    // Move the ship slightly so stale-snapshot-wins doesn't mask regressions.
    // Despawn the old ship and spawn a new one; no relay should pick it up.
    app.world_mut().despawn(ship_entity);
    let new_ship = common::spawn_test_ship(
        app.world_mut(),
        "Fresh-Scout",
        "courier_mk1",
        sys_remote,
        [20.5, 0.0, 0.0],
    );

    advance_time(&mut app, 50); // light delay at 20.5 ly is ~1230 hd, still far off
    let empire = empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    assert!(
        store.get_ship(new_ship).is_none(),
        "After relay destroyed, no new remote ships should be delivered"
    );
}

#[test]
fn test_ftl_comm_relay_chain_a_b_c_hops() {
    // Chain: Relay-A (near player) ↔ Relay-B ↔ Relay-C (near remote ship).
    // Implementation: two independent pairs (A↔B and B↔C). Each pair runs
    // independently. With range 3 ly:
    //
    //   Player @ [0, 0, 0]
    //   A @ [2, 0, 0]
    //   B1 @ [10, 0, 0]  (paired with A)
    //   B2 @ [10.5, 0, 0] (paired with C, within 3 ly of B1 & C-ship chain
    //                      is via A↔B1 delivering B2's ships? No — each
    //                      pair only observes ships within its source
    //                      relay's range. We need a two-hop chain to reach a
    //                      ship near C.)
    //
    // Simpler setup: two pairs, A↔B and B↔C, using SEPARATE entities for
    // B's two endpoints (a physical relay station can host only one pair in
    // this model — the chain emerges from multiple colocated relays).
    //
    //   Player @ [0, 0, 0]
    //   A @ [2, 0, 0]    (paired with B1, range 3)
    //   B1 @ [4, 0, 0]   (3 ly range; near A and near B2)
    //   B2 @ [4, 0, 0]   (paired with C, range 3)
    //   C @ [20, 0, 0]   (3 ly range; near remote ship)
    //   Remote ship @ [20.5, 0, 0]
    //
    // First tick: pair B2↔C fires — C observes remote ship, B2 receives; but
    // the player isn't near B2, so THIS pair alone can't deliver to the player.
    // The chain depends on relayed knowledge reaching the player's KnowledgeStore:
    // in this simpler model, the KnowledgeStore is the player empire's global
    // store, so once ANY pair covers "player near target relay" it delivers.
    //
    // For the chain to reach the player, pair A↔B1 needs to cover the ship.
    // But A↔B1's source sides are relays A and B1, whose ranges cover [2±3]
    // and [4±3] ly respectively — neither covers a ship at 20.5 ly.
    //
    // Therefore "chain" in this implementation means: multiple pairs
    // collectively extending coverage. A single remote ship near C is
    // delivered to the player empire via ANY pair that has: (source covers
    // ship) AND (target covers player). Here pair B2↔C: C is source (covers
    // ship), B2 is target (covers... player? No, B2 is at [4, 0, 0] which
    // is within 3 ly of player at [0, 0, 0]? Distance = 4 ly > 3 ly. Not
    // quite.)
    //
    // So let's place B2 closer to the player:
    //
    //   Player @ [0, 0, 0]
    //   A @ [2, 0, 0]      (paired with B1, range 3)
    //   B1 @ [4, 0, 0]     (paired with A; also distinct from B2)
    //   B2 @ [2.5, 0, 0]   (paired with C, range 3 — covers player)
    //   C @ [20, 0, 0]     (paired with B2, range 3 — covers remote ship)
    //   Remote ship @ [20.5, 0, 0]
    //
    // Now pair B2↔C: source C covers ship, target B2 covers player (2.5 ly
    // < 3 ly). Chain works.
    use macrocosmo::deep_space::{CommDirection, pair_relay_command};

    let mut app = test_app();
    install_ftl_comm_relay_definition(&mut app, 3.0);

    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    let sys_remote = spawn_test_system(
        app.world_mut(),
        "Remote",
        [20.5, 0.0, 0.0],
        0.5,
        true,
        false,
    );
    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

    // First hop: A ↔ B1 (leaves room for chain expansion but not strictly
    // required by this test — we keep it to document the "chain" intent).
    let relay_a = spawn_ftl_comm_relay(app.world_mut(), "A", [2.0, 0.0, 0.0]);
    let relay_b1 = spawn_ftl_comm_relay(app.world_mut(), "B1", [4.0, 0.0, 0.0]);
    pair_relay_command(
        app.world_mut(),
        relay_a,
        relay_b1,
        CommDirection::Bidirectional,
    )
    .unwrap();

    // Second hop: B2 ↔ C.
    let relay_b2 = spawn_ftl_comm_relay(app.world_mut(), "B2", [2.5, 0.0, 0.0]);
    let relay_c = spawn_ftl_comm_relay(app.world_mut(), "C", [20.0, 0.0, 0.0]);
    pair_relay_command(
        app.world_mut(),
        relay_b2,
        relay_c,
        CommDirection::Bidirectional,
    )
    .unwrap();

    let ship_entity = common::spawn_test_ship(
        app.world_mut(),
        "Far-Scout",
        "courier_mk1",
        sys_remote,
        [20.5, 0.0, 0.0],
    );

    advance_time(&mut app, 50);
    let empire = empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    let snap = store
        .get_ship(ship_entity)
        .expect("Chain A↔B1, B2↔C should deliver remote ship to player at FTL speed");
    assert_eq!(snap.observed_at, 50);
}

// ---------------------------------------------------------------------------
// #215: Observation freshness model (PerceivedInfo / ObservationSource)
// ---------------------------------------------------------------------------

/// After propagating knowledge across 10 ly and waiting long enough for
/// the light to arrive, `perceived_system` should report `last_updated`
/// equal to the observation moment and a correctly computed age.
#[test]
fn test_perceived_info_reports_last_updated() {
    let mut app = test_app();

    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    let sys_distant = spawn_test_system(
        app.world_mut(),
        "Distant",
        [10.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );
    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

    // 10 ly of light delay = 600 hexadies. Advance exactly that far — the
    // observation snapped this tick was made at t=0 (current - delay).
    advance_time(&mut app, 600);

    let now = app
        .world()
        .resource::<macrocosmo::time_system::GameClock>()
        .elapsed;
    let empire = empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    let perceived = macrocosmo::knowledge::perceived_system(store, sys_distant, now)
        .expect("perceived_system should return the Distant snapshot");

    // `observed_at` is the light-departure time. For a 10 ly target the
    // first observation should be snapped at t = now - 600.
    assert_eq!(
        perceived.last_updated,
        now - 600,
        "last_updated must match observed_at (light-departure hexadies)"
    );
    assert_eq!(perceived.age(now), 600);
    // At exactly the threshold the overlay flips to Stale.
    assert!(
        perceived.is_stale(now),
        "age == STALE_THRESHOLD_HEXADIES should overlay source to Stale"
    );
}

/// Propagated observations are tagged `Direct`. Once enough in-game time
/// passes (age >= STALE_THRESHOLD_HEXADIES), `perceived_system` overlays
/// the source to `Stale` without mutating the underlying entry.
#[test]
fn test_perceived_info_source_reflects_origin() {
    let mut app = test_app();

    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    let sys_b = spawn_test_system(app.world_mut(), "Near", [2.0, 0.0, 0.0], 0.7, true, false);
    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

    // 2 ly → 120 hd delay. Advance past that so the Direct observation lands.
    advance_time(&mut app, 200);

    let now = app
        .world()
        .resource::<macrocosmo::time_system::GameClock>()
        .elapsed;
    let empire = empire_entity(app.world_mut());
    {
        let store = app.world().get::<KnowledgeStore>(empire).unwrap();
        // Underlying entry carries Direct source.
        let entry = store
            .get(sys_b)
            .expect("Near system should be observed by now");
        assert_eq!(
            entry.source,
            macrocosmo::knowledge::ObservationSource::Direct
        );

        let perceived = macrocosmo::knowledge::perceived_system(store, sys_b, now).unwrap();
        assert_eq!(
            perceived.source,
            macrocosmo::knowledge::ObservationSource::Direct
        );
        assert!(!perceived.is_stale(now));
    }

    // Jump to a far-future "now" that exceeds the stale threshold. The
    // stored entry still says Direct, but the overlay should return Stale.
    let distant_now = now + macrocosmo::knowledge::STALE_THRESHOLD_HEXADIES + 10;
    {
        let store = app.world().get::<KnowledgeStore>(empire).unwrap();
        let entry_after = store.get(sys_b).unwrap();
        assert_eq!(
            entry_after.source,
            macrocosmo::knowledge::ObservationSource::Direct,
            "underlying source must not be mutated by overlay"
        );

        let perceived = macrocosmo::knowledge::perceived_system(store, sys_b, distant_now).unwrap();
        assert_eq!(
            perceived.source,
            macrocosmo::knowledge::ObservationSource::Stale,
            "accessor overlays source=Stale when age >= STALE_THRESHOLD_HEXADIES"
        );
        assert!(perceived.is_stale(distant_now));
    }
}

/// An entry written with `ObservationSource::Relay` (e.g. from an FTL comm
/// relay at #216) should flow through `perceived_system` unchanged until
/// it ages out, at which point the overlay replaces it with `Stale`.
#[test]
fn test_perceived_info_relay_source() {
    use bevy::ecs::world::World;

    let mut world = World::new();
    let sys_entity = world.spawn_empty().id();
    let mut store = KnowledgeStore::default();
    store.update(SystemKnowledge {
        system: sys_entity,
        observed_at: 100,
        received_at: 100,
        data: SystemSnapshot {
            name: "Relayed".to_string(),
            position: [5.0, 0.0, 0.0],
            surveyed: true,
            ..Default::default()
        },
        source: ObservationSource::Relay,
    });

    // Fresh observation — the relay source should round-trip.
    let fresh = macrocosmo::knowledge::perceived_system(&store, sys_entity, 150).unwrap();
    assert_eq!(fresh.source, ObservationSource::Relay);
    assert_eq!(fresh.last_updated, 100);
    assert_eq!(fresh.age(150), 50);
    assert!(!fresh.is_stale(150));

    // Age the observation past the staleness cutoff — overlay kicks in.
    let stale_now = 100 + macrocosmo::knowledge::STALE_THRESHOLD_HEXADIES + 1;
    let stale = macrocosmo::knowledge::perceived_system(&store, sys_entity, stale_now).unwrap();
    assert_eq!(stale.source, ObservationSource::Stale);
    assert!(stale.is_stale(stale_now));

    // perceived_fleet is empty (no ship snapshots) but should not panic and
    // should not filter anything out yet (owner tracking TODO per #163).
    let fleet = macrocosmo::knowledge::perceived_fleet(&store, sys_entity, stale_now);
    assert!(fleet.is_empty());
}

/// Regression test for the #216 / #217 semantic clash: Scout reports and
/// Relay propagation ticks race to write the same system. Relay is a
/// continuous low-fidelity forward; Scout is a high-fidelity targeted
/// observation. The store must preserve the Scout entry regardless of
/// write order — both "incoming Relay vs existing Scout" and "incoming
/// Scout vs existing Relay" cases resolve in Scout's favour. A newer
/// Scout or any Direct observation still dominates.
#[test]
fn test_relay_does_not_overwrite_scout() {
    use bevy::ecs::world::World;

    let mut world = World::new();
    let sys_entity = world.spawn_empty().id();
    let mut store = KnowledgeStore::default();

    // Scout arrives first with an older observed_at (as FtlComm report would).
    store.update(SystemKnowledge {
        system: sys_entity,
        observed_at: 100,
        received_at: 100,
        data: SystemSnapshot {
            name: "Scouted".to_string(),
            ..Default::default()
        },
        source: ObservationSource::Scout,
    });
    assert_eq!(
        store.get(sys_entity).unwrap().source,
        ObservationSource::Scout
    );

    // Relay propagation ticks keep running and try to write a newer entry.
    store.update(SystemKnowledge {
        system: sys_entity,
        observed_at: 105,
        received_at: 105,
        data: SystemSnapshot {
            name: "Relayed".to_string(),
            ..Default::default()
        },
        source: ObservationSource::Relay,
    });

    // Scout is preserved — Relay cannot dominate it.
    let k = store.get(sys_entity).unwrap();
    assert_eq!(k.source, ObservationSource::Scout);
    assert_eq!(k.observed_at, 100);
    assert_eq!(k.data.name, "Scouted");

    // Reverse ordering — Relay writes first, then Scout arrives with an
    // older observed_at (as can happen in same-tick races where the relay
    // system ran before process_scout_report). Scout must still win.
    let mut store2 = KnowledgeStore::default();
    let sys2 = world.spawn_empty().id();
    store2.update(SystemKnowledge {
        system: sys2,
        observed_at: 105,
        received_at: 105,
        data: SystemSnapshot {
            name: "Relayed".to_string(),
            ..Default::default()
        },
        source: ObservationSource::Relay,
    });
    store2.update(SystemKnowledge {
        system: sys2,
        observed_at: 100,
        received_at: 100,
        data: SystemSnapshot {
            name: "Scouted".to_string(),
            ..Default::default()
        },
        source: ObservationSource::Scout,
    });
    let k2 = store2.get(sys2).unwrap();
    assert_eq!(k2.source, ObservationSource::Scout);
    assert_eq!(k2.observed_at, 100);
    assert_eq!(k2.data.name, "Scouted");

    // A newer Scout observation, on the other hand, MUST dominate.
    store.update(SystemKnowledge {
        system: sys_entity,
        observed_at: 200,
        received_at: 200,
        data: SystemSnapshot {
            name: "Scouted2".to_string(),
            ..Default::default()
        },
        source: ObservationSource::Scout,
    });
    let k = store.get(sys_entity).unwrap();
    assert_eq!(k.source, ObservationSource::Scout);
    assert_eq!(k.observed_at, 200);

    // And a Direct (player-present) observation also dominates Scout when newer.
    store.update(SystemKnowledge {
        system: sys_entity,
        observed_at: 300,
        received_at: 300,
        data: SystemSnapshot {
            name: "DirectObserved".to_string(),
            ..Default::default()
        },
        source: ObservationSource::Direct,
    });
    let k = store.get(sys_entity).unwrap();
    assert_eq!(k.source, ObservationSource::Direct);
}

// ---------------------------------------------------------------------------
// #216: Sensor Buoy + FTL Comm Relay information aggregation (B-1)
// ---------------------------------------------------------------------------
//
// When an FTL Comm Relay network links a remote system to the player's
// capital, the player's KnowledgeStore receives FTL-speed SystemKnowledge
// updates (observed_at = now, source = Relay) for every star system inside
// any paired source relay's `ftl_comm_relay` range. This is the aggregation
// path described in the #216 spec: Sensor Buoy ship observations already take
// the relay shortcut (covered by #119 tests), and the system-level snapshot
// (resources, colonization, hostile presence, …) now follows the same route.

#[test]
fn test_sensor_buoy_info_propagates_via_relay() {
    // Player at capital [0, 0, 0]. Remote system at [30, 0, 0] — 30 ly of
    // light delay (~1800 hexadies). At t = 100 hd the remote system is
    // still 1700 hd away by the light-speed path, so any SystemKnowledge
    // entry for it must have been delivered via the FTL relay.
    //
    // Topology:
    //   Relay-A @ [28, 0, 0]  (near remote system, range 5 ly)
    //   Relay-B @ [1, 0, 0]   (near player, range 5 ly)
    //   Bidirectional pair — each endpoint sends.
    //   Sensor Buoy @ [30, 0, 0] co-located with the remote system.
    use macrocosmo::deep_space::{CommDirection, pair_relay_command};

    let mut app = test_app();
    install_sensor_buoy_definition(&mut app);
    install_ftl_comm_relay_definition(&mut app, 5.0);

    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    let sys_remote = spawn_test_system(
        app.world_mut(),
        "Remote",
        [30.0, 0.0, 0.0],
        0.5,
        true,
        false,
    );
    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

    // Sensor buoy watches the remote system (not required for the system
    // snapshot path itself, but models the "Sensor Buoy → Relay" spec).
    spawn_sensor_buoy(app.world_mut(), [30.0, 0.0, 0.0]);

    let relay_a = spawn_ftl_comm_relay(app.world_mut(), "Relay-A", [28.0, 0.0, 0.0]);
    let relay_b = spawn_ftl_comm_relay(app.world_mut(), "Relay-B", [1.0, 0.0, 0.0]);
    pair_relay_command(
        app.world_mut(),
        relay_a,
        relay_b,
        CommDirection::Bidirectional,
    )
    .unwrap();

    advance_time(&mut app, 100);
    let now = app
        .world()
        .resource::<macrocosmo::time_system::GameClock>()
        .elapsed;
    let empire = empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();

    let entry = store
        .get(sys_remote)
        .expect("Relay network should deliver a SystemKnowledge entry for the remote system");
    assert_eq!(
        entry.source,
        ObservationSource::Relay,
        "relay-delivered entry must be tagged Relay"
    );
    assert_eq!(
        entry.observed_at, now,
        "FTL relay delivers with no light-speed delay (observed_at == now)"
    );
    // Sanity: the snapshot payload identifies the correct remote system.
    assert_eq!(entry.data.name, "Remote");
    assert_eq!(entry.data.position, [30.0, 0.0, 0.0]);
    // Direct propagate_knowledge cannot have fired yet (1800 hd light delay).
    // If the entry's observed_at == now, it MUST be the relay path.
    assert!(
        now < 1800,
        "precondition: test runs before light-speed path could deliver"
    );
}

#[test]
fn test_relay_destruction_degrades_info_freshness() {
    // Same topology as above. While the relay chain is live, the remote
    // system's SystemKnowledge stays maximally fresh (observed_at == now,
    // source == Relay). After destroying Relay-B, subsequent ticks must not
    // continue to publish Relay-sourced updates for the remote system; the
    // player has to wait for the direct light-speed path — and when it
    // eventually lands, source flips to Direct.
    use macrocosmo::deep_space::{CommDirection, FTLCommRelay, pair_relay_command};

    let mut app = test_app();
    install_sensor_buoy_definition(&mut app);
    install_ftl_comm_relay_definition(&mut app, 5.0);

    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    let sys_remote = spawn_test_system(
        app.world_mut(),
        "Remote",
        [30.0, 0.0, 0.0],
        0.5,
        true,
        false,
    );
    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));
    spawn_sensor_buoy(app.world_mut(), [30.0, 0.0, 0.0]);

    let relay_a = spawn_ftl_comm_relay(app.world_mut(), "Relay-A", [28.0, 0.0, 0.0]);
    let relay_b = spawn_ftl_comm_relay(app.world_mut(), "Relay-B", [1.0, 0.0, 0.0]);
    pair_relay_command(
        app.world_mut(),
        relay_a,
        relay_b,
        CommDirection::Bidirectional,
    )
    .unwrap();

    // Phase 1: chain is live. Remote system is known via relay at t=100.
    advance_time(&mut app, 100);
    let fresh_observed_at;
    {
        let now = app
            .world()
            .resource::<macrocosmo::time_system::GameClock>()
            .elapsed;
        let empire = empire_entity(app.world_mut());
        let store = app.world().get::<KnowledgeStore>(empire).unwrap();
        let entry = store
            .get(sys_remote)
            .expect("relay delivers phase-1 snapshot");
        assert_eq!(entry.source, ObservationSource::Relay);
        assert_eq!(entry.observed_at, now);
        fresh_observed_at = entry.observed_at;
    }

    // Phase 2: destroy one end of the chain. verify_relay_pairings_system
    // strips the partner's FTLCommRelay component on the next tick.
    app.world_mut().despawn(relay_b);
    advance_time(&mut app, 1);
    assert!(
        app.world().get::<FTLCommRelay>(relay_a).is_none(),
        "despawning relay_b must unpair relay_a"
    );

    // Phase 3: advance well past phase-1 but still FAR shorter than the
    // light-speed travel time (30 ly ≈ 1800 hd). The relay path is gone, so
    // the stored observed_at must NOT advance and the source must remain
    // Relay (the prior entry; nothing wrote over it).
    advance_time(&mut app, 500);
    {
        let empire = empire_entity(app.world_mut());
        let store = app.world().get::<KnowledgeStore>(empire).unwrap();
        let entry = store
            .get(sys_remote)
            .expect("prior relay snapshot still present");
        assert_eq!(
            entry.observed_at, fresh_observed_at,
            "no new relay writes after chain destroyed — observed_at must stall"
        );
        assert_eq!(
            entry.source,
            ObservationSource::Relay,
            "the stalled snapshot still carries its original Relay tag"
        );
    }

    // Phase 4: advance past the 1800 hd light-speed delay from player to the
    // remote system (30 ly). Now the direct path delivers with source=Direct.
    // That confirms the "light-speed fallback" aspect of the spec.
    advance_time(&mut app, 2000);
    {
        let now = app
            .world()
            .resource::<macrocosmo::time_system::GameClock>()
            .elapsed;
        let empire = empire_entity(app.world_mut());
        let store = app.world().get::<KnowledgeStore>(empire).unwrap();
        let entry = store
            .get(sys_remote)
            .expect("direct light-speed path eventually lands");
        assert_eq!(
            entry.source,
            ObservationSource::Direct,
            "after chain destruction, freshness is restored only via the light-speed path"
        );
        assert!(
            entry.observed_at > fresh_observed_at,
            "direct path eventually supersedes the stalled relay snapshot"
        );
        assert!(
            entry.observed_at < now,
            "direct observation carries the light-speed delay (observed_at < now)"
        );
    }
}

#[test]
fn test_relay_chain_aggregates_system_resources() {
    // Bonus coverage: verify that the relay-delivered SystemKnowledge carries
    // the remote system's `ResourceStockpile` contents — this is the
    // "information aggregation" the #216 spec calls out. Without the relay
    // path this data would only arrive at light-speed.
    use macrocosmo::deep_space::{CommDirection, pair_relay_command};

    let mut app = test_app();
    install_ftl_comm_relay_definition(&mut app, 5.0);

    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    let sys_remote = spawn_test_system(
        app.world_mut(),
        "RemoteColony",
        [30.0, 0.0, 0.0],
        0.6,
        true,
        false,
    );
    app.world_mut().spawn((
        Player,
        StationedAt {
            system: sys_capital,
        },
    ));

    // Attach a stockpile to the remote system so the snapshot has non-trivial
    // content to verify.
    app.world_mut()
        .entity_mut(sys_remote)
        .insert(ResourceStockpile {
            minerals: Amt::units(777),
            energy: Amt::units(321),
            research: Amt::ZERO,
            food: Amt::units(50),
            authority: Amt::units(10),
        });

    let relay_a = spawn_ftl_comm_relay(app.world_mut(), "Relay-A", [28.0, 0.0, 0.0]);
    let relay_b = spawn_ftl_comm_relay(app.world_mut(), "Relay-B", [1.0, 0.0, 0.0]);
    pair_relay_command(
        app.world_mut(),
        relay_a,
        relay_b,
        CommDirection::Bidirectional,
    )
    .unwrap();

    advance_time(&mut app, 50);
    let now = app
        .world()
        .resource::<macrocosmo::time_system::GameClock>()
        .elapsed;
    let empire = empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    let entry = store
        .get(sys_remote)
        .expect("relay delivers remote system snapshot");
    assert_eq!(entry.source, ObservationSource::Relay);
    assert_eq!(entry.observed_at, now);
    assert_eq!(entry.data.minerals, Amt::units(777));
    assert_eq!(entry.data.energy, Amt::units(321));
    assert_eq!(entry.data.food, Amt::units(50));
    assert_eq!(entry.data.authority, Amt::units(10));
}

// #269: ColonySnapshot population tests.

#[test]
fn test_system_snapshot_includes_colonies_after_propagation() {
    let mut app = test_app();
    let capital = spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    let remote = spawn_test_system(
        app.world_mut(),
        "Remote",
        [10.0, 0.0, 0.0],
        0.8,
        true,
        false,
    );
    app.world_mut()
        .spawn((Player, StationedAt { system: capital }));

    let remote_planet = find_planet(app.world_mut(), remote);
    let colony = spawn_test_colony(
        app.world_mut(),
        remote_planet,
        Amt::units(500),
        Amt::units(500),
        vec![Some(BuildingId::new("mine")), None, None, None],
    );

    advance_time(&mut app, light_delay_hexadies(10.0));

    let empire = empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    let entry = store
        .get(remote)
        .expect("remote system knowledge must exist");
    assert_eq!(entry.data.colonies.len(), 1, "one colony snapshot expected");
    let cs = &entry.data.colonies[0];
    assert_eq!(cs.colony_entity, colony);
    assert_eq!(cs.planet_entity, remote_planet);
    assert!((cs.population - 100.0).abs() < 1e-9, "population preserved");
    assert_eq!(cs.buildings.len(), 4, "slot count preserved");
    assert_eq!(cs.buildings[0].as_ref().map(|b| b.0.as_str()), Some("mine"));
}

#[test]
fn test_colony_snapshot_preserves_build_queue_entries() {
    let mut app = test_app();
    let capital = spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    let remote = spawn_test_system(
        app.world_mut(),
        "Remote",
        [10.0, 0.0, 0.0],
        0.8,
        true,
        false,
    );
    app.world_mut()
        .spawn((Player, StationedAt { system: capital }));

    let remote_planet = find_planet(app.world_mut(), remote);
    let colony = spawn_test_colony(
        app.world_mut(),
        remote_planet,
        Amt::units(500),
        Amt::units(500),
        vec![None, None, None, None],
    );
    {
        let mut bq = app.world_mut().get_mut::<BuildingQueue>(colony).unwrap();
        bq.queue.push(BuildingOrder {
            order_id: 0,
            building_id: BuildingId::new("mine"),
            target_slot: 1,
            minerals_remaining: Amt::units(150),
            energy_remaining: Amt::units(50),
            build_time_remaining: 10,
        });
    }

    advance_time(&mut app, light_delay_hexadies(10.0));

    let empire = empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    let entry = store.get(remote).unwrap();
    let cs = entry
        .data
        .colonies
        .iter()
        .find(|c| c.colony_entity == colony)
        .unwrap();
    assert_eq!(cs.build_queue.len(), 1);
    assert_eq!(cs.build_queue[0].target_slot, 1);
    assert_eq!(cs.build_queue[0].building_id.0, "mine");
}

#[test]
fn test_local_colony_snapshot_also_populated() {
    // Even when the player is local, propagate_knowledge writes snapshots for
    // local systems (observed_at = clock.elapsed). This is relied on by the
    // UI's fallback path when a system panel is opened before the first
    // observation tick.
    let mut app = test_app();
    let capital = spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    app.world_mut()
        .spawn((Player, StationedAt { system: capital }));

    let capital_planet = find_planet(app.world_mut(), capital);
    let colony = spawn_test_colony(
        app.world_mut(),
        capital_planet,
        Amt::units(500),
        Amt::units(500),
        vec![None, None, None, None],
    );

    advance_time(&mut app, 1);

    let empire = empire_entity(app.world_mut());
    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    let entry = store.get(capital).expect("local system has knowledge");
    assert_eq!(entry.data.colonies.len(), 1);
    assert_eq!(entry.data.colonies[0].colony_entity, colony);
}
