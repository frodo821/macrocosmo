mod common;

use bevy::prelude::*;
use macrocosmo::amount::Amt;
use macrocosmo::colony::*;
use macrocosmo::modifier::ModifiedValue;

use common::{advance_time, find_planet, spawn_test_system, test_app};

#[test]
fn test_job_auto_assignment() {
    use macrocosmo::species::*;

    let mut app = test_app();

    let sys = common::spawn_test_system(
        app.world_mut(),
        "Job Test",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );

    // Spawn a colony with population 10, job slots [miner:5, farmer:5]
    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().entity_mut(sys).insert((
        ResourceStockpile {
            minerals: Amt::units(100),
            energy: Amt::units(100),
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
    ));
    let colony = app
        .world_mut()
        .spawn((
            Colony {
                planet: planet_sys,
                population: 10.0,
                growth_rate: 0.01,
            },
            Production {
                minerals_per_hexadies: ModifiedValue::new(Amt::units(5)),
                energy_per_hexadies: ModifiedValue::new(Amt::units(5)),
                research_per_hexadies: ModifiedValue::new(Amt::units(1)),
                food_per_hexadies: ModifiedValue::new(Amt::ZERO),
            },
            BuildQueue::default(),
            Buildings {
                slots: vec![None; 4],
            },
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
                        capacity_from_buildings: 0,
                    },
                    JobSlot {
                        job_id: "farmer".to_string(),
                        capacity: 5,
                        assigned: 0,
                        capacity_from_buildings: 0,
                    },
                ],
            },
        ))
        .id();

    // Run one update to trigger sync_job_assignment
    advance_time(&mut app, 1);

    // Verify all 10 pops are assigned
    let jobs = app.world().get::<ColonyJobs>(colony).unwrap();
    assert_eq!(jobs.total_employed(), 10);
    assert_eq!(jobs.slots[0].assigned, 5); // miner full
    assert_eq!(jobs.slots[1].assigned, 5); // farmer full

    // Now reduce population to 7
    app.world_mut()
        .get_mut::<ColonyPopulation>(colony)
        .unwrap()
        .species[0]
        .population = 7;

    advance_time(&mut app, 1);

    // Verify assignment adjusts: miner=5, farmer=2
    let jobs = app.world().get::<ColonyJobs>(colony).unwrap();
    assert_eq!(jobs.total_employed(), 7);
    assert_eq!(jobs.slots[0].assigned, 5); // miner still full
    assert_eq!(jobs.slots[1].assigned, 2); // farmer reduced

    // Reduce population to 3
    app.world_mut()
        .get_mut::<ColonyPopulation>(colony)
        .unwrap()
        .species[0]
        .population = 3;

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
        1.0,
        true,
        true,
    );

    let planet_sys = find_planet(app.world_mut(), sys);
    app.world_mut().entity_mut(sys).insert((
        ResourceStockpile {
            minerals: Amt::units(100),
            energy: Amt::units(100),
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        },
        ResourceCapacity::default(),
    ));
    let colony = app
        .world_mut()
        .spawn((
            Colony {
                planet: planet_sys,
                population: 15.0,
                growth_rate: 0.01,
            },
            Production {
                minerals_per_hexadies: ModifiedValue::new(Amt::units(5)),
                energy_per_hexadies: ModifiedValue::new(Amt::units(5)),
                research_per_hexadies: ModifiedValue::new(Amt::units(1)),
                food_per_hexadies: ModifiedValue::new(Amt::ZERO),
            },
            BuildQueue::default(),
            Buildings {
                slots: vec![None; 4],
            },
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
                        capacity_from_buildings: 0,
                    },
                    JobSlot {
                        job_id: "farmer".to_string(),
                        capacity: 5,
                        assigned: 0,
                        capacity_from_buildings: 0,
                    },
                ],
            },
        ))
        .id();

    advance_time(&mut app, 1);

    // 15 pop but only 10 capacity -> 10 employed, 5 unemployed
    let jobs = app.world().get::<ColonyJobs>(colony).unwrap();
    let pop = app.world().get::<ColonyPopulation>(colony).unwrap();
    assert_eq!(jobs.total_employed(), 10);
    assert_eq!(pop.total() - jobs.total_employed(), 5); // 5 unemployed
}
