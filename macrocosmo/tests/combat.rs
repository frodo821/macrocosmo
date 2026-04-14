mod common;

use bevy::prelude::*;
use macrocosmo::amount::Amt;
use macrocosmo::components::Position;
use macrocosmo::galaxy::{AtSystem, Hostile, HostileHitpoints, HostilePresence, HostileStats, HostileType};
use macrocosmo::ship::*;

use common::{advance_time, spawn_test_system, test_app};

#[test]
fn test_hostile_destroyed_when_hp_zero() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Battle-System",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    // Spawn a hostile with low HP so it gets destroyed quickly
    let hostile_entity = app.world_mut().spawn((AtSystem(sys), HostileHitpoints { hp: 0.05, max_hp: 10.0 }, HostileStats { strength: 0.0, evasion: 0.0 }, Hostile)).id();

    // Register a weapon module in the ModuleRegistry
    app.world_mut().resource_mut::<macrocosmo::ship_design::ModuleRegistry>().insert(
        macrocosmo::ship_design::ModuleDefinition {
            id: "test_laser".to_string(),
            name: "Test Laser".to_string(),
            description: String::new(),
            slot_type: "weapon".to_string(),
            modifiers: Vec::new(),
            weapon: Some(macrocosmo::ship_design::WeaponStats {
                track: 100.0,
                precision: 1.0,
                cooldown: 1,
                range: 100.0,
                shield_damage: 5.0,
                shield_damage_div: 0.0,
                shield_piercing: 0.0,
                armor_damage: 5.0,
                armor_damage_div: 0.0,
                armor_piercing: 0.0,
                hull_damage: 5.0,
                hull_damage_div: 0.0,
            }),
            cost_minerals: Amt::ZERO,
            cost_energy: Amt::ZERO,
            prerequisites: None,
            upgrade_to: Vec::new(),
        },
    );

    // Spawn a strong explorer docked at that system with a weapon module
    app.world_mut().spawn((
        Ship {
            name: "Warship-1".to_string(),
            design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: vec![EquippedModule { slot_type: "weapon".to_string(), module_id: "test_laser".to_string() }],
            owner: Owner::Neutral,
            sublight_speed: 0.75,
            ftl_range: 0.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
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
        0.7,
        true,
        false,
    );

    // Spawn a powerful hostile
    app.world_mut().spawn((AtSystem(sys), HostileHitpoints { hp: 1000.0, max_hp: 1000.0 }, HostileStats { strength: 100.0, evasion: 0.0 }, Hostile));

    // Spawn a very weak ship with nearly no hull HP
    let ship_entity = app.world_mut().spawn((
        Ship {
            name: "Doomed-1".to_string(),
            design_id: "courier_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 0.85,
            ftl_range: 0.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
        },
        ShipState::Docked { system: sys },
        Position::from([0.0, 0.0, 0.0]),
        ShipHitpoints {
            hull: 0.01, hull_max: 20.0,
            armor: 0.0, armor_max: 0.0,
            shield: 0.0, shield_max: 0.0,
            shield_regen: 0.0,
        },
        ShipModifiers::default(),
        CommandQueue::default(),
        Cargo::default(),
    )).id();

    // Run one tick of combat
    advance_time(&mut app, 1);

    // Ship should be destroyed (hull <= 0)
    assert!(
        app.world().get_entity(ship_entity).is_err(),
        "Ship should be despawned after hull reaches 0 in combat"
    );
}

#[test]
fn test_no_combat_when_no_ships_present() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Empty-System",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    // Spawn hostile - should not be affected without ships present
    let hostile_entity = app.world_mut().spawn((AtSystem(sys), HostileHitpoints { hp: 10.0, max_hp: 10.0 }, HostileStats { strength: 5.0, evasion: 0.0 }, Hostile)).id();

    advance_time(&mut app, 1);

    // Hostile should still exist with full HP
    let hostile = app.world().get::<HostileHitpoints>(hostile_entity).unwrap();
    assert!((hostile.hp - 10.0).abs() < f64::EPSILON, "Hostile HP should be unchanged");
}

#[test]
fn test_combat_takes_multiple_ticks() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Prolonged-Battle",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    // Hostile with significant HP
    let hostile_entity = app.world_mut().spawn((AtSystem(sys), HostileHitpoints { hp: 1000.0, max_hp: 1000.0 }, HostileStats { strength: 0.01, evasion: 0.0 }, Hostile)).id();

    // Register a weapon module
    app.world_mut().resource_mut::<macrocosmo::ship_design::ModuleRegistry>().insert(
        macrocosmo::ship_design::ModuleDefinition {
            id: "small_laser".to_string(),
            name: "Small Laser".to_string(),
            description: String::new(),
            slot_type: "weapon".to_string(),
            modifiers: Vec::new(),
            weapon: Some(macrocosmo::ship_design::WeaponStats {
                track: 100.0,
                precision: 1.0,
                cooldown: 1,
                range: 100.0,
                shield_damage: 1.0,
                shield_damage_div: 0.0,
                shield_piercing: 0.0,
                armor_damage: 1.0,
                armor_damage_div: 0.0,
                armor_piercing: 0.0,
                hull_damage: 1.0,
                hull_damage_div: 0.0,
            }),
            cost_minerals: Amt::ZERO,
            cost_energy: Amt::ZERO,
            prerequisites: None,
            upgrade_to: Vec::new(),
        },
    );

    // Ship with a weapon module
    app.world_mut().spawn((
        Ship {
            name: "Fighter-1".to_string(),
            design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: vec![EquippedModule { slot_type: "weapon".to_string(), module_id: "small_laser".to_string() }],
            owner: Owner::Neutral,
            sublight_speed: 0.75,
            ftl_range: 0.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
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
    ));

    // After 1 tick, hostile should still be alive but damaged
    // 1 hexadies = 12 combat turns, weapon cooldown=1, precision=1.0, track=100 vs evasion=0
    // => 12 shots * 1.0 damage = 12 damage to 1000 HP hostile
    advance_time(&mut app, 1);

    let hostile = app.world().get::<HostileHitpoints>(hostile_entity).unwrap();
    assert!(hostile.hp < 1000.0, "Hostile should have taken some damage");
    assert!(hostile.hp > 0.0, "Hostile should still be alive after one tick");
}

// --- #97: 3-layer HP model tests ---

#[test]
fn test_shield_regenerates() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Shield-Test",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    // Register a shield generator module
    {
        let mut module_reg = app.world_mut().resource_mut::<macrocosmo::ship_design::ModuleRegistry>();
        module_reg.insert(macrocosmo::ship_design::ModuleDefinition {
            id: "shield_gen".to_string(),
            name: "Shield Generator".to_string(),
            description: String::new(),
            slot_type: "utility".to_string(),
            modifiers: vec![
                macrocosmo::ship_design::ModuleModifier {
                    target: "ship.shield_max".to_string(),
                    base_add: 20.0, multiplier: 0.0, add: 0.0,
                },
                macrocosmo::ship_design::ModuleModifier {
                    target: "ship.shield_regen".to_string(),
                    base_add: 2.0, multiplier: 0.0, add: 0.0,
                },
            ],
            weapon: None,
            cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisites: None,
            upgrade_to: Vec::new(),
        });
    }

    // Spawn ship with the shield module equipped
    let ship_entity = app.world_mut().spawn((
        Ship {
            name: "Shielded-1".to_string(),
            design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: vec![EquippedModule { slot_type: "utility".to_string(), module_id: "shield_gen".to_string() }],
            owner: Owner::Neutral,
            sublight_speed: 0.75,
            ftl_range: 0.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
        },
        ShipState::Docked { system: sys },
        Position::from([0.0, 0.0, 0.0]),
        ShipHitpoints {
            hull: 50.0, hull_max: 50.0,
            armor: 0.0, armor_max: 0.0,
            shield: 5.0, shield_max: 20.0,
            shield_regen: 2.0,
        },
        ShipModifiers::default(),
        CommandQueue::default(),
        Cargo::default(),
    )).id();

    // Advance 3 hexadies — shield should regenerate 2.0 * 3 = 6.0 (5+6=11)
    advance_time(&mut app, 3);

    let hp = app.world().get::<ShipHitpoints>(ship_entity).unwrap();
    assert!((hp.shield - 11.0).abs() < 0.01, "Shield should have regenerated to 11.0, got {}", hp.shield);
}

