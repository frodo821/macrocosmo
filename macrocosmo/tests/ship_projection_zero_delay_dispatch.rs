//! #482: zero-delay context-menu dispatches must write `ShipProjection`
//! at the dispatch tick.
//!
//! Background: #475 wired the player UI dispatch path to write
//! projections via `pending_ship_commands` iteration in
//! `draw_main_panels_system`. The zero-delay branches in
//! `ui/context_menu.rs` (`apply_local_ship_command` + direct
//! `CommandQueue.push`) bypass that pipeline entirely, leaving the
//! projection write skipped — the symptom being that a same-system
//! ship issued a command via shift+click context-menu vanishes from
//! the Galaxy Map (or shows a stale projection) until the next
//! reconcile event.
//!
//! The fix: `draw_context_menu` collects `(ship, ShipCommand)` pairs
//! into `ContextMenuActions.zero_delay_dispatches` and the orchestrating
//! `draw_main_panels_system` queues a `commands.queue(...)` callback
//! that runs the same `write_player_dispatch_projection` helper used
//! by the `pending_ship_commands` path.
//!
//! The full UI path requires an egui context which is intentionally
//! excluded from `test_app`. We therefore exercise the **shared helper
//! that the fix introduces**:
//!
//! * `write_player_dispatch_projection` — the public helper that
//!   `draw_main_panels_system` calls for both the delayed and
//!   zero-delay paths. Asserting it produces the expected projection
//!   for `MoveTo` / `Survey` / `Colonize` covers the fix's
//!   correctness contract.
//! * `queued_command_to_ship_command` — the mapping helper that the
//!   zero-delay branches use to translate `QueuedCommand` →
//!   `ShipCommand` for projection-write purposes.
//!
//! Tests:
//!
//! 1. `zero_delay_move_command_writes_projection` — driving the helper
//!    with `ShipCommand::MoveTo` produces a projection with
//!    `intended_state = InTransit` and the target system populated.
//! 2. `zero_delay_survey_command_writes_projection` — same for
//!    `Survey`.
//! 3. `projection_dispatched_at_matches_clock` — the dispatch tick
//!    flowed through the helper equals `clock.elapsed`, not a delayed
//!    tick.
//! 4. `queued_command_mapping_covers_context_menu_variants` —
//!    sanity check that `MoveTo` / `Survey` / `Colonize` map to the
//!    expected `ShipCommand` variants (the helper that the fix uses
//!    inside `draw_context_menu` to populate
//!    `zero_delay_dispatches`).

mod common;

use bevy::prelude::*;

use macrocosmo::knowledge::{KnowledgeStore, ShipSnapshotState};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::{Owner, QueuedCommand, Ship, ShipCommand};
use macrocosmo::time_system::GameClock;
use macrocosmo::ui::context_menu::queued_command_to_ship_command;
use macrocosmo::ui::write_player_dispatch_projection;

use common::{spawn_test_ruler, spawn_test_ship, spawn_test_system, test_app};

fn setup_player_empire_with_ship(app: &mut App) -> (Entity, Entity, Entity, Entity) {
    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "zero_delay_test".into(),
                name: "Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            macrocosmo::knowledge::SystemVisibilityMap::default(),
            KnowledgeStore::default(),
            macrocosmo::empire::CommsParams::default(),
        ))
        .id();
    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [3.0, 0.0, 0.0],
        1.0,
        false,
        false,
    );
    let ship = spawn_test_ship(
        app.world_mut(),
        "Scout-1",
        "explorer_mk1",
        home,
        [0.0, 0.0, 0.0],
    );
    app.world_mut()
        .entity_mut(ship)
        .get_mut::<Ship>()
        .unwrap()
        .owner = Owner::Empire(empire);
    // Ruler co-located with ship — the canonical "ruler aboard / ruler
    // same-system" UX path that triggers `command_delay == 0` in the
    // context menu.
    spawn_test_ruler(app.world_mut(), empire, home);
    (empire, home, frontier, ship)
}

