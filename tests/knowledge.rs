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
