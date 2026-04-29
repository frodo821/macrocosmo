//! #491 PR-6: `ui::mod`'s map tooltip status string and the `ship_pos`
//! camera-centering derivation must flow through the `ShipView` helper
//! (= projection-mediated for own-empire ships, snapshot-mediated for
//! foreign ships) instead of reading the realtime ECS `ShipState`.
//!
//! These tests pin the **data extraction layer** — `ship_view`,
//! `ShipView::position`, and the `ShipSnapshotState` → tooltip-word
//! mapping that the new tooltip code performs inline. The egui-driven
//! `draw_map_tooltips` itself is not invoked (egui systems are excluded
//! from `test_app()`), but the helper output it consumes is exhaustively
//! exercised here.
//!
//! See `tests/outline_tree_ftl_leak.rs` for the analogous outline-tree
//! contract — this file mirrors that pattern for the map tooltip and
//! ship_pos camera-centering surface.

mod common;

use bevy::prelude::*;

use macrocosmo::components::Position;
use macrocosmo::knowledge::ship_view::ship_view;
use macrocosmo::knowledge::{
    KnowledgeStore, ObservationSource, ShipProjection, ShipSnapshot, ShipSnapshotState,
};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::fleet::{Fleet, FleetMembers};
use macrocosmo::ship::{
    Cargo, CommandQueue, Owner, RulesOfEngagement, Ship, ShipHitpoints, ShipModifiers, ShipState,
};

use common::{spawn_test_system, test_app};

// ---------------------------------------------------------------------------
// Helpers (mirrored from outline_tree_ftl_leak.rs)
// ---------------------------------------------------------------------------

fn spawn_minimal_empire(app: &mut App) -> Entity {
    app.world_mut()
        .spawn((
            Empire {
                name: "Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "tooltip_test".into(),
                name: "Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            KnowledgeStore::default(),
            macrocosmo::knowledge::SystemVisibilityMap::default(),
        ))
        .id()
}

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

fn set_ship_state(world: &mut World, ship: Entity, new_state: ShipState) {
    *world.get_mut::<ShipState>(ship).unwrap() = new_state;
}

/// Build an owned `KnowledgeStore` populated from the given empire's
/// store, so the test can pass it without holding a borrow that conflicts
/// with the (mutable) ship query in the same block.
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

/// Reproduce the `ui::mod` map tooltip status string mapping (the inline
/// match in `draw_map_tooltips`). Pins the contract that
/// `InTransitSubLight` and `InTransitFTL` produce different tooltip
/// words.
fn tooltip_status_word(state: &ShipSnapshotState) -> &'static str {
    match state {
        ShipSnapshotState::InSystem => "Docked",
        ShipSnapshotState::InTransitSubLight => "Sub-light",
        ShipSnapshotState::InTransitFTL => "In FTL",
        ShipSnapshotState::Surveying => "Surveying",
        ShipSnapshotState::Settling => "Settling",
        ShipSnapshotState::Refitting => "Refitting",
        ShipSnapshotState::Loitering { .. } => "Loitering",
        ShipSnapshotState::Destroyed => "Destroyed",
        ShipSnapshotState::Missing => "Missing",
    }
}

// ---------------------------------------------------------------------------
// 1. Map tooltip — projection-mediated for own ships
// ---------------------------------------------------------------------------

/// Own ship dispatched to a remote system: realtime ECS already shows
/// `SubLight`, but the projection still says `InSystem` (= dispatcher
/// hasn't propagated the command effect to the player POV yet). The
/// tooltip must render the **projected** state — "Docked" — not the
/// realtime "Sub-light".
#[test]
fn map_tooltip_status_uses_projection() {
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

    // Projection: still at home (= player POV).
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

    // Realtime ECS advances ahead of the projection (the FTL leak the
    // tooltip must NOT surface).
    set_ship_state(
        app.world_mut(),
        ship,
        ShipState::SubLight {
            origin: [0.0, 0.0, 0.0],
            destination: [50.0, 0.0, 0.0],
            target_system: Some(frontier),
            departed_at: 0,
            arrival_at: 5,
        },
    );

    let knowledge = snapshot_knowledge(&app, empire);
    let ship_ref = app.world().entity(ship);
    let ship_comp = ship_ref.get::<Ship>().expect("Ship");
    let state = ship_ref.get::<ShipState>().expect("ShipState");
    let view = ship_view(ship, ship_comp, state, Some(&knowledge), Some(empire))
        .expect("own-ship projection produces a view");

    assert_eq!(view.state, ShipSnapshotState::InSystem);
    assert_eq!(
        tooltip_status_word(&view.state),
        "Docked",
        "tooltip must surface the projected (light-coherent) state, not the realtime SubLight"
    );
}