#[test]
fn test_shield_regen_caps_at_max() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Shield-Cap-Test",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    // Register a shield module with shield_max=20, shield_regen=10
    {
        let mut module_reg = app.world_mut().resource_mut::<macrocosmo::ship_design::ModuleRegistry>();
        module_reg.insert(macrocosmo::ship_design::ModuleDefinition {
            id: "big_shield".to_string(),
            name: "Big Shield".to_string(),
            description: String::new(),
            slot_type: "utility".to_string(),
            modifiers: vec![
                macrocosmo::ship_design::ModuleModifier {
                    target: "ship.shield_max".to_string(),
                    base_add: 20.0, multiplier: 0.0, add: 0.0,
                },
                macrocosmo::ship_design::ModuleModifier {
                    target: "ship.shield_regen".to_string(),
                    base_add: 10.0, multiplier: 0.0, add: 0.0,
                },
            ],
            weapon: None,
            cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisites: None,
            upgrade_to: Vec::new(),
        });
    }

    let ship_entity = app.world_mut().spawn((
        Ship {
            name: "Shield-Cap".to_string(),
            design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: vec![EquippedModule { slot_type: "utility".to_string(), module_id: "big_shield".to_string() }],
            owner: Owner::Neutral,
            sublight_speed: 0.75,
            ftl_range: 0.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
        },
        ShipState::Docked { system: sys },
        Position::from([0.0, 0.0, 0.0]),
        ShipHitpoints {
            hull: 50.0, hull_max: 50.0,
            armor: 0.0, armor_max: 0.0,
            shield: 18.0, shield_max: 20.0,
            shield_regen: 10.0,
        },
        ShipModifiers::default(),
        CommandQueue::default(),
        Cargo::default(),
    )).id();

    advance_time(&mut app, 1);

    let hp = app.world().get::<ShipHitpoints>(ship_entity).unwrap();
    assert!((hp.shield - 20.0).abs() < 0.01, "Shield should be capped at max 20.0, got {}", hp.shield);
}

#[test]
fn test_combat_damages_3_layers() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Layer-Test",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    // Spawn hostile with significant strength
    app.world_mut().spawn((AtSystem(sys), HostileHitpoints { hp: 10000.0, max_hp: 10000.0 }, HostileStats { strength: 10.0, evasion: 0.0 }, Hostile));

    // Register armor and shield modules
    {
        let mut module_reg = app.world_mut().resource_mut::<macrocosmo::ship_design::ModuleRegistry>();
        module_reg.insert(macrocosmo::ship_design::ModuleDefinition {
            id: "armor_plating".to_string(),
            name: "Armor Plating".to_string(),
            description: String::new(),
            slot_type: "utility".to_string(),
            modifiers: vec![macrocosmo::ship_design::ModuleModifier {
                target: "ship.armor_max".to_string(),
                base_add: 50.0, multiplier: 0.0, add: 0.0,
            }],
            weapon: None,
            cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisites: None,
            upgrade_to: Vec::new(),
        });
        module_reg.insert(macrocosmo::ship_design::ModuleDefinition {
            id: "shield_unit".to_string(),
            name: "Shield Unit".to_string(),
            description: String::new(),
            slot_type: "utility".to_string(),
            modifiers: vec![macrocosmo::ship_design::ModuleModifier {
                target: "ship.shield_max".to_string(),
                base_add: 30.0, multiplier: 0.0, add: 0.0,
            }],
            weapon: None,
            cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisites: None,
            upgrade_to: Vec::new(),
        });
    }

    // Ship with shield + armor + hull (modules provide the max values)
    let ship_entity = app.world_mut().spawn((
        Ship {
            name: "Tanky-1".to_string(),
            design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: vec![
                EquippedModule { slot_type: "utility".to_string(), module_id: "armor_plating".to_string() },
                EquippedModule { slot_type: "utility".to_string(), module_id: "shield_unit".to_string() },
            ],
            owner: Owner::Neutral,
            sublight_speed: 0.75,
            ftl_range: 0.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
        },
        ShipState::Docked { system: sys },
        Position::from([0.0, 0.0, 0.0]),
        ShipHitpoints {
            hull: 100.0, hull_max: 100.0,
            armor: 50.0, armor_max: 50.0,
            shield: 30.0, shield_max: 30.0,
            shield_regen: 0.0,
        },
        ShipModifiers::default(),
        CommandQueue::default(),
        Cargo::default(),
    )).id();

    // 1 hexadies = 12 combat turns, hostile strength=10 damage/turn
    // total damage = 10 * 12 = 120 distributed across shield->armor->hull
    advance_time(&mut app, 1);

    let hp = app.world().get::<ShipHitpoints>(ship_entity).unwrap();
    // Shield should be depleted first (was 30), then armor (was 50), then hull
    assert_eq!(hp.shield, 0.0, "Shield should be fully depleted");
    // Total damage = 120. Shield absorbs 30, remaining = 90. Armor absorbs 50, remaining = 40.
    // Hull = 100 - 40 = 60
    assert!(hp.armor == 0.0, "Armor should be fully depleted, got {}", hp.armor);
    assert!((hp.hull - 60.0).abs() < 0.01, "Hull should be ~60.0, got {}", hp.hull);
}

#[test]
fn test_hull_zero_destroys_ship() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Destroy-Test",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    app.world_mut().spawn((AtSystem(sys), HostileHitpoints { hp: 10000.0, max_hp: 10000.0 }, HostileStats { strength: 100.0, evasion: 0.0 }, Hostile));

    let ship_entity = app.world_mut().spawn((
        Ship {
            name: "Fragile-1".to_string(),
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
        ShipState::Docked { system: sys },
        Position::from([0.0, 0.0, 0.0]),
        ShipHitpoints {
            hull: 1.0, hull_max: 50.0,
            armor: 0.0, armor_max: 0.0,
            shield: 0.0, shield_max: 0.0,
            shield_regen: 0.0,
        },
        ShipModifiers::default(),
        CommandQueue::default(),
        Cargo::default(),
    )).id();

    advance_time(&mut app, 1);

    assert!(
        app.world().get_entity(ship_entity).is_err(),
        "Ship should be despawned when hull reaches 0"
    );
}

