mod common;

use bevy::prelude::*;
use macrocosmo::amount::{Amt, SignedAmt};
use macrocosmo::colony::*;
use macrocosmo::galaxy::Habitability;
use macrocosmo::modifier::{ModifiedValue, Modifier};
use macrocosmo::ship::*;

use common::{advance_time, empire_entity, find_planet, spawn_test_colony, spawn_test_system, test_app};

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
    app.world_mut().get_mut::<macrocosmo::galaxy::StarSystem>(sys).unwrap().is_capital = true;

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
        "explorer_mk1",
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

// Timed modifier expiry in-game

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
