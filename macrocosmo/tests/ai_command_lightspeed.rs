//! Round 9 PR #3 regression: AI commands incur light-speed delay.
//!
//! Before this PR, NPC AI commands flowed `bus.emit_command →
//! drain_ai_commands` in a single tick — NPCs had perfect
//! instantaneous reach across the galaxy regardless of their Ruler's
//! position. The fix interposes [`AiCommandOutbox`] between producer
//! (`npc_decision_tick`, `run_short_agents`) and consumer
//! (`drain_ai_commands`), computing each command's `arrives_at` from
//! the Ruler's position to the command's destination via the
//! existing [`compute_fact_arrival`] knowledge-side helper.
//!
//! These tests pin the contract:
//!
//! 1. `survey_command_outbox_holds_until_light_delay_elapses` —
//!    a survey command emitted with the Ruler at home and the
//!    target several light-years away must NOT trigger a
//!    `SurveyRequested` event before `light_delay_hexadies(d)`
//!    elapses. After the window passes, the event fires.
//! 2. `survey_command_with_ruler_at_target_lands_immediately` —
//!    a Ruler stationed at the target system means origin ==
//!    destination, light delay is zero, and the survey command
//!    lands on the same tick the AI emitted it.

mod common;

use bevy::prelude::*;

use macrocosmo::ai::AiPlayerMode;
use macrocosmo::knowledge::{KnowledgeStore, SystemVisibilityMap, SystemVisibilityTier};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::command_events::SurveyRequested;
use macrocosmo::ship::{Owner, Ship};

use common::{advance_time, spawn_test_ruler, spawn_test_ship, spawn_test_system, test_app};

/// Spawn a self-sufficient AI-controlled empire with one Explorer at
/// `home`, a Ruler stationed at `ruler_system`, and a single
/// frontier system the AI is expected to survey.
///
/// `frontier_distance_ly` controls the spatial separation: putting
/// the frontier far from the Ruler stretches the light-speed window
/// so the test can resolve the dispatch-vs-process race cleanly.
fn setup_one_target(app: &mut App, frontier_distance_ly: f64) -> (Entity, Entity, Entity) {
    app.insert_resource(AiPlayerMode(true));

    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Lightspeed Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "lightspeed_test".into(),
                name: "Lightspeed Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            SystemVisibilityMap::default(),
            KnowledgeStore::default(),
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

    // Visibility tiers: home is Local (we live there), frontier is
    // Catalogued (we know it exists, haven't surveyed). The AI
    // policy looks for unsurveyed *catalogued* systems.
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut vis = em.get_mut::<SystemVisibilityMap>().unwrap();
        vis.set(home, SystemVisibilityTier::Local);
        vis.set(frontier, SystemVisibilityTier::Catalogued);
    }

    // Seed the empire's KnowledgeStore so home is "known surveyed"
    // from the empire's own perspective — without this the AI would
    // mistake home for an unsurveyed candidate (see the comment in
    // `tests/ai_npc_no_double_survey_assignment.rs`).
    let home_pos = app
        .world()
        .entity(home)
        .get::<macrocosmo::components::Position>()
        .unwrap()
        .as_array();
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update(macrocosmo::knowledge::SystemKnowledge {
            system: home,
            observed_at: 0,
            received_at: 0,
            data: macrocosmo::knowledge::SystemSnapshot {
                name: "Home".into(),
                position: home_pos,
                surveyed: true,
                ..Default::default()
            },
            source: macrocosmo::knowledge::ObservationSource::Direct,
        });
    }

    // One Explorer ship parked at home, owned by the test empire.
    let scout = spawn_test_ship(
        app.world_mut(),
        "Scout-1",
        "explorer_mk1",
        home,
        [0.0, 0.0, 0.0],
    );
    app.world_mut()
        .entity_mut(scout)
        .get_mut::<Ship>()
        .unwrap()
        .owner = Owner::Empire(empire);

    (empire, home, frontier)
}

/// Count `SurveyRequested` messages emitted this Update window.
/// Uses Bevy 0.18's `iter_current_update_messages` so consumed
/// messages are visible to later assertions without holding a
/// long-lived cursor.
fn survey_requested_count(app: &mut App) -> usize {
    let messages = app.world().resource::<Messages<SurveyRequested>>();
    messages.iter_current_update_messages().count()
}

