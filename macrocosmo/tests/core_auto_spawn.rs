//! #299 (S-5): Integration tests for Core auto-spawn on game start and the
//! settle gate requiring a Core in the target system.

mod common;

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use common::{advance_time, empire_entity, full_test_app, spawn_test_system_with_planet};
use macrocosmo::amount::Amt;
use macrocosmo::components::Position;
use macrocosmo::faction::FactionOwner;
use macrocosmo::galaxy::AtSystem;
use macrocosmo::scripting::game_start_ctx::GameStartActions;
use macrocosmo::ship::command_events::{ColonizeRequested, CommandId};
use macrocosmo::ship::core_deliverable::CoreShip;
use macrocosmo::ship::handlers::handle_colonize_requested;
use macrocosmo::ship::{CommandQueue, Owner, Ship, ShipHitpoints, ShipState};
use macrocosmo::ship_design::ShipDesignRegistry;

/// Helper: insert the infrastructure_core_v1 design into the ShipDesignRegistry.
fn insert_core_design(app: &mut App) {
    let mut reg = app.world_mut().resource_mut::<ShipDesignRegistry>();
    if reg.get("infrastructure_core_v1").is_some() {
        return;
    }
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

/// Helper: spawn a colony ship owned by `faction` at `system`.
fn spawn_colony_ship(world: &mut World, system: Entity, faction: Entity) -> Entity {
    let pos = world
        .get::<Position>(system)
        .map(|p| p.as_array())
        .unwrap_or([0.0; 3]);
    let ship_entity = world.spawn_empty().id();
    let fleet_entity = world.spawn_empty().id();
    world.entity_mut(ship_entity).insert((
        Ship {
            name: "Colony Ship".to_string(),
            design_id: "colony_ship_mk1".to_string(),
            hull_id: "frigate".to_string(),
            modules: Vec::new(),
            owner: Owner::Empire(faction),
            sublight_speed: 0.5,
            ftl_range: 10.0,
            player_aboard: false,
            home_port: system,
            design_revision: 0,
            fleet: Some(fleet_entity),
        },
        ShipState::InSystem { system },
        Position::from(pos),
        ShipHitpoints {
            hull: 120.0,
            hull_max: 120.0,
            armor: 0.0,
            armor_max: 0.0,
            shield: 0.0,
            shield_max: 0.0,
            shield_regen: 0.0,
        },
        CommandQueue::default(),
        macrocosmo::ship::Cargo::default(),
        macrocosmo::ship::ShipModifiers::default(),
        macrocosmo::ship::ShipStats::default(),
        macrocosmo::ship::RulesOfEngagement::default(),
        FactionOwner(faction),
    ));
    world.entity_mut(fleet_entity).insert((
        macrocosmo::ship::Fleet {
            name: "Colony Ship".to_string(),
            flagship: Some(ship_entity),
        },
        macrocosmo::ship::FleetMembers(vec![ship_entity]),
    ));
    ship_entity
}

// ---- Test: apply_game_start_actions spawns Core when spawn_core=true ----

#[test]
fn test_game_start_spawns_core_in_capital() {
    let mut app = full_test_app();
    insert_core_design(&mut app);

    let empire = empire_entity(app.world_mut());

    // Create a capital system
    let (sys, _planet) =
        spawn_test_system_with_planet(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true);
    app.world_mut()
        .get_mut::<macrocosmo::galaxy::StarSystem>(sys)
        .unwrap()
        .is_capital = true;

    // Apply actions with spawn_core = true
    let mut actions = GameStartActions::default();
    actions.spawn_core = true;
    macrocosmo::setup::apply_game_start_actions(app.world_mut(), "humanity_empire", actions);

    // Verify a CoreShip entity exists at the capital system
    let mut core_q = app
        .world_mut()
        .query_filtered::<(&AtSystem, &FactionOwner), With<CoreShip>>();
    let cores: Vec<_> = core_q
        .iter(app.world())
        .filter(|(at, _)| at.0 == sys)
        .collect();
    assert_eq!(
        cores.len(),
        1,
        "exactly one CoreShip should exist at capital"
    );
    assert_eq!(
        cores[0].1.0, empire,
        "CoreShip should be owned by the player empire"
    );
}

#[test]
fn test_game_start_no_core_when_flag_false() {
    let mut app = full_test_app();
    insert_core_design(&mut app);

    let _empire = empire_entity(app.world_mut());

    let (sys, _planet) =
        spawn_test_system_with_planet(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true);
    app.world_mut()
        .get_mut::<macrocosmo::galaxy::StarSystem>(sys)
        .unwrap()
        .is_capital = true;

    // Apply actions with spawn_core = false (default)
    let actions = GameStartActions::default();
    macrocosmo::setup::apply_game_start_actions(app.world_mut(), "humanity_empire", actions);

    // No CoreShip should exist
    let mut core_q = app
        .world_mut()
        .query_filtered::<&AtSystem, With<CoreShip>>();
    let cores: Vec<_> = core_q.iter(app.world()).collect();
    assert!(cores.is_empty(), "no CoreShip should be spawned");
}

// ---- Test: colonize rejected without Core ----

#[test]
fn test_colonize_rejected_without_core() {
    let mut app = full_test_app();

    let empire = empire_entity(app.world_mut());
    let (sys, _planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [5.0, 0.0, 0.0], 0.9, true);
    let ship = spawn_colony_ship(app.world_mut(), sys, empire);

    // Write a ColonizeRequested message (no Core in system)
    {
        let mut msgs = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<ColonizeRequested>>();
        msgs.write(ColonizeRequested {
            command_id: CommandId::ZERO,
            ship,
            target_system: sys,
            planet: None,
            issued_at: 0,
        });
    }

    // Run the handler
    app.world_mut()
        .run_system_once(handle_colonize_requested)
        .expect("run colonize handler");

    // Ship should remain Docked (not Settling)
    let state = app.world().get::<ShipState>(ship).unwrap();
    assert!(
        matches!(state, ShipState::InSystem { .. }),
        "ship should remain Docked when no Core is present"
    );
}

// ---- Test: colonize accepted with Core ----

#[test]
fn test_colonize_accepted_with_core() {
    let mut app = full_test_app();

    let empire = empire_entity(app.world_mut());
    let (sys, _planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [5.0, 0.0, 0.0], 0.9, true);
    let ship = spawn_colony_ship(app.world_mut(), sys, empire);

    // Spawn a Core owned by the same faction
    common::spawn_mock_core_ship(app.world_mut(), sys, empire);

    // Write ColonizeRequested
    {
        let mut msgs = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<ColonizeRequested>>();
        msgs.write(ColonizeRequested {
            command_id: CommandId::ZERO,
            ship,
            target_system: sys,
            planet: None,
            issued_at: 0,
        });
    }

    // Run the handler
    app.world_mut()
        .run_system_once(handle_colonize_requested)
        .expect("run colonize handler");

    // Ship should now be Settling
    let state = app.world().get::<ShipState>(ship).unwrap();
    assert!(
        matches!(state, ShipState::Settling { .. }),
        "ship should be Settling when Core is present"
    );
}

// ---- Test: colonize rejected with enemy Core ----

#[test]
fn test_colonize_rejected_with_enemy_core() {
    let mut app = full_test_app();

    let empire = empire_entity(app.world_mut());
    let (sys, _planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [5.0, 0.0, 0.0], 0.9, true);
    let ship = spawn_colony_ship(app.world_mut(), sys, empire);

    // Spawn a Core owned by a DIFFERENT faction
    let enemy = app
        .world_mut()
        .spawn(macrocosmo::player::Faction::new("enemy_faction", "Enemy"))
        .id();
    common::spawn_mock_core_ship(app.world_mut(), sys, enemy);

    // Write ColonizeRequested
    {
        let mut msgs = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<ColonizeRequested>>();
        msgs.write(ColonizeRequested {
            command_id: CommandId::ZERO,
            ship,
            target_system: sys,
            planet: None,
            issued_at: 0,
        });
    }

    // Run the handler
    app.world_mut()
        .run_system_once(handle_colonize_requested)
        .expect("run colonize handler");

    // Ship should remain Docked (enemy Core doesn't count)
    let state = app.world().get::<ShipState>(ship).unwrap();
    assert!(
        matches!(state, ShipState::InSystem { .. }),
        "ship should remain Docked when only enemy Core present"
    );
}

// ---- Test: settle aborted when Core destroyed mid-settle ----

#[test]
fn test_settle_aborted_when_core_destroyed() {
    let mut app = full_test_app();

    let empire = empire_entity(app.world_mut());
    let (sys, _planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [5.0, 0.0, 0.0], 0.9, true);

    // First, verify that settling succeeds when Core IS present.
    {
        let ship = spawn_colony_ship(app.world_mut(), sys, empire);
        let core = common::spawn_mock_core_ship(app.world_mut(), sys, empire);

        let now = app
            .world()
            .resource::<macrocosmo::time_system::GameClock>()
            .elapsed;
        *app.world_mut().get_mut::<ShipState>(ship).unwrap() = ShipState::Settling {
            system: sys,
            planet: None,
            started_at: now,
            completes_at: now + 60,
        };

        advance_time(&mut app, 61);

        // Ship should have been consumed (despawned) — colony established.
        assert!(
            app.world().get_entity(ship).is_err(),
            "ship should be despawned after successful settle with Core"
        );

        // Clean up the Core for the next sub-test
        app.world_mut().despawn(core);
    }

    // Now test the abort case: Core removed before settling completes.
    {
        let ship2 = spawn_colony_ship(app.world_mut(), sys, empire);
        let core2 = common::spawn_mock_core_ship(app.world_mut(), sys, empire);

        let now = app
            .world()
            .resource::<macrocosmo::time_system::GameClock>()
            .elapsed;
        *app.world_mut().get_mut::<ShipState>(ship2).unwrap() = ShipState::Settling {
            system: sys,
            planet: None,
            started_at: now,
            completes_at: now + 60,
        };

        // Destroy the Core before settling completes
        app.world_mut().despawn(core2);

        advance_time(&mut app, 61);

        // Ship should still exist and be back to Docked
        let state = app
            .world()
            .get::<ShipState>(ship2)
            .expect("ship should still exist after Core removed mid-settle");
        assert!(
            matches!(state, ShipState::InSystem { .. }),
            "ship should be Docked after Core removed mid-settle"
        );
    }
}
