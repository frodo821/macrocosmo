//! #478: Galaxy Map intended-trajectory overlay (dashed/translucent
//! layer) atop the projected layer #477 landed.
//!
//! These tests pin the **input data** the renderer consumes — the actual
//! gizmo emission is excluded from `test_app()` (visualization systems
//! need an EguiContext) so we exercise the pure helpers
//! [`compute_intended_render_inputs`] and [`intended_layer_alpha`].
//!
//! The visual contract: the dashed line + ring are drawn *only* when
//! the intended target diverges from the projected position (= command
//! in flight or ship en route). Once the reconciler converges them, the
//! overlay disappears.

mod common;

use std::collections::HashMap;

use bevy::prelude::*;

use macrocosmo::knowledge::{KnowledgeStore, ShipProjection, ShipSnapshotState};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::visualization::ships::{
    OwnShipMetadata, compute_intended_render_inputs, intended_layer_alpha,
};

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
                id: "intended_render_test".into(),
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

fn projection_with_intent(
    ship: Entity,
    projected_system: Option<Entity>,
    projected_state: ShipSnapshotState,
    intended_system: Option<Entity>,
    intended_state: Option<ShipSnapshotState>,
    dispatched_at: i64,
    takes_effect_at: Option<i64>,
) -> ShipProjection {
    ShipProjection {
        entity: ship,
        dispatched_at,
        expected_arrival_at: takes_effect_at.map(|t| t + 5),
        expected_return_at: None,
        projected_state,
        projected_system,
        intended_state,
        intended_system,
        intended_takes_effect_at: takes_effect_at,
    }
}

// ---------------------------------------------------------------------------
// 1. intended_layer_renders_when_diverged
// ---------------------------------------------------------------------------

/// Projection at A with intent toward B → intended item is emitted.
#[test]
fn intended_layer_renders_when_diverged() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);

    let a = spawn_test_system(app.world_mut(), "A", [0.0, 0.0, 0.0], 1.0, true, true);
    let b = spawn_test_system(app.world_mut(), "B", [10.0, 0.0, 0.0], 1.0, true, true);

    let ship = app.world_mut().spawn_empty().id();
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(projection_with_intent(
            ship,
            Some(a),
            ShipSnapshotState::InSystem,
            Some(b),
            Some(ShipSnapshotState::Surveying),
            0,
            Some(5),
        ));
    }

    let mut metadata = HashMap::new();
    metadata.insert(ship, meta("explorer_mk1", true));

    let store = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire has KnowledgeStore");

    // At t=0 (dispatch tick): full divergence — should appear with high alpha.
    let items = compute_intended_render_inputs(store, &metadata, 0);
    assert_eq!(items.len(), 1, "diverged projection must produce one item");
    let item = &items[0];
    assert_eq!(item.entity, ship);
    assert_eq!(item.projected_system, Some(a));
    assert_eq!(item.intended_system, Some(b));
    assert!(
        item.alpha > 0.4,
        "alpha at dispatch tick must be elevated above the floor (got {})",
        item.alpha
    );
}

// ---------------------------------------------------------------------------
// 2. intended_layer_hidden_when_converged
// ---------------------------------------------------------------------------

/// Once projected_system == intended_system (= reconciler observed
/// arrival), the dashed overlay must disappear.
#[test]
fn intended_layer_hidden_when_converged() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);

    let target = spawn_test_system(app.world_mut(), "T", [0.0, 0.0, 0.0], 1.0, true, true);

    let ship = app.world_mut().spawn_empty().id();
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(projection_with_intent(
            ship,
            Some(target),
            ShipSnapshotState::Surveying,
            Some(target),
            Some(ShipSnapshotState::Surveying),
            0,
            Some(5),
        ));
    }

    let mut metadata = HashMap::new();
    metadata.insert(ship, meta("explorer_mk1", true));

    let store = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire has KnowledgeStore");

    let items = compute_intended_render_inputs(store, &metadata, 10);
    assert!(
        items.is_empty(),
        "converged projection must not produce any intended overlay"
    );
}

// ---------------------------------------------------------------------------
// 3. intended_layer_hidden_when_no_intent
// ---------------------------------------------------------------------------

/// Steady-state projection (no in-flight command) → no intended overlay.
#[test]
fn intended_layer_hidden_when_no_intent() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);

    let home = spawn_test_system(app.world_mut(), "H", [0.0, 0.0, 0.0], 1.0, true, true);

    let ship = app.world_mut().spawn_empty().id();
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(projection_with_intent(
            ship,
            Some(home),
            ShipSnapshotState::InSystem,
            None, // no intent
            None,
            0,
            None,
        ));
    }

    let mut metadata = HashMap::new();
    metadata.insert(ship, meta("explorer_mk1", true));

    let store = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire has KnowledgeStore");

    let items = compute_intended_render_inputs(store, &metadata, 0);
    assert!(
        items.is_empty(),
        "projection with intended_system=None must not produce overlay"
    );
}

// ---------------------------------------------------------------------------
// 4. intended_alpha_fades_with_clock
// ---------------------------------------------------------------------------

