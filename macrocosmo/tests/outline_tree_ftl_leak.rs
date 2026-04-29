//! #487: Outline tree must not leak realtime ECS `ShipState` for
//! own-empire ships. The "In Transit" / "Stationed Elsewhere" / docked /
//! station sections all flow through the viewing empire's
//! `KnowledgeStore::projections`, mirroring the Galaxy Map fix #477
//! installed for the gizmo renderer.
//!
//! These tests pin the **data extraction layer** — `compute_in_transit_entries`
//! / `ship_outline_view` — exercised through `test_app()` (egui systems are
//! excluded so the egui-driven `draw_outline` itself is not invoked).

mod common;

use bevy::prelude::*;

use macrocosmo::knowledge::{
    KnowledgeStore, ObservationSource, ShipProjection, ShipSnapshot, ShipSnapshotState,
};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::fleet::{Fleet, FleetMembers};
use macrocosmo::ship::{
    Cargo, CommandQueue, Owner, RulesOfEngagement, Ship, ShipHitpoints, ShipModifiers, ShipState,
    SurveyData,
};
use macrocosmo::ui::outline::{
    InTransitEntry, ShipOutlineView, compute_in_transit_entries, ship_outline_view,
};

use common::{spawn_test_system, test_app};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn spawn_minimal_empire(app: &mut App) -> Entity {
    app.world_mut()
        .spawn((
            Empire {
                name: "Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "outline_test".into(),
                name: "Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            KnowledgeStore::default(),
            macrocosmo::knowledge::SystemVisibilityMap::default(),
        ))
        .id()
}

/// Spawn a minimal own-empire ship Component bundle. We hand-roll instead of
/// `common::spawn_test_ship` to avoid pulling in the design registry —
/// only the `Ship` + `ShipState` + neighbour Components matter for the
/// outline data extraction. Returns the ship entity.
fn spawn_test_ship(world: &mut World, name: &str, owner: Entity, system: Entity) -> Entity {
    let ship_entity = world.spawn_empty().id();
    let fleet_entity = world.spawn_empty().id();
    world.entity_mut(ship_entity).insert((
        Ship {
            name: name.into(),
            design_id: "explorer_mk1".into(),
            hull_id: "frigate".into(),
            modules: Vec::new(),
            owner: Owner::Empire(owner),
            sublight_speed: 1.0,
            ftl_range: 5.0,
            ruler_aboard: false,
            home_port: system,
            design_revision: 0,
            fleet: Some(fleet_entity),
        },
        ShipState::InSystem { system },
        ShipHitpoints {
            hull: 100.0,
            hull_max: 100.0,
            armor: 0.0,
            armor_max: 0.0,
            shield: 0.0,
            shield_max: 0.0,
            shield_regen: 0.0,
        },
        CommandQueue::default(),
        Cargo::default(),
        ShipModifiers::default(),
        macrocosmo::ship::ShipStats::default(),
        RulesOfEngagement::default(),
    ));
    world.entity_mut(fleet_entity).insert((
        Fleet {
            name: name.into(),
            flagship: Some(ship_entity),
        },
        FleetMembers(vec![ship_entity]),
    ));
    ship_entity
}

/// Force-mutate the ship's realtime `ShipState`. Simulates a tick where
/// the realtime ECS has advanced past the dispatcher's projection — the
/// FTL-leak window the test pins.
fn set_ship_state(world: &mut World, ship: Entity, new_state: ShipState) {
    *world.get_mut::<ShipState>(ship).unwrap() = new_state;
}

/// Build a fresh `KnowledgeStore` populated from the given empire's
/// store. This lets the test pass an owned store to the helper without
/// holding a borrow that conflicts with the mutable ship query.
///
/// Walks `iter_projections()` / `iter_ships()` and re-inserts the
/// entries into a fresh store.
fn snapshot_knowledge(app: &App, empire: Entity) -> KnowledgeStore {
    let src = app
        .world()
        .entity(empire)
        .get::<KnowledgeStore>()
        .expect("KnowledgeStore");
    let mut out = KnowledgeStore::default();
    for (_, projection) in src.iter_projections() {
        out.update_projection(projection.clone());
    }
    for (_, snapshot) in src.iter_ships() {
        out.update_ship(snapshot.clone());
    }
    out
}

/// Run a query against the world's ship table the same way `draw_outline`
/// does, then call `compute_in_transit_entries`. Returns the produced
/// entries.
///
/// #491 (D-M-9): the legacy `is_observer` parameter was removed —
/// observer mode is now expressed via `viewing_empire = Some(observed)`.
/// Callers that previously passed `is_observer = true` with
/// `viewing_empire = None` should pass the observed empire entity
/// instead.
fn run_in_transit(app: &mut App, knowledge_owner: Option<Entity>) -> Vec<InTransitEntry> {
    let knowledge_snap = knowledge_owner.map(|e| snapshot_knowledge(app, e));
    let mut state: bevy::ecs::system::SystemState<
        Query<(
            Entity,
            &mut Ship,
            &mut ShipState,
            Option<&mut Cargo>,
            &ShipHitpoints,
            Option<&SurveyData>,
        )>,
    > = bevy::ecs::system::SystemState::new(app.world_mut());
    let ships = state.get_mut(app.world_mut());
    compute_in_transit_entries(&ships, knowledge_snap.as_ref(), knowledge_owner)
}

/// Same shape as `run_in_transit` but for the per-ship `ship_outline_view`
/// helper. Returns the resolved view (or `None` if no entry exists).
fn run_outline_view(
    app: &mut App,
    ship: Entity,
    knowledge_owner: Option<Entity>,
) -> Option<ShipOutlineView> {
    let knowledge_snap = knowledge_owner.map(|e| snapshot_knowledge(app, e));
    let ship_ref = app.world().entity(ship);
    let ship_comp = ship_ref.get::<Ship>().expect("Ship Component");
    let state = ship_ref.get::<ShipState>().expect("ShipState Component");
    ship_outline_view(
        ship,
        ship_comp,
        state,
        knowledge_snap.as_ref(),
        knowledge_owner,
    )
}

// ---------------------------------------------------------------------------
// 1. FTL leak regression — outline must read projection, not realtime
// ---------------------------------------------------------------------------

/// At dispatcher's tick T+1 after dispatching a survey at T, the realtime
/// ECS state would have the ship in `InTransit`/`Surveying` — but the
/// projection still says `InSystem`. The outline tree must show the ship
/// in the docked sections (NOT "In Transit"), exactly the way the Galaxy
/// Map renders it (#477).
#[test]
fn outline_in_transit_section_uses_projection_not_realtime() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [50.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );

    let ship = spawn_test_ship(app.world_mut(), "Explorer-1", empire, home);

    // Projection: ship is at home (= dispatch tick has not yet "reached"
    // the ship from the dispatcher's POV).
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 0,
            expected_arrival_at: Some(10),
            expected_return_at: Some(20),
            projected_state: ShipSnapshotState::InSystem,
            projected_system: Some(home),
            intended_state: Some(ShipSnapshotState::Surveying),
            intended_system: Some(frontier),
            intended_takes_effect_at: Some(5),
        });
    }

    // Realtime ECS advances ahead of the projection (= the FTL leak).
    set_ship_state(
        app.world_mut(),
        ship,
        ShipState::InFTL {
            origin_system: home,
            destination_system: frontier,
            departed_at: 0,
            arrival_at: 5,
        },
    );

    let entries = run_in_transit(&mut app, Some(empire));
    assert!(
        entries.is_empty(),
        "FTL leak regression: own-empire ship must NOT appear in 'In Transit' \
         while its projection still says InSystem. Entries: {:?}",
        entries
    );

    // The same ship through `ship_outline_view` should report the
    // projected state (`InSystem`), not the realtime `InFTL`.
    let view = run_outline_view(&mut app, ship, Some(empire)).expect("view");
    assert_eq!(view.state, ShipSnapshotState::InSystem);
    assert_eq!(view.system, Some(home));
}

