mod common;

use bevy::prelude::*;
use macrocosmo::amount::Amt;
use macrocosmo::components::Position;
use macrocosmo::galaxy::{Habitability, HostilePresence, HostileType};
use macrocosmo::ship::*;

use common::{advance_time, spawn_test_system, test_app};

#[test]
fn test_hostile_destroyed_when_hp_zero() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Battle-System",
        [0.0, 0.0, 0.0],
        Habitability::Adequate,
        true,
        false,
    );

    // Spawn a hostile with low HP so it gets destroyed quickly
    let hostile_entity = app.world_mut().spawn(HostilePresence {
        system: sys,
        strength: 0.0,  // no attack
        hp: 0.05,       // very low HP
        max_hp: 10.0,
        hostile_type: HostileType::SpaceCreature,
        evasion: 0.0,
    }).id();

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
            prerequisite_tech: None,
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
        Habitability::Adequate,
        true,
        false,
    );

    // Spawn a powerful hostile
    app.world_mut().spawn(HostilePresence {
        system: sys,
        strength: 100.0,  // very strong attack (damage per combat turn)
        hp: 1000.0,
        max_hp: 1000.0,
        hostile_type: HostileType::AncientDefense,
        evasion: 0.0,
    });

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
        Habitability::Adequate,
        true,
        false,
    );

    // Spawn hostile - should not be affected without ships present
    let hostile_entity = app.world_mut().spawn(HostilePresence {
        system: sys,
        strength: 5.0,
        hp: 10.0,
        max_hp: 10.0,
        hostile_type: HostileType::SpaceCreature,
        evasion: 0.0,
    }).id();

    advance_time(&mut app, 1);

    // Hostile should still exist with full HP
    let hostile = app.world().get::<HostilePresence>(hostile_entity).unwrap();
    assert!((hostile.hp - 10.0).abs() < f64::EPSILON, "Hostile HP should be unchanged");
}