#[test]
fn test_weapon_cooldown() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Cooldown-Test",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    // Register two weapon types: fast (cooldown 1) and slow (cooldown 6)
    let mut module_reg = app.world_mut().resource_mut::<macrocosmo::ship_design::ModuleRegistry>();
    module_reg.insert(macrocosmo::ship_design::ModuleDefinition {
        id: "fast_gun".to_string(),
        name: "Fast Gun".to_string(),
            description: String::new(),
        slot_type: "weapon".to_string(),
        modifiers: Vec::new(),
        weapon: Some(macrocosmo::ship_design::WeaponStats {
            track: 1000.0, precision: 1.0, cooldown: 1, range: 100.0,
            shield_damage: 0.0, shield_damage_div: 0.0, shield_piercing: 0.0,
            armor_damage: 0.0, armor_damage_div: 0.0, armor_piercing: 0.0,
            hull_damage: 1.0, hull_damage_div: 0.0,
        }),
        cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisites: None,
        upgrade_to: Vec::new(),
    });
    module_reg.insert(macrocosmo::ship_design::ModuleDefinition {
        id: "slow_gun".to_string(),
        name: "Slow Gun".to_string(),
            description: String::new(),
        slot_type: "weapon".to_string(),
        modifiers: Vec::new(),
        weapon: Some(macrocosmo::ship_design::WeaponStats {
            track: 1000.0, precision: 1.0, cooldown: 6, range: 100.0,
            shield_damage: 0.0, shield_damage_div: 0.0, shield_piercing: 0.0,
            armor_damage: 0.0, armor_damage_div: 0.0, armor_piercing: 0.0,
            hull_damage: 1.0, hull_damage_div: 0.0,
        }),
        cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisites: None,
        upgrade_to: Vec::new(),
    });

    // Hostile A: attacked by fast gun
    let hostile_a = app.world_mut().spawn((AtSystem(sys), HostileHitpoints { hp: 10000.0, max_hp: 10000.0 }, HostileStats { strength: 0.0, evasion: 0.0 }, Hostile)).id();

    // Hostile B: attacked by slow gun (separate system)
    let sys_b = spawn_test_system(app.world_mut(), "Cooldown-B", [100.0, 0.0, 0.0], 0.7, true, false);
    let hostile_b = app.world_mut().spawn((AtSystem(sys_b), HostileHitpoints { hp: 10000.0, max_hp: 10000.0 }, HostileStats { strength: 0.0, evasion: 0.0 }, Hostile)).id();

    // Ship with fast gun at sys
    app.world_mut().spawn((
        Ship {
            name: "Fast-Ship".to_string(), design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: vec![EquippedModule { slot_type: "weapon".to_string(), module_id: "fast_gun".to_string() }],
            owner: Owner::Neutral, sublight_speed: 0.75, ftl_range: 0.0,
            player_aboard: false, home_port: Entity::PLACEHOLDER,
            design_revision: 0,
        },
        ShipState::Docked { system: sys },
        Position::from([0.0, 0.0, 0.0]),
        ShipHitpoints { hull: 100.0, hull_max: 100.0, armor: 0.0, armor_max: 0.0, shield: 0.0, shield_max: 0.0, shield_regen: 0.0 },
        ShipModifiers::default(), CommandQueue::default(), Cargo::default(),
    ));

    // Ship with slow gun at sys_b
    app.world_mut().spawn((
        Ship {
            name: "Slow-Ship".to_string(), design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: vec![EquippedModule { slot_type: "weapon".to_string(), module_id: "slow_gun".to_string() }],
            owner: Owner::Neutral, sublight_speed: 0.75, ftl_range: 0.0,
            player_aboard: false, home_port: Entity::PLACEHOLDER,
            design_revision: 0,
        },
        ShipState::Docked { system: sys_b },
        Position::from([100.0, 0.0, 0.0]),
        ShipHitpoints { hull: 100.0, hull_max: 100.0, armor: 0.0, armor_max: 0.0, shield: 0.0, shield_max: 0.0, shield_regen: 0.0 },
        ShipModifiers::default(), CommandQueue::default(), Cargo::default(),
    ));

    advance_time(&mut app, 1);

    let hp_a = app.world().get::<HostileHitpoints>(hostile_a).unwrap().hp;
    let hp_b = app.world().get::<HostileHitpoints>(hostile_b).unwrap().hp;
    // Fast gun (cooldown=1): 12 shots per hexadies. Slow gun (cooldown=6): 2 shots per hexadies.
    let damage_a = 10000.0 - hp_a;
    let damage_b = 10000.0 - hp_b;
    assert!(damage_a > damage_b * 3.0,
        "Fast gun should deal much more damage ({}) than slow gun ({})", damage_a, damage_b);
}

#[test]
fn test_shield_piercing() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Piercing-Test",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    // Register a weapon with 100% shield piercing
    app.world_mut().resource_mut::<macrocosmo::ship_design::ModuleRegistry>().insert(
        macrocosmo::ship_design::ModuleDefinition {
            id: "shield_piercer".to_string(),
            name: "Shield Piercer".to_string(),
            description: String::new(),
            slot_type: "weapon".to_string(),
            modifiers: Vec::new(),
            weapon: Some(macrocosmo::ship_design::WeaponStats {
                track: 1000.0, precision: 1.0, cooldown: 1, range: 100.0,
                shield_damage: 5.0, shield_damage_div: 0.0, shield_piercing: 1.0,  // always pierce shields
                armor_damage: 5.0, armor_damage_div: 0.0, armor_piercing: 1.0,     // always pierce armor
                hull_damage: 5.0, hull_damage_div: 0.0,
            }),
            cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisites: None,
            upgrade_to: Vec::new(),
        },
    );

    // Hostile with no attack
    let hostile = app.world_mut().spawn((AtSystem(sys), HostileHitpoints { hp: 10000.0, max_hp: 10000.0 }, HostileStats { strength: 0.0, evasion: 0.0 }, Hostile)).id();

    // Ship with full shields, armor, and the piercing weapon
    let _ship_entity = app.world_mut().spawn((
        Ship {
            name: "Piercer-1".to_string(), design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: vec![EquippedModule { slot_type: "weapon".to_string(), module_id: "shield_piercer".to_string() }],
            owner: Owner::Neutral, sublight_speed: 0.75, ftl_range: 0.0,
            player_aboard: false, home_port: Entity::PLACEHOLDER,
            design_revision: 0,
        },
        ShipState::Docked { system: sys },
        Position::from([0.0, 0.0, 0.0]),
        ShipHitpoints {
            hull: 100.0, hull_max: 100.0,
            armor: 50.0, armor_max: 50.0,
            shield: 100.0, shield_max: 100.0,
            shield_regen: 0.0,
        },
        ShipModifiers::default(), CommandQueue::default(), Cargo::default(),
    )).id();

    advance_time(&mut app, 1);

    // With 100% shield+armor piercing, all damage goes to hull of hostile.
    // 12 combat turns * 5 hull_damage = 60 damage.
    let h = app.world().get::<HostileHitpoints>(hostile).unwrap();
    assert!(h.hp < 10000.0, "Hostile should have taken hull damage through piercing");
    let expected_hp = 10000.0 - 60.0;
    assert!((h.hp - expected_hp).abs() < 1.0,
        "Hostile HP should be ~{}, got {}", expected_hp, h.hp);
}

// --- #57: Rules of Engagement tests ---

#[test]
fn test_default_roe_is_defensive() {
    let roe = RulesOfEngagement::default();
    assert_eq!(roe, RulesOfEngagement::Defensive);
}

