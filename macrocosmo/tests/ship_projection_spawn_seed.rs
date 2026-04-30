//! #481: spawn-time `ShipProjection` seed regression tests.
//!
//! Background: epic #473 / #477 flipped the Galaxy Map's own-ship render
//! branch to iterate `KnowledgeStore.projections` exclusively. Without a
//! spawn-time seed, ships that have never received a command (initial
//! fleet at game start, freshly shipyard-built ships, idle stations) are
//! invisible to their owner's map until the first dispatch site (#475)
//! writes a projection.
//!
//! `seed_own_ship_projections` (in `knowledge/mod.rs`) installs a
//! steady-state projection (`InSystem` at `home_port`) for every newly
//! spawned own-empire ship that doesn't already have one.
//!
//! Tests:
//!
//! 1. `spawned_own_ship_has_projection_after_first_update` — primary
//!    contract: spawn an empire-owned ship, run one Update, assert the
//!    projection is present and `compute_own_ship_render_inputs`
//!    returns a non-empty render set.
//! 2. `projection_seed_idempotent_across_updates` — running multiple
//!    updates does not clobber an existing projection.
//! 3. `neutral_ships_skipped` — `Owner::Neutral` ships don't seed any
//!    empire's projection store.
//! 4. `shipyard_built_ship_gets_projection` — ship spawned mid-game
//!    (post-Startup, post-NewGame) still gets seeded on the first
//!    Update after spawn.
//! 5. `existing_projection_not_overwritten` — when a dispatch site has
//!    already written a projection with intended_* fields, the seed
//!    does not erase them.

mod common;

use std::collections::HashMap;

use bevy::prelude::*;

use macrocosmo::knowledge::{
    KnowledgeStore, SEED_DISPATCHED_AT_SENTINEL, ShipProjection, ShipSnapshotState,
    seed_own_ship_projections,
};
use macrocosmo::player::Empire;
use macrocosmo::ship::{Owner, Ship};
use macrocosmo::time_system::GameClock;
use macrocosmo::visualization::ships::{OwnShipMetadata, compute_own_ship_render_inputs};

use common::{spawn_test_empire, spawn_test_ship, spawn_test_system, test_app};

/// Build a minimal app with the seed system installed (`test_app` does
/// not register `KnowledgePlugin`, so we wire the system directly).
fn seed_test_app() -> App {
    let mut app = test_app();
    app.add_systems(
        Update,
        seed_own_ship_projections.after(macrocosmo::time_system::advance_game_time),
    );
    app
}

#[test]
fn spawned_own_ship_has_projection_after_first_update() {
    let mut app = seed_test_app();
    let empire = spawn_test_empire(app.world_mut());
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let ship = spawn_test_ship(
        app.world_mut(),
        "Scout-1",
        "explorer_mk1",
        home,
        [0.0, 0.0, 0.0],
    );
    app.world_mut()
        .entity_mut(ship)
        .get_mut::<Ship>()
        .unwrap()
        .owner = Owner::Empire(empire);

    app.update();

    let store = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire should have KnowledgeStore");
    let projection = store
        .get_projection(ship)
        .expect("seed system must have installed a projection");
    assert_eq!(projection.entity, ship);
    assert_eq!(projection.projected_state, ShipSnapshotState::InSystem);
    assert_eq!(projection.projected_system, Some(home));
    assert!(projection.intended_state.is_none());
    assert!(projection.intended_system.is_none());

    // Galaxy Map render path must now find the ship.
    let mut metadata = HashMap::new();
    metadata.insert(
        ship,
        OwnShipMetadata {
            design_id: "explorer_mk1".into(),
            is_station: false,
            is_harbour: false,
            owned_by_viewing_empire: true,
        },
    );
    let render = compute_own_ship_render_inputs(store, &metadata);
    assert_eq!(render.len(), 1, "renderer should now see the ship");
    assert_eq!(render[0].entity, ship);
    assert_eq!(render[0].projected_system, Some(home));
}

