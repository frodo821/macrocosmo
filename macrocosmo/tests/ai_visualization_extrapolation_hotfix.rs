//! Hotfix #2 regression tests: AI visualization extrapolation contract.
//!
//! Two root causes covered:
//!
//! 1. **F9 (Omniscient toggle) silent fail.** The #490 fold-in removed
//!    the `UI_TOGGLE_OMNISCIENT → F9` default binding from
//!    `register_engine_defaults`. In normal play `KeybindingPlugin` is
//!    installed, so the registry path always wins over the hardcoded
//!    `F9` fallback — but with no entry in the registry, the key did
//!    nothing. The fix re-registers the default; the regression test
//!    here pins it to the registry so it cannot disappear again.
//!
//! 2. **Dispatch extrapolation lost for `deploy_deliverable`-expanded
//!    chains.** PR #528 introduced eager macro decomposition: the
//!    macro `deploy_deliverable` is expanded in the outbox into
//!    `build_deliverable → load_deliverable → reposition →
//!    unload_deliverable`, all emitted in the same tick. The
//!    per-command projection write in `dispatch_ship_command_per_ship`
//!    was unconditional, so the trailing `unload_deliverable`
//!    (intended_state = None, intended_system = home_port sentinel)
//!    overwrote the meaningful `reposition` extrapolation. End result:
//!    the renderer had no dashed extrapolation line and the ship
//!    marker froze at origin. The fix skips the projection write when
//!    `intended_state.is_none()`, matching the player-side contract
//!    from #493 (`dispatcher_skips_spatial_less_commands`).
//!
//! The negative-result observation contract (`ShipMissing` →
//! projected_state = Missing, intended cleared) is verified only —
//! the existing `apply_reconciliation` arm already implements it
//! correctly; the test pins the behaviour against regression.

mod common;

use bevy::input::ButtonInput;
use bevy::prelude::*;

use macrocosmo::ai::AiPlayerMode;
use macrocosmo::input::{KeyCombo, KeybindingRegistry, actions};
use macrocosmo::knowledge::{
    KnowledgeStore, ObservationSource, ShipProjection, ShipSnapshot, ShipSnapshotState,
    SystemVisibilityMap, SystemVisibilityTier,
};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::{Owner, Ship};
use macrocosmo::time_system::GameClock;

use common::{spawn_test_ruler, spawn_test_ship, spawn_test_system, test_app};

// ---------------------------------------------------------------------------
// Root cause 1: F9 binding regression
// ---------------------------------------------------------------------------

/// The `UI_TOGGLE_OMNISCIENT` action MUST have `F9` as its default
/// binding after `register_engine_defaults` runs. The #490 fold-in
/// silently removed this default; without it, the registry-aware
/// path in `toggle_omniscient_mode` never fires F9 in normal play
/// (the hardcoded fallback only runs when `KeybindingPlugin` is
/// absent, i.e. in tests).
#[test]
fn f9_default_binding_registered_for_omniscient_toggle() {
    let r = KeybindingRegistry::with_engine_defaults();
    assert_eq!(
        r.get(actions::UI_TOGGLE_OMNISCIENT),
        Some(KeyCombo::key(KeyCode::F9)),
        "ui.toggle_omniscient must default to F9 — #490 fold-in regression",
    );

    // End-to-end pin: `is_just_pressed` returns true when F9 fires.
    let mut input = ButtonInput::<KeyCode>::default();
    assert!(
        !r.is_just_pressed(actions::UI_TOGGLE_OMNISCIENT, &input),
        "F9 not pressed → toggle should not fire"
    );
    input.press(KeyCode::F9);
    assert!(
        r.is_just_pressed(actions::UI_TOGGLE_OMNISCIENT, &input),
        "F9 press must drive the omniscient toggle via the registry path"
    );
}

// ---------------------------------------------------------------------------
// Root cause 2: dispatch extrapolation write contract
// ---------------------------------------------------------------------------

/// Shared scenario: one empire, Ruler at home, frontier system, and a
/// ship spawned at home owned by that empire. Mirrors the scaffolding
/// in `tests/ship_projection_dispatch.rs::setup_scenario` but trimmed
/// for our needs.
fn setup_scenario(app: &mut App, frontier_distance_ly: f64) -> (Entity, Entity, Entity, Entity) {
    // Intentionally keep `AiPlayerMode(false)` (the default). The hotfix
    // tests dispatch hand-crafted commands directly; enabling the
    // policy would auto-emit `survey_system` for catalogued frontier
    // systems, contaminating the dispatch trace under test.
    app.insert_resource(AiPlayerMode(false));

    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "extrapolation_test".into(),
                name: "Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            SystemVisibilityMap::default(),
            KnowledgeStore::default(),
            macrocosmo::empire::CommsParams::default(),
        ))
        .id();

    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [frontier_distance_ly, 0.0, 0.0],
        1.0,
        false,
        false,
    );
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut vis = em.get_mut::<SystemVisibilityMap>().unwrap();
        vis.set(home, SystemVisibilityTier::Local);
        vis.set(frontier, SystemVisibilityTier::Catalogued);
    }

    let ship = spawn_test_ship(
        app.world_mut(),
        "Hauler-1",
        "explorer_mk1",
        home,
        [0.0, 0.0, 0.0],
    );
    app.world_mut()
        .entity_mut(ship)
        .get_mut::<Ship>()
        .unwrap()
        .owner = Owner::Empire(empire);

    spawn_test_ruler(app.world_mut(), empire, home);

    (empire, home, frontier, ship)
}