#[test]
fn test_retreat_ships_skip_combat_no_damage_dealt() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Retreat-Test",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    // Hostile with no attack but trackable HP
    let hostile_entity = app.world_mut().spawn((AtSystem(sys), HostileHitpoints { hp: 100.0, max_hp: 100.0 }, HostileStats { strength: 0.0, evasion: 0.0 }, Hostile)).id();

    // Register a weapon module
    app.world_mut().resource_mut::<macrocosmo::ship_design::ModuleRegistry>().insert(
        macrocosmo::ship_design::ModuleDefinition {
            id: "roe_laser".to_string(),
            name: "ROE Laser".to_string(),
            description: String::new(),
            slot_type: "weapon".to_string(),
            modifiers: Vec::new(),
            weapon: Some(macrocosmo::ship_design::WeaponStats {
                track: 1000.0, precision: 1.0, cooldown: 1, range: 100.0,
                shield_damage: 5.0, shield_damage_div: 0.0, shield_piercing: 0.0,
                armor_damage: 5.0, armor_damage_div: 0.0, armor_piercing: 0.0,
                hull_damage: 5.0, hull_damage_div: 0.0,
            }),
            cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisites: None,
            upgrade_to: Vec::new(),
        },
    );

    // Ship with Retreat ROE — should NOT engage
    app.world_mut().spawn((
        Ship {
            name: "Coward-1".to_string(),
            design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: vec![EquippedModule { slot_type: "weapon".to_string(), module_id: "roe_laser".to_string() }],
            owner: Owner::Neutral,
            sublight_speed: 0.75,
            ftl_range: 0.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
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
        RulesOfEngagement::Retreat,
    ));

    advance_time(&mut app, 1);

    // Hostile should be undamaged because the only ship has Retreat ROE
    let hostile = app.world().get::<HostileHitpoints>(hostile_entity).unwrap();
    assert!(
        (hostile.hp - 100.0).abs() < f64::EPSILON,
        "Hostile HP should be unchanged when only Retreat ships present, got {}",
        hostile.hp
    );
}

#[test]
fn test_retreat_ships_dont_take_damage() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Retreat-NoDmg",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    // Hostile with strong attack
    app.world_mut().spawn((AtSystem(sys), HostileHitpoints { hp: 10000.0, max_hp: 10000.0 }, HostileStats { strength: 100.0, evasion: 0.0 }, Hostile));

    // Ship with Retreat ROE — should not take damage
    let ship_entity = app.world_mut().spawn((
        Ship {
            name: "Runner-1".to_string(),
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
        ShipState::Docked { system: sys },
        Position::from([0.0, 0.0, 0.0]),
        ShipHitpoints {
            hull: 1.0, hull_max: 50.0,
            armor: 0.0, armor_max: 0.0,
            shield: 0.0, shield_max: 0.0,
            shield_regen: 0.0,
        },
        ShipModifiers::default(),
        CommandQueue::default(),
        Cargo::default(),
        RulesOfEngagement::Retreat,
    )).id();

    advance_time(&mut app, 1);

    // Ship should still exist and be undamaged (Retreat skips combat)
    let hp = app.world().get::<ShipHitpoints>(ship_entity).unwrap();
    assert!(
        (hp.hull - 1.0).abs() < f64::EPSILON,
        "Retreat ship should not take damage, hull is {}",
        hp.hull
    );
}

#[test]
fn test_aggressive_ships_engage_combat() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Aggro-Test",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    let hostile_entity = app.world_mut().spawn((AtSystem(sys), HostileHitpoints { hp: 0.05, max_hp: 10.0 }, HostileStats { strength: 0.0, evasion: 0.0 }, Hostile)).id();

    app.world_mut().resource_mut::<macrocosmo::ship_design::ModuleRegistry>().insert(
        macrocosmo::ship_design::ModuleDefinition {
            id: "aggro_laser".to_string(),
            name: "Aggro Laser".to_string(),
            description: String::new(),
            slot_type: "weapon".to_string(),
            modifiers: Vec::new(),
            weapon: Some(macrocosmo::ship_design::WeaponStats {
                track: 1000.0, precision: 1.0, cooldown: 1, range: 100.0,
                shield_damage: 5.0, shield_damage_div: 0.0, shield_piercing: 0.0,
                armor_damage: 5.0, armor_damage_div: 0.0, armor_piercing: 0.0,
                hull_damage: 5.0, hull_damage_div: 0.0,
            }),
            cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisites: None,
            upgrade_to: Vec::new(),
        },
    );

    // Ship with Aggressive ROE
    app.world_mut().spawn((
        Ship {
            name: "Aggressor-1".to_string(),
            design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: vec![EquippedModule { slot_type: "weapon".to_string(), module_id: "aggro_laser".to_string() }],
            owner: Owner::Neutral,
            sublight_speed: 0.75,
            ftl_range: 0.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
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
        RulesOfEngagement::Aggressive,
    ));

    advance_time(&mut app, 1);

    // Hostile should be destroyed by the Aggressive ship
    assert!(
        app.world().get_entity(hostile_entity).is_err(),
        "Hostile should be destroyed by Aggressive ship"
    );
}

