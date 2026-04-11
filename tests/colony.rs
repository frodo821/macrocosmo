mod common;

use bevy::prelude::*;
use macrocosmo::amount::Amt;
use macrocosmo::colony::*;
use macrocosmo::modifier::ModifiedValue;
use macrocosmo::components::Position;
use macrocosmo::galaxy::{Habitability, Planet, ResourceLevel, StarSystem, SystemAttributes, Sovereignty};
use macrocosmo::ship::*;

use common::{advance_time, find_planet, spawn_test_colony, spawn_test_system, test_app};

/// Helper: add ResourceStockpile and ResourceCapacity to a star system entity.
/// If the system already has a stockpile, it replaces it.
fn set_system_stockpile(world: &mut World, sys: Entity, stockpile: ResourceStockpile) {
    world.entity_mut(sys).insert((stockpile, ResourceCapacity::default()));
}

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
    set_system_stockpile(app.world_mut(), sys, ResourceStockpile {
        minerals: Amt::ZERO,
        energy: Amt::ZERO,
        research: Amt::ZERO,
        food: Amt::ZERO,
        authority: Amt::ZERO,
    });
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 50.0,
            growth_rate: 0.005,
        },
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
    let (minerals_cost, energy_cost) = (Amt::units(150), Amt::units(50));
    let build_time = 10;

    let planet_sys = find_planet(app.world_mut(), sys);
    set_system_stockpile(app.world_mut(), sys, ResourceStockpile {
            minerals: Amt::units(500),
            energy: Amt::units(500),
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        });
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
        },
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
                building_id: BuildingId::new("mine"),
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
        Some(BuildingId::new("mine")),
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

    let demo_time = 5;
    let (m_refund, e_refund) = (Amt::milli(150000/2), Amt::milli(50000/2));

    let planet_sys = find_planet(app.world_mut(), sys);
    set_system_stockpile(app.world_mut(), sys, ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        });
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
        },
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings {
            slots: vec![Some(BuildingId::new("mine")), None, None, None],
        },
        BuildingQueue {
            queue: Vec::new(),
            demolition_queue: vec![DemolitionOrder {
                target_slot: 0,
                building_id: BuildingId::new("mine"),
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

    let demo_time = 5;
    let (m_refund, e_refund) = (Amt::milli(150000/2), Amt::milli(50000/2));

    let planet_sys = find_planet(app.world_mut(), sys);
    set_system_stockpile(app.world_mut(), sys, ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        });
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
        },
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings {
            slots: vec![Some(BuildingId::new("mine")), None, None, None],
        },
        BuildingQueue {
            queue: Vec::new(),
            demolition_queue: vec![DemolitionOrder {
                target_slot: 0,
                building_id: BuildingId::new("mine"),
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

    let demo_time = 15; // 30 / 2 = 15
    let (m_refund, e_refund) = (Amt::milli(300000/2), Amt::milli(200000/2));

    let planet_sys = find_planet(app.world_mut(), sys);
    set_system_stockpile(app.world_mut(), sys, ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        });
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
        },
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        },
        BuildQueue { queue: Vec::new() },
        Buildings {
            slots: vec![Some(BuildingId::new("shipyard")), None, None, None],
        },
        BuildingQueue {
            queue: Vec::new(),
            demolition_queue: vec![DemolitionOrder {
                target_slot: 0,
                building_id: BuildingId::new("shipyard"),
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
            Some(BuildingId::new("shipyard")),
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
    set_system_stockpile(app.world_mut(), sys, ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::units(100),
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        });
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
        },
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::units(5)),
        },
        BuildQueue { queue: Vec::new() },
        Buildings {
            slots: vec![Some(BuildingId::new("farm"))],
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
    let _remote_sys = spawn_test_system(
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
    set_system_stockpile(app.world_mut(), cap_sys, ResourceStockpile {
            minerals: Amt::units(1000),
            energy: Amt::units(1000),
            research: Amt::ZERO,
            food: Amt::units(1000),
            authority: Amt::ZERO,
        });
    app.world_mut().spawn((
        Colony {
            planet: planet_cap_sys,
            population: 1.0,
            growth_rate: 0.0,
        },
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
        set_system_stockpile(app.world_mut(), sys, ResourceStockpile {
                minerals: Amt::ZERO,
                energy: Amt::ZERO,
                research: Amt::ZERO,
                food: Amt::ZERO,
                authority: Amt::ZERO,
            });
        app.world_mut().spawn((
            Colony {
                planet: planet_sys,
                population: 1.0,
                growth_rate: 0.0,
            },
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
    let stockpile = app.world().get::<ResourceStockpile>(remote_systems[0]).unwrap();
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
    set_system_stockpile(app.world_mut(), sys, ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::units(100),
            research: Amt::ZERO,
            food: Amt::units(10000),
            authority: Amt::ZERO,
        });
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
        },
        Production {
            minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
            energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
            research_per_hexadies: ModifiedValue::new(Amt::ZERO),
            food_per_hexadies: ModifiedValue::new(Amt::units(10)),
        },
        BuildQueue { queue: Vec::new() },
        Buildings {
            slots: vec![Some(BuildingId::new("mine")), Some(BuildingId::new("shipyard"))],
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
    set_system_stockpile(app.world_mut(), sys, ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::units(10000),
            authority: Amt::ZERO,
        });
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 70.0,
            growth_rate: 0.05,
        },
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
    set_system_stockpile(app.world_mut(), sys, ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::units(10000),
            authority: Amt::ZERO,
        });
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 40.0,
            growth_rate: 0.05,
        },
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
    app.world_mut().entity_mut(sys).insert((
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
    ));
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
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
    set_system_stockpile(app.world_mut(), sys, ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        });
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
        },
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
    let (minerals_cost, energy_cost) = (Amt::units(150), Amt::units(50));
    let build_time = 10;

    let planet_sys = find_planet(app.world_mut(), sys);
    set_system_stockpile(app.world_mut(), sys, ResourceStockpile {
            minerals: Amt::units(20),
            energy: Amt::units(200),
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        });
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
        },
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
                building_id: BuildingId::new("mine"),
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
        Some(BuildingId::new("mine")),
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
    set_system_stockpile(app.world_mut(), sys, ResourceStockpile {
            minerals: Amt::units(500),
            energy: Amt::units(500),
            research: Amt::ZERO,
            food: Amt::units(100),
            authority: Amt::ZERO,
        });
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 10.0,
            growth_rate: 0.0,
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
                minerals_cost: Amt::units(100),
                minerals_invested: Amt::ZERO,
                energy_cost: Amt::units(50),
                energy_invested: Amt::ZERO,
                build_time_total: 60,
                build_time_remaining: 60,
            }],
        },
        Buildings { slots: vec![Some(BuildingId::new("mine")), None, None, None] }, // No Shipyard!
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
    set_system_stockpile(app.world_mut(), sys, ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::ZERO, // No food!
            authority: Amt::ZERO,
        });
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
    set_system_stockpile(app.world_mut(), sys, ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        });
    app.world_mut().spawn((
        Colony {
            planet: planet_sys,
            population: 1.5,
            growth_rate: 0.01,
        },
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

// Authority production and consumption (#73)

#[test]
fn test_capital_produces_authority() {
    let mut app = test_app();

    let cap_sys = spawn_capital_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0]);

    // Spawn capital colony with zero authority
    let planet_cap_sys = find_planet(app.world_mut(), cap_sys);
    set_system_stockpile(app.world_mut(), cap_sys, ResourceStockpile {
            minerals: Amt::units(500),
            energy: Amt::units(500),
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        });
    let colony_entity = app.world_mut().spawn((
        Colony {
            planet: planet_cap_sys,
            population: 100.0,
            growth_rate: 0.01,
        },
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

    let stockpile = app.world().get::<ResourceStockpile>(cap_sys).unwrap();
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
    set_system_stockpile(app.world_mut(), cap_sys, ResourceStockpile {
            minerals: Amt::units(500),
            energy: Amt::units(500),
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::units(5), // start with 5
        });
    let capital_colony = app.world_mut().spawn((
        Colony {
            planet: planet_cap_sys,
            population: 100.0,
            growth_rate: 0.01,
        },
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
    set_system_stockpile(app.world_mut(), remote_sys, ResourceStockpile {
            minerals: Amt::units(100),
            energy: Amt::units(100),
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        });
    app.world_mut().spawn((
        Colony {
            planet: planet_remote_sys,
            population: 50.0,
            growth_rate: 0.005,
        },
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

    let stockpile = app.world().get::<ResourceStockpile>(cap_sys).unwrap();
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
    set_system_stockpile(app.world_mut(), cap_sys, ResourceStockpile {
            minerals: Amt::units(500),
            energy: Amt::units(500),
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        });
    app.world_mut().spawn((
        Colony {
            planet: planet_cap_sys,
            population: 100.0,
            growth_rate: 0.01,
        },
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
    set_system_stockpile(app.world_mut(), remote_sys, ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        });
    let remote_colony = app.world_mut().spawn((
        Colony {
            planet: planet_remote_sys,
            population: 50.0,
            growth_rate: 0.005,
        },
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
    set_system_stockpile(app.world_mut(), remote_sys2, ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        });
    app.world_mut().spawn((
        Colony {
            planet: planet_remote_sys2,
            population: 50.0,
            growth_rate: 0.005,
        },
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
    set_system_stockpile(app.world_mut(), remote_sys3, ResourceStockpile {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            research: Amt::ZERO,
            food: Amt::ZERO,
            authority: Amt::ZERO,
        });
    app.world_mut().spawn((
        Colony {
            planet: planet_remote_sys3,
            population: 50.0,
            growth_rate: 0.005,
        },
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

    let stockpile = app.world().get::<ResourceStockpile>(remote_sys).unwrap();
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