// ---------------------------------------------------------------------------
// 2. Post-arrival — projection reconciled to Surveying
// ---------------------------------------------------------------------------

/// Once the projection's reconciler has updated `projected_state` to
/// `Surveying` (= the dispatcher has light-coherently observed the
/// arrival), the outline tree shows "Surveying" — and the entry appears
/// in the In Transit section.
#[test]
fn outline_post_arrival_uses_projection() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [50.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );

    let ship = spawn_test_ship(app.world_mut(), "Explorer-1", empire, home);

    // Projection: ship is now Surveying at frontier (= reconciled).
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 0,
            expected_arrival_at: Some(5),
            expected_return_at: Some(15),
            projected_state: ShipSnapshotState::Surveying,
            projected_system: Some(frontier),
            intended_state: Some(ShipSnapshotState::Surveying),
            intended_system: Some(frontier),
            intended_takes_effect_at: Some(5),
        });
    }

    // Realtime: same Surveying state. (Confluent — projection is now
    // current with realtime.)
    set_ship_state(
        app.world_mut(),
        ship,
        ShipState::Surveying {
            target_system: frontier,
            started_at: 0,
            completes_at: 10,
        },
    );

    let entries = run_in_transit(&mut app, Some(empire));
    assert_eq!(entries.len(), 1, "ship must appear in 'In Transit'");
    assert_eq!(entries[0].entity, ship);
    assert_eq!(entries[0].status, "Surveying");

    let view = run_outline_view(&mut app, ship, Some(empire)).expect("view");
    assert_eq!(view.state, ShipSnapshotState::Surveying);
    assert_eq!(view.system, Some(frontier));
}