#[test]
fn test_defensive_ships_engage_combat_same_as_before() {
    // Defensive is the default and should behave the same as current (always fight)
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Defensive-Test",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    let hostile_entity = app.world_mut().spawn((AtSystem(sys), HostileHitpoints { hp: 0.05, max_hp: 10.0 }, HostileStats { strength: 0.0, evasion: 0.0 }, Hostile)).id();

    app.world_mut().resource_mut::<macrocosmo::ship_design::ModuleRegistry>().insert(
        macrocosmo::ship_design::ModuleDefinition {
            id: "def_laser".to_string(),
            name: "Defensive Laser".to_string(),
            description: String::new(),
            slot_type: "weapon".to_string(),
            modifiers: Vec::new(),
            weapon: Some(macrocosmo::ship_design::WeaponStats {
                track: 1000.0, precision: 1.0, cooldown: 1, range: 100.0,
                shield_damage: 5.0, shield_damage_div: 0.0, shield_piercing: 0.0,
                armor_damage: 5.0, armor_damage_div: 0.0, armor_piercing: 0.0,
                hull_damage: 5.0, hull_damage_div: 0.0,
            }),
            cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisites: None,
            upgrade_to: Vec::new(),
        },
    );

    // Ship with Defensive ROE (default) — should still fight
    app.world_mut().spawn((
        Ship {
            name: "Defender-1".to_string(),
            design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: vec![EquippedModule { slot_type: "weapon".to_string(), module_id: "def_laser".to_string() }],
            owner: Owner::Neutral,
            sublight_speed: 0.75,
            ftl_range: 0.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
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
        RulesOfEngagement::Defensive,
    ));

    advance_time(&mut app, 1);

    // Hostile should be destroyed (Defensive ships engage when hostiles are present)
    assert!(
        app.world().get_entity(hostile_entity).is_err(),
        "Hostile should be destroyed by Defensive ship"
    );
}

#[test]
fn test_mixed_roe_only_non_retreat_fight() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Mixed-ROE",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    // Hostile with moderate HP and strong attack
    app.world_mut().spawn((AtSystem(sys), HostileHitpoints { hp: 0.05, max_hp: 10.0 }, HostileStats { strength: 50.0, evasion: 0.0 }, Hostile));

    app.world_mut().resource_mut::<macrocosmo::ship_design::ModuleRegistry>().insert(
        macrocosmo::ship_design::ModuleDefinition {
            id: "mix_laser".to_string(),
            name: "Mix Laser".to_string(),
            description: String::new(),
            slot_type: "weapon".to_string(),
            modifiers: Vec::new(),
            weapon: Some(macrocosmo::ship_design::WeaponStats {
                track: 1000.0, precision: 1.0, cooldown: 1, range: 100.0,
                shield_damage: 5.0, shield_damage_div: 0.0, shield_piercing: 0.0,
                armor_damage: 5.0, armor_damage_div: 0.0, armor_piercing: 0.0,
                hull_damage: 5.0, hull_damage_div: 0.0,
            }),
            cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisites: None,
            upgrade_to: Vec::new(),
        },
    );

    // Aggressive ship — will fight
    app.world_mut().spawn((
        Ship {
            name: "Fighter".to_string(),
            design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: vec![EquippedModule { slot_type: "weapon".to_string(), module_id: "mix_laser".to_string() }],
            owner: Owner::Neutral,
            sublight_speed: 0.75,
            ftl_range: 0.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
        },
        ShipState::Docked { system: sys },
        Position::from([0.0, 0.0, 0.0]),
        ShipHitpoints {
            hull: 5000.0, hull_max: 5000.0,
            armor: 0.0, armor_max: 0.0,
            shield: 0.0, shield_max: 0.0,
            shield_regen: 0.0,
        },
        ShipModifiers::default(),
        CommandQueue::default(),
        Cargo::default(),
        RulesOfEngagement::Aggressive,
    ));

    // Retreat ship — should NOT take damage from hostile
    let retreat_ship = app.world_mut().spawn((
        Ship {
            name: "Pacifist".to_string(),
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
        ShipState::Docked { system: sys },
        Position::from([0.0, 0.0, 0.0]),
        ShipHitpoints {
            hull: 1.0, hull_max: 50.0,
            armor: 0.0, armor_max: 0.0,
            shield: 0.0, shield_max: 0.0,
            shield_regen: 0.0,
        },
        ShipModifiers::default(),
        CommandQueue::default(),
        Cargo::default(),
        RulesOfEngagement::Retreat,
    )).id();

    advance_time(&mut app, 1);

    // Retreat ship should survive (not included in combat damage distribution)
    let retreat_hp = app.world().get::<ShipHitpoints>(retreat_ship).unwrap();
    assert!(
        (retreat_hp.hull - 1.0).abs() < f64::EPSILON,
        "Retreat ship should not take any damage, hull is {}",
        retreat_hp.hull
    );
}

#[test]
fn test_set_roe_via_pending_command() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "ROE-Cmd-Test",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    let ship_entity = app.world_mut().spawn((
        Ship {
            name: "ROE-Target".to_string(),
            design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 0.75,
            ftl_range: 0.0,
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
        RulesOfEngagement::Defensive,
    )).id();

    // Spawn a pending SetROE command that arrives at tick 5
    app.world_mut().spawn(PendingShipCommand {
        ship: ship_entity,
        command: ShipCommand::SetROE { roe: RulesOfEngagement::Aggressive },
        arrives_at: 5,
    });

    // Before arrival, ROE should still be Defensive
    advance_time(&mut app, 3);
    let roe = app.world().get::<RulesOfEngagement>(ship_entity).unwrap();
    assert_eq!(*roe, RulesOfEngagement::Defensive, "ROE should still be Defensive before command arrives");

    // After arrival (tick 5), ROE should change to Aggressive
    advance_time(&mut app, 3);
    let roe = app.world().get::<RulesOfEngagement>(ship_entity).unwrap();
    assert_eq!(*roe, RulesOfEngagement::Aggressive, "ROE should be Aggressive after command arrives");
}

// --- #52/#56: Galaxy hostile spawning + colonization restriction tests ---

#[test]
fn test_galaxy_has_hostiles() {
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
        description: String::new(),
        color: [1.0, 1.0, 1.0],
        planet_lambda: 2.0,
        max_planets: 3,
        habitability_bonus: 0.0,
        weight: 1.0,
        modifiers: Vec::new(),
    });
    app.insert_resource(star_reg);

    let mut planet_reg = PlanetTypeRegistry::default();
    planet_reg.types.push(PlanetTypeDefinition {
        id: "test_planet".to_string(),
        name: "Test Planet".to_string(),
        description: String::new(),
        base_habitability: 0.7,
        base_slots: 4,
        resource_bias: ResourceBias { minerals: 1.0, energy: 1.0, research: 1.0 },
        weight: 1.0,
    });
    app.insert_resource(planet_reg);

    app.add_systems(Startup, macrocosmo::galaxy::generate_galaxy);
    app.update();

    // Check hostiles exist
    let hostile_count = app
        .world_mut()
        .query::<&Hostile>()
        .iter(app.world())
        .count();
    assert!(hostile_count > 0, "Galaxy should have at least some hostile presences");

    // Check that no hostile is at the capital system
    let capital_entity = app
        .world_mut()
        .query::<(Entity, &macrocosmo::galaxy::StarSystem)>()
        .iter(app.world())
        .find(|(_, s)| s.is_capital)
        .map(|(e, _)| e)
        .expect("Should have a capital system");

    let capital_pos = *app.world().get::<Position>(capital_entity).unwrap();

    for at_system in app
        .world_mut()
        .query_filtered::<&AtSystem, With<Hostile>>()
        .iter(app.world())
    {
        assert_ne!(
            at_system.0, capital_entity,
            "No hostile should be spawned at the capital system"
        );
        // Check capital proximity exclusion (10 ly)
        let hostile_pos = app.world().get::<Position>(at_system.0).unwrap();
        let dx = hostile_pos.x - capital_pos.x;
        let dy = hostile_pos.y - capital_pos.y;
        let dz = hostile_pos.z - capital_pos.z;
        let dist = (dx * dx + dy * dy + dz * dz).sqrt();
        assert!(
            dist >= 10.0,
            "Hostile at distance {:.1} ly from capital — should be >= 10.0 ly",
            dist
        );
    }
}

#[test]
fn test_colonize_blocked_by_hostile() {
    use macrocosmo::galaxy::StarSystem;

    let mut app = test_app();

    let (sys, _planet) = common::spawn_test_system_with_planet(
        app.world_mut(),
        "Hostile-Colony-System",
        [0.0, 0.0, 0.0],
        0.7,
        true,
    );

    // Spawn hostile at this system (low strength so it won't kill the ship via combat)
    app.world_mut().spawn((AtSystem(sys), HostileHitpoints { hp: 500.0, max_hp: 500.0 }, HostileStats { strength: 0.0, evasion: 0.0 }, Hostile));

    // Spawn a colony ship that is settling at this system (completes at tick 1)
    let ship_entity = app
        .world_mut()
        .spawn((
            Ship {
                name: "Colony-Ship-1".to_string(),
                design_id: "colony_ship_mk1".to_string(),
                hull_id: "corvette".to_string(),
                modules: Vec::new(),
                owner: Owner::Neutral,
                sublight_speed: 0.5,
                ftl_range: 0.0,
                player_aboard: false,
                home_port: Entity::PLACEHOLDER,
                design_revision: 0,
            },
            ShipState::Settling {
                system: sys,
                planet: None,
                started_at: 0,
                completes_at: 1,
            },
            Position::from([0.0, 0.0, 0.0]),
            ShipHitpoints {
                hull: 20.0,
                hull_max: 20.0,
                armor: 0.0,
                armor_max: 0.0,
                shield: 0.0,
                shield_max: 0.0,
                shield_regen: 0.0,
            },
            ShipModifiers::default(),
            CommandQueue::default(),
            Cargo::default(),
        ))
        .id();

    // Advance time so settling completes
    advance_time(&mut app, 2);

    // Ship should still exist (not despawned) and be Docked, not settled
    let ship_state = app
        .world()
        .get::<ShipState>(ship_entity)
        .expect("Ship should still exist — colonization should be blocked by hostile");
    match ship_state {
        ShipState::Docked { system } => {
            assert_eq!(*system, sys, "Ship should be docked at the hostile system");
        }
        _ => panic!("Ship should be in Docked state after failed colonization"),
    }

    // No colony should have been established
    let colony_count = app
        .world_mut()
        .query::<&macrocosmo::colony::Colony>()
        .iter(app.world())
        .count();
    assert_eq!(colony_count, 0, "No colony should exist when hostile presence blocks colonization");
}

