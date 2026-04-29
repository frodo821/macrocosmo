//! #491 (PR-3): Context menu must not leak realtime ECS `ShipState` for
//! own-empire ships. The `docked_system` / `current_destination_system` /
//! `loitering_pos` fields and the `remaining_travel` derivation in
//! `compute_context_menu_ship_data` flow through the viewing empire's
//! `KnowledgeStore::projections` (own ship) / `ship_snapshots` (foreign
//! ship), mirroring the outline-tree fix #487 / #491 PR #2 and the
//! Galaxy Map fix #477.
//!
//! These tests pin the **data extraction layer**
//! ([`compute_context_menu_ship_data`]) — the egui-driven
//! `draw_context_menu` itself is not invoked since egui systems are
//! excluded from `test_app()`. Bare `World::new()` is sufficient because
//! the helper is pure-data over `KnowledgeStore` + `Ship` + `ShipState`,
//! with no Bevy plugin / system / resource dependency.

use bevy::prelude::*;

use macrocosmo::knowledge::{
    KnowledgeStore, ObservationSource, ShipProjection, ShipSnapshot, ShipSnapshotState,
};
use macrocosmo::ship::{Owner, Ship, ShipState};
use macrocosmo::time_system::GameClock;
use macrocosmo::ui::context_menu::compute_context_menu_ship_data;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_ship(name: &str, owner: Entity, home: Entity) -> Ship {
    Ship {
        name: name.into(),
        design_id: "explorer_mk1".into(),
        hull_id: "frigate".into(),
        modules: Vec::new(),
        owner: Owner::Empire(owner),
        sublight_speed: 1.0,
        ftl_range: 5.0,
        ruler_aboard: false,
        home_port: home,
        design_revision: 0,
        fleet: None,
    }
}

// ---------------------------------------------------------------------------
// 1. docked_system uses projection (FTL leak regression)
// ---------------------------------------------------------------------------

/// Own-ship dispatched to a remote system. Realtime ECS already advanced
/// to `SubLight`, but the projection has not yet caught up — `projected_state`
/// is still `InSystem` at home. The context menu's `docked_system` field
/// must reflect the *projection*, not the realtime state.
#[test]
fn context_menu_docked_system_uses_projection() {
    let mut world = World::new();
    let empire = world.spawn_empty().id();
    let home = world.spawn_empty().id();
    let frontier = world.spawn_empty().id();
    let ship_entity = world.spawn_empty().id();
    let ship = make_ship("Explorer-1", empire, home);

    // Realtime: SubLight (= dispatcher's command has propagated to the
    // ship at the engine level, but the dispatcher hasn't yet observed
    // it — that's the FTL leak window).
    let realtime = ShipState::SubLight {
        origin: [0.0, 0.0, 0.0],
        destination: [50.0, 0.0, 0.0],
        target_system: Some(frontier),
        departed_at: 0,
        arrival_at: 5,
    };

    let mut store = KnowledgeStore::default();
    store.update_projection(ShipProjection {
        entity: ship_entity,
        dispatched_at: 0,
        expected_arrival_at: Some(10),
        expected_return_at: None,
        projected_state: ShipSnapshotState::InSystem,
        projected_system: Some(home),
        intended_state: Some(ShipSnapshotState::InTransitSubLight),
        intended_system: Some(frontier),
        intended_takes_effect_at: Some(5),
    });

    let clock = GameClock::new(1);
    let data = compute_context_menu_ship_data(
        ship_entity,
        &ship,
        &realtime,
        &clock,
        Some(&store),
        Some(empire),
    )
    .expect("projection-driven path produces data");

    assert_eq!(
        data.docked_system,
        Some(home),
        "FTL leak regression: docked_system must reflect the projection \
         (= still home), not realtime SubLight"
    );
    assert!(
        data.is_docked,
        "is_docked must mirror docked_system.is_some"
    );
    assert_eq!(
        data.current_destination_system, None,
        "InSystem projection has no destination",
    );

    // The previously-attached helper used in the production code derives
    // `origin_system` as `docked_system.or(current_destination_system)`,
    // so let's pin that the reconstructed origin is `home`.
    let origin_system = data.docked_system.or(data.current_destination_system);
    assert_eq!(origin_system, Some(home));
}

// ---------------------------------------------------------------------------
// 2. remaining_travel uses ShipProjection.expected_arrival_at
// ---------------------------------------------------------------------------

