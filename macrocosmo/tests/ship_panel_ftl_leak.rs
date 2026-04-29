//! #491 PR-2: Ship panel must not leak realtime ECS `ShipState` for
//! own-empire ships. Status label, docked-system derivation, cancel /
//! refit gating all flow through the viewing empire's
//! `KnowledgeStore::projections` (own ships) or `ship_snapshots`
//! (foreign ships), mirroring the Galaxy Map fix #477 and the outline
//! tree fix #487.
//!
//! Pins the **data extraction layer** — `ship_view` /
//! `build_status_info_from_view` — exercised through `test_app()`
//! (egui systems are excluded so the egui-driven `draw_ship_panel`
//! itself is not invoked).

mod common;

use bevy::ecs::system::SystemState;
use bevy::prelude::*;

use macrocosmo::components::Position;
use macrocosmo::galaxy::{StarSystem, SystemAttributes};
use macrocosmo::knowledge::{
    KnowledgeStore, ObservationSource, ShipProjection, ShipSnapshot, ShipSnapshotState,
};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::fleet::{Fleet, FleetMembers};
use macrocosmo::ship::{
    Cargo, CommandQueue, Owner, RulesOfEngagement, Ship, ShipHitpoints, ShipModifiers, ShipState,
};
use macrocosmo::time_system::GameClock;
use macrocosmo::ui::ship_panel::build_status_info_from_view;
use macrocosmo::ui::ship_view::ship_view;

use common::{spawn_test_system, test_app};

// ---------------------------------------------------------------------------
// Helpers (mirror outline_tree_ftl_leak.rs setup so the regression suite
// stays consistent across panel rewires).
// ---------------------------------------------------------------------------