#[test]
fn test_hostile_cleared_allows_colonization() {
    let mut app = test_app();

    let (sys, _planet) = common::spawn_test_system_with_planet(
        app.world_mut(),
        "Cleared-System",
        [0.0, 0.0, 0.0],
        0.7,
        true,
    );

    // Spawn and immediately despawn a hostile (simulating it was defeated)
    let hostile_entity = app.world_mut().spawn((AtSystem(sys), HostileHitpoints { hp: 10.0, max_hp: 10.0 }, HostileStats { strength: 10.0, evasion: 0.0 }, Hostile)).id();
    app.world_mut().despawn(hostile_entity);

    // Spawn a colony ship settling at this system
    let ship_entity = app
        .world_mut()
        .spawn((
            Ship {
                name: "Colony-Ship-2".to_string(),
                design_id: "colony_ship_mk1".to_string(),
                hull_id: "corvette".to_string(),
                modules: Vec::new(),
                owner: Owner::Neutral,
                sublight_speed: 0.5,
                ftl_range: 0.0,
                player_aboard: false,
                home_port: Entity::PLACEHOLDER,
                design_revision: 0,
            },
            ShipState::Settling {
                system: sys,
                planet: None,
                started_at: 0,
                completes_at: 1,
            },
            Position::from([0.0, 0.0, 0.0]),
            ShipHitpoints {
                hull: 20.0,
                hull_max: 20.0,
                armor: 0.0,
                armor_max: 0.0,
                shield: 0.0,
                shield_max: 0.0,
                shield_regen: 0.0,
            },
            ShipModifiers::default(),
            CommandQueue::default(),
            Cargo::default(),
        ))
        .id();

    // Advance time
    advance_time(&mut app, 2);

    // Ship should be despawned (consumed by colony establishment)
    assert!(
        app.world().get_entity(ship_entity).is_err(),
        "Colony ship should be despawned after successful colonization"
    );

    // Colony should exist
    let colony_count = app
        .world_mut()
        .query::<&macrocosmo::colony::Colony>()
        .iter(app.world())
        .count();
    assert_eq!(colony_count, 1, "Colony should be established after hostile is cleared");
}

// ---------------------------------------------------------------------------
// #168 — HostilePresence migrated to Faction-gated combat.
// These tests validate that combat is now resolved through `FactionRelations`
// rather than by the mere presence of a `HostilePresence` component.
// ---------------------------------------------------------------------------

/// Spawn a hostile entity with a weak attack and high HP so the test ship
/// always survives the round; combat triggering is detected by HP delta on
/// the hostile, not by ship destruction. `_hostile_type` is retained for
/// backwards-compatibility with call sites but is unused — tests pair up
/// factions via `setup_test_hostile_factions` which tags new-component
/// hostiles with the `space_creature` faction by default.
fn spawn_test_hostile(world: &mut World, sys: Entity, hostile_type: HostileType) -> Entity {
    world
        .spawn((
            AtSystem(sys),
            HostileHitpoints {
                hp: 1000.0,
                max_hp: 1000.0,
            },
            HostileStats {
                strength: 0.0,
                evasion: 0.0,
            },
            Hostile,
            // #293: `TestHostileFactionTag` carries the intended faction
            // bucket so `setup_test_hostile_factions` can attach the right
            // FactionOwner without relying on the removed HostilePresence.hostile_type.
            common::TestHostileFactionTag(hostile_type),
        ))
        .id()
}

/// Register a high-precision laser module for combat trigger detection.
fn install_test_weapon_module(app: &mut App) {
    app.world_mut()
        .resource_mut::<macrocosmo::ship_design::ModuleRegistry>()
        .insert(macrocosmo::ship_design::ModuleDefinition {
            id: "trigger_laser".to_string(),
            name: "Trigger Laser".to_string(),
            description: String::new(),
            slot_type: "weapon".to_string(),
            modifiers: Vec::new(),
            weapon: Some(macrocosmo::ship_design::WeaponStats {
                track: 100.0,
                precision: 1.0,
                cooldown: 1,
                range: 100.0,
                shield_damage: 1.0,
                shield_damage_div: 0.0,
                shield_piercing: 0.0,
                armor_damage: 1.0,
                armor_damage_div: 0.0,
                armor_piercing: 0.0,
                hull_damage: 1.0,
                hull_damage_div: 0.0,
            }),
            cost_minerals: Amt::ZERO,
            cost_energy: Amt::ZERO,
            prerequisites: None,
            upgrade_to: Vec::new(),
        });
}

/// Spawn an armed ship docked at `sys` with `owner`. Returns the ship entity.
fn spawn_test_armed_ship(world: &mut World, sys: Entity, owner: Owner) -> Entity {
    world
        .spawn((
            Ship {
                name: "Test-Warship".to_string(),
                design_id: "explorer_mk1".to_string(),
                hull_id: "corvette".to_string(),
                modules: vec![EquippedModule {
                    slot_type: "weapon".to_string(),
                    module_id: "trigger_laser".to_string(),
                }],
                owner,
                sublight_speed: 0.75,
                ftl_range: 10.0,
                player_aboard: false,
                home_port: Entity::PLACEHOLDER,
                design_revision: 0,
            },
            ShipState::Docked { system: sys },
            Position::from([0.0, 0.0, 0.0]),
            ShipHitpoints {
                hull: 100.0,
                hull_max: 100.0,
                armor: 0.0,
                armor_max: 0.0,
                shield: 0.0,
                shield_max: 0.0,
                shield_regen: 0.0,
            },
            ShipModifiers::default(),
            CommandQueue::default(),
            Cargo::default(),
        ))
        .id()
}

/// #168 — Without faction migration, a `HostilePresence` and a ship co-located
/// at the same system must not engage. (The legacy `advance_time` helper
/// auto-migrates, so we drive `app.update()` ourselves to bypass migration.)
#[test]
fn test_combat_skipped_when_hostile_lacks_faction_owner() {
    let mut app = test_app();
    install_test_weapon_module(&mut app);

    let sys = spawn_test_system(app.world_mut(), "Unmigrated-System", [0.0, 0.0, 0.0], 0.7, true, false);

    let hostile = spawn_test_hostile(app.world_mut(), sys, HostileType::SpaceCreature);
    let _ship = spawn_test_armed_ship(app.world_mut(), sys, Owner::Neutral);

    // Drive the world directly — bypass the auto-migrating `advance_time`
    // helper. Hostile has no FactionOwner; ship has no Faction-bearing owner.
    app.world_mut().resource_mut::<macrocosmo::time_system::GameClock>().elapsed += 1;
    app.update();

    let h = app.world().get::<HostileHitpoints>(hostile).unwrap();
    assert!(
        (h.hp - 1000.0).abs() < f64::EPSILON,
        "Hostile HP must be untouched when no FactionOwner is attached; got {}",
        h.hp
    );
}

/// #168 — With the standard migration (Neutral / -100 standing) and an
/// armed empire-owned ship, combat triggers and HP drops as before.
#[test]
fn test_combat_triggers_with_default_hostile_relations() {
    let mut app = test_app();
    install_test_weapon_module(&mut app);

    let sys = spawn_test_system(app.world_mut(), "Migrated-System", [0.0, 0.0, 0.0], 0.7, true, false);

    let hostile = spawn_test_hostile(app.world_mut(), sys, HostileType::SpaceCreature);
    let _ship = spawn_test_armed_ship(app.world_mut(), sys, Owner::Neutral);

    // advance_time auto-migrates the hostile (FactionOwner) and ship (empire owner).
    advance_time(&mut app, 1);

    let h = app.world().get::<HostileHitpoints>(hostile).unwrap();
    assert!(
        h.hp < 1000.0,
        "Hostile HP must drop when faction migration enables combat; got {}",
        h.hp
    );
}

