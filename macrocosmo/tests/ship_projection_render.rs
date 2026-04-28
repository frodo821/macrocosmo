//! #477: Galaxy Map own-ship rendering reads from
//! [`KnowledgeStore::projections`], not realtime ECS [`ShipState`].
//!
//! These tests pin the **input data** the renderer consumes — the actual
//! gizmo emission is excluded from `test_app()` (visualization systems
//! need an EguiContext) so we exercise the pure helper
//! [`compute_own_ship_render_inputs`].
//!
//! The FTL-leak regression is the central invariant: at the dispatcher's
//! tick `T+1` after dispatching a survey at `T` (light delay = 5), the
//! viewing empire's projection must still place the ship at the
//! pre-dispatch system, regardless of what realtime [`ShipState`] says.

mod common;

use std::collections::HashMap;

use bevy::prelude::*;

use macrocosmo::knowledge::{
    KnowledgeStore, ObservationSource, ShipProjection, ShipSnapshot, ShipSnapshotState,
};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::visualization::ships::{OwnShipMetadata, compute_own_ship_render_inputs};

use common::{spawn_test_system, test_app};

// ---------------------------------------------------------------------------
// Shared scenario helpers
// ---------------------------------------------------------------------------

fn spawn_minimal_empire(app: &mut App) -> Entity {
    app.world_mut()
        .spawn((
            Empire {
                name: "Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "render_test".into(),
                name: "Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            KnowledgeStore::default(),
            macrocosmo::knowledge::SystemVisibilityMap::default(),
        ))
        .id()
}

fn meta(design_id: &str, owned_by_viewing_empire: bool) -> OwnShipMetadata {
    OwnShipMetadata {
        design_id: design_id.into(),
        is_station: false,
        is_harbour: false,
        owned_by_viewing_empire,
    }
}

// ---------------------------------------------------------------------------
// 1. render_uses_projection_not_realtime
// ---------------------------------------------------------------------------

/// A ship's realtime ECS state would place it at system B, but the viewing
/// empire's projection still says system A. The renderer's input feed must
/// reflect system A — the entire FTL-leak fix.
#[test]
fn render_uses_projection_not_realtime() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);

    let system_a = spawn_test_system(app.world_mut(), "A", [0.0, 0.0, 0.0], 1.0, true, true);
    let system_b = spawn_test_system(app.world_mut(), "B", [10.0, 0.0, 0.0], 1.0, true, true);

    // Spawn a ship entity placeholder. We don't need a full Ship Component for
    // this test — only the entity id matters since the helper takes metadata
    // by HashMap.
    let ship = app.world_mut().spawn_empty().id();

    // Projection: ship is at system A (light-coherent view).
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 0,
            expected_arrival_at: None,
            expected_return_at: None,
            projected_state: ShipSnapshotState::InSystem,
            projected_system: Some(system_a),
            intended_state: None,
            intended_system: None,
            intended_takes_effect_at: None,
        });
    }

    let mut metadata = HashMap::new();
    metadata.insert(ship, meta("explorer_mk1", true));

    // Note: realtime `ShipState` would be `InSystem { system: system_b }` if
    // the ship were a real Ship — but the helper IGNORES realtime state. The
    // absence of a Ship+ShipState entity here demonstrates that explicitly:
    // rendering does not consult them.
    let _ = system_b;

    let store = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire has KnowledgeStore");
    let items = compute_own_ship_render_inputs(store, &metadata);

    assert_eq!(items.len(), 1, "exactly one render item from projection");
    assert_eq!(items[0].entity, ship);
    assert_eq!(items[0].projected_system, Some(system_a));
    assert!(matches!(
        items[0].projected_state,
        ShipSnapshotState::InSystem
    ));
}

// ---------------------------------------------------------------------------
// 2. ftl_leak_regression_dispatch_window
// ---------------------------------------------------------------------------

