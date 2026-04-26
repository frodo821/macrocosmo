//! Regression test for #128 counter-leak fix.
//!
//! Background: previously `RouteCalculationsPending` was a `Resource { count: u32 }`
//! incremented when a `PendingRoute` component was inserted on a ship and
//! decremented in `poll_pending_routes`. If a ship was despawned (combat,
//! settlement, colonization) while still holding `PendingRoute`, the
//! component was auto-cleaned by Bevy but the counter was never decremented
//! — leaking. The leaked count made `advance_game_time` return early
//! forever, freezing game time.
//!
//! Fix: replaced the counter with an existence check
//! (`Query<(), With<PendingRoute>>`) inside `advance_game_time`. There is
//! no counter to leak anymore — when the last `PendingRoute` disappears
//! (whether removed normally or via despawn) the gate opens.
//!
//! This test verifies the structural invariant: a ship with `PendingRoute`
//! freezes time, and despawning it (without going through
//! `poll_pending_routes`) restores time advancement.

use std::time::Duration;

use bevy::prelude::*;
use bevy::tasks::AsyncComputeTaskPool;
use bevy::time::{TimeUpdateStrategy, Virtual};
use macrocosmo::ship::routing::PendingRoute;
use macrocosmo::time_system::{GameClock, GameSpeed, advance_game_time};

/// Build a minimal app exercising only `advance_game_time` and the
/// `PendingRoute` component. Avoids `test_app()` so this test stays
/// laser-focused on the gate logic.
fn build_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    // Deterministic frame delta — eliminates wall-clock variance flake.
    // Each `app.update()` injects 1.0 s of real time, which the virtual
    // clock forwards into `Time` after `max_delta` clamping. We bump
    // `max_delta` to 1 s so the full second reaches `advance_game_time`.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs(1)));
    app.world_mut()
        .resource_mut::<Time<Virtual>>()
        .set_max_delta(Duration::from_secs(1));
    app.insert_resource(GameClock::default());
    app.insert_resource(GameSpeed {
        hexadies_per_second: 1.0,
        previous_speed: 1.0,
    });
    app.add_systems(Update, advance_game_time);
    // First Bevy frame is special-cased to delta=0 — pump it past so
    // subsequent frames accumulate the full 1 hexadies / frame budget.
    app.update();
    app
}

/// Spawn a ship-like entity carrying a `PendingRoute` whose underlying
/// async task is already complete (returns `None` immediately). The task
/// itself is not what blocks time — the *presence* of the component is.
fn spawn_pending_route(world: &mut World) -> Entity {
    let task = AsyncComputeTaskPool::get().spawn(async { None });
    world
        .spawn(PendingRoute {
            task,
            target_system: Entity::PLACEHOLDER,
            command_id: None,
        })
        .id()
}

#[test]
fn time_freezes_while_pending_route_exists_and_resumes_after_despawn() {
    let mut app = build_app();

    // Pre-condition: clock at 0.
    assert_eq!(app.world().resource::<GameClock>().elapsed, 0);

    // Spawn an entity with PendingRoute — gate should hold time at 0.
    let ship = spawn_pending_route(app.world_mut());

    // Advance several frames worth of game time. The clock must stay at
    // 0 because PendingRoute exists. We don't go through
    // `poll_pending_routes` here — that's the whole point: even if the
    // polling system never runs (or never gets to remove the component),
    // time must resume the moment the entity carrying PendingRoute
    // leaves the world.
    for _ in 0..5 {
        app.update();
    }
    assert_eq!(
        app.world().resource::<GameClock>().elapsed,
        0,
        "GameClock must not advance while a PendingRoute component exists \
         (existence-based gate, #128)"
    );

    // Despawn the ship — Bevy auto-removes its components, including
    // PendingRoute. Pre-fix this would have leaked the counter; with the
    // existence check, the query is empty and time resumes.
    app.world_mut().entity_mut(ship).despawn();

    // One frame post-despawn must advance the clock.
    app.update();

    let elapsed_after = app.world().resource::<GameClock>().elapsed;
    assert!(
        elapsed_after > 0,
        "GameClock must resume after the last PendingRoute-carrying entity \
         is despawned (got elapsed={elapsed_after}). Pre-fix this would \
         freeze forever because the leaked counter never reached zero.",
    );
}

#[test]
fn time_advances_normally_with_no_pending_routes() {
    // Sanity check: the new query-based gate doesn't accidentally freeze
    // time when no PendingRoute exists.
    let mut app = build_app();
    assert_eq!(app.world().resource::<GameClock>().elapsed, 0);

    for _ in 0..3 {
        app.update();
    }

    assert!(
        app.world().resource::<GameClock>().elapsed > 0,
        "Time must advance in the absence of any PendingRoute component"
    );
}