#[test]
fn projection_seed_idempotent_across_updates() {
    let mut app = seed_test_app();
    let empire = spawn_test_empire(app.world_mut());
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let ship = spawn_test_ship(
        app.world_mut(),
        "Scout-1",
        "explorer_mk1",
        home,
        [0.0, 0.0, 0.0],
    );
    app.world_mut()
        .entity_mut(ship)
        .get_mut::<Ship>()
        .unwrap()
        .owner = Owner::Empire(empire);

    app.update();
    let projection_count_before = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .unwrap()
        .iter_projections()
        .count();
    assert_eq!(projection_count_before, 1);

    // Multiple subsequent updates must not duplicate or alter the entry.
    for _ in 0..5 {
        app.update();
    }
    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    assert_eq!(
        store.iter_projections().count(),
        1,
        "seed system must not duplicate existing entries"
    );
    let projection = store.get_projection(ship).unwrap();
    assert_eq!(projection.projected_state, ShipSnapshotState::InSystem);
    assert_eq!(projection.projected_system, Some(home));
}

#[test]
fn neutral_ships_skipped() {
    let mut app = seed_test_app();
    let empire = spawn_test_empire(app.world_mut());
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    // spawn_test_ship leaves owner = Owner::Neutral by default — that's
    // exactly the case under test.
    let neutral_ship = spawn_test_ship(
        app.world_mut(),
        "Pirate-1",
        "explorer_mk1",
        home,
        [0.0, 0.0, 0.0],
    );
    assert_eq!(
        app.world()
            .entity(neutral_ship)
            .get::<Ship>()
            .unwrap()
            .owner,
        Owner::Neutral
    );

    app.update();

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    assert!(
        store.get_projection(neutral_ship).is_none(),
        "Owner::Neutral ships must not seed any empire's projections"
    );
    assert_eq!(
        store.iter_projections().count(),
        0,
        "no projections should exist for the empire"
    );
}

#[test]
fn shipyard_built_ship_gets_projection() {
    // Same shape as the initial-fleet case, but the ship is spawned
    // *after* the first Update (= simulating a shipyard build queue
    // completion mid-game). The `Added<Ship>` filter must still fire.
    let mut app = seed_test_app();
    let empire = spawn_test_empire(app.world_mut());
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    app.update();
    // Advance the clock — the seed's dispatched_at is independent of
    // `clock.elapsed` (#497 sentinel), but we still bump the clock to
    // confirm the seed does not capture it accidentally.
    app.world_mut().resource_mut::<GameClock>().elapsed = 42;
    let ship = spawn_test_ship(
        app.world_mut(),
        "Mk1",
        "explorer_mk1",
        home,
        [0.0, 0.0, 0.0],
    );
    app.world_mut()
        .entity_mut(ship)
        .get_mut::<Ship>()
        .unwrap()
        .owner = Owner::Empire(empire);
    app.update();

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let projection = store
        .get_projection(ship)
        .expect("post-Startup-spawned ship must still be seeded");
    assert_eq!(projection.projected_system, Some(home));
    // #497: seed always uses the sentinel `i64::MIN` so the reconciler
    // staleness gate (#484) accepts any in-flight fact against a
    // never-dispatched seed.
    assert_eq!(projection.dispatched_at, SEED_DISPATCHED_AT_SENTINEL);
}

#[test]
fn existing_projection_not_overwritten() {
    // If a dispatch site (#475) wrote a projection in the same tick the
    // ship was spawned (e.g. an AI policy issuing a command immediately
    // on spawn), the seed must not clobber the more-specific intended
    // trajectory.
    let mut app = seed_test_app();
    let empire = spawn_test_empire(app.world_mut());
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [10.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );
    let ship = spawn_test_ship(
        app.world_mut(),
        "Scout-1",
        "explorer_mk1",
        home,
        [0.0, 0.0, 0.0],
    );
    app.world_mut()
        .entity_mut(ship)
        .get_mut::<Ship>()
        .unwrap()
        .owner = Owner::Empire(empire);

    // Pre-populate a projection with `intended_*` set, mimicking a
    // dispatch-time write that happened before the seed system ran.
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 0,
            expected_arrival_at: Some(100),
            expected_return_at: None,
            projected_state: ShipSnapshotState::InSystem,
            projected_system: Some(home),
            intended_state: Some(ShipSnapshotState::Surveying),
            intended_system: Some(frontier),
            intended_takes_effect_at: Some(50),
        });
    }

    app.update();

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let projection = store.get_projection(ship).unwrap();
    assert_eq!(
        projection.intended_state,
        Some(ShipSnapshotState::Surveying),
        "seed must not overwrite an existing intended trajectory"
    );
    assert_eq!(projection.intended_system, Some(frontier));
    assert_eq!(projection.expected_arrival_at, Some(100));
}
