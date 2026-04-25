//! Regression: Round 9 PR #2 Step 4 — NPC AI must not double-assign two
//! ships to the same survey target.
//!
//! Background: BRP `world.query` observed Vesk Scout-1 (`Surveying`) +
//! Vesk Scout-2 (`SubLight to same target with Survey in queue`) — NPC
//! AI assigned 2 ships to identical target system because
//! `idle_surveyors.iter().zip(unsurveyed_systems.iter())` doesn't dedupe
//! across overlapping AI ticks. This test pins the
//! [`PendingAssignment`](macrocosmo::ai::assignments::PendingAssignment)
//! dedup behaviour: after one tick that issues two surveys, two
//! distinct ships must be marked against two distinct targets, and a
//! second tick must not re-issue commands while both ships and targets
//! are pending.

mod common;

use bevy::prelude::*;
use macrocosmo::ai::AiPlayerMode;
use macrocosmo::ai::assignments::{AssignmentKind, AssignmentTarget, PendingAssignment};
use macrocosmo::knowledge::{KnowledgeStore, SystemVisibilityMap, SystemVisibilityTier};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::{Owner, Ship};

use common::{advance_time, spawn_test_ship, spawn_test_system, test_app};

/// Spawn a player empire driven by AI with two surveyor ships at the
/// same starting system, plus two distinct unsurveyed-but-catalogued
/// frontier systems within range.
fn setup_two_surveyors_two_targets(app: &mut App) -> (Entity, [Entity; 2], Entity, [Entity; 2]) {
    app.insert_resource(AiPlayerMode(true));

    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Vesk".into(),
            },
            PlayerEmpire,
            Faction {
                id: "vesk".into(),
                name: "Vesk".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            SystemVisibilityMap::default(),
            KnowledgeStore::default(),
        ))
        .id();

    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let frontier_a = spawn_test_system(
        app.world_mut(),
        "Frontier-A",
        [3.0, 0.0, 0.0],
        1.0,
        false,
        false,
    );
    let frontier_b = spawn_test_system(
        app.world_mut(),
        "Frontier-B",
        [0.0, 3.0, 0.0],
        1.0,
        false,
        false,
    );

    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut vis = em.get_mut::<SystemVisibilityMap>().unwrap();
        vis.set(home, SystemVisibilityTier::Local);
        vis.set(frontier_a, SystemVisibilityTier::Catalogued);
        vis.set(frontier_b, SystemVisibilityTier::Catalogued);
    }

    let scout1 = spawn_test_ship(
        app.world_mut(),
        "Scout-1",
        "explorer_mk1",
        home,
        [0.0, 0.0, 0.0],
    );
    let scout2 = spawn_test_ship(
        app.world_mut(),
        "Scout-2",
        "explorer_mk1",
        home,
        [0.0, 0.0, 0.0],
    );
    for ship in [scout1, scout2] {
        app.world_mut()
            .entity_mut(ship)
            .get_mut::<Ship>()
            .unwrap()
            .owner = Owner::Empire(empire);
    }

    (empire, [scout1, scout2], home, [frontier_a, frontier_b])
}

/// Drive the AI long enough for `npc_decision_tick` to emit
/// `survey_system` commands, `drain_ai_commands` to translate them into
/// `SurveyRequested` events + `PendingAssignment` markers, and
/// `handle_survey_requested` to absorb the events.
fn drive_ai(app: &mut App, ticks: i64) {
    use macrocosmo::ship::command_events::SurveyRequested;
    // Required by Bevy's message reader bookkeeping in headless tests.
    app.world_mut()
        .resource_mut::<Messages<SurveyRequested>>()
        .update();
    for _ in 0..ticks {
        advance_time(app, 1);
    }
}

fn collect_pending(app: &mut App, empire: Entity) -> Vec<(Entity, Entity)> {
    let mut q = app.world_mut().query::<(Entity, &PendingAssignment)>();
    q.iter(app.world())
        .filter(|(_, pa)| pa.faction == empire && pa.kind == AssignmentKind::Survey)
        .map(|(ship, pa)| {
            let target = match pa.target {
                AssignmentTarget::System(e) => e,
            };
            (ship, target)
        })
        .collect()
}