/// #168 / #169 — When the player's view of the hostile faction is `Peace`,
/// an `Aggressive` ROE ship still must not preemptively attack
/// (`can_attack_aggressive` is false). #169 introduced retaliation under
/// `Defensive`, so this test pins the ROE to `Aggressive` to keep the
/// "Peace forbids first strikes" invariant under regression coverage.
#[test]
fn test_combat_skipped_when_player_at_peace_with_hostile_faction() {
    use macrocosmo::faction::{FactionRelations, FactionView, RelationState};

    let mut app = test_app();
    install_test_weapon_module(&mut app);

    let sys = spawn_test_system(app.world_mut(), "Peace-System", [0.0, 0.0, 0.0], 0.7, true, false);

    let hostile = spawn_test_hostile(app.world_mut(), sys, HostileType::SpaceCreature);
    let ship = spawn_test_armed_ship(app.world_mut(), sys, Owner::Neutral);
    // #169: pin to Aggressive — Defensive would now retaliate against the
    // co-located HostilePresence, which is a separate behaviour covered by
    // its own test.
    app.world_mut()
        .entity_mut(ship)
        .insert(RulesOfEngagement::Aggressive);

    // Migrate first.
    let (space_creature, _) = common::setup_test_hostile_factions(app.world_mut());
    let empire = common::empire_entity(app.world_mut());

    // Override: peace with space_creature (both directions).
    {
        let mut rel = app.world_mut().resource_mut::<FactionRelations>();
        rel.set(empire, space_creature, FactionView::new(RelationState::Peace, 0.0));
        rel.set(space_creature, empire, FactionView::new(RelationState::Peace, 0.0));
    }

    advance_time(&mut app, 1);

    let h = app.world().get::<HostileHitpoints>(hostile).unwrap();
    assert!(
        (h.hp - 1000.0).abs() < f64::EPSILON,
        "Aggressive ROE must not first-strike at Peace; got {}",
        h.hp
    );
}

/// #168 — Alliance forbids attack regardless of standing or ROE.
#[test]
fn test_combat_skipped_when_player_allied_with_hostile_faction() {
    use macrocosmo::faction::{FactionRelations, FactionView, RelationState};

    let mut app = test_app();
    install_test_weapon_module(&mut app);

    let sys = spawn_test_system(app.world_mut(), "Alliance-System", [0.0, 0.0, 0.0], 0.7, true, false);

    let hostile = spawn_test_hostile(app.world_mut(), sys, HostileType::AncientDefense);

    // Spawn an Aggressive ROE ship to verify the gate trumps ROE.
    let ship = spawn_test_armed_ship(app.world_mut(), sys, Owner::Neutral);
    app.world_mut()
        .entity_mut(ship)
        .insert(RulesOfEngagement::Aggressive);

    let (_, ancient_defense) = common::setup_test_hostile_factions(app.world_mut());
    let empire = common::empire_entity(app.world_mut());

    {
        let mut rel = app.world_mut().resource_mut::<FactionRelations>();
        rel.set(empire, ancient_defense, FactionView::new(RelationState::Alliance, 100.0));
        rel.set(ancient_defense, empire, FactionView::new(RelationState::Alliance, 100.0));
    }

    advance_time(&mut app, 1);

    let h = app.world().get::<HostileHitpoints>(hostile).unwrap();
    assert!(
        (h.hp - 1000.0).abs() < f64::EPSILON,
        "Alliance must forbid attack regardless of ROE; HP changed to {}",
        h.hp
    );
}

/// #168 — Open war (state=War) always permits engagement, even with positive
/// standing. Sanity check that the gate uses the full FactionView semantics.
#[test]
fn test_combat_triggers_when_at_war_even_with_positive_standing() {
    use macrocosmo::faction::{FactionRelations, FactionView, RelationState};

    let mut app = test_app();
    install_test_weapon_module(&mut app);

    let sys = spawn_test_system(app.world_mut(), "War-System", [0.0, 0.0, 0.0], 0.7, true, false);

    let hostile = spawn_test_hostile(app.world_mut(), sys, HostileType::SpaceCreature);
    let _ship = spawn_test_armed_ship(app.world_mut(), sys, Owner::Neutral);

    let (space_creature, _) = common::setup_test_hostile_factions(app.world_mut());
    let empire = common::empire_entity(app.world_mut());
    {
        let mut rel = app.world_mut().resource_mut::<FactionRelations>();
        rel.set(empire, space_creature, FactionView::new(RelationState::War, 80.0));
    }

    advance_time(&mut app, 1);

    let h = app.world().get::<HostileHitpoints>(hostile).unwrap();
    assert!(h.hp < 1000.0, "War must permit attack regardless of standing; HP={}", h.hp);
}

// ---------------------------------------------------------------------------
// #169 — ROE × FactionRelations matrix.
// ---------------------------------------------------------------------------

/// Helper: assert hostile took damage (HP < starting 1000.0).
fn assert_hostile_engaged(app: &App, hostile: Entity) {
    let h = app.world().get::<HostileHitpoints>(hostile).unwrap();
    assert!(
        h.hp < 1000.0,
        "Expected hostile to have taken damage (engagement), got hp={}",
        h.hp
    );
}

/// Helper: assert hostile was untouched (HP == starting 1000.0).
fn assert_hostile_untouched(app: &App, hostile: Entity) {
    let h = app.world().get::<HostileHitpoints>(hostile).unwrap();
    assert!(
        (h.hp - 1000.0).abs() < f64::EPSILON,
        "Expected hostile HP to be untouched, got hp={}",
        h.hp
    );
}

/// Override the player↔hostile relation to a single `(state, standing)` view.
fn set_player_view(app: &mut App, hostile_faction: Entity, state: macrocosmo::faction::RelationState, standing: f64) {
    use macrocosmo::faction::{FactionRelations, FactionView};
    let empire = common::empire_entity(app.world_mut());
    let mut rel = app.world_mut().resource_mut::<FactionRelations>();
    rel.set(empire, hostile_faction, FactionView::new(state, standing));
}

/// #169 — Aggressive + Neutral with negative standing → engage even without
/// formal war. This is the existing #168 behaviour, re-asserted under the
/// new ROE-aware code path.
#[test]
fn test_aggressive_engages_neutral_negative_standing() {
    use macrocosmo::faction::RelationState;

    let mut app = test_app();
    install_test_weapon_module(&mut app);

    let sys = spawn_test_system(app.world_mut(), "Aggro-Neg", [0.0, 0.0, 0.0], 0.7, true, false);
    let hostile = spawn_test_hostile(app.world_mut(), sys, HostileType::SpaceCreature);
    let ship = spawn_test_armed_ship(app.world_mut(), sys, Owner::Neutral);
    app.world_mut().entity_mut(ship).insert(RulesOfEngagement::Aggressive);

    let (space_creature, _) = common::setup_test_hostile_factions(app.world_mut());
    set_player_view(&mut app, space_creature, RelationState::Neutral, -25.0);

    advance_time(&mut app, 1);
    assert_hostile_engaged(&app, hostile);
}

/// #169 — Defensive + Neutral with negative standing → must NOT preemptively
/// attack when no hostile is present. Uses the helper directly because the
/// resolve_combat loop only iterates over actual HostilePresences.
#[test]
fn test_defensive_does_not_first_strike_when_no_hostile_present() {
    use macrocosmo::faction::{FactionView, RelationState};
    let v = FactionView::new(RelationState::Neutral, -100.0);
    assert!(
        !v.should_engage_defensive(false),
        "Defensive must wait when standing<0 but no hostile is present"
    );
}