/// Helper: stamp a `last_known` snapshot of the ship at `home` so the
/// dispatcher's belief matches the runtime position.
fn stamp_snapshot_at_home(app: &mut App, empire: Entity, ship: Entity, home: Entity, name: &str) {
    let mut em = app.world_mut().entity_mut(empire);
    let mut store = em.get_mut::<KnowledgeStore>().unwrap();
    store.update_ship(ShipSnapshot {
        entity: ship,
        name: name.into(),
        design_id: "explorer_mk1".into(),
        last_known_state: ShipSnapshotState::InSystem,
        last_known_system: Some(home),
        observed_at: 0,
        hp: 100.0,
        hp_max: 100.0,
        source: ObservationSource::Direct,
    });
}

/// Helper: emit a per-ship AI command directly to the bus, set the
/// clock, and tick. Returns the dispatched_at tick used.
fn emit_and_dispatch(
    app: &mut App,
    kind: &str,
    empire: Entity,
    ship: Entity,
    target_system: Option<Entity>,
    dispatch_clock: i64,
) {
    use macrocosmo::ai::convert::{to_ai_entity, to_ai_faction, to_ai_system};
    use macrocosmo_ai::{Command, CommandKindId, CommandValue};

    let mut c = Command::new(
        CommandKindId::from(kind),
        to_ai_faction(empire),
        dispatch_clock,
    );
    if let Some(t) = target_system {
        c.params.insert(
            "target_system".into(),
            CommandValue::System(to_ai_system(t)),
        );
    }
    c.params.insert("ship_count".into(), CommandValue::I64(1));
    c.params
        .insert("ship_0".into(), CommandValue::Entity(to_ai_entity(ship)));

    {
        let mut bus = app
            .world_mut()
            .resource_mut::<macrocosmo::ai::plugin::AiBusResource>();
        bus.emit_command(c);
    }
    app.world_mut().resource_mut::<GameClock>().elapsed = dispatch_clock;
    app.update();
}

/// Baseline: `survey_system` dispatch writes a meaningful extrapolation
/// (intended_state = Surveying, intended_system = frontier). Mirrors
/// the existing baseline in `ship_projection_dispatch.rs` so any
/// regression here means the dispatch-time projection plumbing is
/// broken at a deeper level.
#[test]
fn dispatch_survey_writes_intended_to_projection() {
    let mut app = test_app();
    let (empire, home, frontier, ship) = setup_scenario(&mut app, 5.0);
    app.update(); // Warmup so AI schemas are declared.
    stamp_snapshot_at_home(&mut app, empire, ship, home, "Hauler-1");

    emit_and_dispatch(&mut app, "survey_system", empire, ship, Some(frontier), 100);

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let p = store
        .get_projection(ship)
        .expect("survey_system dispatch must write a ShipProjection");
    assert_eq!(p.intended_state, Some(ShipSnapshotState::Surveying));
    assert_eq!(p.intended_system, Some(frontier));
}