/// Once the projection is in transit (`InTransitFTL`) with a known
/// `expected_arrival_at`, the context menu's `remaining_travel` is
/// `expected_arrival_at - clock.elapsed`, NOT the realtime
/// `arrival_at - clock.elapsed`. Pins the dispatcher-timeline contract
/// from #491.
#[test]
fn context_menu_remaining_travel_uses_projection_eta() {
    let mut world = World::new();
    let empire = world.spawn_empty().id();
    let home = world.spawn_empty().id();
    let frontier = world.spawn_empty().id();
    let ship_entity = world.spawn_empty().id();
    let ship = make_ship("Explorer-1", empire, home);

    // Realtime: ship has already arrived (= ground truth). Projection
    // still says InTransitFTL — the dispatcher hasn't observed it yet.
    let realtime = ShipState::InSystem { system: frontier };

    let mut store = KnowledgeStore::default();
    let now: i64 = 5;
    let projected_arrival = now + 100;
    store.update_projection(ShipProjection {
        entity: ship_entity,
        dispatched_at: 0,
        expected_arrival_at: Some(projected_arrival),
        expected_return_at: None,
        projected_state: ShipSnapshotState::InTransitFTL,
        projected_system: Some(frontier),
        intended_state: Some(ShipSnapshotState::InTransitFTL),
        intended_system: Some(frontier),
        intended_takes_effect_at: Some(2),
    });

    let clock = GameClock::new(now);
    let data = compute_context_menu_ship_data(
        ship_entity,
        &ship,
        &realtime,
        &clock,
        Some(&store),
        Some(empire),
    )
    .expect("projection-driven path produces data");

    assert!(
        !data.is_docked,
        "InTransitFTL projection must not mark ship as docked"
    );
    assert_eq!(
        data.remaining_travel, 100,
        "remaining_travel must be projection.expected_arrival_at - clock \
         (= 100), NOT a realtime-derived value"
    );
    assert_eq!(
        data.current_destination_system,
        Some(frontier),
        "InTransitFTL projection's view.system is the destination",
    );
}

// ---------------------------------------------------------------------------
// 3. loitering_pos extracted via ShipView::position()
// ---------------------------------------------------------------------------

/// An own-ship loitering at a deep-space coordinate exposes that
/// coordinate via `ShipView::position()`. The context menu's
/// `loitering_pos` field must mirror the projection's loitering
/// coordinate, not pull from realtime ECS.
#[test]
fn context_menu_loitering_pos_extracted_via_view_accessor() {
    let mut world = World::new();
    let empire = world.spawn_empty().id();
    let home = world.spawn_empty().id();
    let ship_entity = world.spawn_empty().id();
    let ship = make_ship("Explorer-1", empire, home);

    // Realtime: anything — projection should win.
    let realtime = ShipState::InSystem { system: home };

    let loiter_pos = [1.0, 2.0, 3.0];
    let mut store = KnowledgeStore::default();
    store.update_projection(ShipProjection {
        entity: ship_entity,
        dispatched_at: 0,
        expected_arrival_at: None,
        expected_return_at: None,
        projected_state: ShipSnapshotState::Loitering {
            position: loiter_pos,
        },
        projected_system: None,
        intended_state: None,
        intended_system: None,
        intended_takes_effect_at: None,
    });

    let clock = GameClock::new(0);
    let data = compute_context_menu_ship_data(
        ship_entity,
        &ship,
        &realtime,
        &clock,
        Some(&store),
        Some(empire),
    )
    .expect("projection-driven path produces data");

    assert_eq!(
        data.loitering_pos,
        Some(loiter_pos),
        "Loitering projection must surface its coordinate via \
         ShipView::position() accessor"
    );
    assert_eq!(data.docked_system, None);
    assert_eq!(data.current_destination_system, None);
    assert_eq!(
        data.remaining_travel, 0,
        "Loitering with expected_arrival_at = None => remaining_travel 0",
    );
    assert!(!data.is_docked);
}

// ---------------------------------------------------------------------------
// 4. No projection — own ship returns None (skip menu)
// ---------------------------------------------------------------------------

/// An own-ship with no projection (= edge case: spawn before #481's
/// seed-projection lands, or test setup that didn't dispatch a command)
/// produces `None` so the caller knows to close the menu rather than
/// fall through with stale realtime data.
#[test]
fn context_menu_own_ship_no_projection_returns_none() {
    let mut world = World::new();
    let empire = world.spawn_empty().id();
    let home = world.spawn_empty().id();
    let frontier = world.spawn_empty().id();
    let ship_entity = world.spawn_empty().id();
    let ship = make_ship("Explorer-1", empire, home);

    let realtime = ShipState::InFTL {
        origin_system: home,
        destination_system: frontier,
        departed_at: 0,
        arrival_at: 5,
    };

    let store = KnowledgeStore::default(); // no projection seeded
    let clock = GameClock::new(2);
    let data = compute_context_menu_ship_data(
        ship_entity,
        &ship,
        &realtime,
        &clock,
        Some(&store),
        Some(empire),
    );
    assert!(
        data.is_none(),
        "own-ship with no projection must return None — caller closes menu \
         instead of using realtime FTL state"
    );
}

// ---------------------------------------------------------------------------
// 5. No KnowledgeStore — realtime fallback
// ---------------------------------------------------------------------------

