//! #387: Integration tests for auto-spawning Shipyard station on colonization
//! and Core deploy.

mod common;

use bevy::prelude::*;
use common::{advance_time, empire_entity, spawn_test_system_with_planet, test_app};
use macrocosmo::amount::Amt;
use macrocosmo::colony::{
    ColonizationOrder, ColonizationQueue, ResourceCapacity, ResourceStockpile,
};
use macrocosmo::components::Position;
use macrocosmo::faction::FactionOwner;
use macrocosmo::ship::{Ship, ShipState};

/// Count how many ships with a given design_id exist InSystem at a given system.
fn count_station_ships(world: &mut World, design_id: &str, system: Entity) -> usize {
    let mut query = world.query::<(&Ship, &ShipState)>();
    query
        .iter(world)
        .filter(|(ship, state)| {
            ship.design_id == design_id
                && matches!(state, ShipState::InSystem { system: s } if *s == system)
        })
        .count()
}

/// Helper: set up a system with a colony source and a colonization order that
/// completes in 1 tick (zero cost, zero build time).
fn setup_colonization_order(app: &mut App) -> (Entity, Entity) {
    let empire = empire_entity(app.world_mut());
    let (sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [5.0, 0.0, 0.0], 1.0, true);

    // Spawn a source colony on a different planet in the same system so
    // FactionOwner propagation works.
    let source_planet = app
        .world_mut()
        .spawn((
            macrocosmo::galaxy::Planet {
                name: "Source I".to_string(),
                system: sys,
                planet_type: "default".to_string(),
            },
            macrocosmo::galaxy::SystemAttributes {
                habitability: 1.0,
                mineral_richness: 0.5,
                energy_potential: 0.5,
                research_potential: 0.5,
                max_building_slots: 4,
            },
            Position::from([5.0, 0.0, 0.0]),
        ))
        .id();

    let source_colony = app
        .world_mut()
        .spawn((
            macrocosmo::colony::Colony {
                planet: source_planet,
                growth_rate: 0.01,
            },
            FactionOwner(empire),
        ))
        .id();

    // Add resource stockpile + capacity to system
    app.world_mut().entity_mut(sys).insert((
        ResourceStockpile {
            minerals: Amt::units(1000),
            energy: Amt::units(1000),
            research: Amt::ZERO,
            food: Amt::units(200),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
        macrocosmo::colony::SystemBuildings::default(),
        macrocosmo::colony::SystemBuildingQueue::default(),
    ));

    // Queue a colonization order that completes immediately (zero cost + 1 tick build).
    app.world_mut().entity_mut(sys).insert(ColonizationQueue {
        orders: vec![ColonizationOrder {
            target_planet: planet,
            source_colony,
            minerals_remaining: Amt::ZERO,
            energy_remaining: Amt::ZERO,
            build_time_remaining: 1,
            initial_population: 10.0,
        }],
    });

    // Spawn a Core ship so sovereignty is established (required for settling).
    common::spawn_mock_core_ship(app.world_mut(), sys, empire);

    (sys, planet)
}

#[test]
fn test_colonization_auto_spawns_shipyard() {
    let mut app = test_app();
    let (sys, _planet) = setup_colonization_order(&mut app);

    // Before advancing time, no station_shipyard_v1 should exist.
    assert_eq!(
        count_station_ships(app.world_mut(), "station_shipyard_v1", sys),
        0,
        "no Shipyard should exist before colonization completes"
    );

    // Advance 1 tick to complete the colonization order.
    advance_time(&mut app, 1);

    // A station_shipyard_v1 ship should now exist at the system.
    assert_eq!(
        count_station_ships(app.world_mut(), "station_shipyard_v1", sys),
        1,
        "exactly one Shipyard should be auto-spawned on colonization"
    );
}

#[test]
fn test_colonization_does_not_duplicate_shipyard() {
    let mut app = test_app();
    let (sys, _planet) = setup_colonization_order(&mut app);

    // Pre-spawn a station_shipyard_v1 ship at this system so the duplicate
    // check should prevent a second one from being created.
    let empire = empire_entity(app.world_mut());
    let pos = *app.world().get::<Position>(sys).unwrap();
    // Spawn directly in tests (no Commands needed):
    app.world_mut().spawn((
        Ship {
            name: "Existing Shipyard".to_string(),
            design_id: "station_shipyard_v1".to_string(),
            hull_id: "station_shipyard_hull".to_string(),
            modules: Vec::new(),
            owner: macrocosmo::ship::Owner::Empire(empire),
            sublight_speed: 0.0,
            ftl_range: 0.0,
            player_aboard: false,
            home_port: sys,
            design_revision: 0,
            fleet: None,
        },
        ShipState::InSystem { system: sys },
        pos,
        macrocosmo::ship::ShipHitpoints {
            hull: 200.0,
            hull_max: 200.0,
            armor: 0.0,
            armor_max: 0.0,
            shield: 0.0,
            shield_max: 0.0,
            shield_regen: 0.0,
        },
        macrocosmo::ship::CommandQueue::default(),
        macrocosmo::ship::Cargo::default(),
        macrocosmo::ship::ShipModifiers::default(),
        macrocosmo::ship::ShipStats::default(),
        macrocosmo::ship::RulesOfEngagement::default(),
        FactionOwner(empire),
    ));

    assert_eq!(
        count_station_ships(app.world_mut(), "station_shipyard_v1", sys),
        1,
        "one pre-existing Shipyard should be present"
    );

    // Advance 1 tick to complete the colonization order.
    advance_time(&mut app, 1);

    // Still only one Shipyard should exist — no duplicate.
    assert_eq!(
        count_station_ships(app.world_mut(), "station_shipyard_v1", sys),
        1,
        "colonization should not duplicate an existing Shipyard"
    );
}

#[test]
fn test_core_deploy_auto_spawns_shipyard() {
    use macrocosmo::ship::Owner;
    use macrocosmo::ship::command_events::{CommandId, CoreDeployRequested};

    let mut app = test_app();
    let empire = empire_entity(app.world_mut());
    let (sys, _planet) =
        spawn_test_system_with_planet(app.world_mut(), "Empty", [10.0, 0.0, 0.0], 0.5, true);

    // Insert the infrastructure_core_v1 design if not present.
    {
        let mut reg = app
            .world_mut()
            .resource_mut::<macrocosmo::ship_design::ShipDesignRegistry>();
        if reg.get("infrastructure_core_v1").is_none() {
            reg.insert(macrocosmo::ship_design::ShipDesignDefinition {
                id: "infrastructure_core_v1".to_string(),
                name: "Infrastructure Core".to_string(),
                description: String::new(),
                hull_id: "infrastructure_core_hull".to_string(),
                modules: Vec::new(),
                can_survey: false,
                can_colonize: false,
                maintenance: Amt::units(2),
                build_cost_minerals: Amt::ZERO,
                build_cost_energy: Amt::ZERO,
                build_time: 0,
                hp: 400.0,
                sublight_speed: 0.0,
                ftl_range: 0.0,
                revision: 0,
                is_direct_buildable: true,
            });
        }
    }

    // No Shipyard should exist initially.
    assert_eq!(
        count_station_ships(app.world_mut(), "station_shipyard_v1", sys),
        0,
    );

    // Spawn a dummy deployer entity (the CoreDeployRequested message needs one).
    let deployer = app.world_mut().spawn_empty().id();

    // Write a CoreDeployRequested message.
    {
        let pos = app
            .world()
            .get::<Position>(sys)
            .map(|p| p.as_array())
            .unwrap_or([10.0, 0.0, 0.0]);
        let mut msgs = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<CoreDeployRequested>>();
        msgs.write(CoreDeployRequested {
            command_id: CommandId::ZERO,
            deployer,
            target_system: sys,
            deploy_pos: pos,
            design_id: "infrastructure_core_v1".to_string(),
            owner: Owner::Empire(empire),
            faction_owner: Some(empire),
            submitted_at: 0,
        });
    }

    // Run one update to process the Core deploy.
    advance_time(&mut app, 1);

    // A Shipyard should have been auto-spawned alongside the Core.
    assert_eq!(
        count_station_ships(app.world_mut(), "station_shipyard_v1", sys),
        1,
        "Shipyard should be auto-spawned on Core deploy"
    );
}
