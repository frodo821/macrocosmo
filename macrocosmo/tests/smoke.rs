mod common;

use bevy::prelude::*;
use macrocosmo::amount::Amt;
use macrocosmo::colony::*;
use macrocosmo::components::Position;
use macrocosmo::galaxy::{Planet, Sovereignty, StarSystem, SystemAttributes};
use macrocosmo::modifier::ModifiedValue;
use macrocosmo::player::*;
use macrocosmo::ship::*;
use macrocosmo::time_system::GameClock;

use common::spawn_test_system;

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
            Sovereignty {
                owner: None,
                control_score: 100.0,
            },
        ))
        .id();
    let capital_planet = app
        .world_mut()
        .spawn((
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
                max_building_slots: 6,
            },
            Position::from([0.0, 0.0, 0.0]),
        ))
        .id();

    // Second star system (unsurveyed target)
    let _target = app
        .world_mut()
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
            habitability: 0.7,
            mineral_richness: 0.8,
            energy_potential: 0.2,
            research_potential: 0.5,
            max_building_slots: 4,
        },
        Position::from([5.0, 0.0, 0.0]),
    ));

    // Third star system (surveyed, not colonized)
    let _surveyed = app
        .world_mut()
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
            habitability: 0.4,
            mineral_richness: 0.2,
            energy_potential: 0.8,
            research_potential: 0.0,
            max_building_slots: 3,
        },
        Position::from([10.0, 3.0, 0.0]),
    ));

    // Player stationed at capital
    app.world_mut()
        .spawn((Player, StationedAt { system: capital }));

    // Colony at capital
    app.world_mut().entity_mut(capital).insert((
        ResourceStockpile {
            minerals: Amt::units(500),
            energy: Amt::units(500),
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
    ));
    app.world_mut().spawn((
        Colony {
            planet: capital_planet,
            population: 100.0,
            growth_rate: 0.01,
        },
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::units(5)),
            energy_per_hexadies: ModifiedValue::new(Amt::units(5)),
            research_per_hexadies: ModifiedValue::new(Amt::units(1)),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue {
            queue: vec![],
            next_order_id: 0,
        },
        Buildings {
            slots: vec![
                Some(BuildingId::new("mine")),
                Some(BuildingId::new("shipyard")),
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
            design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 0.75,
            ftl_range: 0.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
            fleet: None,
        },
        ShipState::InSystem { system: capital },
        Position::from([0.0, 0.0, 0.0]),
        CommandQueue::default(),
        Cargo::default(),
    ));

    // Colony ship docked at capital
    app.world_mut().spawn((
        Ship {
            name: "Colony Ship-1".into(),
            design_id: "colony_ship_mk1".to_string(),
            hull_id: "freighter".to_string(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 0.5,
            ftl_range: 30.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
            fleet: None,
        },
        ShipState::InSystem { system: capital },
        Position::from([0.0, 0.0, 0.0]),
        CommandQueue::default(),
        Cargo::default(),
    ));

    // Courier docked at capital
    app.world_mut().spawn((
        Ship {
            name: "Courier-1".into(),
            design_id: "courier_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 0.85,
            ftl_range: 0.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
            fleet: None,
        },
        ShipState::InSystem { system: capital },
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
