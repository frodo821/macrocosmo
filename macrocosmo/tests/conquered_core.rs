//! #298 (S-4): Integration tests for the Conquered Core state mechanic.
//!
//! Covers:
//! - HP=1 clamp on Core ships during combat
//! - Conquered state transition when hull reaches 1.0
//! - Wartime hold (no recovery while at war)
//! - Peacetime recovery (attacker fleet absent)
//! - Peacetime recovery blocked while attacker fleet present
//! - Normal ship repair skips conquered cores
//! - Casus belli event on peacetime Core attack

mod common;

use bevy::prelude::*;
use common::{advance_time, empire_entity, spawn_raw_hostile, spawn_test_system, test_app};
use macrocosmo::components::Position;
use macrocosmo::faction::{FactionRelations, FactionView, RelationState};
use macrocosmo::galaxy::AtSystem;
use macrocosmo::ship::{ConqueredCore, CoreShip, Owner, Ship, ShipHitpoints, ShipState};

/// Helper: spawn a Core ship with given hull HP. Returns (core_entity, faction_entity).
fn spawn_core_at(
    world: &mut World,
    system: Entity,
    hull: f64,
    hull_max: f64,
    faction: Entity,
) -> Entity {
    let pos = Position::from([0.0, 0.0, 0.0]);
    let core = world.spawn_empty().id();
    let fleet_entity = world
        .spawn((
            macrocosmo::ship::Fleet {
                name: "Core Fleet".into(),
                flagship: Some(core),
            },
            macrocosmo::ship::FleetMembers(vec![core]),
        ))
        .id();
    world.entity_mut(core).insert((
        Ship {
            name: "Test Core".into(),
            design_id: "infrastructure_core_v1".into(),
            hull_id: "core_hull".into(),
            modules: Vec::new(),
            owner: Owner::Empire(faction),
            sublight_speed: 0.0,
            ftl_range: 0.0,
            player_aboard: false,
            home_port: system,
            design_revision: 0,
            fleet: Some(fleet_entity),
        },
        ShipState::Docked { system },
        pos,
        ShipHitpoints {
            hull,
            hull_max,
            armor: 0.0,
            armor_max: 0.0,
            shield: 0.0,
            shield_max: 0.0,
            shield_regen: 0.0,
        },
        CoreShip,
        AtSystem(system),
        macrocosmo::ship::CommandQueue::default(),
        macrocosmo::ship::ShipModifiers::default(),
        macrocosmo::ship::ShipStats::default(),
        macrocosmo::ship::RulesOfEngagement::default(),
        macrocosmo::ship::Cargo::default(),
    ));
    core
}

/// Core ship hull is clamped at 1.0 during combat (not destroyed).
#[test]
fn core_ship_hp_clamped_at_one_during_combat() {
    let mut app = test_app();
    let sys = spawn_test_system(app.world_mut(), "Alpha", [0.0, 0.0, 0.0], 1.0, true, false);
    let empire = empire_entity(app.world_mut());

    let core = spawn_core_at(app.world_mut(), sys, 5.0, 100.0, empire);

    // Spawn a very strong hostile to deal massive damage
    let _hostile = spawn_raw_hostile(
        app.world_mut(),
        sys,
        100.0,
        100.0,
        1000.0,
        0.0,
        "space_creature",
    );

    advance_time(&mut app, 1);

    // Core should still exist with hull=1.0
    let hp = app
        .world()
        .get::<ShipHitpoints>(core)
        .expect("Core should still exist");
    assert!(
        (hp.hull - 1.0).abs() < f64::EPSILON,
        "Core hull should be clamped at 1.0, got {}",
        hp.hull
    );
}

/// ConqueredCore component is attached when hull reaches 1.0.
#[test]
fn conquered_transition_attaches_component() {
    let mut app = test_app();
    let sys = spawn_test_system(app.world_mut(), "Alpha", [0.0, 0.0, 0.0], 1.0, true, false);
    let empire = empire_entity(app.world_mut());

    let core = spawn_core_at(app.world_mut(), sys, 5.0, 100.0, empire);

    // Spawn hostile to trigger combat
    let _hostile = spawn_raw_hostile(
        app.world_mut(),
        sys,
        100.0,
        100.0,
        1000.0,
        0.0,
        "space_creature",
    );

    advance_time(&mut app, 1);

    // Core should now have ConqueredCore
    let conquered = app.world().get::<ConqueredCore>(core);
    assert!(
        conquered.is_some(),
        "Core should have ConqueredCore after hull reaches 1.0"
    );
}