// ---------------------------------------------------------------------------
// 3. No projection — outline gracefully skips the ship
// ---------------------------------------------------------------------------

/// An own-empire ship without any projection (= edge case for a ship
/// spawned before #481's spawn-time seed lands, or in tests that don't
/// dispatch a command first) must not appear in the In Transit section.
/// `ship_outline_view` returns `None` so the caller skips the ship rather
/// than rendering stale realtime ECS state.
#[test]
fn outline_no_projection_handles_gracefully() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [50.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );

    let ship = spawn_test_ship(app.world_mut(), "Explorer-1", empire, home);

    // No projection populated for the ship.
    set_ship_state(
        app.world_mut(),
        ship,
        ShipState::InFTL {
            origin_system: home,
            destination_system: frontier,
            departed_at: 0,
            arrival_at: 5,
        },
    );

    let entries = run_in_transit(&mut app, Some(empire));
    assert!(
        entries.is_empty(),
        "without projection the ship must be skipped, not rendered with \
         realtime FTL state. Entries: {:?}",
        entries
    );

    let view = run_outline_view(&mut app, ship, Some(empire));
    assert!(
        view.is_none(),
        "ship_outline_view returns None for own-ship without projection"
    );
}

// ---------------------------------------------------------------------------
// 4. Foreign ship — snapshot path unchanged
// ---------------------------------------------------------------------------

/// Foreign ships flow through `ship_snapshots` — the existing #175
/// light-delayed visibility — and the outline tree's view of them is
/// unchanged by the #487 rewire. Pin that contract.
#[test]
fn outline_foreign_ship_uses_snapshot() {
    let mut app = test_app();
    let viewing_empire = spawn_minimal_empire(&mut app);

    // Spawn a separate "foreign" empire entity (does not need PlayerEmpire).
    let foreign_empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Foreign".into(),
            },
            Faction {
                id: "foreign".into(),
                name: "Foreign".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            KnowledgeStore::default(),
            macrocosmo::knowledge::SystemVisibilityMap::default(),
        ))
        .id();

    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let foreign_sys = spawn_test_system(
        app.world_mut(),
        "ForeignSys",
        [100.0, 0.0, 0.0],
        1.0,
        true,
        true,
    );

    let foreign_ship = spawn_test_ship(app.world_mut(), "EnemyScout", foreign_empire, foreign_sys);

    // Viewing empire's snapshot says the foreign ship was last seen
    // surveying somewhere.
    {
        let mut em = app.world_mut().entity_mut(viewing_empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_ship(ShipSnapshot {
            entity: foreign_ship,
            name: "EnemyScout".into(),
            design_id: "explorer_mk1".into(),
            last_known_state: ShipSnapshotState::Surveying,
            last_known_system: Some(foreign_sys),
            observed_at: 0,
            hp: 100.0,
            hp_max: 100.0,
            source: ObservationSource::Direct,
        });
    }

    // The ship's realtime state advances ahead of the snapshot — the
    // snapshot is what the viewing empire is allowed to see. The
    // outline view must reflect the snapshot.
    set_ship_state(
        app.world_mut(),
        foreign_ship,
        ShipState::InSystem { system: home },
    );

    let view = run_outline_view(&mut app, foreign_ship, Some(viewing_empire)).expect("view");
    assert_eq!(view.state, ShipSnapshotState::Surveying);
    assert_eq!(view.system, Some(foreign_sys));

    // `compute_in_transit_entries` filters out foreign-empire ships
    // (they have their own UI surface, not the In Transit section).
    let entries = run_in_transit(&mut app, Some(viewing_empire));
    assert!(
        entries.iter().all(|e| e.entity != foreign_ship),
        "foreign ship must not appear in In Transit. Entries: {:?}",
        entries
    );
}

// ---------------------------------------------------------------------------
// 5. Observer mode — ground truth realtime fallback
// ---------------------------------------------------------------------------

