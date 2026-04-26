//! Regression: Round 9 PR #2 Step 4 + follow-up commit `7ad059a` —
//! NPC AI must not double-assign two ships to the same survey target,
//! and the [`PendingAssignment`](macrocosmo::ai::assignments::PendingAssignment)
//! marker must persist until the issuing empire's `KnowledgeStore`
//! reflects the survey result (knowledge-driven cleanup), not the
//! moment the handler dispatches the command.
//!
//! Background: BRP `world.query` observed Vesk Scout-1 (`Surveying`) +
//! Vesk Scout-2 (`SubLight to same target with Survey in queue`) — NPC
//! AI assigned 2 ships to identical target system because
//! `idle_surveyors.iter().zip(unsurveyed_systems.iter())` doesn't dedupe
//! across overlapping AI ticks. The eager-remove fix was sound for the
//! same-tick race but too aggressive for the cross-tick race: the
//! handler `Ok` arm cleared the marker the moment it fired, leaving the
//! window open between dispatch and the success fact propagating back
//! to the issuing empire. The fix moved the cleanup to a knowledge-
//! driven sweep that watches the issuing empire's `KnowledgeStore`.
//!
//! Coverage:
//!
//! - `ai_does_not_double_assign_two_ships_to_same_survey_target`
//!   (scenario A, in-flight dedup): one AI tick must mark **two
//!   distinct ships** against **two distinct frontier targets** —
//!   no shared (ship) or shared (target) across the two markers.
//! - `ai_second_tick_does_not_re_emit_when_all_ships_and_targets_are_pending`
//!   (scenario A continued): a second batch of ticks while every
//!   surveyor is still in flight must not introduce any *new* (ship,
//!   target) pairs — equivalent to "no second dispatch while the
//!   first is still pending."
//! - `pending_assignment_outlives_handler_ok_until_knowledge_arrives`
//!   (scenario B, lifetime contract): drive a survey to the
//!   `Surveying` state and verify the marker is **still attached**
//!   after the handler `Ok` arm fired — the eager-remove regression
//!   would clear it. Then directly populate the empire's
//!   `KnowledgeStore` with `surveyed = true` for the target and one
//!   more tick must let `sweep_resolved_survey_assignments` clear it.
//! - `sweep_stale_assignments_removes_expired_markers`: the time-based
//!   fallback sweeper still works on its own.

mod common;

use bevy::prelude::*;
use macrocosmo::ai::AiPlayerMode;
use macrocosmo::ai::assignments::{AssignmentKind, AssignmentTarget, PendingAssignment};
use macrocosmo::knowledge::{
    KnowledgeStore, ObservationSource, SystemKnowledge, SystemSnapshot, SystemVisibilityMap,
    SystemVisibilityTier,
};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::{Owner, Ship, ShipState};

use common::{advance_time, spawn_test_ship, spawn_test_system, test_app};

/// Spawn a fresh AI-controlled empire with two scout ships at a shared
/// home and `n_targets` distinct unsurveyed-but-catalogued frontier
/// systems within range.
///
/// Crucially, the empire's `KnowledgeStore` is seeded so that **home is
/// already known surveyed** from this empire's perspective. Without
/// this, `npc_decision_tick` would treat home itself as a candidate
/// survey target (it derives `surveyed_ids` from the store, not from
/// the visibility map), and the AI would dispatch a surveyor onto its
/// own dock — see the handoff note in `docs/session-handoff-2026-04-26-round-7-9.md`.
fn setup_surveyors(
    app: &mut App,
    n_ships: usize,
    n_targets: usize,
) -> (Entity, Vec<Entity>, Entity, Vec<Entity>) {
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

    // Spread frontiers along the unit circle in XY at ~0.5 LY so they're
    // close enough that a sublight leg from home reaches them in a
    // handful of hexadies (FTL routing rejects unsurveyed destinations
    // — see the FTL gate in `plan_ftl_route` — so the surveyor is
    // forced onto sublight). With `corvette` hull base_speed = 0.75
    // LY/year and `HEXADIES_PER_YEAR = 60`, 0.5 LY ≈ 40 hexadies one
    // way; close enough to drive the full Surveying state inside a
    // bounded loop in `pending_assignment_outlives_handler_ok_*`.
    let mut frontiers = Vec::with_capacity(n_targets);
    for i in 0..n_targets {
        let theta = (i as f64) * std::f64::consts::TAU / (n_targets as f64);
        let pos = [0.5 * theta.cos(), 0.5 * theta.sin(), 0.0];
        let f = spawn_test_system(
            app.world_mut(),
            &format!("Frontier-{}", i),
            pos,
            1.0,
            false,
            false,
        );
        frontiers.push(f);
    }

    // Visibility tiers — home is Local (we're stationed here), each
    // frontier is Catalogued (we know it exists but haven't surveyed).
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut vis = em.get_mut::<SystemVisibilityMap>().unwrap();
        vis.set(home, SystemVisibilityTier::Local);
        for &f in &frontiers {
            vis.set(f, SystemVisibilityTier::Catalogued);
        }
    }

    // Seed the empire's KnowledgeStore so home registers as surveyed
    // for *this* empire's perspective. Without this, `npc_decision_tick`
    // would mistake home for an unsurveyed candidate and dispatch a
    // surveyor onto its own dock. (`spawn_test_system` only sets
    // `star.surveyed = true` on the global ground truth, plus
    // `SystemVisibilityTier::Surveyed` on every empire's vis_map; the
    // KnowledgeStore is intentionally left empty.)
    let home_pos = app
        .world()
        .entity(home)
        .get::<macrocosmo::components::Position>()
        .unwrap()
        .as_array();
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update(SystemKnowledge {
            system: home,
            observed_at: 0,
            received_at: 0,
            data: SystemSnapshot {
                name: "Home".into(),
                position: home_pos,
                surveyed: true,
                ..Default::default()
            },
            source: ObservationSource::Direct,
        });
    }

    let mut ships = Vec::with_capacity(n_ships);
    for i in 0..n_ships {
        let s = spawn_test_ship(
            app.world_mut(),
            &format!("Scout-{}", i),
            "explorer_mk1",
            home,
            [0.0, 0.0, 0.0],
        );
        app.world_mut()
            .entity_mut(s)
            .get_mut::<Ship>()
            .unwrap()
            .owner = Owner::Empire(empire);
        ships.push(s);
    }

    (empire, ships, home, frontiers)
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