#[test]
fn ai_does_not_double_assign_two_ships_to_same_survey_target() {
    let mut app = test_app();
    let (empire, scouts, _home, frontiers) = setup_two_surveyors_two_targets(&mut app);

    drive_ai(&mut app, 5);

    let pending = collect_pending(&mut app, empire);

    assert!(
        !pending.is_empty(),
        "expected at least one PendingAssignment after AI tick (got 0)",
    );

    // Each marker must point to one of the spawned ships and one of the
    // spawned frontiers.
    for (ship, target) in &pending {
        assert!(
            scouts.contains(ship),
            "PendingAssignment on unexpected ship: {:?}",
            ship,
        );
        assert!(
            frontiers.contains(target),
            "PendingAssignment targets unexpected system: {:?}",
            target,
        );
    }

    // No two markers may share the same target. (Two markers may share
    // the same ship only if the ship was re-assigned mid-test, which
    // shouldn't happen here.)
    let mut seen_targets = std::collections::HashSet::new();
    let mut seen_ships = std::collections::HashSet::new();
    for (ship, target) in &pending {
        assert!(
            seen_targets.insert(*target),
            "two PendingAssignments share target {:?} — double-assignment regression",
            target,
        );
        assert!(
            seen_ships.insert(*ship),
            "two PendingAssignments share ship {:?}",
            ship,
        );
    }
}

#[test]
fn ai_second_tick_does_not_re_emit_when_all_ships_and_targets_are_pending() {
    let mut app = test_app();
    let (empire, _scouts, _home, _frontiers) = setup_two_surveyors_two_targets(&mut app);

    // First batch of ticks: AI dispatches surveyors.
    drive_ai(&mut app, 5);
    let first_pending = collect_pending(&mut app, empire);
    assert!(
        !first_pending.is_empty(),
        "expected initial dispatches before second-tick check",
    );

    // Snapshot the (ship, target) pairs that already have markers — the
    // dedup contract is: once a ship is marked, no *new* survey commands
    // for *new* (ship, target) pairs may be emitted while both sides
    // remain pending. (Markers may legitimately be cleared by terminal
    // handler results between ticks; the test only asserts no new
    // assignments appear.)
    let first_set: std::collections::HashSet<(Entity, Entity)> =
        first_pending.iter().copied().collect();

    // Drive a second batch.
    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    let second_pending = collect_pending(&mut app, empire);

    // No (ship, target) pair should appear in the second snapshot that
    // wasn't in the first — equivalent to "no new dispatches happened
    // while the previous ones are still pending" because the marker
    // *only* appears at dispatch time and the handler can only remove,
    // never re-add, it.
    let new_pairs: Vec<_> = second_pending
        .iter()
        .filter(|pair| !first_set.contains(pair))
        .collect();

    assert!(
        new_pairs.is_empty(),
        "new PendingAssignments appeared on a tick where all ships should be \
         busy (first={:?}, new={:?}) — double-assignment regression",
        first_pending,
        new_pairs,
    );
}

#[test]
fn sweep_stale_assignments_removes_expired_markers() {
    use macrocosmo::ai::assignments::sweep_stale_assignments;
    use macrocosmo::time_system::GameClock;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(GameClock::new(0));
    app.add_systems(Update, sweep_stale_assignments);

    let faction = Entity::from_raw_u32(1).unwrap();
    let target = Entity::from_raw_u32(99).unwrap();
    let ship = app
        .world_mut()
        .spawn(PendingAssignment::survey_system(faction, target, 0, 30))
        .id();

    // Tick at hexadies = 10 — marker is still valid (stale_at = 30).
    app.world_mut().resource_mut::<GameClock>().elapsed = 10;
    app.update();
    assert!(
        app.world().get::<PendingAssignment>(ship).is_some(),
        "marker swept too early"
    );

    // Tick at hexadies = 31 — marker should be swept.
    app.world_mut().resource_mut::<GameClock>().elapsed = 31;
    app.update();
    assert!(
        app.world().get::<PendingAssignment>(ship).is_none(),
        "marker survived past stale_at"
    );
}