/// When `viewing_knowledge` is `None` (= early Startup before empires
/// are wired, or future omniscient observer mode), the helper falls
/// back to realtime ECS state via `ship_view`'s no-store branch. This
/// pins the existing fallback behaviour so the egui pipeline doesn't
/// crash before the projection table is available.
#[test]
fn context_menu_no_knowledge_store_realtime_fallback() {
    let mut world = World::new();
    let empire = world.spawn_empty().id();
    let home = world.spawn_empty().id();
    let frontier = world.spawn_empty().id();
    let ship_entity = world.spawn_empty().id();
    let ship = make_ship("Explorer-1", empire, home);

    // Realtime is in-transit sublight to frontier.
    let realtime = ShipState::SubLight {
        origin: [0.0, 0.0, 0.0],
        destination: [50.0, 0.0, 0.0],
        target_system: Some(frontier),
        departed_at: 0,
        arrival_at: 5,
    };

    let clock = GameClock::new(1);
    // No KnowledgeStore => realtime fallback.
    let data = compute_context_menu_ship_data(ship_entity, &ship, &realtime, &clock, None, None)
        .expect("no-store fallback must produce data");

    assert!(!data.is_docked, "SubLight realtime must not mark as docked");
    assert_eq!(data.docked_system, None);
    assert_eq!(
        data.current_destination_system,
        Some(frontier),
        "no-store fallback derives destination from realtime SubLight"
    );
    assert_eq!(data.loitering_pos, None);
    // No KnowledgeStore => no projection => remaining_travel 0.
    assert_eq!(data.remaining_travel, 0);
}

/// `Loitering` realtime state in the no-store fallback path surfaces the
/// position via `ShipView::position()` even when there's no projection
/// to consult.
#[test]
fn context_menu_no_knowledge_store_loitering_realtime_fallback() {
    let mut world = World::new();
    let empire = world.spawn_empty().id();
    let home = world.spawn_empty().id();
    let ship_entity = world.spawn_empty().id();
    let ship = make_ship("Explorer-1", empire, home);

    let pos = [7.0, -3.0, 4.5];
    let realtime = ShipState::Loitering { position: pos };

    let clock = GameClock::new(0);
    let data = compute_context_menu_ship_data(ship_entity, &ship, &realtime, &clock, None, None)
        .expect("no-store fallback must produce data");
    assert_eq!(data.loitering_pos, Some(pos));
    assert_eq!(data.docked_system, None);
    assert_eq!(data.current_destination_system, None);
}

// ---------------------------------------------------------------------------
// 6. Foreign ship — snapshot path
// ---------------------------------------------------------------------------

/// Foreign ships flow through `ship_snapshots` — the existing #175
/// light-delayed visibility — and the context menu's view of them is
/// snapshot-driven. While the production caller suppresses the menu for
/// non-owned ships (#432), the data extraction must still respect the
/// snapshot when invoked, so the contract is uniform across panels.
#[test]
fn context_menu_foreign_ship_uses_snapshot() {
    let mut world = World::new();
    let viewing_empire = world.spawn_empty().id();
    let foreign_empire = world.spawn_empty().id();
    let last_known_sys = world.spawn_empty().id();
    let realtime_dest = world.spawn_empty().id();
    let ship_entity = world.spawn_empty().id();
    let ship = make_ship("EnemyShip", foreign_empire, last_known_sys);

    // Realtime: post-snapshot ground truth (FTL). The viewing empire
    // must NOT see this — the snapshot path is mandatory.
    let realtime = ShipState::InFTL {
        origin_system: last_known_sys,
        destination_system: realtime_dest,
        departed_at: 1,
        arrival_at: 5,
    };

    let mut store = KnowledgeStore::default();
    store.update_ship(ShipSnapshot {
        entity: ship_entity,
        name: "EnemyShip".into(),
        design_id: "explorer_mk1".into(),
        last_known_state: ShipSnapshotState::InTransitFTL,
        last_known_system: Some(last_known_sys),
        observed_at: 0,
        hp: 100.0,
        hp_max: 100.0,
        source: ObservationSource::Direct,
    });

    let clock = GameClock::new(2);
    let data = compute_context_menu_ship_data(
        ship_entity,
        &ship,
        &realtime,
        &clock,
        Some(&store),
        Some(viewing_empire),
    )
    .expect("foreign snapshot path must produce data");

    assert!(
        !data.is_docked,
        "InTransitFTL snapshot must not mark ship as docked"
    );
    assert_eq!(data.docked_system, None);
    assert_eq!(
        data.current_destination_system,
        Some(last_known_sys),
        "foreign-ship view.system from snapshot is last_known_system"
    );
    // Foreign ship has no projection on the viewing empire's store, so
    // remaining_travel is 0 — projection-mediated only.
    assert_eq!(data.remaining_travel, 0);
}