fn spawn_minimal_empire(app: &mut App) -> Entity {
    app.world_mut()
        .spawn((
            Empire {
                name: "Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "ship_panel_test".into(),
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

/// Run `build_status_info_from_view` against the world's star query the
/// way `draw_ship_panel` does. `timing` is constructed by the caller to
/// match the path-under-test (= projection / snapshot / realtime).
fn run_status_label(
    app: &mut App,
    view: &macrocosmo::ui::ship_view::ShipView,
    timing: Option<macrocosmo::ui::ship_view::ShipViewTiming>,
    clock_elapsed: i64,
) -> macrocosmo::ui::ship_panel::ShipStatusInfo {
    let mut state: SystemState<Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>> =
        SystemState::new(app.world_mut());
    let stars = state.get(app.world_mut());
    let clock = GameClock::new(clock_elapsed);
    build_status_info_from_view(view, timing, &clock, &stars)
}

// ---------------------------------------------------------------------------
// 1. Status label uses projection (FTL-leak regression)
// ---------------------------------------------------------------------------

/// Right after dispatching a survey, the realtime ECS has the ship in
/// FTL/Surveying — but the projection still says `InSystem` (the
/// dispatch hasn't propagated to the ship yet). The ship panel's status
/// label MUST read the projection ("Docked at Home"), not realtime
/// ("FTL to Frontier" / "Surveying Frontier").
#[test]
fn ship_panel_status_label_uses_projection_after_dispatch() {
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

    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 0,
            expected_arrival_at: Some(10),
            expected_return_at: None,
            projected_state: ShipSnapshotState::InSystem,
            projected_system: Some(home),
            intended_state: Some(ShipSnapshotState::Surveying),
            intended_system: Some(frontier),
            intended_takes_effect_at: Some(5),
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

    // Build the view the same way the panel does.
    let store = snapshot_knowledge(&app, empire);
    let ship_ref = app.world().entity(ship);
    let ship_comp = ship_ref.get::<Ship>().expect("Ship Component").clone();
    let state = ship_ref.get::<ShipState>().expect("ShipState").clone();
    let view = ship_view(ship, &ship_comp, &state, Some(&store), Some(empire))
        .expect("own-ship projection must produce a view");
    // Projection-anchored timing.
    let timing = {
        let p = store.get_projection(ship).expect("projection");
        macrocosmo::ui::ship_view::ShipViewTiming {
            origin_tick: p.dispatched_at,
            expected_tick: p.expected_arrival_at,
        }
    };

    let info = run_status_label(&mut app, &view, Some(timing), 1);
    assert!(
        info.label.contains("Docked"),
        "FTL-leak regression: status label must reflect projection (Docked) \
         not realtime FTL. Got: {:?}",
        info.label
    );
    assert!(
        info.label.contains("Home"),
        "label must name the projected system (Home). Got: {:?}",
        info.label
    );
    assert!(
        !info.label.contains("FTL"),
        "label must not surface the realtime FTL state. Got: {:?}",
        info.label
    );
    assert!(
        !info.label.contains("Surveying"),
        "label must not surface the intended Surveying state until the \
         projection reconciles. Got: {:?}",
        info.label
    );
    // Steady-state (InSystem) — no progress bar.
    assert_eq!(info.progress, None);
}

// ---------------------------------------------------------------------------
// 2. docked_system derivation uses projection
// ---------------------------------------------------------------------------

/// `docked_system` (used downstream for colony lookup, scrap, set-home-port,
/// refit eligibility, harbour interactions) MUST be `Some(home)` per the
/// projection — not `None` per the realtime InFTL state.
#[test]
fn ship_panel_docked_system_uses_projection() {
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

    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 0,
            expected_arrival_at: Some(10),
            expected_return_at: None,
            projected_state: ShipSnapshotState::InSystem,
            projected_system: Some(home),
            intended_state: Some(ShipSnapshotState::Surveying),
            intended_system: Some(frontier),
            intended_takes_effect_at: Some(5),
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

    let store = snapshot_knowledge(&app, empire);
    let ship_ref = app.world().entity(ship);
    let ship_comp = ship_ref.get::<Ship>().expect("Ship").clone();
    let state = ship_ref.get::<ShipState>().expect("ShipState").clone();
    let view = ship_view(ship, &ship_comp, &state, Some(&store), Some(empire)).expect("view");
    // Mirrors `draw_ship_panel`'s derivation:
    //   docked_system = (view.state == InSystem) ? view.system : None
    let docked_system = if matches!(view.state, ShipSnapshotState::InSystem) {
        view.system
    } else {
        None
    };
    assert_eq!(
        docked_system,
        Some(home),
        "docked_system must come from projection (Some(home)), not realtime InFTL (None)"
    );
}

// ---------------------------------------------------------------------------
// 3. is_cancellable uses projection
// ---------------------------------------------------------------------------

/// Cancel button visibility is the player's *belief* — the projection
/// state. Realtime says Surveying, but the projection still says InSystem
/// (= the player hasn't been told the survey started yet). The Cancel
/// button must NOT appear until the projection reconciles.
#[test]
fn ship_panel_is_cancellable_uses_projection() {
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

    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 0,
            expected_arrival_at: Some(10),
            expected_return_at: None,
            projected_state: ShipSnapshotState::InSystem,
            projected_system: Some(home),
            intended_state: Some(ShipSnapshotState::Surveying),
            intended_system: Some(frontier),
            intended_takes_effect_at: Some(5),
        });
    }
    // Realtime advances ahead — survey has begun.
    set_ship_state(
        app.world_mut(),
        ship,
        ShipState::Surveying {
            target_system: frontier,
            started_at: 0,
            completes_at: 10,
        },
    );

    // Helper closure: snapshot knowledge, look up ship+state, build view,
    // and return the projection-mediated cancel-button predicate.
    fn is_cancellable_now(app: &App, ship: Entity, empire: Entity) -> bool {
        let store = snapshot_knowledge(app, empire);
        let ship_ref = app.world().entity(ship);
        let ship_comp = ship_ref.get::<Ship>().expect("Ship");
        let state = ship_ref.get::<ShipState>().expect("ShipState");
        let view = ship_view(ship, ship_comp, state, Some(&store), Some(empire)).expect("view");
        matches!(
            view.state,
            ShipSnapshotState::Surveying | ShipSnapshotState::Settling
        )
    }

    assert!(
        !is_cancellable_now(&app, ship, empire),
        "FTL-leak regression: cancel button must not be shown while \
         projection still says InSystem (the player has not yet been \
         informed the survey started)."
    );

    // Once the projection reconciles to Surveying, cancel becomes available.
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 0,
            expected_arrival_at: Some(10),
            expected_return_at: None,
            projected_state: ShipSnapshotState::Surveying,
            projected_system: Some(frontier),
            intended_state: Some(ShipSnapshotState::Surveying),
            intended_system: Some(frontier),
            intended_takes_effect_at: Some(5),
        });
    }
    assert!(
        is_cancellable_now(&app, ship, empire),
        "cancel button must appear once projection reconciles to Surveying"
    );
}

// ---------------------------------------------------------------------------
// 4. No projection — falls through to realtime ECS
// ---------------------------------------------------------------------------