/// As the clock advances from `dispatched_at` toward
/// `intended_takes_effect_at`, the alpha decays monotonically.
#[test]
fn intended_alpha_fades_with_clock() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);

    let a = spawn_test_system(app.world_mut(), "A", [0.0, 0.0, 0.0], 1.0, true, true);
    let b = spawn_test_system(app.world_mut(), "B", [10.0, 0.0, 0.0], 1.0, true, true);

    let ship = app.world_mut().spawn_empty().id();
    let proj = projection_with_intent(
        ship,
        Some(a),
        ShipSnapshotState::InSystem,
        Some(b),
        Some(ShipSnapshotState::Surveying),
        0,
        Some(10),
    );
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(proj.clone());
    }

    let mut metadata = HashMap::new();
    metadata.insert(ship, meta("explorer_mk1", true));

    let store = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire has KnowledgeStore");

    let alpha_t0 = compute_intended_render_inputs(store, &metadata, 0)[0].alpha;
    let alpha_t5 = compute_intended_render_inputs(store, &metadata, 5)[0].alpha;
    let alpha_t10 = compute_intended_render_inputs(store, &metadata, 10)[0].alpha;
    let alpha_t20 = compute_intended_render_inputs(store, &metadata, 20)[0].alpha;

    assert!(
        alpha_t0 > alpha_t5,
        "alpha must decrease as clock approaches takes_effect_at \
         (t0={}, t5={})",
        alpha_t0,
        alpha_t5
    );
    assert!(
        alpha_t5 > alpha_t10,
        "alpha must continue decreasing pre-arrival \
         (t5={}, t10={})",
        alpha_t5,
        alpha_t10
    );
    assert!(
        (alpha_t10 - 0.4).abs() < 1e-4,
        "alpha at takes_effect_at must reach the floor 0.4 (got {})",
        alpha_t10
    );
    assert!(
        (alpha_t20 - 0.4).abs() < 1e-4,
        "alpha after takes_effect_at must hold at the floor 0.4 (got {})",
        alpha_t20
    );

    // Pure helper agreement.
    assert!((intended_layer_alpha(&proj, 0) - alpha_t0).abs() < 1e-6);
    assert!((intended_layer_alpha(&proj, 5) - alpha_t5).abs() < 1e-6);
}

// ---------------------------------------------------------------------------
// 5. destroyed_projection_no_intended_layer
// ---------------------------------------------------------------------------

/// A projection whose projected_state is Destroyed must not surface an
/// intended overlay even if `intended_*` happens to still be populated.
/// (The reconciler clears intent on Destroyed/Missing, but the renderer
/// is defense-in-depth here.)
#[test]
fn destroyed_projection_no_intended_layer() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);

    let a = spawn_test_system(app.world_mut(), "A", [0.0, 0.0, 0.0], 1.0, true, true);
    let b = spawn_test_system(app.world_mut(), "B", [10.0, 0.0, 0.0], 1.0, true, true);

    let ship = app.world_mut().spawn_empty().id();
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(projection_with_intent(
            ship,
            Some(a),
            ShipSnapshotState::Destroyed,
            Some(b),
            Some(ShipSnapshotState::Surveying),
            0,
            Some(5),
        ));
    }

    let mut metadata = HashMap::new();
    metadata.insert(ship, meta("explorer_mk1", true));

    let store = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire has KnowledgeStore");

    let items = compute_intended_render_inputs(store, &metadata, 0);
    assert!(
        items.is_empty(),
        "Destroyed projection must NOT produce an intended overlay item"
    );
}

// ---------------------------------------------------------------------------
// 6. foreign_or_despawned_filtered (bonus)
// ---------------------------------------------------------------------------

/// Foreign-empire and despawned ships must be filtered, mirroring the
/// projected-layer filter (#477).
#[test]
fn foreign_or_despawned_filtered() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);

    let a = spawn_test_system(app.world_mut(), "A", [0.0, 0.0, 0.0], 1.0, true, true);
    let b = spawn_test_system(app.world_mut(), "B", [10.0, 0.0, 0.0], 1.0, true, true);

    let foreign_ship = app.world_mut().spawn_empty().id();
    let despawned_ship = app.world_mut().spawn_empty().id();
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(projection_with_intent(
            foreign_ship,
            Some(a),
            ShipSnapshotState::InSystem,
            Some(b),
            Some(ShipSnapshotState::Surveying),
            0,
            Some(5),
        ));
        store.update_projection(projection_with_intent(
            despawned_ship,
            Some(a),
            ShipSnapshotState::InSystem,
            Some(b),
            Some(ShipSnapshotState::Surveying),
            0,
            Some(5),
        ));
    }

    let mut metadata = HashMap::new();
    metadata.insert(foreign_ship, meta("explorer_mk1", false));
    // despawned_ship has no metadata entry.

    let store = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("empire has KnowledgeStore");

    let items = compute_intended_render_inputs(store, &metadata, 0);
    assert!(
        items.is_empty(),
        "foreign + despawned ships must be filtered from the intended overlay"
    );
}
