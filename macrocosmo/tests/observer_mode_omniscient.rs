//! Integration tests for the Omniscient (god-view) observer mode (#490).
//!
//! These tests pin the three-state contract established by #490:
//!
//! * `Disabled` — the canonical empire is the `PlayerEmpire`; UI panels
//!   read through the player's `KnowledgeStore` (light-coherent).
//! * `EmpireView` — the canonical empire is `ObserverView.viewing`; UI
//!   panels read through THAT empire's `KnowledgeStore` (still
//!   light-coherent, just from a different perspective; #499).
//! * `Omniscient` — there is no canonical empire's `KnowledgeStore` to
//!   read from; the helpers return `None` and callers drop into the
//!   realtime-ECS ground-truth path (god view; #490).
//!
//! The two hook helpers under test are `ui::resolve_viewing_knowledge`
//! / `ui::resolve_viewing_knowledge_omniscient` and
//! `ui::empire_view_knowledge` / `ui::empire_view_knowledge_omniscient`.
//! The `_omniscient` variants are the explicit god-view branches; the
//! plain variants preserve the pre-#490 path.

use bevy::ecs::system::SystemState;
use bevy::prelude::*;

use macrocosmo::knowledge::KnowledgeStore;
use macrocosmo::observer::{NonOmniscientKind, ObserverMode, ObserverModeKind, ObserverView};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};

fn spawn_empire(world: &mut World, name: &str, is_player: bool) -> Entity {
    let mut e = world.spawn((
        Empire { name: name.into() },
        Faction {
            id: name.to_lowercase(),
            name: name.into(),
            can_diplomacy: false,
            allowed_diplomatic_options: Default::default(),
        },
        KnowledgeStore::default(),
    ));
    if is_player {
        e.insert(PlayerEmpire);
    }
    e.id()
}

// ---------------------------------------------------------------------------
// ObserverMode enum predicates (3-variant)
// ---------------------------------------------------------------------------

/// #490: Default is `Disabled`.
#[test]
fn observer_mode_default_is_disabled() {
    let mode = ObserverMode::default();
    assert_eq!(mode.kind, ObserverModeKind::Disabled);
    assert!(!mode.is_any_observer());
    assert!(!mode.is_empire_view());
    assert!(!mode.is_omniscient());
}

/// #490: `EmpireView` is `is_any_observer()` and `is_empire_view()`,
/// not `is_omniscient()`.
#[test]
fn empire_view_predicates() {
    let mode = ObserverMode {
        kind: ObserverModeKind::EmpireView,
        ..Default::default()
    };
    assert!(mode.is_any_observer());
    assert!(mode.is_empire_view());
    assert!(!mode.is_omniscient());
}

/// #490: `Omniscient` is `is_any_observer()` and `is_omniscient()`,
/// not `is_empire_view()`.
#[test]
fn omniscient_predicates() {
    let mode = ObserverMode {
        kind: ObserverModeKind::Omniscient,
        ..Default::default()
    };
    assert!(mode.is_any_observer());
    assert!(!mode.is_empire_view());
    assert!(mode.is_omniscient());
}

// ---------------------------------------------------------------------------
// resolve_viewing_empire 3-state contract
// ---------------------------------------------------------------------------

/// #490: `Disabled` mode resolves to the `PlayerEmpire` entity (= the
/// pre-#490 path).
#[test]
fn resolve_viewing_empire_disabled_returns_player_empire() {
    let mut world = World::new();
    world.insert_resource(ObserverMode::default());
    world.insert_resource(ObserverView::default());
    let player = spawn_empire(&mut world, "Player", true);
    let _other = spawn_empire(&mut world, "Other", false);

    let resolved = macrocosmo::observer::resolve_viewing_empire(&world);
    assert_eq!(
        resolved,
        Some(player),
        "Disabled must resolve to PlayerEmpire"
    );
}