/// Edge case: own-empire ship without a projection (= early Startup
/// before #481's spawn-time seed lands). `ship_view` returns `None` for
/// the projection path — but the same ship through the realtime
/// fallback (no KnowledgeStore at all) must use the ECS state.
///
/// Production caller (= `draw_ship_panel`) skips the panel entirely
/// when `ship_view` returns `None` — that is the safer behaviour and
/// matches the outline tree's no-projection contract. This test
/// verifies the fallback path itself produces a sensible label so the
/// helper layer's contract is preserved.
#[test]
fn ship_panel_no_projection_falls_through_to_realtime() {
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

    // No projection populated. Realtime says InFTL.
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

    // First check the production path: with KnowledgeStore but no
    // projection, `ship_view` returns None and the panel skips the ship.
    let store = snapshot_knowledge(&app, empire);
    let ship_ref = app.world().entity(ship);
    let ship_comp = ship_ref.get::<Ship>().expect("Ship").clone();
    let state = ship_ref.get::<ShipState>().expect("ShipState").clone();
    let view_with_store = ship_view(ship, &ship_comp, &state, Some(&store), Some(empire));
    assert!(
        view_with_store.is_none(),
        "own-ship without projection must produce no view — caller skips panel"
    );

    // Now the realtime fallback path: no KnowledgeStore at all
    // (= early Startup). The view collapses from realtime ECS, and the
    // status label uses the realtime arrival tick.
    let view_no_store =
        ship_view(ship, &ship_comp, &state, None, Some(empire)).expect("realtime fallback view");
    assert_eq!(view_no_store.state, ShipSnapshotState::InTransitFTL);
    assert_eq!(view_no_store.system, Some(frontier));

    // Realtime-anchored timing — matches what the panel constructs.
    let timing = macrocosmo::ui::ship_view::ShipViewTiming {
        origin_tick: 0,
        expected_tick: Some(5),
    };
    let info = run_status_label(&mut app, &view_no_store, Some(timing), 2);
    assert!(
        info.label.contains("FTL"),
        "realtime fallback must reflect ECS InFTL state. Got: {:?}",
        info.label
    );
    assert!(
        info.label.contains("Frontier"),
        "label must name the realtime destination. Got: {:?}",
        info.label
    );
    let progress = info.progress.expect("InTransitFTL must surface progress");
    assert_eq!(progress.0, 2, "elapsed = clock - departed");
    assert_eq!(progress.1, 5, "total = arrival - departed");
}

// ---------------------------------------------------------------------------
// 5. Foreign ship reads snapshot
// ---------------------------------------------------------------------------

/// Foreign ships are rendered through the viewing empire's
/// `ship_snapshots` — observer mode (when `viewing_empire` points at a
/// non-owner empire) MUST NOT see realtime ECS ground truth.
#[test]
fn ship_panel_foreign_ship_uses_snapshot() {
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

    let last_known = spawn_test_system(
        app.world_mut(),
        "LastKnown",
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

    let ship = spawn_test_ship(app.world_mut(), "EnemyShip", foreign_empire, last_known);

    // Snapshot: viewing empire saw the foreign ship in sub-light transit.
    {
        let mut em = app.world_mut().entity_mut(viewing_empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_ship(ShipSnapshot {
            entity: ship,
            name: "EnemyShip".into(),
            design_id: "explorer_mk1".into(),
            last_known_state: ShipSnapshotState::InTransitSubLight,
            last_known_system: Some(last_known),
            observed_at: 0,
            hp: 100.0,
            hp_max: 100.0,
            source: ObservationSource::Direct,
        });
    }
    // Realtime advances ahead — the ship has actually entered FTL. The
    // viewing empire must NOT see this.
    set_ship_state(
        app.world_mut(),
        ship,
        ShipState::InFTL {
            origin_system: last_known,
            destination_system: realtime_dest,
            departed_at: 1,
            arrival_at: 5,
        },
    );

    let store = snapshot_knowledge(&app, viewing_empire);
    let ship_ref = app.world().entity(ship);
    let ship_comp = ship_ref.get::<Ship>().expect("Ship").clone();
    let state = ship_ref.get::<ShipState>().expect("ShipState").clone();
    let view = ship_view(ship, &ship_comp, &state, Some(&store), Some(viewing_empire))
        .expect("foreign-ship snapshot must produce a view");
    assert_eq!(
        view.state,
        ShipSnapshotState::InTransitSubLight,
        "foreign ship view must reflect snapshot (sublight), not realtime FTL"
    );
    assert_eq!(view.system, Some(last_known));

    // Foreign ship snapshot timing has no expected_tick — progress
    // collapses to None even though the state is in-transit.
    let timing = {
        let snap = store.get_ship(ship).expect("snapshot");
        macrocosmo::ui::ship_view::ShipViewTiming {
            origin_tick: snap.observed_at,
            expected_tick: None,
        }
    };
    let info = run_status_label(&mut app, &view, Some(timing), 5);
    assert!(
        info.label.contains("Moving") || info.label.contains("Transit"),
        "label must reflect sublight transit. Got: {:?}",
        info.label
    );
    assert!(
        info.label.contains("LastKnown"),
        "label must name snapshot's last-known system. Got: {:?}",
        info.label
    );
    assert!(
        !info.label.contains("FTL"),
        "label must not leak the realtime FTL state. Got: {:?}",
        info.label
    );
    assert_eq!(
        info.progress, None,
        "foreign-ship snapshot has no expected_tick — progress must be None"
    );
}