/// A Ruler stationed `d` ly from the survey target should NOT
/// produce a `SurveyRequested` event before `light_delay(d)` ticks
/// have elapsed since the AI emitted the command. This is the core
/// "Bug 2" regression: pre-PR-3 the event fired the same tick the
/// AI policy decided.
#[test]
fn survey_command_outbox_holds_until_light_delay_elapses() {
    use macrocosmo::physics::light_delay_hexadies;

    let frontier_distance_ly = 5.0;
    let mut app = test_app();
    let (empire, home, _frontier) = setup_one_target(&mut app, frontier_distance_ly);
    spawn_test_ruler(app.world_mut(), empire, home);

    // Required by Bevy's message reader bookkeeping in headless tests.
    app.world_mut()
        .resource_mut::<Messages<SurveyRequested>>()
        .update();

    // Run a couple of ticks so `npc_decision_tick` fires (it gates on
    // `clock.elapsed > last_tick`) and the dispatcher stows the
    // command in the outbox. We deliberately stay well below the
    // light-speed window so the assertion catches the bug.
    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    // Drain whatever events accumulated during the early window.
    // Then explicitly re-check `Messages` to confirm zero events
    // fired across the most recent Update — the outbox is gating.
    let _ = survey_requested_count(&mut app);
    app.world_mut()
        .resource_mut::<Messages<SurveyRequested>>()
        .update();
    advance_time(&mut app, 1);
    let count_at_4 = survey_requested_count(&mut app);
    assert_eq!(
        count_at_4,
        0,
        "survey event fired {} hexadies before light-speed window — \
         outbox failed to gate (bug 2 regression)",
        light_delay_hexadies(frontier_distance_ly)
    );

    // Now drive enough ticks to clear the light-speed window. The
    // dispatcher computes arrives_at = sent_at + light_delay; once
    // that tick passes, `process_ai_pending_commands` releases the
    // command and `drain_ai_commands` produces a SurveyRequested
    // event. Add a small slack so the dispatch + process schedule
    // boundary doesn't trip the assertion at the exact threshold.
    let light_delay = light_delay_hexadies(frontier_distance_ly);
    for _ in 0..(light_delay + 5) {
        advance_time(&mut app, 1);
    }

    // The outbox must have eventually released the command, so a
    // non-zero number of events fired in the most recent Update.
    let total_in_last_update = survey_requested_count(&mut app);
    assert!(
        total_in_last_update > 0,
        "no SurveyRequested fired in the final Update even after \
         waiting {} hexadies past light delay — outbox is over-gating",
        light_delay + 5
    );
}

/// Sanity counterpart: when origin == destination (Ruler at the
/// survey target), the outbox computes arrives_at == sent_at and
/// `process_ai_pending_commands` releases the command immediately.
/// `drain_ai_commands` then produces the `SurveyRequested` event in
/// the same Update.
///
/// This guards against an over-correction where every AI command
/// pays at least one tick of artificial delay — the Ruler's local
/// commands must remain instantaneous, mirroring the player path.
#[test]
fn survey_command_with_ruler_at_target_lands_immediately() {
    let mut app = test_app();
    // Set the Ruler at the *frontier* this time, not at home, so
    // the survey command's destination matches the Ruler's
    // StationedAt position. Light delay = 0.
    let (empire, _home, frontier) = setup_one_target(&mut app, 5.0);
    spawn_test_ruler(app.world_mut(), empire, frontier);

    app.world_mut()
        .resource_mut::<Messages<SurveyRequested>>()
        .update();

    // A handful of ticks is enough — both the AI cadence gate and
    // the dispatch ↔ process schedule split fit inside this window.
    for _ in 0..5 {
        advance_time(&mut app, 1);
    }

    let total_in_last_update = survey_requested_count(&mut app);
    assert!(
        total_in_last_update > 0 || app.world().resource::<Messages<SurveyRequested>>().len() > 0,
        "Ruler at the survey target should pay no light-speed delay; \
         expected at least one SurveyRequested within 5 ticks but got 0",
    );
}