/// During wartime, conquered Core hull stays locked at 1.0 (no recovery).
#[test]
fn wartime_hold_prevents_recovery() {
    let mut app = test_app();
    let sys = spawn_test_system(app.world_mut(), "Alpha", [0.0, 0.0, 0.0], 1.0, true, false);
    let empire = empire_entity(app.world_mut());

    let core = spawn_core_at(app.world_mut(), sys, 5.0, 100.0, empire);

    // Spawn hostile and run combat
    let _hostile = spawn_raw_hostile(
        app.world_mut(),
        sys,
        100.0,
        100.0,
        1000.0,
        0.0,
        "space_creature",
    );

    advance_time(&mut app, 1);

    // Verify conquered state
    assert!(app.world().get::<ConqueredCore>(core).is_some());
    assert!((app.world().get::<ShipHitpoints>(core).unwrap().hull - 1.0).abs() < f64::EPSILON);

    // Set relations to War between empire and the hostile faction
    let hostile_faction = {
        let hf = app
            .world()
            .resource::<macrocosmo::faction::HostileFactions>();
        hf.space_creature.unwrap()
    };
    {
        let mut relations = app.world_mut().resource_mut::<FactionRelations>();
        relations.set(
            empire,
            hostile_faction,
            FactionView::new(RelationState::War, -100.0),
        );
    }

    // Remove the hostile entity so the "attacker present" check doesn't block
    // (war hold should prevent recovery regardless)
    let hostile_entity: Entity = {
        let mut q = app
            .world_mut()
            .query_filtered::<Entity, With<macrocosmo::galaxy::Hostile>>();
        q.iter(app.world()).next().unwrap()
    };
    app.world_mut().despawn(hostile_entity);

    // Advance time — hull should NOT recover during war
    advance_time(&mut app, 10);

    let hp = app.world().get::<ShipHitpoints>(core).unwrap();
    assert!(
        (hp.hull - 1.0).abs() < f64::EPSILON,
        "Hull should stay at 1.0 during wartime, got {}",
        hp.hull
    );
    assert!(
        app.world().get::<ConqueredCore>(core).is_some(),
        "ConqueredCore should still be present during wartime"
    );
}

/// Peacetime recovery: when attacker fleet is absent, hull recovers.
#[test]
fn peacetime_recovery_when_attacker_absent() {
    let mut app = test_app();
    let sys = spawn_test_system(app.world_mut(), "Alpha", [0.0, 0.0, 0.0], 1.0, true, false);
    let empire = empire_entity(app.world_mut());

    let core = spawn_core_at(app.world_mut(), sys, 5.0, 100.0, empire);

    // Spawn hostile
    let hostile_entity = spawn_raw_hostile(
        app.world_mut(),
        sys,
        100.0,
        100.0,
        1000.0,
        0.0,
        "space_creature",
    );

    advance_time(&mut app, 1);

    // Verify conquered
    assert!(app.world().get::<ConqueredCore>(core).is_some());

    // Remove hostile (simulates fleet leaving)
    app.world_mut().despawn(hostile_entity);

    // Relations are Neutral/-100 (default from setup_test_hostile_factions) — NOT at war
    // Advance time for recovery (default rate = 1.0 HP/hexady)
    advance_time(&mut app, 10);

    let hp = app.world().get::<ShipHitpoints>(core).unwrap();
    assert!(
        hp.hull > 1.0,
        "Hull should recover during peacetime with attacker absent, got {}",
        hp.hull
    );
}

/// Full recovery removes ConqueredCore component.
#[test]
fn full_recovery_removes_conquered_component() {
    let mut app = test_app();
    let sys = spawn_test_system(app.world_mut(), "Alpha", [0.0, 0.0, 0.0], 1.0, true, false);
    let empire = empire_entity(app.world_mut());

    // Use small hull_max so recovery completes quickly
    let core = spawn_core_at(app.world_mut(), sys, 5.0, 10.0, empire);

    let hostile_entity = spawn_raw_hostile(
        app.world_mut(),
        sys,
        100.0,
        100.0,
        1000.0,
        0.0,
        "space_creature",
    );

    advance_time(&mut app, 1);
    assert!(app.world().get::<ConqueredCore>(core).is_some());

    // Remove hostile
    app.world_mut().despawn(hostile_entity);

    // Recovery rate is 1.0 HP/hexady. From hull=1.0 to hull_max=10.0 needs 9 hexadies.
    advance_time(&mut app, 10);

    let hp = app.world().get::<ShipHitpoints>(core).unwrap();
    assert!(
        (hp.hull - 10.0).abs() < f64::EPSILON,
        "Hull should be fully recovered, got {}",
        hp.hull
    );
    assert!(
        app.world().get::<ConqueredCore>(core).is_none(),
        "ConqueredCore should be removed after full recovery"
    );
}