/// #490: `EmpireView` mode resolves to `ObserverView.viewing` (= the
/// observed empire, NOT the PlayerEmpire).
#[test]
fn resolve_viewing_empire_empire_view_returns_observer_view() {
    let mut world = World::new();
    world.insert_resource(ObserverMode {
        kind: ObserverModeKind::EmpireView,
        ..Default::default()
    });
    let player = spawn_empire(&mut world, "Player", true);
    let observed = spawn_empire(&mut world, "Observed", false);
    world.insert_resource(ObserverView {
        viewing: Some(observed),
    });

    let resolved = macrocosmo::observer::resolve_viewing_empire(&world);
    assert_eq!(
        resolved,
        Some(observed),
        "EmpireView must resolve to ObserverView.viewing, not PlayerEmpire {:?}",
        player
    );
}

/// #490: `Omniscient` mode resolves to `None` (= no canonical
/// per-empire perspective; caller falls through to realtime ECS).
#[test]
fn resolve_viewing_empire_omniscient_returns_none() {
    let mut world = World::new();
    world.insert_resource(ObserverMode {
        kind: ObserverModeKind::Omniscient,
        ..Default::default()
    });
    let _player = spawn_empire(&mut world, "Player", true);
    let observed = spawn_empire(&mut world, "Observed", false);
    world.insert_resource(ObserverView {
        viewing: Some(observed),
    });

    let resolved = macrocosmo::observer::resolve_viewing_empire(&world);
    assert_eq!(
        resolved, None,
        "Omniscient must return None even if ObserverView.viewing is set"
    );
}

// ---------------------------------------------------------------------------
// ui::resolve_viewing_knowledge_omniscient — the #490 hook on helper 1
// ---------------------------------------------------------------------------

/// #490 contract pin: when `omniscient = true`, the helper returns
/// `None` regardless of which empire is passed. Caller drops to
/// realtime-ECS ground truth.
///
/// (Renamed in #490 fold-in: the old name "leaks through" misread the
/// intent — the helper *correctly* returns None so the caller takes
/// the realtime fallback. "Leak" implied a contract violation.)
#[test]
fn omniscient_helper_returns_none_for_realtime_fallback() {
    let mut world = World::new();
    let observed = spawn_empire(&mut world, "Observed", false);
    let mut state: SystemState<Query<&KnowledgeStore, With<Empire>>> = SystemState::new(&mut world);
    let q = state.get(&world);

    // Confirm the empire-view path resolves to a KnowledgeStore.
    let empire_view_resolved =
        macrocosmo::ui::resolve_viewing_knowledge_omniscient(Some(observed), &q, false);
    assert!(
        empire_view_resolved.is_some(),
        "EmpireView (omniscient=false) must surface the empire's KnowledgeStore"
    );

    // Now flip the omniscient flag — the same call must collapse to
    // None so the caller knows to read realtime ECS.
    let omniscient_resolved =
        macrocosmo::ui::resolve_viewing_knowledge_omniscient(Some(observed), &q, true);
    assert!(
        omniscient_resolved.is_none(),
        "Omniscient (omniscient=true) must return None — realtime ECS path"
    );
}

// ---------------------------------------------------------------------------
// ui::empire_view_knowledge_omniscient — the #490 hook on helper 2
// ---------------------------------------------------------------------------

/// #490 contract pin: helper 2 mirrors helper 1's god-view branch.
#[test]
fn empire_view_knowledge_omniscient_branch() {
    let store = KnowledgeStore::default();

    // EmpireView path: wraps the store in Some.
    let empire_view = macrocosmo::ui::empire_view_knowledge_omniscient(&store, false);
    assert!(
        empire_view.is_some(),
        "EmpireView must surface the resolved KnowledgeStore"
    );
    assert!(std::ptr::eq(empire_view.unwrap(), &store));

    // Omniscient path: drops to None.
    let omniscient = macrocosmo::ui::empire_view_knowledge_omniscient(&store, true);
    assert!(
        omniscient.is_none(),
        "Omniscient must drop to None so the panel reads realtime ECS"
    );
}