// ---------------------------------------------------------------------------
// 2. Map tooltip distinguishes InTransitSubLight / InTransitFTL
// ---------------------------------------------------------------------------

/// Once the projection's reconciler has upgraded `projected_state` to
/// `InTransitFTL` (= the dispatcher has light-coherently observed the
/// ship engaging FTL), the tooltip shows "In FTL" — distinct from
/// "Sub-light". This pins the FTL/sub-light separation the player UI
/// must honour (FTL ships cannot be intercepted by game contract).
#[test]
fn map_tooltip_distinguishes_intransit_ftl_sublight() {
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

    // Projection: InTransitFTL, reconciled.
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 0,
            expected_arrival_at: Some(5),
            expected_return_at: None,
            projected_state: ShipSnapshotState::InTransitFTL,
            projected_system: Some(frontier),
            intended_state: None,
            intended_system: None,
            intended_takes_effect_at: None,
        });
    }
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

    // Each `ship_view` call is scoped so the immutable `app.world()` borrow
    // is released before we mutate `KnowledgeStore` for the next phase.
    {
        let knowledge = snapshot_knowledge(&app, empire);
        let ship_ref = app.world().entity(ship);
        let ship_comp = ship_ref.get::<Ship>().expect("Ship");
        let state = ship_ref.get::<ShipState>().expect("ShipState");
        let view = ship_view(ship, ship_comp, state, Some(&knowledge), Some(empire)).expect("view");

        assert_eq!(view.state, ShipSnapshotState::InTransitFTL);
        assert_eq!(
            tooltip_status_word(&view.state),
            "In FTL",
            "FTL transit must produce the 'In FTL' tooltip word, not a generic 'In Transit'"
        );
    }

    // And the sublight branch produces "Sub-light", different from the
    // FTL branch — this is the load-bearing distinction.
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 0,
            expected_arrival_at: Some(5),
            expected_return_at: None,
            projected_state: ShipSnapshotState::InTransitSubLight,
            projected_system: Some(frontier),
            intended_state: None,
            intended_system: None,
            intended_takes_effect_at: None,
        });
    }
    {
        let knowledge2 = snapshot_knowledge(&app, empire);
        let ship_ref = app.world().entity(ship);
        let ship_comp = ship_ref.get::<Ship>().expect("Ship");
        let state = ship_ref.get::<ShipState>().expect("ShipState");
        let view_sl =
            ship_view(ship, ship_comp, state, Some(&knowledge2), Some(empire)).expect("view");
        assert_eq!(view_sl.state, ShipSnapshotState::InTransitSubLight);
        assert_eq!(tooltip_status_word(&view_sl.state), "Sub-light");
    }
}

// ---------------------------------------------------------------------------
// 3. Map tooltip — foreign ship reads snapshot
// ---------------------------------------------------------------------------