/// `load_deliverable` is a spatial-less primitive (intended_state =
/// None per `command_kind_to_intended_state`). After this hotfix the
/// dispatcher MUST skip the projection write for it — the player
/// dispatch path already does (#493
/// `dispatcher_skips_spatial_less_commands`), and aligning AI
/// dispatch preserves any prior meaningful extrapolation in the same
/// tick.
#[test]
fn dispatch_load_deliverable_does_not_overwrite_existing_projection() {
    let mut app = test_app();
    let (empire, home, frontier, ship) = setup_scenario(&mut app, 5.0);
    app.update();
    stamp_snapshot_at_home(&mut app, empire, ship, home, "Hauler-1");

    // Pre-seed a meaningful projection (= what `reposition` would have
    // written earlier in the eager-expanded `deploy_deliverable`
    // chain). The hotfix must preserve this projection when
    // `load_deliverable` dispatches next.
    let sentinel_dispatched_at = 50_i64;
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: sentinel_dispatched_at,
            expected_arrival_at: Some(120),
            expected_return_at: None,
            projected_state: ShipSnapshotState::InSystem,
            projected_system: Some(home),
            intended_state: Some(ShipSnapshotState::InTransitSubLight),
            intended_system: Some(frontier),
            intended_takes_effect_at: Some(60),
        });
    }

    // Dispatch a `load_deliverable` (target_system = frontier as the
    // pickup system). Per the hotfix this must NOT overwrite the
    // pre-existing intended_* extrapolation.
    emit_and_dispatch(
        &mut app,
        "load_deliverable",
        empire,
        ship,
        Some(frontier),
        100,
    );

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let p = store
        .get_projection(ship)
        .expect("pre-seeded projection must still exist after load_deliverable dispatch");
    assert_eq!(
        p.dispatched_at, sentinel_dispatched_at,
        "load_deliverable dispatch must NOT overwrite the prior projection — \
         the eager-expanded chain's `reposition` write must survive"
    );
    assert_eq!(
        p.intended_state,
        Some(ShipSnapshotState::InTransitSubLight),
        "intended_state from the prior reposition must survive load_deliverable dispatch"
    );
    assert_eq!(
        p.intended_system,
        Some(frontier),
        "intended_system from the prior reposition must survive load_deliverable dispatch"
    );
}

/// `unload_deliverable` is the trailing primitive in the
/// `deploy_deliverable` eager-expanded chain and uses
/// `ship.home_port` as a sentinel target_system. Before the hotfix
/// its projection write (intended_state = None, intended_system =
/// home_port) would clobber the prior `reposition` extrapolation.
/// After the hotfix it skips the write entirely.
#[test]
fn dispatch_unload_deliverable_does_not_overwrite_existing_projection() {
    let mut app = test_app();
    let (empire, home, frontier, ship) = setup_scenario(&mut app, 5.0);
    app.update();
    stamp_snapshot_at_home(&mut app, empire, ship, home, "Hauler-1");

    let sentinel_dispatched_at = 50_i64;
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: sentinel_dispatched_at,
            expected_arrival_at: Some(120),
            expected_return_at: None,
            projected_state: ShipSnapshotState::InSystem,
            projected_system: Some(home),
            intended_state: Some(ShipSnapshotState::InTransitSubLight),
            intended_system: Some(frontier),
            intended_takes_effect_at: Some(60),
        });
    }

    // `unload_deliverable` carries no `target_system` param — the
    // dispatcher derives the sentinel from `ship.home_port`. Emit it
    // without a target.
    emit_and_dispatch(&mut app, "unload_deliverable", empire, ship, None, 100);

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let p = store
        .get_projection(ship)
        .expect("pre-seeded projection must survive unload_deliverable dispatch");
    assert_eq!(
        p.dispatched_at, sentinel_dispatched_at,
        "unload_deliverable dispatch must NOT clobber the prior projection"
    );
    assert_eq!(
        p.intended_state,
        Some(ShipSnapshotState::InTransitSubLight),
        "intended_state from prior reposition must survive unload_deliverable dispatch — \
         renderer relies on this for the dashed extrapolation line"
    );
    assert_eq!(
        p.intended_system,
        Some(frontier),
        "intended_system must NOT collapse to the home_port sentinel"
    );
}

/// `reposition` IS a meaningful spatial primitive (intended_state =
/// InTransitSubLight). Its dispatch must continue to write a fresh
/// projection — this test pins the pre-hotfix path's normal behaviour
/// for the eager-expanded primitive that produces the extrapolation
/// the load/unload skips preserve.
#[test]
fn dispatch_reposition_writes_intended_to_projection() {
    let mut app = test_app();
    let (empire, home, frontier, ship) = setup_scenario(&mut app, 5.0);
    app.update();
    stamp_snapshot_at_home(&mut app, empire, ship, home, "Hauler-1");

    emit_and_dispatch(&mut app, "reposition", empire, ship, Some(frontier), 100);

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let p = store
        .get_projection(ship)
        .expect("reposition dispatch must write a ShipProjection");
    assert_eq!(p.dispatched_at, 100);
    assert_eq!(
        p.intended_state,
        Some(ShipSnapshotState::InTransitSubLight),
        "reposition is a movement-style command, intended_state must be InTransitSubLight"
    );
    assert_eq!(p.intended_system, Some(frontier));
}