// ---------------------------------------------------------------------------
// Disabled-mode invariant: PlayerEmpire-only resource visibility
// ---------------------------------------------------------------------------

/// #490 / #499: In `Disabled` mode the viewing empire is exclusively
/// the PlayerEmpire — observer-mode resources are not consulted. This
/// is the "PlayerEmpire のみ表示の contract pin" requested in the
/// issue.
#[test]
fn disabled_mode_only_surfaces_player_empire() {
    let mut world = World::new();
    world.insert_resource(ObserverMode::default());
    let player = spawn_empire(&mut world, "Player", true);
    let other = spawn_empire(&mut world, "Other", false);
    // Even with ObserverView pointing somewhere else, Disabled must
    // ignore it.
    world.insert_resource(ObserverView {
        viewing: Some(other),
    });

    let resolved = macrocosmo::observer::resolve_viewing_empire(&world);
    assert_eq!(
        resolved,
        Some(player),
        "Disabled must always pick PlayerEmpire, ignoring ObserverView"
    );
    assert_ne!(
        resolved,
        Some(other),
        "Disabled must NOT leak ObserverView selection"
    );
}

// ---------------------------------------------------------------------------
// Omniscient toggle round-trip (covers the ui.toggle_omniscient action)
// ---------------------------------------------------------------------------

/// #490: Toggling Omniscient on then off restores the prior kind. This
/// is the runtime invariant behind the `ui.toggle_omniscient` action.
/// (No default keybinding after #490 fold-in — see NICE-TO-FIX 5b.)
#[test]
fn omniscient_toggle_restores_prior_kind() {
    // Start from EmpireView (the most interesting case — we must not
    // collapse back to Disabled).
    let mut mode = ObserverMode {
        kind: ObserverModeKind::EmpireView,
        ..Default::default()
    };

    // Flip on via the type-safe newtype constructor.
    mode.previous_kind = NonOmniscientKind::from_observer_kind(mode.kind);
    mode.kind = ObserverModeKind::Omniscient;
    assert!(mode.is_omniscient());

    // Flip off — must restore EmpireView, not Disabled.
    let restore = mode.previous_kind.take().unwrap();
    mode.kind = restore.to_observer_kind();
    assert_eq!(mode.kind, ObserverModeKind::EmpireView);
    assert!(mode.is_empire_view());
    assert!(!mode.is_omniscient());
}

// ---------------------------------------------------------------------------
// #490 fold-in: regression tests covering the adversarial-review BLOCKERs
// (bug + design) that the fold-in PR addresses.
// ---------------------------------------------------------------------------

/// #490 fold-in (DESIGN BLOCKER 1): `NonOmniscientKind::from_observer_kind`
/// rejects `Omniscient`. This pins the type-system invariant that no
/// future code path can accidentally store `Omniscient` into
/// `ObserverMode.previous_kind` and create an
/// `Omniscient → Omniscient → Omniscient` restore loop.
#[test]
fn previous_kind_rejects_omniscient_via_newtype() {
    assert_eq!(
        NonOmniscientKind::from_observer_kind(ObserverModeKind::Disabled),
        Some(NonOmniscientKind::Disabled)
    );
    assert_eq!(
        NonOmniscientKind::from_observer_kind(ObserverModeKind::EmpireView),
        Some(NonOmniscientKind::EmpireView)
    );
    assert_eq!(
        NonOmniscientKind::from_observer_kind(ObserverModeKind::Omniscient),
        None,
        "NonOmniscientKind must reject Omniscient — see DESIGN BLOCKER 1"
    );
}

