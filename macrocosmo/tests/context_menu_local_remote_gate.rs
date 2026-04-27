//! #462: gate `context_menu`'s direct `ShipState` writes behind the
//! local / zero-delay invariant.
//!
//! `apply_local_ship_command` is the single point where the context
//! menu mutates a ship's `ShipState` directly (without going through
//! `PendingShipCommand`). Its `expected_delay` argument is checked via
//! `debug_assert_eq!(0)` so any future regression — e.g. a UI change
//! that routes a remote command through this path — fails loudly in
//! dev/test builds instead of silently bypassing light-speed transport.
//!
//! Tests:
//! 1. `apply_local_ship_command_writes_state_when_delay_zero` —
//!    the helper updates `ShipState` for the local case (legacy
//!    behavior preserved).
//! 2. `apply_local_ship_command_panics_on_nonzero_delay` —
//!    primary acceptance criterion, encoded as a unit assertion on
//!    the helper itself. A debug build panics with the diagnostic
//!    message naming the bad delay.
//! 3. `apply_local_ship_command_returns_false_for_unknown_entity` —
//!    despawn-mid-frame edge case: helper reports failure so callers
//!    can leave `SelectedShip` untouched (matches pre-#462 behavior).
//! 4. `pending_ship_command_path_does_not_mutate_state` —
//!    integration-level pin: building a `PendingShipCommand` (the
//!    path the context menu must use for nonzero delay) leaves the
//!    target ship's `ShipState` untouched until the dispatcher fires.

mod common;

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;

use macrocosmo::ship::{
    Cargo, PendingShipCommand, Ship, ShipCommand, ShipHitpoints, ShipState, SurveyData,
};
use macrocosmo::ui::context_menu::apply_local_ship_command;

use common::{spawn_test_ship, spawn_test_system, test_app};

/// Drive the helper through a one-shot system so we get a real
/// `Query` with the same component set the production callsite uses.
fn run_helper(app: &mut App, ship: Entity, new_state: ShipState, expected_delay: i64) -> bool {
    app.world_mut()
        .run_system_once(
            move |mut q: Query<
                (
                    Entity,
                    &mut Ship,
                    &mut ShipState,
                    Option<&mut Cargo>,
                    &ShipHitpoints,
                    Option<&SurveyData>,
                ),
                Without<macrocosmo::colony::SlotAssignment>,
            >|
             -> bool {
                apply_local_ship_command(ship, new_state.clone(), expected_delay, &mut q)
            },
        )
        .expect("run_system_once failed")
}

#[test]
fn apply_local_ship_command_writes_state_when_delay_zero() {
    let mut app = test_app();
    let system_a = spawn_test_system(
        app.world_mut(),
        "Alpha",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    let system_b = spawn_test_system(
        app.world_mut(),
        "Beta",
        [1.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let ship = spawn_test_ship(
        app.world_mut(),
        "Scout",
        "explorer_mk1",
        system_a,
        [0.0, 0.0, 0.0],
    );

    // Pre-condition: docked at system_a.
    assert!(matches!(
        app.world().get::<ShipState>(ship).unwrap(),
        ShipState::InSystem { system } if *system == system_a
    ));

    let new_state = ShipState::Surveying {
        target_system: system_b,
        started_at: 0,
        completes_at: 10,
    };

    let ok = run_helper(&mut app, ship, new_state, 0);
    assert!(ok, "helper should report success when ship exists");

    // Post-condition: state mutated to Surveying.
    let st = app.world().get::<ShipState>(ship).unwrap();
    assert!(
        matches!(st, ShipState::Surveying { target_system, .. } if *target_system == system_b),
        "expected Surveying with target=system_b after local apply",
    );
}

#[test]
#[should_panic(expected = "non-zero delay")]
fn apply_local_ship_command_panics_on_nonzero_delay() {
    // Primary acceptance criterion (#462): the helper MUST refuse to
    // apply a state change when the caller's light-speed delay is
    // nonzero. `debug_assert_eq!` panics in test builds, locking in
    // the invariant so any future regression that routes a remote
    // command through the local path is caught at CI time.
    let mut app = test_app();
    let system_a = spawn_test_system(
        app.world_mut(),
        "Alpha",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    let system_b = spawn_test_system(
        app.world_mut(),
        "Beta",
        [10.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let ship = spawn_test_ship(
        app.world_mut(),
        "Scout",
        "explorer_mk1",
        system_a,
        [0.0, 0.0, 0.0],
    );

    let new_state = ShipState::Surveying {
        target_system: system_b,
        started_at: 0,
        completes_at: 10,
    };

    // 600 hd ≈ 10 ly light delay — caller would normally route this
    // through PendingShipCommand. Calling the local helper instead is
    // a programming error.
    let _ = run_helper(&mut app, ship, new_state, 600);
}

#[test]
fn apply_local_ship_command_returns_false_for_unknown_entity() {
    // Despawn mid-frame edge case: helper reports `false` so the
    // caller in `context_menu` knows not to clear `SelectedShip`.
    let mut app = test_app();
    let system_a = spawn_test_system(
        app.world_mut(),
        "Alpha",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    let ship = spawn_test_ship(
        app.world_mut(),
        "Scout",
        "explorer_mk1",
        system_a,
        [0.0, 0.0, 0.0],
    );
    // Despawn the ship before the helper runs.
    app.world_mut().entity_mut(ship).despawn();

    let ok = run_helper(
        &mut app,
        ship,
        ShipState::InSystem { system: system_a },
        0,
    );
    assert!(!ok, "helper should report failure for missing entity");
}

#[test]
fn pending_ship_command_path_does_not_mutate_state() {
    // Integration-level pin: the path the context menu MUST use for
    // any nonzero delay (= a distant docked ship) is
    // `PendingShipCommand`, which is consumed asynchronously by the
    // command dispatcher. Spawning a `PendingShipCommand` (= what the
    // remote branch in `draw_context_menu` does) MUST NOT mutate the
    // target ship's `ShipState` until the dispatcher actually fires.
    //
    // This is the structural complement to test #2: we cannot easily
    // invoke `draw_context_menu` directly here (it requires an
    // `egui::Context` and 16+ Bevy queries), but we can pin the
    // contract that `PendingShipCommand` is a queue entry, not an
    // immediate state write.
    let mut app = test_app();
    let home_sys = spawn_test_system(
        app.world_mut(),
        "Home",
        [0.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    let target_sys = spawn_test_system(
        app.world_mut(),
        "Target",
        [10.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let ship = spawn_test_ship(
        app.world_mut(),
        "Scout",
        "explorer_mk1",
        home_sys,
        [0.0, 0.0, 0.0],
    );

    // Mirror the `draw_context_menu` remote path: push a
    // PendingShipCommand instead of writing the new state.
    let arrives_at = 600; // approx 10 ly light delay
    app.world_mut().spawn(PendingShipCommand {
        ship,
        command: ShipCommand::Survey {
            target: target_sys,
        },
        arrives_at,
    });

    // The ship must still be docked at home — no immediate mutation.
    let st = app.world().get::<ShipState>(ship).unwrap();
    assert!(
        matches!(st, ShipState::InSystem { system } if *system == home_sys),
        "expected ship to remain InSystem(home_sys) until arrival",
    );

    // And the PendingShipCommand should be present, awaiting the
    // dispatcher.
    let pending_count = app
        .world_mut()
        .query::<&PendingShipCommand>()
        .iter(app.world())
        .count();
    assert_eq!(
        pending_count, 1,
        "exactly one in-flight PendingShipCommand should exist"
    );
}
