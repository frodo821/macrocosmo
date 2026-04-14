//! #295 (S-1): Regression tests for Core-ship-derived sovereignty.
//!
//! These tests pin down the behaviour of `faction::system_owner` and
//! `update_sovereignty` after the refactor that removed colony-population
//! based ownership in favour of Core ship presence.

mod common;

use bevy::prelude::*;
use common::{
    advance_time, empire_entity, full_test_app, spawn_mock_core_ship, spawn_test_colony,
    spawn_test_system,
};
use macrocosmo::amount::Amt;
use macrocosmo::faction::{FactionOwner, system_owner};
use macrocosmo::galaxy::{AtSystem, Sovereignty};
use macrocosmo::ship::Owner;

/// With no Core ship stationed in a system, `system_owner` returns None.
#[test]
fn system_owner_returns_none_when_no_core_ship() {
    let mut app = full_test_app();
    let sys = spawn_test_system(app.world_mut(), "Lonely", [0.0, 0.0, 0.0], 1.0, true, false);

    // No Core ship spawned — query should yield nothing for this system.
    let mut q = app
        .world_mut()
        .query::<(&AtSystem, &FactionOwner)>();
    let query: Vec<_> = q.iter(app.world()).collect();
    // The query type mirrors the helper; run the helper directly through an
    // adapter system to exercise the exact signature.
    let _ = query; // exercise the query at least once

    // Use a tiny system to invoke the helper with a real `Query` handle.
    let result = std::sync::Arc::new(std::sync::Mutex::new(Some(Entity::PLACEHOLDER)));
    let result_w = result.clone();
    let sys_target = sys;
    app.add_systems(
        Update,
        move |at_system: Query<(&AtSystem, &FactionOwner)>| {
            *result_w.lock().unwrap() = system_owner(sys_target, &at_system);
        },
    );
    app.update();
    assert_eq!(*result.lock().unwrap(), None);
}

/// With a Core ship stationed, `system_owner` returns that ship's faction.
#[test]
fn system_owner_returns_faction_when_core_ship_present() {
    let mut app = full_test_app();
    let sys = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, false);
    let empire = empire_entity(app.world_mut());
    spawn_mock_core_ship(app.world_mut(), sys, empire);

    let result = std::sync::Arc::new(std::sync::Mutex::new(None));
    let result_w = result.clone();
    app.add_systems(
        Update,
        move |at_system: Query<(&AtSystem, &FactionOwner)>| {
            *result_w.lock().unwrap() = system_owner(sys, &at_system);
        },
    );
    app.update();
    assert_eq!(*result.lock().unwrap(), Some(empire));
}

/// `update_sovereignty` respects Core ship presence: a colony alone is not
/// enough to confer ownership. This is the key regression guard against the
/// previous "any colony means player_empire owns it" heuristic.
#[test]
fn update_sovereignty_ignores_colony_without_core_ship() {
    let mut app = full_test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "NoCoreYet",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );

    // Colony without any Core ship.
    spawn_test_colony(
        app.world_mut(),
        sys,
        Amt::units(100),
        Amt::units(100),
        vec![],
    );

    advance_time(&mut app, 1);

    let sov = app.world().get::<Sovereignty>(sys).unwrap();
    assert_eq!(sov.owner, None);
    assert_eq!(sov.control_score, 0.0);
}

/// With a Core ship present, `update_sovereignty` writes the Core ship's
/// faction into `Sovereignty.owner`.
#[test]
fn update_sovereignty_sets_owner_when_core_ship_present() {
    let mut app = full_test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "CoreHeld",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let empire = empire_entity(app.world_mut());
    spawn_mock_core_ship(app.world_mut(), sys, empire);

    advance_time(&mut app, 1);

    let sov = app.world().get::<Sovereignty>(sys).unwrap();
    assert_eq!(sov.owner, Some(Owner::Empire(empire)));
    assert_eq!(sov.control_score, 1.0);
}

/// When the Core ship despawns, sovereignty reverts to None on the next tick.
#[test]
fn update_sovereignty_reverts_when_core_ship_despawned() {
    let mut app = full_test_app();
    let sys = spawn_test_system(
        app.world_mut(),
        "TransitionSys",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let empire = empire_entity(app.world_mut());
    let core = spawn_mock_core_ship(app.world_mut(), sys, empire);

    advance_time(&mut app, 1);

    assert_eq!(
        app.world().get::<Sovereignty>(sys).unwrap().owner,
        Some(Owner::Empire(empire))
    );

    // Despawn the Core ship and re-tick.
    app.world_mut().despawn(core);
    advance_time(&mut app, 1);

    let sov = app.world().get::<Sovereignty>(sys).unwrap();
    assert_eq!(sov.owner, None);
    assert_eq!(sov.control_score, 0.0);
}

// TODO(#297): multi-faction test (two Core ships, two factions, one
// system) requires FactionOwner cascade and clearer ownership conflict
// semantics — gated behind S-2.
