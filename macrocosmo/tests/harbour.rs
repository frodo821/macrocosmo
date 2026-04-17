//! #384: Integration tests for harbour dock/undock, ROE combat, modifier propagation.
mod common;

use bevy::prelude::*;
use macrocosmo::amount::Amt;
use macrocosmo::components::Position;
use macrocosmo::modifier::{CachedValue, ScopedModifiers};
use macrocosmo::ship::harbour::AppliedDockedModifiers;
use macrocosmo::ship::*;

use common::{advance_time, spawn_test_system, test_app};

/// Helper: spawn a ship with specific ROE and hull, docked at a harbour.
fn spawn_ship_with_roe(
    world: &mut World,
    name: &str,
    hull_id: &str,
    system: Entity,
    pos: [f64; 3],
    roe: RulesOfEngagement,
) -> Entity {
    let ship_entity = world.spawn_empty().id();
    let fleet_entity = world.spawn_empty().id();
    world.entity_mut(ship_entity).insert((
        Ship {
            name: name.to_string(),
            design_id: "test".to_string(),
            hull_id: hull_id.to_string(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 1.0,
            ftl_range: 10.0,
            player_aboard: false,
            home_port: system,
            design_revision: 0,
            fleet: Some(fleet_entity),
        },
        ShipState::InSystem { system },
        Position::from(pos),
        ShipHitpoints {
            hull: 50.0,
            hull_max: 50.0,
            armor: 0.0,
            armor_max: 0.0,
            shield: 0.0,
            shield_max: 0.0,
            shield_regen: 0.0,
        },
        CommandQueue::default(),
        Cargo::default(),
        ShipModifiers::default(),
        ShipStats::default(),
        roe,
    ));
    world.entity_mut(fleet_entity).insert((
        Fleet {
            name: name.to_string(),
            flagship: Some(ship_entity),
        },
        FleetMembers(vec![ship_entity]),
    ));
    ship_entity
}

/// Helper: spawn a harbour ship with given capacity.
fn spawn_harbour(
    world: &mut World,
    name: &str,
    system: Entity,
    pos: [f64; 3],
    capacity: u32,
) -> Entity {
    let ship_entity = world.spawn_empty().id();
    let fleet_entity = world.spawn_empty().id();

    let mut stats = ShipStats::default();
    let scope = ScopedModifiers::new(Amt::units(capacity as u64));
    stats.harbour_capacity = CachedValue::default();
    stats.harbour_capacity.recompute(&[&scope]);

    world.entity_mut(ship_entity).insert((
        Ship {
            name: name.to_string(),
            design_id: "harbour_test".to_string(),
            hull_id: "carrier".to_string(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 0.5,
            ftl_range: 5.0,
            player_aboard: false,
            home_port: system,
            design_revision: 0,
            fleet: Some(fleet_entity),
        },
        ShipState::InSystem { system },
        Position::from(pos),
        ShipHitpoints {
            hull: 200.0,
            hull_max: 200.0,
            armor: 0.0,
            armor_max: 0.0,
            shield: 0.0,
            shield_max: 0.0,
            shield_regen: 0.0,
        },
        CommandQueue::default(),
        Cargo::default(),
        ShipModifiers::default(),
        stats,
        RulesOfEngagement::Defensive,
    ));
    world.entity_mut(fleet_entity).insert((
        Fleet {
            name: name.to_string(),
            flagship: Some(ship_entity),
        },
        FleetMembers(vec![ship_entity]),
    ));
    ship_entity
}

#[test]
fn test_dock_and_position_sync() {
    let mut app = test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "Dock-Sys",
        [10.0, 20.0, 30.0],
        0.7,
        true,
        false,
    );

    let harbour = spawn_harbour(app.world_mut(), "Carrier", sys, [10.0, 20.0, 30.0], 10);
    let docker = spawn_ship_with_roe(
        app.world_mut(),
        "Fighter",
        "corvette",
        sys,
        [0.0, 0.0, 0.0],
        RulesOfEngagement::Defensive,
    );

    // Dock the ship
    app.world_mut().entity_mut(docker).insert(DockedAt(harbour));

    // Advance time to trigger sync_docked_position
    advance_time(&mut app, 1);

    // Verify position was synced
    let docker_pos = app.world().get::<Position>(docker).unwrap();
    let harbour_pos = app.world().get::<Position>(harbour).unwrap();
    assert_eq!(docker_pos.as_array(), harbour_pos.as_array());
}

#[test]
fn test_force_undock_on_harbour_destroy() {
    let mut app = test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "Destroy-Sys",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    let harbour = spawn_harbour(app.world_mut(), "Carrier", sys, [0.0, 0.0, 0.0], 10);
    let docker = spawn_ship_with_roe(
        app.world_mut(),
        "Fighter",
        "corvette",
        sys,
        [0.0, 0.0, 0.0],
        RulesOfEngagement::Defensive,
    );

    // Dock the ship
    app.world_mut().entity_mut(docker).insert(DockedAt(harbour));

    // Verify docked
    assert!(app.world().get::<DockedAt>(docker).is_some());

    // Destroy the harbour
    app.world_mut().despawn(harbour);

    // Advance to trigger force_undock_on_harbour_destroy
    advance_time(&mut app, 1);

    // Verify undocked
    assert!(app.world().get::<DockedAt>(docker).is_none());
}

#[test]
fn test_auto_undock_on_move_command() {
    let mut app = test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "Move-Sys",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );
    let target_sys = spawn_test_system(
        app.world_mut(),
        "Target-Sys",
        [50.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    let harbour = spawn_harbour(app.world_mut(), "Carrier", sys, [0.0, 0.0, 0.0], 10);
    let docker = spawn_ship_with_roe(
        app.world_mut(),
        "Fighter",
        "corvette",
        sys,
        [0.0, 0.0, 0.0],
        RulesOfEngagement::Defensive,
    );

    // Dock the ship
    app.world_mut().entity_mut(docker).insert(DockedAt(harbour));

    // Issue a MoveTo command
    let mut queue = app.world_mut().get_mut::<CommandQueue>(docker).unwrap();
    queue
        .commands
        .push(QueuedCommand::MoveTo { system: target_sys });

    // Advance to trigger auto_undock_on_move_command
    advance_time(&mut app, 1);

    // Verify undocked
    assert!(app.world().get::<DockedAt>(docker).is_none());
}

#[test]
fn test_roe_evasive_stays_docked_during_combat() {
    let mut app = test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "Battle-Sys",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    let harbour = spawn_harbour(app.world_mut(), "Carrier", sys, [0.0, 0.0, 0.0], 10);
    let evasive_ship = spawn_ship_with_roe(
        app.world_mut(),
        "Evasive-Fighter",
        "corvette",
        sys,
        [0.0, 0.0, 0.0],
        RulesOfEngagement::Evasive,
    );

    // Dock the evasive ship
    app.world_mut()
        .entity_mut(evasive_ship)
        .insert(DockedAt(harbour));

    // Spawn a hostile in the system
    common::spawn_raw_hostile(
        app.world_mut(),
        sys,
        100.0,
        100.0,
        5.0,
        0.0,
        "space_creature",
    );

    // Advance time — combat should occur but evasive ship stays docked
    advance_time(&mut app, 1);

    // Evasive ship should still be docked
    assert!(
        app.world().get::<DockedAt>(evasive_ship).is_some(),
        "Evasive ROE ship should stay docked during combat"
    );
}

#[test]
fn test_roe_aggressive_undocks_for_combat() {
    let mut app = test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "Battle-Sys2",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    let harbour = spawn_harbour(app.world_mut(), "Carrier", sys, [0.0, 0.0, 0.0], 10);
    let aggressive_ship = spawn_ship_with_roe(
        app.world_mut(),
        "Aggressive-Fighter",
        "corvette",
        sys,
        [0.0, 0.0, 0.0],
        RulesOfEngagement::Aggressive,
    );

    // Dock the aggressive ship
    app.world_mut()
        .entity_mut(aggressive_ship)
        .insert(DockedAt(harbour));

    // Spawn a hostile in the system
    common::spawn_raw_hostile(
        app.world_mut(),
        sys,
        100.0,
        100.0,
        5.0,
        0.0,
        "space_creature",
    );

    // Advance time — aggressive ship should undock for combat
    advance_time(&mut app, 1);

    // Aggressive ship should be undocked
    assert!(
        app.world().get::<DockedAt>(aggressive_ship).is_none(),
        "Aggressive ROE ship should undock for combat"
    );
}

#[test]
fn test_docked_ship_takes_no_combat_damage() {
    let mut app = test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "Shelter-Sys",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    let harbour = spawn_harbour(app.world_mut(), "Carrier", sys, [0.0, 0.0, 0.0], 10);
    let passive_ship = spawn_ship_with_roe(
        app.world_mut(),
        "Passive-Ship",
        "corvette",
        sys,
        [0.0, 0.0, 0.0],
        RulesOfEngagement::Passive,
    );

    // Dock the passive ship
    app.world_mut()
        .entity_mut(passive_ship)
        .insert(DockedAt(harbour));

    // Record initial HP
    let initial_hp = app.world().get::<ShipHitpoints>(passive_ship).unwrap().hull;

    // Spawn a hostile with high damage
    common::spawn_raw_hostile(
        app.world_mut(),
        sys,
        100.0,
        100.0,
        50.0,
        0.0,
        "space_creature",
    );

    // Advance several ticks
    advance_time(&mut app, 5);

    // Passive docked ship should not have taken damage
    let current_hp = app.world().get::<ShipHitpoints>(passive_ship).unwrap().hull;
    assert_eq!(
        current_hp, initial_hp,
        "Docked passive ship should take no combat damage"
    );
}

#[test]
fn test_roe_all_has_five_variants() {
    assert_eq!(RulesOfEngagement::ALL.len(), 5);
    assert_eq!(RulesOfEngagement::Evasive.label(), "Evasive");
    assert_eq!(RulesOfEngagement::Passive.label(), "Passive");
}

/// Verify no B0001 query conflicts with all harbour systems registered.
#[test]
fn test_harbour_no_query_conflict_in_full_test_app() {
    let mut app = common::full_test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "Conflict-Sys",
        [0.0, 0.0, 0.0],
        0.7,
        true,
        false,
    );

    let harbour = spawn_harbour(app.world_mut(), "Carrier", sys, [0.0, 0.0, 0.0], 10);
    let ship = spawn_ship_with_roe(
        app.world_mut(),
        "Fighter",
        "corvette",
        sys,
        [0.0, 0.0, 0.0],
        RulesOfEngagement::Defensive,
    );
    app.world_mut().entity_mut(ship).insert(DockedAt(harbour));

    // Advancing time should not panic with B0001
    advance_time(&mut app, 1);
    advance_time(&mut app, 1);
}