/// Observer mode (#440 empire-view observer) is **light-coherent** — the
/// player sees the world through the observed empire's KnowledgeStore,
/// same as PlayerEmpire normal play. Realtime ECS ground-truth peeks are
/// reserved for the future omniscient (god-view) mode (#490).
///
/// Verify by setting up a situation where the projection and realtime
/// disagree, then asserting observer mode reads the **projection** side.
#[test]
fn outline_observer_mode_is_light_coherent_via_projection() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [50.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );

    let ship = spawn_test_ship(app.world_mut(), "Explorer-1", empire, home);

    // Projection says InSystem; realtime says Surveying. The viewing
    // empire (here: `empire`, observed in empire-view observer) hasn't
    // seen the dispatch propagate yet — the renderer should respect
    // the projection.
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 0,
            expected_arrival_at: None,
            expected_return_at: None,
            projected_state: ShipSnapshotState::InSystem,
            projected_system: Some(home),
            intended_state: None,
            intended_system: None,
            intended_takes_effect_at: None,
        });
    }
    set_ship_state(
        app.world_mut(),
        ship,
        ShipState::Surveying {
            target_system: frontier,
            started_at: 0,
            completes_at: 10,
        },
    );

    // Observer mode = empire-view, light-coherent: should report the
    // projected InSystem state, NOT the realtime Surveying.
    let view = run_outline_view(&mut app, ship, Some(empire)).expect("view");
    assert_eq!(view.state, ShipSnapshotState::InSystem);
    assert_eq!(view.system, Some(home));

    // Steady-state per projection => not in In-Transit section.
    let entries = run_in_transit(&mut app, Some(empire));
    assert!(
        entries.is_empty(),
        "observer mode must hide realtime ECS state — In-Transit section must be empty when projection says InSystem"
    );
}

/// #491 / #495 sibling: the existing
/// `outline_observer_mode_is_light_coherent_via_projection` test only
/// exercises `viewing_empire == ship.owner` (= observer is looking at
/// their own ship), which collapses to the own-ship projection path
/// and degenerates the foreign-ship branch. This test pins the
/// *foreign-ship-as-observer* contract: an observer that is NOT the
/// ship's owner must read the snapshot side, ignoring realtime ECS
/// ground truth.
#[test]
fn outline_observer_mode_foreign_ship_uses_snapshot() {
    let mut app = test_app();

    // Viewing empire — the empire whose KnowledgeStore we read.
    let viewing_empire = spawn_minimal_empire(&mut app);

    // Foreign empire — owns the ship under test. Spawned as a separate
    // entity (does not need PlayerEmpire / SystemVisibilityMap on the
    // observer-side path; we only read the viewing empire's store).
    let foreign_empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Foreign".into(),
            },
            Faction {
                id: "foreign".into(),
                name: "Foreign".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            KnowledgeStore::default(),
            macrocosmo::knowledge::SystemVisibilityMap::default(),
        ))
        .id();

    let last_known_sys = spawn_test_system(
        app.world_mut(),
        "LastKnownSys",
        [10.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let realtime_dest = spawn_test_system(
        app.world_mut(),
        "RealtimeDest",
        [200.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );

    let ship = spawn_test_ship(app.world_mut(), "EnemyShip", foreign_empire, last_known_sys);

    // Viewing empire's snapshot of the foreign ship: last seen at
    // sub-light transit (= light-delayed observation of an earlier
    // command). The realtime ECS will then advance to FTL — observer
    // mode must NOT see that.
    {
        let mut em = app.world_mut().entity_mut(viewing_empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_ship(ShipSnapshot {
            entity: ship,
            name: "EnemyShip".into(),
            design_id: "explorer_mk1".into(),
            last_known_state: ShipSnapshotState::InTransitSubLight,
            last_known_system: Some(last_known_sys),
            observed_at: 0,
            hp: 100.0,
            hp_max: 100.0,
            source: ObservationSource::Direct,
        });
    }

    // Realtime: the ship has actually entered FTL (post-snapshot
    // ground truth). The observer-as-foreign path must hide this.
    set_ship_state(
        app.world_mut(),
        ship,
        ShipState::InFTL {
            origin_system: last_known_sys,
            destination_system: realtime_dest,
            departed_at: 1,
            arrival_at: 5,
        },
    );

    // Observer: viewing as `viewing_empire`, looking at a ship owned
    // by `foreign_empire`. Must read the snapshot path, NOT realtime.
    let view = run_outline_view(&mut app, ship, Some(viewing_empire))
        .expect("foreign-ship-as-observer must produce a view via snapshot");
    assert_eq!(
        view.state,
        ShipSnapshotState::InTransitSubLight,
        "foreign-ship observer view must reflect snapshot (sublight), \
         not realtime FTL ground truth"
    );
    assert_eq!(view.system, Some(last_known_sys));

    // Foreign ships are filtered out of the In-Transit section by
    // design (they have their own UI surface) — keep the existing
    // contract from `outline_foreign_ship_uses_snapshot`.
    let entries = run_in_transit(&mut app, Some(viewing_empire));
    assert!(
        entries.iter().all(|e| e.entity != ship),
        "foreign ship must not appear in In-Transit (observer mode). Entries: {:?}",
        entries
    );
}
