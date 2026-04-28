//! #474: Round-trip tests for `ShipProjection` persistence.
//!
//! Asserts that:
//! 1. A populated `KnowledgeStore.projections` survives the
//!    `from_live` → postcard encode → decode → `into_live` cycle without
//!    field drift, with `Entity` references correctly remapped through
//!    [`EntityMap`].
//! 2. The shim itself (`SavedShipProjection`) round-trips field-for-field
//!    independent of the parent `SavedKnowledgeStore`.
//! 3. `clear_projection` / `iter_projections` / `update_projection`
//!    semantics on `KnowledgeStore`.
//!
//! Note: postcard's positional encoding does NOT support missing-trailing
//! fields via `#[serde(default)]` (the decoder hits `UnexpectedEnd`).
//! That's exactly why SAVE_VERSION bumps 17 → 18 — `load.rs`' strict
//! version check catches and rejects pre-#474 saves rather than letting
//! them silently mis-decode.

use bevy::prelude::*;

use macrocosmo::knowledge::{
    KnowledgeStore, ObservationSource, ShipProjection, ShipSnapshot, ShipSnapshotState,
};
use macrocosmo::persistence::EntityMap;
use macrocosmo::persistence::savebag::{SavedKnowledgeStore, SavedShipProjection};

/// Build an `EntityMap` that round-trips the supplied entities through
/// their `to_bits()` save ids, mimicking what the load pipeline does for
/// real saves.
fn identity_map(entities: &[Entity]) -> EntityMap {
    let mut map = EntityMap::new();
    for e in entities {
        map.insert(e.to_bits(), *e);
    }
    map
}

#[test]
fn ship_projection_round_trips_through_postcard() {
    let mut world = World::new();
    let ship_a = world.spawn_empty().id();
    let ship_b = world.spawn_empty().id();
    let ship_c = world.spawn_empty().id();
    let sys_x = world.spawn_empty().id();
    let sys_y = world.spawn_empty().id();
    let sys_z = world.spawn_empty().id();

    let mut store = KnowledgeStore::default();

    // Populate a ship_snapshot too so we exercise the parent shim's
    // multi-field round-trip rather than projections in isolation.
    store.update_ship(ShipSnapshot {
        entity: ship_a,
        name: "Voyager".into(),
        design_id: "scout".into(),
        last_known_state: ShipSnapshotState::InSystem,
        last_known_system: Some(sys_x),
        observed_at: 7,
        hp: 50.0,
        hp_max: 50.0,
        source: ObservationSource::Direct,
    });

    // 1) Full intended-leg projection: every Optional field populated.
    store.update_projection(ShipProjection {
        entity: ship_a,
        dispatched_at: 100,
        expected_arrival_at: Some(112),
        expected_return_at: Some(124),
        projected_state: ShipSnapshotState::InSystem,
        projected_system: Some(sys_x),
        intended_state: Some(ShipSnapshotState::Surveying),
        intended_system: Some(sys_y),
        intended_takes_effect_at: Some(106),
    });
    // 2) Steady-state projection: no in-flight command, intended_* all None.
    store.update_projection(ShipProjection {
        entity: ship_b,
        dispatched_at: 90,
        expected_arrival_at: None,
        expected_return_at: None,
        projected_state: ShipSnapshotState::Loitering {
            position: [1.5, -2.0, 3.25],
        },
        projected_system: None,
        intended_state: None,
        intended_system: None,
        intended_takes_effect_at: None,
    });
    // 3) Mixed: in-flight command but no return leg; verifies independent
    //    Optionals encode separately.
    store.update_projection(ShipProjection {
        entity: ship_c,
        dispatched_at: 200,
        expected_arrival_at: Some(215),
        expected_return_at: None,
        projected_state: ShipSnapshotState::InTransit,
        projected_system: Some(sys_z),
        intended_state: Some(ShipSnapshotState::Settling),
        intended_system: Some(sys_z),
        intended_takes_effect_at: Some(208),
    });

    let saved = SavedKnowledgeStore::from_live(&store);
    assert_eq!(saved.projections.len(), 3, "all 3 projections encoded");

    let bytes = postcard::to_stdvec(&saved).expect("encode SavedKnowledgeStore");
    let decoded: SavedKnowledgeStore =
        postcard::from_bytes(&bytes).expect("decode SavedKnowledgeStore");

    let map = identity_map(&[ship_a, ship_b, ship_c, sys_x, sys_y, sys_z]);
    let restored = decoded.into_live(&map);

    // Ship snapshot survives.
    let snap = restored.get_ship(ship_a).expect("ship snapshot round-trip");
    assert_eq!(snap.observed_at, 7);
    assert_eq!(snap.last_known_state, ShipSnapshotState::InSystem);

    // Projection 1: full payload.
    let p_a = restored
        .get_projection(ship_a)
        .expect("ship_a projection round-trip");
    assert_eq!(p_a.entity, ship_a);
    assert_eq!(p_a.dispatched_at, 100);
    assert_eq!(p_a.expected_arrival_at, Some(112));
    assert_eq!(p_a.expected_return_at, Some(124));
    assert_eq!(p_a.projected_state, ShipSnapshotState::InSystem);
    assert_eq!(p_a.projected_system, Some(sys_x));
    assert_eq!(p_a.intended_state, Some(ShipSnapshotState::Surveying));
    assert_eq!(p_a.intended_system, Some(sys_y));
    assert_eq!(p_a.intended_takes_effect_at, Some(106));

    // Projection 2: steady-state with Loitering payload preserved exactly.
    let p_b = restored
        .get_projection(ship_b)
        .expect("ship_b projection round-trip");
    assert_eq!(p_b.dispatched_at, 90);
    assert_eq!(p_b.expected_arrival_at, None);
    assert_eq!(p_b.expected_return_at, None);
    match &p_b.projected_state {
        ShipSnapshotState::Loitering { position } => {
            assert_eq!(*position, [1.5, -2.0, 3.25]);
        }
        other => panic!("expected Loitering, got {other:?}"),
    }
    assert_eq!(p_b.projected_system, None);
    assert!(p_b.intended_state.is_none());
    assert_eq!(p_b.intended_system, None);
    assert_eq!(p_b.intended_takes_effect_at, None);

    // Projection 3: mixed Optionals.
    let p_c = restored
        .get_projection(ship_c)
        .expect("ship_c projection round-trip");
    assert_eq!(p_c.expected_arrival_at, Some(215));
    assert_eq!(p_c.expected_return_at, None);
    assert_eq!(p_c.projected_state, ShipSnapshotState::InTransit);
    assert_eq!(p_c.intended_state, Some(ShipSnapshotState::Settling));
    assert_eq!(p_c.intended_takes_effect_at, Some(208));

    assert_eq!(restored.iter_projections().count(), 3);
}