/// #490 fold-in: `Disabled → Omniscient → Disabled` round-trip via the
/// real `toggle_omniscient_mode` system, end-to-end through
/// `App::update`. The existing `omniscient_toggle_restores_prior_kind`
/// test only exercises the field mutation — this one drives the
/// Bevy system path.
#[test]
fn disabled_to_omniscient_round_trip() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.init_resource::<ButtonInput<KeyCode>>();
    app.insert_resource(ObserverMode::default());
    app.insert_resource(ObserverView::default());
    app.add_systems(
        Update,
        macrocosmo::interactions::observer_controls::toggle_omniscient_mode,
    );

    // No key pressed — stays Disabled.
    app.update();
    assert_eq!(
        app.world().resource::<ObserverMode>().kind,
        ObserverModeKind::Disabled
    );

    // Press F9 (default hardcoded fallback in toggle system).
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::F9);
    app.update();
    assert!(app.world().resource::<ObserverMode>().is_omniscient());

    // Release + clear, then press again — flips back to Disabled.
    {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(KeyCode::F9);
        keys.clear_just_pressed(KeyCode::F9);
    }
    app.update();
    // Still Omniscient (no press this frame).
    assert!(app.world().resource::<ObserverMode>().is_omniscient());

    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::F9);
    app.update();
    assert_eq!(
        app.world().resource::<ObserverMode>().kind,
        ObserverModeKind::Disabled,
        "F9 toggle must restore Disabled via NonOmniscientKind::Disabled"
    );
}

/// #490 fold-in: end-to-end `F9` press through `App::update` drives
/// `ObserverMode` state. Complements the field-mutation test by
/// pinning the keybinding-fallback wiring (no `KeybindingRegistry`
/// installed — hardcoded `F9` fallback kicks in).
#[test]
fn f9_toggle_drives_state_through_app_update() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.init_resource::<ButtonInput<KeyCode>>();
    app.insert_resource(ObserverMode {
        kind: ObserverModeKind::EmpireView,
        ..Default::default()
    });
    app.insert_resource(ObserverView::default());
    app.add_systems(
        Update,
        macrocosmo::interactions::observer_controls::toggle_omniscient_mode,
    );

    // Press F9 → Omniscient (previous_kind = EmpireView via newtype).
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::F9);
    app.update();
    {
        let mode = app.world().resource::<ObserverMode>();
        assert!(mode.is_omniscient());
        assert_eq!(mode.previous_kind, Some(NonOmniscientKind::EmpireView));
    }
}

/// #490 fold-in (BUG BLOCKER 2): `auto_pause_on_event` only drains-
/// and-bails in spawn-architecture observer mode (`EmpireView`).
/// Omniscient toggled on top of a normal session must still let
/// auto-pause fire (= a player who F9-toggles to inspect ground
/// truth doesn't want the game to stop pausing on important events).
///
/// This is verified at the predicate level — auto_pause itself
/// requires a full Bevy world with `MessageReader<GameEvent>` /
/// `PlayerEmpire` / colony queries, which the headless fixture
/// can't easily produce. The predicate is the surgical fix point.
#[test]
fn auto_pause_fires_in_omniscient_mode() {
    let omniscient = ObserverMode {
        kind: ObserverModeKind::Omniscient,
        ..Default::default()
    };
    let empire_view = ObserverMode {
        kind: ObserverModeKind::EmpireView,
        ..Default::default()
    };
    let disabled = ObserverMode::default();

    // The check inside `auto_pause_on_event` is `m.is_empire_view()`.
    // Omniscient + Disabled both *DO NOT* take the drain-and-bail
    // branch, so auto-pause runs as normal.
    assert!(
        !omniscient.is_empire_view(),
        "Omniscient must run auto-pause"
    );
    assert!(
        !disabled.is_empire_view(),
        "Disabled (normal play) must run auto-pause"
    );
    assert!(
        empire_view.is_empire_view(),
        "EmpireView is the only mode that drains"
    );
}