#[test]
fn test_combat_takes_multiple_ticks() {
    let mut app = test_app();

    let sys = spawn_test_system(
        app.world_mut(),
        "Prolonged-Battle",
        [0.0, 0.0, 0.0],
        Habitability::Adequate,
        true,
        false,
    );

    // Hostile with significant HP
    let hostile_entity = app.world_mut().spawn(HostilePresence {
        system: sys,
        strength: 0.01,  // very low damage
        hp: 1000.0,
        max_hp: 1000.0,
        hostile_type: HostileType::SpaceCreature,
        evasion: 0.0,
    }).id();

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
            prerequisite_tech: None,
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

    let hostile = app.world().get::<HostilePresence>(hostile_entity).unwrap();
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
        Habitability::Adequate,
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
            cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisite_tech: None,
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
        Habitability::Adequate,
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
            cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisite_tech: None,
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
        Habitability::Adequate,
        true,
        false,
    );

    // Spawn hostile with significant strength
    app.world_mut().spawn(HostilePresence {
        system: sys,
        strength: 10.0,  // 10 damage per combat turn
        hp: 10000.0,
        max_hp: 10000.0,
        hostile_type: HostileType::AncientDefense,
        evasion: 0.0,
    });

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
            cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisite_tech: None,
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
            cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisite_tech: None,
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
        Habitability::Adequate,
        true,
        false,
    );

    app.world_mut().spawn(HostilePresence {
        system: sys,
        strength: 100.0,
        hp: 10000.0,
        max_hp: 10000.0,
        hostile_type: HostileType::AncientDefense,
        evasion: 0.0,
    });

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
        Habitability::Adequate,
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
        cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisite_tech: None,
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
        cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisite_tech: None,
        upgrade_to: Vec::new(),
    });

    // Hostile A: attacked by fast gun
    let hostile_a = app.world_mut().spawn(HostilePresence {
        system: sys, strength: 0.0,
        hp: 10000.0, max_hp: 10000.0,
        hostile_type: HostileType::SpaceCreature, evasion: 0.0,
    }).id();

    // Hostile B: attacked by slow gun (separate system)
    let sys_b = spawn_test_system(app.world_mut(), "Cooldown-B", [100.0, 0.0, 0.0], Habitability::Adequate, true, false);
    let hostile_b = app.world_mut().spawn(HostilePresence {
        system: sys_b, strength: 0.0,
        hp: 10000.0, max_hp: 10000.0,
        hostile_type: HostileType::SpaceCreature, evasion: 0.0,
    }).id();

    // Ship with fast gun at sys
    app.world_mut().spawn((
        Ship {
            name: "Fast-Ship".to_string(), design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: vec![EquippedModule { slot_type: "weapon".to_string(), module_id: "fast_gun".to_string() }],
            owner: Owner::Neutral, sublight_speed: 0.75, ftl_range: 0.0,
            player_aboard: false, home_port: Entity::PLACEHOLDER,
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
        },
        ShipState::Docked { system: sys_b },
        Position::from([100.0, 0.0, 0.0]),
        ShipHitpoints { hull: 100.0, hull_max: 100.0, armor: 0.0, armor_max: 0.0, shield: 0.0, shield_max: 0.0, shield_regen: 0.0 },
        ShipModifiers::default(), CommandQueue::default(), Cargo::default(),
    ));

    advance_time(&mut app, 1);

    let hp_a = app.world().get::<HostilePresence>(hostile_a).unwrap().hp;
    let hp_b = app.world().get::<HostilePresence>(hostile_b).unwrap().hp;
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
        Habitability::Adequate,
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
            cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisite_tech: None,
            upgrade_to: Vec::new(),
        },
    );

    // Hostile with no attack
    let hostile = app.world_mut().spawn(HostilePresence {
        system: sys, strength: 0.0,
        hp: 10000.0, max_hp: 10000.0,
        hostile_type: HostileType::SpaceCreature, evasion: 0.0,
    }).id();

    // Ship with full shields, armor, and the piercing weapon
    let _ship_entity = app.world_mut().spawn((
        Ship {
            name: "Piercer-1".to_string(), design_id: "explorer_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: vec![EquippedModule { slot_type: "weapon".to_string(), module_id: "shield_piercer".to_string() }],
            owner: Owner::Neutral, sublight_speed: 0.75, ftl_range: 0.0,
            player_aboard: false, home_port: Entity::PLACEHOLDER,
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
    let h = app.world().get::<HostilePresence>(hostile).unwrap();
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
        Habitability::Adequate,
        true,
        false,
    );

    // Hostile with no attack but trackable HP
    let hostile_entity = app.world_mut().spawn(HostilePresence {
        system: sys,
        strength: 0.0,
        hp: 100.0,
        max_hp: 100.0,
        hostile_type: HostileType::SpaceCreature,
        evasion: 0.0,
    }).id();

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
            cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisite_tech: None,
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
    let hostile = app.world().get::<HostilePresence>(hostile_entity).unwrap();
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
        Habitability::Adequate,
        true,
        false,
    );

    // Hostile with strong attack
    app.world_mut().spawn(HostilePresence {
        system: sys,
        strength: 100.0,
        hp: 10000.0,
        max_hp: 10000.0,
        hostile_type: HostileType::AncientDefense,
        evasion: 0.0,
    });

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
        Habitability::Adequate,
        true,
        false,
    );

    let hostile_entity = app.world_mut().spawn(HostilePresence {
        system: sys,
        strength: 0.0,
        hp: 0.05,
        max_hp: 10.0,
        hostile_type: HostileType::SpaceCreature,
        evasion: 0.0,
    }).id();

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
            cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisite_tech: None,
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
        Habitability::Adequate,
        true,
        false,
    );

    let hostile_entity = app.world_mut().spawn(HostilePresence {
        system: sys,
        strength: 0.0,
        hp: 0.05,
        max_hp: 10.0,
        hostile_type: HostileType::SpaceCreature,
        evasion: 0.0,
    }).id();

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
            cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisite_tech: None,
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
        Habitability::Adequate,
        true,
        false,
    );

    // Hostile with moderate HP and strong attack
    app.world_mut().spawn(HostilePresence {
        system: sys,
        strength: 50.0,
        hp: 0.05,
        max_hp: 10.0,
        hostile_type: HostileType::SpaceCreature,
        evasion: 0.0,
    });

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
            cost_minerals: Amt::ZERO, cost_energy: Amt::ZERO, prerequisite_tech: None,
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
        Habitability::Adequate,
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
        .query::<&HostilePresence>()
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

    for hostile in app.world_mut().query::<&HostilePresence>().iter(app.world()) {
        assert_ne!(
            hostile.system, capital_entity,
            "No hostile should be spawned at the capital system"
        );
        // Check capital proximity exclusion (10 ly)
        let hostile_pos = app.world().get::<Position>(hostile.system).unwrap();
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
        Habitability::Adequate,
        true,
    );

    // Spawn hostile at this system (low strength so it won't kill the ship via combat)
    app.world_mut().spawn(HostilePresence {
        system: sys,
        strength: 0.0,  // no attack — we only want to test colonization blocking
        hp: 500.0,
        max_hp: 500.0,
        hostile_type: HostileType::AncientDefense,
        evasion: 0.0,
    });

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
        Habitability::Adequate,
        true,
    );

    // Spawn and immediately despawn a hostile (simulating it was defeated)
    let hostile_entity = app.world_mut().spawn(HostilePresence {
        system: sys,
        strength: 10.0,
        hp: 10.0,
        max_hp: 10.0,
        hostile_type: HostileType::SpaceCreature,
        evasion: 0.0,
    }).id();
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
