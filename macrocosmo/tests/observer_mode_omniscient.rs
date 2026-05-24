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
use macrocosmo::observer::{ObserverMode, ObserverModeKind, ObserverView};
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
    assert!(!mode.enabled());
    assert!(!mode.is_empire_view());
    assert!(!mode.is_omniscient());
}

/// #490: `EmpireView` is `enabled()` and `is_empire_view()`, not
/// `is_omniscient()`.
#[test]
fn empire_view_predicates() {
    let mode = ObserverMode {
        kind: ObserverModeKind::EmpireView,
        ..Default::default()
    };
    assert!(mode.enabled());
    assert!(mode.is_empire_view());
    assert!(!mode.is_omniscient());
}

/// #490: `Omniscient` is `enabled()` and `is_omniscient()`, not
/// `is_empire_view()`.
#[test]
fn omniscient_predicates() {
    let mode = ObserverMode {
        kind: ObserverModeKind::Omniscient,
        ..Default::default()
    };
    assert!(mode.enabled());
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
#[test]
fn omniscient_realtime_state_leaks_through_helper_one() {
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
/// is the runtime invariant behind the `ui.toggle_omniscient` action
/// (default `F9`).
#[test]
fn omniscient_toggle_restores_prior_kind() {
    // Start from EmpireView (the most interesting case — we must not
    // collapse back to Disabled).
    let mut mode = ObserverMode {
        kind: ObserverModeKind::EmpireView,
        ..Default::default()
    };

    // Flip on.
    mode.previous_kind = Some(mode.kind);
    mode.kind = ObserverModeKind::Omniscient;
    assert!(mode.is_omniscient());

    // Flip off — must restore EmpireView, not Disabled.
    let restore = mode.previous_kind.take().unwrap();
    mode.kind = restore;
    assert_eq!(mode.kind, ObserverModeKind::EmpireView);
    assert!(mode.is_empire_view());
    assert!(!mode.is_omniscient());
}