/// At dispatcher's tick T+1 after dispatching a survey at T (light delay = 5),
/// the projection still says `InSystem` at the home system — the command has
/// not had time to "reach" the ship from the dispatcher's POV. The renderer
/// must show the ship at home, NOT at the survey target.
#[test]
fn ftl_leak_regression_dispatch_window() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);

    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [50.0, 0.0, 0.0],
        1.0,
        false,
        false,
    );

    let ship = app.world_mut().spawn_empty().id();

    // Dispatch tick is T=0. Pre-dispatch snapshot: ship at home.
    // Projection at T+1 must still report `InSystem { home }`. The
    // intended_state/intended_system encode the pending command (survey at
    // frontier) but the projected layer is what the renderer feeds gizmos.
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 0,
            expected_arrival_at: Some(10),
            expected_return_at: Some(20),
            // Projected layer: ship is still at home. This is the
            // light-coherent view at tick T+1.
            projected_state: ShipSnapshotState::InSystem,
            projected_system: Some(home),
            // Intended layer: ship will be surveying frontier. Sub-issue E
            // will surface this in a separate translucent layer; for #477
            // it's just metadata that the renderer ignores.
            intended_state: Some(ShipSnapshotState::Surveying),
            intended_system: Some(frontier),
            intended_takes_effect_at: Some(5),
        });
    }

    let mut metadata = HashMap::new();
    metadata.insert(ship, meta("explorer_mk1", true));

    let store = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire has KnowledgeStore");
    let items = compute_own_ship_render_inputs(store, &metadata);

    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(
        item.projected_system,
        Some(home),
        "renderer must place the ship at home before the command takes effect"
    );
    assert!(
        matches!(item.projected_state, ShipSnapshotState::InSystem),
        "renderer must see the projected (pre-dispatch) state, not the intended Surveying"
    );
    assert_ne!(
        item.projected_system,
        Some(frontier),
        "FTL leak regression: command target must NOT appear in render input \
         until the projection is reconciled to `Surveying`"
    );
}

// ---------------------------------------------------------------------------
// 3. projection_destroyed_renders_terminal
// ---------------------------------------------------------------------------

/// A projection in terminal `Destroyed` / `Missing` state is filtered out of
/// the own-ship render path — those ships are drawn by the snapshot ghost
/// branch (which handles both own and foreign empires) for visual parity
/// with foreign-ship rendering.
#[test]
fn projection_destroyed_filtered_from_own_render() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);
    let system = spawn_test_system(app.world_mut(), "Sys", [0.0, 0.0, 0.0], 1.0, true, true);

    let ship_destroyed = app.world_mut().spawn_empty().id();
    let ship_missing = app.world_mut().spawn_empty().id();
    let ship_alive = app.world_mut().spawn_empty().id();

    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship_destroyed,
            dispatched_at: 0,
            expected_arrival_at: None,
            expected_return_at: None,
            projected_state: ShipSnapshotState::Destroyed,
            projected_system: Some(system),
            intended_state: None,
            intended_system: None,
            intended_takes_effect_at: None,
        });
        store.update_projection(ShipProjection {
            entity: ship_missing,
            dispatched_at: 0,
            expected_arrival_at: None,
            expected_return_at: None,
            projected_state: ShipSnapshotState::Missing,
            projected_system: Some(system),
            intended_state: None,
            intended_system: None,
            intended_takes_effect_at: None,
        });
        store.update_projection(ShipProjection {
            entity: ship_alive,
            dispatched_at: 0,
            expected_arrival_at: None,
            expected_return_at: None,
            projected_state: ShipSnapshotState::InSystem,
            projected_system: Some(system),
            intended_state: None,
            intended_system: None,
            intended_takes_effect_at: None,
        });
    }

    let mut metadata = HashMap::new();
    metadata.insert(ship_destroyed, meta("explorer_mk1", true));
    metadata.insert(ship_missing, meta("explorer_mk1", true));
    metadata.insert(ship_alive, meta("explorer_mk1", true));

    let store = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire has KnowledgeStore");
    let items = compute_own_ship_render_inputs(store, &metadata);

    let entities: Vec<Entity> = items.iter().map(|i| i.entity).collect();
    assert!(
        !entities.contains(&ship_destroyed),
        "Destroyed projections must NOT appear in own-ship render input \
         (they render via the snapshot ghost path)"
    );
    assert!(
        !entities.contains(&ship_missing),
        "Missing projections must NOT appear in own-ship render input"
    );
    assert!(
        entities.contains(&ship_alive),
        "Alive projections must appear in own-ship render input"
    );
}

// ---------------------------------------------------------------------------
// 4. Despawned ship (no metadata) is skipped — caller hands snapshot ghost path
// ---------------------------------------------------------------------------