/// #490 fold-in (BUG BLOCKER 1): when Omniscient is toggled on top of
/// a Disabled (normal-play) session, `compute_ui_state`'s observer
/// branch MUST NOT fire — the player's resources stay populated via
/// the `PlayerEmpire` `KnowledgeStore` path. The fold-in changes the
/// branch predicate to `is_empire_view()` for exactly this reason.
///
/// Mirrors `auto_pause_fires_in_omniscient_mode`'s predicate-level
/// strategy — the inline branch is what we're pinning, not the
/// full top-bar render.
#[test]
fn compute_ui_state_preserves_resources_when_omniscient_toggled_in_single_player() {
    let omniscient = ObserverMode {
        kind: ObserverModeKind::Omniscient,
        ..Default::default()
    };
    // Omniscient does NOT take the observer branch in compute_ui_state —
    // it falls through to the PlayerEmpire path so resources stay live.
    assert!(!omniscient.is_empire_view());
    // Sanity: Disabled stays on the PlayerEmpire path too.
    let disabled = ObserverMode::default();
    assert!(!disabled.is_empire_view());
    // And EmpireView takes the dedicated observer compute path.
    let empire_view = ObserverMode {
        kind: ObserverModeKind::EmpireView,
        ..Default::default()
    };
    assert!(empire_view.is_empire_view());
}

/// #490 fold-in (NICE-TO-FIX 5d): in Omniscient mode the top-bar
/// empire selector is hidden — the bar predicate is `is_empire_view()`
/// (= the spawn-architecture observer mode), not `is_any_observer()`.
/// This prevents a player who F9-toggled Omniscient from silently
/// mutating `ObserverView.viewing` (= the `previous_kind` restore
/// target for EmpireView ↔ Omniscient flicks).
#[test]
fn selector_hidden_in_omniscient() {
    let omniscient = ObserverMode {
        kind: ObserverModeKind::Omniscient,
        ..Default::default()
    };
    assert!(
        !omniscient.is_empire_view(),
        "Omniscient hides the top-bar selector"
    );

    let empire_view = ObserverMode {
        kind: ObserverModeKind::EmpireView,
        ..Default::default()
    };
    assert!(
        empire_view.is_empire_view(),
        "EmpireView shows the selector"
    );

    let disabled = ObserverMode::default();
    assert!(
        !disabled.is_empire_view(),
        "normal play hides the selector (the top-bar omits the observer bar entirely)"
    );
}

/// #490 fold-in: `selected_vis_tier` is a three-way branch tied to
/// the observer-mode kind. Pins the per-mode tier choice so future
/// refactors don't accidentally collapse the branches.
#[test]
fn vis_tier_three_way_branch() {
    // The branch logic lives in `ui::draw_main_panels_system`:
    //
    //   if observer_mode.is_any_observer() → Local (ground-truth)
    //   else                                → per-empire visibility
    //
    // Three modes, two outcomes:
    let disabled = ObserverMode::default();
    let empire_view = ObserverMode {
        kind: ObserverModeKind::EmpireView,
        ..Default::default()
    };
    let omniscient = ObserverMode {
        kind: ObserverModeKind::Omniscient,
        ..Default::default()
    };
    assert!(
        !disabled.is_any_observer(),
        "Disabled: per-empire visibility map"
    );
    assert!(
        empire_view.is_any_observer(),
        "EmpireView: ground-truth (Local tier)"
    );
    assert!(
        omniscient.is_any_observer(),
        "Omniscient: ground-truth (Local tier)"
    );
}