/// #169 — Defensive + War → engage. Mirrors aggressive engagement at war but
/// via the Defensive code path.
#[test]
fn test_defensive_engages_at_war() {
    use macrocosmo::faction::RelationState;

    let mut app = test_app();
    install_test_weapon_module(&mut app);

    let sys = spawn_test_system(app.world_mut(), "Def-War", [0.0, 0.0, 0.0], 0.7, true, false);
    let hostile = spawn_test_hostile(app.world_mut(), sys, HostileType::SpaceCreature);
    let ship = spawn_test_armed_ship(app.world_mut(), sys, Owner::Neutral);
    app.world_mut().entity_mut(ship).insert(RulesOfEngagement::Defensive);

    let (space_creature, _) = common::setup_test_hostile_factions(app.world_mut());
    // Positive standing on purpose: only the War state should drive engagement.
    set_player_view(&mut app, space_creature, RelationState::War, 50.0);

    advance_time(&mut app, 1);
    assert_hostile_engaged(&app, hostile);
}

/// #169 — Defensive + Peace + co-located HostilePresence → retaliate.
/// `should_engage_defensive(true)` is true even at Peace because the local
/// "being attacked" signal trumps the (potentially stale) Peace state.
#[test]
fn test_defensive_retaliates_against_hostile_at_peace() {
    use macrocosmo::faction::RelationState;

    let mut app = test_app();
    install_test_weapon_module(&mut app);

    let sys = spawn_test_system(app.world_mut(), "Def-Peace-Hostile", [0.0, 0.0, 0.0], 0.7, true, false);
    let hostile = spawn_test_hostile(app.world_mut(), sys, HostileType::SpaceCreature);
    let ship = spawn_test_armed_ship(app.world_mut(), sys, Owner::Neutral);
    app.world_mut().entity_mut(ship).insert(RulesOfEngagement::Defensive);

    let (space_creature, _) = common::setup_test_hostile_factions(app.world_mut());
    set_player_view(&mut app, space_creature, RelationState::Peace, 0.0);

    advance_time(&mut app, 1);
    assert_hostile_engaged(&app, hostile);
}

/// #169 — Retreat + War → still skip. ROE always trumps relations for
/// non-engagement.
#[test]
fn test_retreat_skips_combat_at_war() {
    use macrocosmo::faction::RelationState;

    let mut app = test_app();
    install_test_weapon_module(&mut app);

    let sys = spawn_test_system(app.world_mut(), "Retreat-War", [0.0, 0.0, 0.0], 0.7, true, false);
    let hostile = spawn_test_hostile(app.world_mut(), sys, HostileType::SpaceCreature);
    let ship = spawn_test_armed_ship(app.world_mut(), sys, Owner::Neutral);
    app.world_mut().entity_mut(ship).insert(RulesOfEngagement::Retreat);

    let (space_creature, _) = common::setup_test_hostile_factions(app.world_mut());
    set_player_view(&mut app, space_creature, RelationState::War, -100.0);

    advance_time(&mut app, 1);
    assert_hostile_untouched(&app, hostile);
}

/// #169 — Retreat + co-located HostilePresence → still skip, even with
/// default Neutral/-100 hostile relations that would normally trigger
/// Aggressive engagement.
#[test]
fn test_retreat_skips_combat_with_hostile_present() {
    let mut app = test_app();
    install_test_weapon_module(&mut app);

    let sys = spawn_test_system(app.world_mut(), "Retreat-Hostile", [0.0, 0.0, 0.0], 0.7, true, false);
    let hostile = spawn_test_hostile(app.world_mut(), sys, HostileType::SpaceCreature);
    let ship = spawn_test_armed_ship(app.world_mut(), sys, Owner::Neutral);
    app.world_mut().entity_mut(ship).insert(RulesOfEngagement::Retreat);

    // Default migration keeps Neutral / -100 standing — would normally engage
    // under Aggressive or Defensive.
    let _ = common::setup_test_hostile_factions(app.world_mut());

    advance_time(&mut app, 1);
    assert_hostile_untouched(&app, hostile);
}

/// #168 — `setup_test_hostile_factions` correctly tags hostiles by type:
/// SpaceCreature → space_creature_faction, AncientDefense → ancient_defense_faction.
#[test]
fn test_faction_owner_attached_by_hostile_type() {
    use macrocosmo::faction::FactionOwner;

    let mut app = test_app();

    let sys = spawn_test_system(app.world_mut(), "Tagging", [0.0, 0.0, 0.0], 0.7, true, false);

    let creature = spawn_test_hostile(app.world_mut(), sys, HostileType::SpaceCreature);
    let ancient = spawn_test_hostile(app.world_mut(), sys, HostileType::AncientDefense);

    let (space_creature, ancient_defense) = common::setup_test_hostile_factions(app.world_mut());

    let creature_owner = app.world().get::<FactionOwner>(creature).unwrap().0;
    let ancient_owner = app.world().get::<FactionOwner>(ancient).unwrap().0;
    assert_eq!(creature_owner, space_creature);
    assert_eq!(ancient_owner, ancient_defense);
    assert_ne!(creature_owner, ancient_owner);
}

/// #293 regression: a bare `(AtSystem, FactionOwner, HostileHitpoints,
/// HostileStats, Hostile)` entity — spawned without any legacy
/// `HostilePresence` component — must be treated as a hostile by the
/// knowledge-snapshot / combat / settlement layers.
#[test]
fn test_hostile_viz_uses_factionowner() {
    use macrocosmo::faction::{FactionOwner, FactionRelations, FactionView, RelationState};
    use macrocosmo::knowledge::KnowledgeStore;

    let mut app = test_app();

    let sys_capital =
        spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    let sys_remote = spawn_test_system(
        app.world_mut(),
        "Remote",
        [1.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    // Spawn player stationed at capital.
    app.world_mut().spawn((
        macrocosmo::player::Player,
        macrocosmo::player::StationedAt {
            system: sys_capital,
        },
    ));

    // Spawn hostile faction + hostile entity with NO HostilePresence.
    let (space_creature, _) = common::setup_test_hostile_factions(app.world_mut());
    app.world_mut().spawn((
        AtSystem(sys_remote),
        HostileHitpoints {
            hp: 500.0,
            max_hp: 500.0,
        },
        HostileStats {
            strength: 12.5,
            evasion: 0.0,
        },
        Hostile,
        FactionOwner(space_creature),
    ));

    // Empire must consider space_creature hostile.
    let empire = {
        let mut q = app.world_mut().query_filtered::<Entity, With<macrocosmo::player::PlayerEmpire>>();
        q.iter(app.world()).next().expect("empire")
    };
    {
        let mut rel = app.world_mut().resource_mut::<FactionRelations>();
        rel.set(
            empire,
            space_creature,
            FactionView::new(RelationState::Neutral, -100.0),
        );
    }

    // Let light-speed observation catch up (1 ly = 60 hexadies).
    common::advance_time(&mut app, 61);

    let store = app.world().get::<KnowledgeStore>(empire).unwrap();
    let k = store
        .get(sys_remote)
        .expect("remote system knowledge should exist");
    assert!(
        k.data.has_hostile,
        "FactionOwner-only hostile entity must register in knowledge snapshot"
    );
    assert!(
        (k.data.hostile_strength - 12.5).abs() < 0.01,
        "hostile_strength should come from HostileStats, got {}",
        k.data.hostile_strength
    );
}