/// When the `Ship` Component has been despawned (e.g. after destruction
/// reconcile clears realtime state but the projection lingers), the renderer
/// must skip the projection entry — the snapshot ghost path renders it.
#[test]
fn despawned_ship_is_skipped() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);
    let system = spawn_test_system(app.world_mut(), "Sys", [0.0, 0.0, 0.0], 1.0, true, true);

    let ship = app.world_mut().spawn_empty().id();
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 0,
            expected_arrival_at: None,
            expected_return_at: None,
            projected_state: ShipSnapshotState::InSystem,
            projected_system: Some(system),
            intended_state: None,
            intended_system: None,
            intended_takes_effect_at: None,
        });
    }

    // Empty metadata = ship Component does not exist (despawned).
    let metadata = HashMap::new();

    let store = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire has KnowledgeStore");
    let items = compute_own_ship_render_inputs(store, &metadata);

    assert!(
        items.is_empty(),
        "despawned ships must be deferred to the snapshot ghost path"
    );
}

// ---------------------------------------------------------------------------
// 5. Foreign-empire ship is filtered (defense-in-depth)
// ---------------------------------------------------------------------------

/// A projection whose metadata says the ship is NOT owned by the viewing
/// empire is filtered out. In normal play the projection store only contains
/// the viewing empire's own ships, but the explicit owner check is
/// defense-in-depth.
#[test]
fn foreign_owned_ship_filtered() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);
    let system = spawn_test_system(app.world_mut(), "Sys", [0.0, 0.0, 0.0], 1.0, true, true);

    let foreign_ship = app.world_mut().spawn_empty().id();
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: foreign_ship,
            dispatched_at: 0,
            expected_arrival_at: None,
            expected_return_at: None,
            projected_state: ShipSnapshotState::InSystem,
            projected_system: Some(system),
            intended_state: None,
            intended_system: None,
            intended_takes_effect_at: None,
        });
    }

    let mut metadata = HashMap::new();
    metadata.insert(foreign_ship, meta("explorer_mk1", false));

    let store = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire has KnowledgeStore");
    let items = compute_own_ship_render_inputs(store, &metadata);

    assert!(
        items.is_empty(),
        "ships not owned by the viewing empire must not be rendered as own ships"
    );
}

// ---------------------------------------------------------------------------
// 6. Loitering projection passes through the inline coordinate
// ---------------------------------------------------------------------------

/// Loitering ships have no `projected_system` — the inline `position` in the
/// `ShipSnapshotState::Loitering` variant is the source of truth. Verify the
/// helper preserves it.
#[test]
fn loitering_projection_carries_inline_position() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);

    let ship = app.world_mut().spawn_empty().id();
    let loiter_pos = [42.0, -7.5, 0.0];
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
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
    }

    let mut metadata = HashMap::new();
    metadata.insert(ship, meta("explorer_mk1", true));

    let store = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire has KnowledgeStore");
    let items = compute_own_ship_render_inputs(store, &metadata);

    assert_eq!(items.len(), 1);
    if let ShipSnapshotState::Loitering { position } = items[0].projected_state {
        assert_eq!(position, loiter_pos);
    } else {
        panic!("expected Loitering, got {:?}", items[0].projected_state);
    }
}

// ---------------------------------------------------------------------------
// 7. Foreign-ship snapshot path is unaffected by projection rewire
// ---------------------------------------------------------------------------

/// Smoke test: foreign-ship `ShipSnapshot` entries (the existing #175
/// light-delayed visibility) are untouched by the projection rewire. This
/// asserts at the data layer — `iter_ships()` still returns snapshots
/// independent of `iter_projections()`.
#[test]
fn foreign_ship_snapshot_unaffected() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);
    let system = spawn_test_system(app.world_mut(), "Sys", [0.0, 0.0, 0.0], 1.0, true, true);

    let foreign_ship = app.world_mut().spawn_empty().id();
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_ship(ShipSnapshot {
            entity: foreign_ship,
            name: "Enemy".into(),
            design_id: "explorer_mk1".into(),
            last_known_state: ShipSnapshotState::InSystem,
            last_known_system: Some(system),
            observed_at: 0,
            hp: 100.0,
            hp_max: 100.0,
            source: ObservationSource::Direct,
        });
    }

    let store = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire has KnowledgeStore");

    assert_eq!(store.iter_ships().count(), 1, "ship snapshot retained");
    assert_eq!(
        store.iter_projections().count(),
        0,
        "no own-empire projection wired up — snapshot path is independent"
    );
}