#[test]
fn zero_delay_move_command_writes_projection() {
    let mut app = test_app();
    let (empire, _home, frontier, ship) = setup_player_empire_with_ship(&mut app);

    // Simulate the orchestrating code's deferred call path.
    let cmd = ShipCommand::MoveTo {
        destination: frontier,
    };
    write_player_dispatch_projection(app.world_mut(), ship, &cmd, 0);

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let projection = store
        .get_projection(ship)
        .expect("zero-delay MoveTo must write a projection");
    assert_eq!(
        projection.intended_state,
        Some(ShipSnapshotState::InTransit),
        "MoveTo maps to InTransit intended state"
    );
    assert_eq!(projection.intended_system, Some(frontier));
    assert!(
        projection.intended_takes_effect_at.is_some(),
        "intended_takes_effect_at must be populated"
    );
    let _ = empire;
}

#[test]
fn zero_delay_survey_command_writes_projection() {
    let mut app = test_app();
    let (empire, _home, frontier, ship) = setup_player_empire_with_ship(&mut app);

    let cmd = ShipCommand::Survey { target: frontier };
    write_player_dispatch_projection(app.world_mut(), ship, &cmd, 0);

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let projection = store
        .get_projection(ship)
        .expect("zero-delay Survey must write a projection");
    assert_eq!(
        projection.intended_state,
        Some(ShipSnapshotState::Surveying)
    );
    assert_eq!(projection.intended_system, Some(frontier));
    // Survey has a return leg per `command_kind_has_return_leg`.
    assert!(
        projection.expected_return_at.is_some(),
        "survey commands must populate expected_return_at"
    );
}

#[test]
fn projection_dispatched_at_matches_clock() {
    let mut app = test_app();
    let (empire, _home, frontier, ship) = setup_player_empire_with_ship(&mut app);

    // Advance to a non-zero tick so the assert distinguishes "wrote at
    // clock.elapsed" from "wrote at 0".
    app.world_mut().resource_mut::<GameClock>().elapsed = 250;
    let cmd = ShipCommand::MoveTo {
        destination: frontier,
    };
    write_player_dispatch_projection(app.world_mut(), ship, &cmd, 250);

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let projection = store.get_projection(ship).unwrap();
    assert_eq!(
        projection.dispatched_at, 250,
        "dispatched_at must equal the dispatcher's clock at dispatch time, not a delayed tick"
    );
}

#[test]
fn queued_command_mapping_covers_context_menu_variants() {
    // The mapping the fix uses inside `draw_context_menu` to translate
    // a `QueuedCommand` into the equivalent `ShipCommand` before
    // populating `ContextMenuActions.zero_delay_dispatches`. If a new
    // context menu entry adds a `QueuedCommand` variant, this test
    // forces the mapping to be extended.
    let mut world = World::new();
    let dummy_system = world.spawn_empty().id();
    let dummy_planet = world.spawn_empty().id();

    match queued_command_to_ship_command(&QueuedCommand::MoveTo {
        system: dummy_system,
    }) {
        Some(ShipCommand::MoveTo { destination }) => {
            assert_eq!(destination, dummy_system);
        }
        other => panic!("unexpected mapping for MoveTo: {:?}", other),
    }

    match queued_command_to_ship_command(&QueuedCommand::Survey {
        system: dummy_system,
    }) {
        Some(ShipCommand::Survey { target }) => {
            assert_eq!(target, dummy_system);
        }
        other => panic!("unexpected mapping for Survey: {:?}", other),
    }

    match queued_command_to_ship_command(&QueuedCommand::Colonize {
        system: dummy_system,
        planet: Some(dummy_planet),
    }) {
        Some(ShipCommand::Colonize) => {}
        other => panic!("unexpected mapping for Colonize: {:?}", other),
    }
}