/// `clear_projection` removes the entry and returns it; iter no longer
/// surfaces the cleared id.
#[test]
fn clear_projection_removes_entry() {
    let mut world = World::new();
    let ship = world.spawn_empty().id();

    let mut store = KnowledgeStore::default();
    store.update_projection(ShipProjection {
        entity: ship,
        dispatched_at: 1,
        expected_arrival_at: None,
        expected_return_at: None,
        projected_state: ShipSnapshotState::InSystem,
        projected_system: None,
        intended_state: None,
        intended_system: None,
        intended_takes_effect_at: None,
    });
    assert!(store.get_projection(ship).is_some());

    let removed = store.clear_projection(ship).expect("entry removed");
    assert_eq!(removed.dispatched_at, 1);
    assert!(store.get_projection(ship).is_none());
    assert_eq!(store.iter_projections().count(), 0);
    assert!(
        store.clear_projection(ship).is_none(),
        "second clear is None"
    );
}

/// `SavedKnowledgeStore::default()` produces an empty projections Vec —
/// the entry point used by `Default`-shaped shims when the parent
/// `SavedComponentBag` carries `knowledge_store: None`.
#[test]
fn default_saved_knowledge_store_has_no_projections() {
    let saved = SavedKnowledgeStore::default();
    assert!(saved.projections.is_empty());
    let store = saved.into_live(&EntityMap::new());
    assert_eq!(store.iter_projections().count(), 0);
}

/// The shim itself round-trips field-for-field without touching the
/// parent. Catches stray field drift in `SavedShipProjection` independent
/// of `SavedKnowledgeStore`.
#[test]
fn saved_ship_projection_field_round_trip() {
    let mut world = World::new();
    let ship = world.spawn_empty().id();
    let sys_a = world.spawn_empty().id();
    let sys_b = world.spawn_empty().id();

    let original = ShipProjection {
        entity: ship,
        dispatched_at: 42,
        expected_arrival_at: Some(50),
        expected_return_at: Some(60),
        projected_state: ShipSnapshotState::Refitting,
        projected_system: Some(sys_a),
        intended_state: Some(ShipSnapshotState::InSystem),
        intended_system: Some(sys_b),
        intended_takes_effect_at: Some(45),
    };

    let saved = SavedShipProjection::from_live(&original);
    let bytes = postcard::to_stdvec(&saved).expect("encode SavedShipProjection");
    let decoded: SavedShipProjection =
        postcard::from_bytes(&bytes).expect("decode SavedShipProjection");

    let map = identity_map(&[ship, sys_a, sys_b]);
    let restored = decoded.into_live(&map);

    assert_eq!(restored.entity, ship);
    assert_eq!(restored.dispatched_at, 42);
    assert_eq!(restored.expected_arrival_at, Some(50));
    assert_eq!(restored.expected_return_at, Some(60));
    assert_eq!(restored.projected_state, ShipSnapshotState::Refitting);
    assert_eq!(restored.projected_system, Some(sys_a));
    assert_eq!(restored.intended_state, Some(ShipSnapshotState::InSystem));
    assert_eq!(restored.intended_system, Some(sys_b));
    assert_eq!(restored.intended_takes_effect_at, Some(45));
}