/// Audit pin: `command_kind_to_intended_state` must yield the expected
/// mapping for every AI ship-control kind. Catches accidental regressions
/// in either direction (None → Some or Some → None).
#[test]
fn command_kind_to_intended_state_full_audit() {
    use macrocosmo::knowledge::command_kind_to_intended_state as map;

    // Movement-style → InTransitSubLight (route planner upgrades to
    // FTL when the first segment is an FTL hop).
    assert_eq!(
        map("attack_target"),
        Some(ShipSnapshotState::InTransitSubLight)
    );
    assert_eq!(
        map("reposition"),
        Some(ShipSnapshotState::InTransitSubLight)
    );
    assert_eq!(map("blockade"), Some(ShipSnapshotState::InTransitSubLight));
    assert_eq!(
        map("fortify_system"),
        Some(ShipSnapshotState::InTransitSubLight)
    );
    assert_eq!(
        map("move_ruler"),
        Some(ShipSnapshotState::InTransitSubLight)
    );
    assert_eq!(map("move_to"), Some(ShipSnapshotState::InTransitSubLight));

    // Survey / colonize variants.
    assert_eq!(map("survey_system"), Some(ShipSnapshotState::Surveying));
    assert_eq!(map("colonize_system"), Some(ShipSnapshotState::Settling));
    assert_eq!(map("colonize_planet"), Some(ShipSnapshotState::Settling));

    // Spatial-less / cargo primitives: explicitly None. The dispatch
    // path skips the projection write when this returns None
    // (`dispatch_ship_command_per_ship` hotfix), which is how the
    // eager-expanded `deploy_deliverable` chain preserves the
    // reposition extrapolation. If a future change wires either of
    // these to a real spatial state, also revisit the dispatch-time
    // skip guard so the renderer's extrapolation contract still holds.
    assert_eq!(
        map("load_deliverable"),
        None,
        "load_deliverable is intentionally spatial-less"
    );
    assert_eq!(
        map("unload_deliverable"),
        None,
        "unload_deliverable is intentionally spatial-less"
    );
}

/// Negative-result contract pin (verification only — `apply_reconciliation`
/// already implements this correctly). When an empire observes
/// `KnowledgeFact::ShipMissing` for a ship it had an extrapolation
/// for, the projection's `projected_state` flips to `Missing` and the
/// intended_* layer is cleared. Matches the user-stated map contract:
/// "観測が negative result だけなら 「不明」 で更新".
#[test]
fn ship_missing_fact_marks_projection_missing_and_clears_intended() {
    use macrocosmo::knowledge::{
        KnowledgeFact, PendingFactQueue, PerceivedFact, reconcile_ship_projections,
    };

    let mut app = test_app();
    let (empire, home, frontier, ship) = setup_scenario(&mut app, 5.0);

    // Seed a projection with an in-flight intended layer (the
    // extrapolation a freshly dispatched survey would write).
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_projection(ShipProjection {
            entity: ship,
            dispatched_at: 10,
            expected_arrival_at: Some(80),
            expected_return_at: Some(160),
            projected_state: ShipSnapshotState::InSystem,
            projected_system: Some(home),
            intended_state: Some(ShipSnapshotState::Surveying),
            intended_system: Some(frontier),
            intended_takes_effect_at: Some(15),
        });
    }

    // Inject a `ShipMissing` fact at t = 200, observed_at the empire's
    // vantage. `arrives_at == observed_at` because the reconciler
    // recomputes per-empire arrival from `origin_pos` vs viewer
    // position anyway.
    let fact_observed_at = 200_i64;
    let pf = PerceivedFact {
        fact: KnowledgeFact::ShipMissing {
            event_id: None,
            system: None,
            ship_name: "Hauler-1".into(),
            detail: "test: presumed missing".into(),
            ship,
        },
        observed_at: fact_observed_at,
        arrives_at: fact_observed_at,
        source: macrocosmo::knowledge::ObservationSource::Direct,
        origin_pos: [0.0, 0.0, 0.0],
        related_system: None,
    };
    app.world_mut()
        .resource_mut::<PendingFactQueue>()
        .record(pf);

    // Bump the clock past `arrives_at` so the reconciler accepts the fact.
    app.world_mut().resource_mut::<GameClock>().elapsed = fact_observed_at + 1;

    // Run only the reconciler in isolation.
    let mut schedule = bevy::ecs::schedule::Schedule::default();
    schedule.add_systems(reconcile_ship_projections);
    schedule.run(app.world_mut());

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let p = store
        .get_projection(ship)
        .expect("projection must still exist after ShipMissing reconciliation");
    assert_eq!(
        p.projected_state,
        ShipSnapshotState::Missing,
        "ShipMissing fact must flip projected_state to Missing — renderer's `不明` state"
    );
    assert!(
        p.intended_state.is_none(),
        "ShipMissing must clear intended_state (the extrapolation is no longer valid)"
    );
    assert!(
        p.intended_system.is_none(),
        "ShipMissing must clear intended_system"
    );
    assert!(
        p.intended_takes_effect_at.is_none(),
        "ShipMissing must clear intended_takes_effect_at"
    );
}