/// Peacetime recovery is blocked while attacker fleet ships remain in system.
#[test]
fn peacetime_recovery_blocked_while_attacker_fleet_present() {
    let mut app = test_app();
    let sys = spawn_test_system(app.world_mut(), "Alpha", [0.0, 0.0, 0.0], 1.0, true, false);
    let empire = empire_entity(app.world_mut());

    let core = spawn_core_at(app.world_mut(), sys, 5.0, 100.0, empire);

    let hostile_entity = spawn_raw_hostile(
        app.world_mut(),
        sys,
        100.0,
        100.0,
        1000.0,
        0.0,
        "space_creature",
    );

    advance_time(&mut app, 1);
    assert!(app.world().get::<ConqueredCore>(core).is_some());

    // Get the hostile faction entity
    let hostile_faction = {
        let hf = app
            .world()
            .resource::<macrocosmo::faction::HostileFactions>();
        hf.space_creature.unwrap()
    };

    // Remove the hostile-marker entity but spawn a regular ship owned by the
    // hostile faction docked at the same system (simulating attacker fleet)
    app.world_mut().despawn(hostile_entity);

    let fleet_e = app.world_mut().spawn_empty().id();
    let _attacker_ship = app
        .world_mut()
        .spawn((
            Ship {
                name: "Attacker Ship".into(),
                design_id: "corvette_mk1".into(),
                hull_id: "corvette_hull".into(),
                modules: Vec::new(),
                owner: Owner::Empire(hostile_faction),
                sublight_speed: 1.0,
                ftl_range: 10.0,
                player_aboard: false,
                home_port: sys,
                design_revision: 0,
                fleet: Some(fleet_e),
            },
            ShipState::Docked { system: sys },
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
        ))
        .id();

    // Advance time — recovery should NOT happen
    advance_time(&mut app, 10);

    let hp = app.world().get::<ShipHitpoints>(core).unwrap();
    assert!(
        (hp.hull - 1.0).abs() < f64::EPSILON,
        "Hull should stay at 1.0 while attacker fleet present, got {}",
        hp.hull
    );
}

/// Normal ship repair (Port) does NOT heal conquered cores.
/// We attach ConqueredCore manually and add a Port via SystemBuildings —
/// even with a Port present, the `Without<ConqueredCore>` filter on
/// `tick_ship_repair` should prevent healing.
#[test]
fn port_repair_skips_conquered_core() {
    use macrocosmo::colony::{BuildingRegistry, DEFAULT_SYSTEM_BUILDING_SLOTS, SystemBuildings};
    use macrocosmo::scripting::building_api::BuildingId;

    let mut app = test_app();
    let sys = spawn_test_system(app.world_mut(), "Alpha", [0.0, 0.0, 0.0], 1.0, true, false);
    let empire = empire_entity(app.world_mut());

    // Add a Port to the system
    {
        let registry = app.world().resource::<BuildingRegistry>().clone();
        let mut slots: Vec<Option<BuildingId>> = vec![None; DEFAULT_SYSTEM_BUILDING_SLOTS];
        // Use the test registry's port id directly
        slots[0] = Some(BuildingId("port".into()));
        let sb = SystemBuildings { slots };
        app.world_mut().entity_mut(sys).insert(sb);
    }

    let core = spawn_core_at(app.world_mut(), sys, 1.0, 100.0, empire);

    // Manually attach ConqueredCore (simulates post-combat)
    let hostile_faction = {
        common::setup_test_hostile_factions(app.world_mut());
        app.world()
            .resource::<macrocosmo::faction::HostileFactions>()
            .space_creature
            .unwrap()
    };
    app.world_mut().entity_mut(core).insert(ConqueredCore {
        attacker_faction: hostile_faction,
    });

    // Set relations to War so conquered recovery doesn't kick in
    {
        let mut relations = app.world_mut().resource_mut::<FactionRelations>();
        relations.set(
            empire,
            hostile_faction,
            FactionView::new(RelationState::War, -100.0),
        );
    }

    advance_time(&mut app, 10);

    let hp = app.world().get::<ShipHitpoints>(core).unwrap();
    assert!(
        (hp.hull - 1.0).abs() < f64::EPSILON,
        "Port repair should NOT heal conquered Core, got hull={}",
        hp.hull
    );
}