/// #490 fold-in (BUG BLOCKER 4): Omniscient renders all empires'
/// ships from realtime ECS state. Verified via the
/// `collect_omniscient_ship_systems` summariser helper (= the
/// per-system count the badge layer would draw).
///
/// Two empire-A ships and one empire-B ship, both InSystem in their
/// home systems, must surface as count-1 entries in the map. The
/// projection-driven `draw_ships` path would only show empire-A's
/// ships when viewing A — Omniscient must show both.
#[test]
fn omniscient_renders_all_empires_ships() {
    use bevy::ecs::system::SystemState;
    use macrocosmo::components::Position;
    use macrocosmo::galaxy::StarSystem;
    use macrocosmo::ship::{Owner, Ship, ShipState, ShipStats};
    use macrocosmo::visualization::ships::collect_omniscient_ship_systems;

    let mut world = World::new();

    // Two empires.
    let empire_a = spawn_empire(&mut world, "EmpireA", false);
    let empire_b = spawn_empire(&mut world, "EmpireB", false);

    // Two star systems for them to inhabit.
    let sys_a = world
        .spawn((
            StarSystem {
                name: "Sys-A".into(),
                surveyed: true,
                is_capital: false,
                star_type: "yellow_dwarf".into(),
            },
            Position::from([0.0, 0.0, 0.0]),
        ))
        .id();
    let sys_b = world
        .spawn((
            StarSystem {
                name: "Sys-B".into(),
                surveyed: true,
                is_capital: false,
                star_type: "yellow_dwarf".into(),
            },
            Position::from([5.0, 5.0, 0.0]),
        ))
        .id();

    // One ship per empire (A also gets a second one to verify count).
    fn spawn_ship(world: &mut World, owner: Entity, system: Entity, name: &str) {
        world.spawn((
            Ship {
                name: name.into(),
                design_id: "explorer_mk1".into(),
                hull_id: "explorer".into(),
                modules: Vec::new(),
                owner: Owner::Empire(owner),
                sublight_speed: 1.0,
                ftl_range: 5.0,
                ruler_aboard: false,
                home_port: system,
                design_revision: 0,
                fleet: None,
            },
            ShipState::InSystem { system },
        ));
    }
    spawn_ship(&mut world, empire_a, sys_a, "A-Explorer-1");
    spawn_ship(&mut world, empire_a, sys_a, "A-Explorer-2");
    spawn_ship(&mut world, empire_b, sys_b, "B-Explorer");

    // Avoid unused-variable warnings for ShipStats import; the
    // collector takes an Option<&ShipStats> which is always None here.
    let _ = std::marker::PhantomData::<ShipStats>;

    // Run the collector inside a SystemState so the query borrows correctly.
    type ShipsQuery<'w, 's> = Query<
        'w,
        's,
        (
            Entity,
            &'static Ship,
            &'static ShipState,
            Option<&'static macrocosmo::ship::CommandQueue>,
            Option<&'static ShipStats>,
        ),
    >;
    let mut state: SystemState<ShipsQuery> = SystemState::new(&mut world);
    let ships = state.get(&world);
    let counts = collect_omniscient_ship_systems(&ships);

    assert_eq!(
        counts.get(&sys_a).copied(),
        Some(2),
        "Sys-A holds 2 empire-A ships"
    );
    assert_eq!(
        counts.get(&sys_b).copied(),
        Some(1),
        "Sys-B holds 1 empire-B ship"
    );
    assert_eq!(
        counts.len(),
        2,
        "exactly two systems have ships in god view"
    );
}

/// #490 fold-in: companion to `ship_ops_tab`'s observer test — when
/// the active mode is `Omniscient`, `resolve_viewing_empire` returns
/// `None` so the tab drops into the realtime-ECS fallback (= the
/// stated god-view contract for situation-center panels).
#[test]
fn ship_ops_tab_realtime_in_omniscient() {
    let mut world = World::new();
    world.insert_resource(ObserverMode {
        kind: ObserverModeKind::Omniscient,
        ..Default::default()
    });
    let player = spawn_empire(&mut world, "Player", true);
    let observed = spawn_empire(&mut world, "Observed", false);
    // Even with a viewing target set, Omniscient short-circuits to None.
    world.insert_resource(ObserverView {
        viewing: Some(observed),
    });
    let _ = player;

    assert_eq!(
        macrocosmo::observer::resolve_viewing_empire(&world),
        None,
        "Omniscient must return None so panels read realtime ECS"
    );
}