/// Foreign ship: realtime ECS may have advanced past the snapshot, but
/// the tooltip must reflect the snapshot (= last-known state via #175
/// light-delayed observation).
#[test]
fn map_tooltip_foreign_ship_uses_snapshot() {
    let mut app = test_app();
    let viewing_empire = spawn_minimal_empire(&mut app);

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

    let ship = spawn_test_ship(app.world_mut(), "EnemyShip", foreign_empire, last_known_sys);

    // Snapshot: last seen entering FTL.
    {
        let mut em = app.world_mut().entity_mut(viewing_empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_ship(ShipSnapshot {
            entity: ship,
            name: "EnemyShip".into(),
            design_id: "explorer_mk1".into(),
            last_known_state: ShipSnapshotState::InTransitFTL,
            last_known_system: Some(last_known_sys),
            observed_at: 0,
            hp: 100.0,
            hp_max: 100.0,
            source: ObservationSource::Direct,
        });
    }

    // Realtime: ship has actually arrived at a different system. The
    // tooltip must hide this — show snapshot's "In FTL".
    let realtime_dest = spawn_test_system(
        app.world_mut(),
        "RealtimeDest",
        [200.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    set_ship_state(
        app.world_mut(),
        ship,
        ShipState::InSystem {
            system: realtime_dest,
        },
    );

    let knowledge = snapshot_knowledge(&app, viewing_empire);
    let ship_ref = app.world().entity(ship);
    let ship_comp = ship_ref.get::<Ship>().expect("Ship");
    let state = ship_ref.get::<ShipState>().expect("ShipState");
    let view = ship_view(
        ship,
        ship_comp,
        state,
        Some(&knowledge),
        Some(viewing_empire),
    )
    .expect("foreign-ship snapshot produces a view");

    assert_eq!(view.state, ShipSnapshotState::InTransitFTL);
    assert_eq!(
        tooltip_status_word(&view.state),
        "In FTL",
        "foreign ship tooltip must reflect snapshot, not realtime ECS ground truth"
    );
}

// ---------------------------------------------------------------------------
// 4. ship_pos camera centering — projection drives anchor for own ships
// ---------------------------------------------------------------------------

/// `ship_pos` (the camera-centering anchor when the player selects a
/// ship) must be derived from the projection's anchor system, not the
/// realtime ECS state. Setup: own ship dispatched to a remote system;
/// projection still says `InSystem` at home; camera must center on
/// **home**, not on the (realtime) in-transit position.
#[test]
fn ship_pos_camera_centering_uses_projection() {
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

    // Projection: at home.
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
    // Realtime: in FTL — the leak we must not surface.
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

    // Scope the immutable world borrow so the projection mutation below
    // can take a fresh `app.world_mut()` (avoids E0502).
    {
        let knowledge = snapshot_knowledge(&app, empire);
        let ship_ref = app.world().entity(ship);
        let ship_comp = ship_ref.get::<Ship>().expect("Ship");
        let state = ship_ref.get::<ShipState>().expect("ShipState");
        let view = ship_view(ship, ship_comp, state, Some(&knowledge), Some(empire))
            .expect("own-ship projection produces a view");

        // Reproduce the `ui::mod` ship_pos derivation: position-accessor
        // first (= Loitering), then fall back to view.system.
        let pos_via_position = view.position();
        assert_eq!(
            pos_via_position, None,
            "InSystem must not produce a loitering coord"
        );
        let anchor_system = view.system;
        assert_eq!(
            anchor_system,
            Some(home),
            "ship_pos must center on the projection's home anchor, not the realtime FTL destination"
        );
    }

    // And confirm the projection of an in-transit ship would semantically
    // anchor to the **destination** (= player's belief of where the ship
    // will arrive), not the realtime in-transit lerp coordinate.
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 0,
            expected_arrival_at: Some(5),
            expected_return_at: None,
            projected_state: ShipSnapshotState::InTransitFTL,
            projected_system: Some(frontier),
            intended_state: None,
            intended_system: None,
            intended_takes_effect_at: None,
        });
    }
    let knowledge2 = snapshot_knowledge(&app, empire);
    let ship_ref = app.world().entity(ship);
    let ship_comp = ship_ref.get::<Ship>().expect("Ship");
    let state = ship_ref.get::<ShipState>().expect("ShipState");
    let view2 = ship_view(ship, ship_comp, state, Some(&knowledge2), Some(empire)).expect("view");
    assert_eq!(
        view2.system,
        Some(frontier),
        "in-transit ship_pos anchors to the projection's destination system"
    );
}

// ---------------------------------------------------------------------------
// 5. ship_pos — Loitering uses ShipView::position() accessor
// ---------------------------------------------------------------------------

/// Loitering ships are deep-space (no system anchor). `ship_pos` must
/// route through `ShipView::position()` so the camera centers on the
/// loitering coordinates.
#[test]
fn ship_pos_loitering_uses_position_accessor() {
    let mut app = test_app();
    let empire = spawn_minimal_empire(&mut app);
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);

    let ship = spawn_test_ship(app.world_mut(), "Explorer-1", empire, home);

    // Projection: loitering at deep-space coords.
    let loiter_coords = [12.5, -7.0, 3.25];
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 0,
            expected_arrival_at: None,
            expected_return_at: None,
            projected_state: ShipSnapshotState::Loitering {
                position: loiter_coords,
            },
            projected_system: None,
            intended_state: None,
            intended_system: None,
            intended_takes_effect_at: None,
        });
    }
    set_ship_state(
        app.world_mut(),
        ship,
        ShipState::Loitering {
            position: loiter_coords,
        },
    );

    let knowledge = snapshot_knowledge(&app, empire);
    let ship_ref = app.world().entity(ship);
    let ship_comp = ship_ref.get::<Ship>().expect("Ship");
    let state = ship_ref.get::<ShipState>().expect("ShipState");
    let view = ship_view(ship, ship_comp, state, Some(&knowledge), Some(empire)).expect("view");

    // Mirror the `ui::mod` ship_pos derivation precedence: position()
    // accessor first.
    let derived_position = view.position().map(Position::from);
    assert_eq!(
        derived_position,
        Some(Position::from(loiter_coords)),
        "Loitering ship_pos must come from ShipView::position(), not view.system"
    );
    // And view.system is None for loitering — the fallback would not
    // produce a position.
    assert_eq!(view.system, None);
    assert_eq!(
        tooltip_status_word(&view.state),
        "Loitering",
        "Loitering tooltip word"
    );
}