fn collect_pending_for(app: &mut App, empire: Entity) -> Vec<(Entity, Entity)> {
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

/// Scenario A (in-flight dedup): with two scouts and two distinct
/// frontier targets, one AI tick must produce two `PendingAssignment`
/// markers — one per ship, one per target — and never both ships
/// against the same target.
///
/// Under the new (`7ad059a`) lifetime semantics the markers persist
/// well past the dispatch tick, so we can read them at any point
/// before the surveyor returns. Under the *old* eager-remove
/// semantics, the markers vanished the same tick they appeared and
/// this assertion's window was effectively empty — which is why the
/// pre-rewrite version of this test sat under `#[ignore]`.
#[test]
fn ai_does_not_double_assign_two_ships_to_same_survey_target() {
    let mut app = test_app();
    let (empire, scouts, _home, frontiers) = setup_surveyors(&mut app, 2, 2);

    drive_ai(&mut app, 5);

    let pending = collect_pending_for(&mut app, empire);

    assert_eq!(
        pending.len(),
        2,
        "expected exactly two PendingAssignments after AI tick (one per scout); got {}: {:?}",
        pending.len(),
        pending,
    );

    // Each marker must point to one of the spawned ships and one of the
    // spawned frontiers — confirms we're picking from the intended
    // candidate pool, not a phantom system.
    for (ship, target) in &pending {
        assert!(
            scouts.contains(ship),
            "PendingAssignment on unexpected ship: {:?} (expected one of {:?})",
            ship,
            scouts,
        );
        assert!(
            frontiers.contains(target),
            "PendingAssignment targets unexpected system: {:?} (expected one of {:?})",
            target,
            frontiers,
        );
    }

    // No two markers may share the same target, and no two markers may
    // share the same ship — this is the actual double-assignment
    // contract.
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

/// Scenario A (continued, cross-tick dedup): once both surveyors are
/// already in flight against the two known frontiers, a second batch
/// of decision ticks must not introduce *new* (ship, target) pairs.
///
/// This is the same guarantee as the test above but framed against
/// the second tick: the dedup contract is "while a marker covers a
/// (ship, target) pair, the AI does not re-emit a survey command for
/// any pair where either side is still pending."
#[test]
fn ai_second_tick_does_not_re_emit_when_all_ships_and_targets_are_pending() {
    let mut app = test_app();
    let (empire, _scouts, _home, _frontiers) = setup_surveyors(&mut app, 2, 2);

    // First batch of ticks: AI dispatches surveyors.
    drive_ai(&mut app, 5);
    let first_pending = collect_pending_for(&mut app, empire);
    assert!(
        !first_pending.is_empty(),
        "expected initial dispatches before second-tick check",
    );

    // Snapshot the (ship, target) pairs that already have markers —
    // the dedup contract is: once a ship is marked, no *new* survey
    // commands for *new* (ship, target) pairs may be emitted while
    // both sides remain pending. Markers may legitimately be cleared
    // by terminal handler results between ticks; the test only asserts
    // no new assignments appear.
    let first_set: std::collections::HashSet<(Entity, Entity)> =
        first_pending.iter().copied().collect();

    // Drive a second batch.
    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    let second_pending = collect_pending_for(&mut app, empire);

    // No (ship, target) pair should appear in the second snapshot that
    // wasn't in the first — equivalent to "no new dispatches happened
    // while the previous ones are still pending." Under the new
    // knowledge-driven cleanup the markers also can't have *expired*
    // between snapshots (KnowledgeStore was never updated), so we
    // additionally expect `second_pending ⊇ first_set`.
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

    let second_set: std::collections::HashSet<(Entity, Entity)> =
        second_pending.iter().copied().collect();
    assert!(
        first_set.is_subset(&second_set),
        "PendingAssignment vanished without KnowledgeStore update — \
         eager-remove regression (first={:?}, second={:?})",
        first_pending,
        second_pending,
    );
}

/// Scenario B (lifetime contract): the handler `Ok` arm must NOT
/// remove `PendingAssignment` — the marker is the NPC's decision
/// memory and outlives the dispatch. Removal is driven by
/// `sweep_resolved_survey_assignments`, which watches the issuing
/// empire's `KnowledgeStore` for `surveyed = true` on the target.
///
/// This pins the post-`7ad059a` lifetime semantics: under the old
/// eager-remove rule the assertion at the `Surveying`-state
/// checkpoint would fail because the marker disappeared the same
/// tick the handler fired.
#[test]
fn pending_assignment_outlives_handler_ok_until_knowledge_arrives() {
    let mut app = test_app();
    let (empire, scouts, _home, frontiers) = setup_surveyors(&mut app, 1, 1);
    let scout = scouts[0];
    let frontier = frontiers[0];

    // Drive enough ticks for the AI to dispatch *and* the handler to
    // have run its `Ok` arm. Sequence per tick (from `test_app()`
    // schedule):
    //   1. npc_decision_tick → emit_command(survey_system)
    //   2. drain_ai_commands → write SurveyRequested + insert
    //      PendingAssignment
    //   3. handle_survey_requested → reads SurveyRequested. Ship is
    //      docked at home, target is frontier — first hit auto-inserts
    //      MoveTo + Survey into the queue (Deferred). After the move
    //      completes the Survey command is dispatched again; on a
    //      subsequent tick the ship is now docked at the target and
    //      the handler takes the `Ok` branch (transitions to
    //      Surveying).
    //
    // Five ticks isn't always enough to reach the FTL move +
    // Survey-Ok branch (route planning is async via `PendingRoute`),
    // so we drive in a small loop and check for the `Surveying` state
    // each step.
    let mut reached_surveying = false;
    for _ in 0..120 {
        drive_ai(&mut app, 1);
        let state = app.world().entity(scout).get::<ShipState>().unwrap();
        if matches!(state, ShipState::Surveying { .. }) {
            reached_surveying = true;
            break;
        }
    }
    assert!(
        reached_surveying,
        "scout never reached Surveying state — handler Ok branch did not fire",
    );

    // Eager-remove regression check: the marker MUST still be on the
    // ship after the handler's `Ok` arm fired. In the old semantics
    // this assertion failed because `handle_survey_requested` cleared
    // the marker on success.
    let marker = app.world().entity(scout).get::<PendingAssignment>();
    assert!(
        marker.is_some(),
        "PendingAssignment was removed when handler returned Ok — \
         eager-remove regression (lifetime should extend until \
         KnowledgeStore reflects surveyed=true)",
    );
    let marker = marker.unwrap();
    assert_eq!(marker.faction, empire);
    assert_eq!(marker.kind, AssignmentKind::Survey);
    assert!(matches!(
        marker.target,
        AssignmentTarget::System(t) if t == frontier
    ));

    // Now simulate the success fact arriving at the issuing empire by
    // directly mutating its KnowledgeStore. (End-to-end, this happens
    // when the FTL ship physically docks back at home and
    // `deliver_survey_results` writes through to the owner's store —
    // see `515e7cf`. We bypass that here to keep the test focused on
    // the sweep contract; the full chain is exercised by the
    // integration tests in `tests/ship_survey_*.rs`.)
    let frontier_pos = app
        .world()
        .entity(frontier)
        .get::<macrocosmo::components::Position>()
        .unwrap()
        .as_array();
    let now = app
        .world()
        .resource::<macrocosmo::time_system::GameClock>()
        .elapsed;
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update(SystemKnowledge {
            system: frontier,
            observed_at: now,
            received_at: now,
            data: SystemSnapshot {
                name: "Frontier-0".into(),
                position: frontier_pos,
                surveyed: true,
                ..Default::default()
            },
            source: ObservationSource::Direct,
        });
    }

    // One more tick — `sweep_resolved_survey_assignments` runs in the
    // `AiTickSet::CommandDrain` set on every Update and should now
    // notice the issuing empire knows the target is surveyed.
    advance_time(&mut app, 1);

    let marker = app.world().entity(scout).get::<PendingAssignment>();
    assert!(
        marker.is_none(),
        "PendingAssignment was not swept after KnowledgeStore reported \
         surveyed=true on the target — sweep_resolved_survey_assignments \
         regression",
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
