//! Round 9 PR #3 / #468 PR-1 regression: AI commands incur light-speed delay.
//!
//! Before Round 9 PR #3, NPC AI commands flowed `bus.emit_command →
//! drain_ai_commands` in a single tick — NPCs had perfect
//! instantaneous reach across the galaxy regardless of their Ruler's
//! position. PR #3 interposed an [`AiCommandOutbox`] between producer
//! (`npc_decision_tick`, `run_short_agents`) and consumer
//! (`drain_ai_commands`), computing each command's `arrives_at` from
//! the Ruler to the command's *destination* (`target_system` for
//! spatial commands, capital for spatial-less).
//!
//! **#468 PR-1 update.** That arrival model was wrong for ship-control
//! commands. The order has to reach the *ship*, not the target —
//! a Ruler at home A dispatching a Scout already at frontier B (5 ly
//! away) was paying ~300 hd delay before any survey could fire ("AI
//! does nothing for years"). PR-1 migrates `survey_system` onto a new
//! per-ship `PendingAiShipCommand` pipeline whose `arrives_at` is
//! `light_delay_ruler_to_ship(ruler, ship)`. PR-2/3 will migrate the
//! remaining ship-control kinds.
//!
//! These tests pin the new survey contract:
//!
//! 1. `survey_ai_scout_at_home_ruler_at_home_target_far_zero_delay` —
//!    Ruler at A, Scout at A, frontier B 5 ly away. The Ruler→ship
//!    distance is zero, so the survey must fire within 1–2 ticks
//!    regardless of the target's distance. This is the core #468
//!    regression: pre-PR-1, the AI would wait ~300 hd before firing.
//! 2. `survey_ai_scout_remote_ruler_at_home_delays_by_ruler_to_ship` —
//!    Ruler at A, Scout at B (5 ly), target C. Delay must equal
//!    `light_delay_hexadies(5)` (~300 hd) — Ruler→ship distance, not
//!    Ruler→target.
//! 3. `survey_ai_scout_at_home_ruler_remote_delays_by_ruler_to_ship` —
//!    Symmetric: Ruler at frontier (5 ly), Scout at home. Same delay.

mod common;

use bevy::prelude::*;

use macrocosmo::ai::AiPlayerMode;
use macrocosmo::ai::command_consumer::PendingAiShipCommand;
use macrocosmo::ai::schema::ids::command as cmd_ids;
use macrocosmo::knowledge::{KnowledgeStore, SystemVisibilityMap, SystemVisibilityTier};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::command_events::SurveyRequested;
use macrocosmo::ship::{Owner, Ship};
use macrocosmo::time_system::GameClock;

use common::{advance_time, spawn_test_ruler, spawn_test_ship, spawn_test_system, test_app};

/// Spawn a self-sufficient AI-controlled empire with one Explorer at
/// `scout_home`, a Ruler stationed at `ruler_system`, and a single
/// frontier system the AI is expected to survey.
///
/// Returns `(empire, home, frontier, scout)` so each test can pin
/// per-empire setup (Ruler placement, Scout location, target system).
fn setup_survey_world(
    app: &mut App,
    frontier_distance_ly: f64,
) -> (Entity, Entity, Entity, Entity) {
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

    // Visibility tiers: home is Local, frontier is Catalogued (known to
    // exist but not surveyed). The AI policy looks for unsurveyed
    // catalogued systems.
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut vis = em.get_mut::<SystemVisibilityMap>().unwrap();
        vis.set(home, SystemVisibilityTier::Local);
        vis.set(frontier, SystemVisibilityTier::Catalogued);
    }

    // Seed the empire's KnowledgeStore so home is "known surveyed".
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

    // Scout at home initially — tests reposition by mutating
    // `ShipState`/`Position` after this helper returns.
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

    (empire, home, frontier, scout)
}

/// Mutate the Scout's location to be in `system` at world-space
/// position `pos`. Used to set up the "Scout off at the frontier"
/// fixture for the Ruler→ship delay assertions.
fn place_ship_at(app: &mut App, ship: Entity, system: Entity, pos: [f64; 3]) {
    let mut em = app.world_mut().entity_mut(ship);
    if let Some(mut state) = em.get_mut::<macrocosmo::ship::ShipState>() {
        *state = macrocosmo::ship::ShipState::InSystem { system };
    }
    if let Some(mut position) = em.get_mut::<macrocosmo::components::Position>() {
        *position = macrocosmo::components::Position::from(pos);
    }
}

/// Count `SurveyRequested` messages emitted this Update window.
fn survey_requested_count(app: &mut App) -> usize {
    let messages = app.world().resource::<Messages<SurveyRequested>>();
    messages.iter_current_update_messages().count()
}

/// True iff a `PendingAiShipCommand` is in flight (= `arrives_at` > now)
/// for a `survey_system` against `target_system`.
fn pending_ship_command_holds_survey_for(app: &mut App, target_system: Entity) -> bool {
    let now = app.world().resource::<GameClock>().elapsed;
    let kind = cmd_ids::survey_system();
    let mut q = app.world_mut().query::<&PendingAiShipCommand>();
    q.iter(app.world())
        .any(|p| p.kind == kind && p.arrives_at > now && p.target_system == target_system)
}

/// #468 PR-1 core regression: Ruler at home, Scout at home, target
/// far away. Pre-fix the AI paid Ruler→target light delay (~300 hd
/// for 5 ly) before firing. Post-fix the delay is Ruler→ship = 0 so
/// the survey fires within 1–2 ticks.
#[test]
fn survey_ai_scout_at_home_ruler_at_home_target_far_zero_delay() {
    let frontier_distance_ly = 5.0;
    let mut app = test_app();
    let (empire, home, frontier, _scout) = setup_survey_world(&mut app, frontier_distance_ly);
    spawn_test_ruler(app.world_mut(), empire, home);

    app.world_mut()
        .resource_mut::<Messages<SurveyRequested>>()
        .update();

    // A handful of ticks is enough for `npc_decision_tick` to fire
    // (gates on `clock.elapsed > last_tick`) AND for
    // `drain_ai_ship_commands` to release the `PendingAiShipCommand`
    // whose `arrives_at == sent_at` (Ruler at the ship → 0 light delay).
    let mut survey_event_total = 0usize;
    for _ in 0..5 {
        advance_time(&mut app, 1);
        survey_event_total += survey_requested_count(&mut app);
    }

    assert!(
        survey_event_total > 0,
        "Ruler at the ship's system must pay zero light-delay (Ruler→ship distance); \
         expected SurveyRequested within 5 ticks for {} ly target but got 0 — #468 regression",
        frontier_distance_ly,
    );
    let _ = frontier;
}

/// #468 PR-1: Ruler at home A, Scout at frontier B (5 ly), target C.
/// The Ruler→ship distance is 5 ly, so the survey must NOT fire until
/// `light_delay_hexadies(5)` ticks have elapsed.
#[test]
fn survey_ai_scout_remote_ruler_at_home_delays_by_ruler_to_ship() {
    use macrocosmo::physics::light_delay_hexadies;

    let mut app = test_app();
    let (empire, home, frontier, scout) = setup_survey_world(&mut app, 5.0);
    spawn_test_ruler(app.world_mut(), empire, home);

    // Move the Scout to the frontier — Ruler→ship distance = 5 ly.
    place_ship_at(&mut app, scout, frontier, [5.0, 0.0, 0.0]);

    app.world_mut()
        .resource_mut::<Messages<SurveyRequested>>()
        .update();

    // Drive a few ticks so the decision system emits a survey_system
    // command and `dispatch_ai_pending_commands` spawns the
    // `PendingAiShipCommand` holder.
    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    app.world_mut()
        .resource_mut::<Messages<SurveyRequested>>()
        .update();
    advance_time(&mut app, 1);

    // Pre-arrival sanity: no survey fired yet, holder still in flight.
    let early_count = survey_requested_count(&mut app);
    assert_eq!(
        early_count, 0,
        "survey fired before Ruler→ship light delay elapsed — #468 regression",
    );
    assert!(
        pending_ship_command_holds_survey_for(&mut app, frontier),
        "PendingAiShipCommand for survey of frontier should be in flight before light delay elapses",
    );

    // Accumulate survey events tick-by-tick across the rest of the
    // light-delay window. `iter_current_update_messages` only sees the
    // current Update, so we must observe each tick individually.
    let light_delay = light_delay_hexadies(5.0);
    let mut survey_event_total = 0usize;
    let mut released_at: Option<i64> = None;
    for _ in 0..(light_delay + 5) {
        advance_time(&mut app, 1);
        survey_event_total += survey_requested_count(&mut app);
        if released_at.is_none() && !pending_ship_command_holds_survey_for(&mut app, frontier) {
            released_at = Some(app.world().resource::<GameClock>().elapsed);
        }
    }
    assert!(
        survey_event_total > 0,
        "no SurveyRequested fired across {} post-threshold ticks (released_at={:?}) — #468 PR-1 over-gating regression",
        light_delay + 5,
        released_at,
    );
}

/// #468 PR-1 symmetry: Ruler at frontier B (5 ly), Scout at home A,
/// target wherever. Same Ruler→ship distance = 5 ly, same delay.
/// Guards against the helper being asymmetric in one direction.
#[test]
fn survey_ai_scout_at_home_ruler_remote_delays_by_ruler_to_ship() {
    use macrocosmo::physics::light_delay_hexadies;

    let mut app = test_app();
    // Frontier is the target system AND the Ruler's posting.
    let (empire, _home, frontier, _scout) = setup_survey_world(&mut app, 5.0);
    spawn_test_ruler(app.world_mut(), empire, frontier);

    app.world_mut()
        .resource_mut::<Messages<SurveyRequested>>()
        .update();

    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    app.world_mut()
        .resource_mut::<Messages<SurveyRequested>>()
        .update();
    advance_time(&mut app, 1);

    let early_count = survey_requested_count(&mut app);
    assert_eq!(
        early_count, 0,
        "survey fired before Ruler→ship light delay elapsed (symmetric case) — #468 regression",
    );

    let light_delay = light_delay_hexadies(5.0);
    let mut survey_event_total = 0usize;
    for _ in 0..(light_delay + 5) {
        advance_time(&mut app, 1);
        survey_event_total += survey_requested_count(&mut app);
    }
    assert!(
        survey_event_total > 0,
        "no SurveyRequested fired across {} post-threshold ticks — #468 PR-1 symmetric over-gating regression",
        light_delay + 5,
    );
}

/// #468 PR-1 BLOCKER regression (adversarial review HIGH #1): when a
/// matured `PendingAiShipCommand` is rejected at drain time (ship no
/// longer idle, owner changed, despawned), the `PendingAssignment`
/// marker stamped at outbox-spawn time must be removed too — otherwise
/// the ship is permanently excluded from future AI dispatches because
/// the dedup scan at `npc_decision.rs::dedup_pending_assignments`
/// filters by `PendingAssignment`.
///
/// Pre-fix: `apply_survey_to_ship` early-returned without touching the
/// marker on any of the reject branches, since no `SurveyRequested`
/// fired and `handle_survey_requested` (the legacy cleaner) never ran.
/// Post-fix: the drain explicitly strips the marker on every reject.
///
/// Test shape: we don't want the AI to keep re-emitting `survey_system`
/// in subsequent ticks (that would re-stamp the marker after every
/// removal). So we set up the holder + marker manually, set the ship
/// to a non-idle state, then drive the drain to maturity and assert
/// the marker is gone. The dispatcher path is exercised by the other
/// tests in this file.
#[test]
fn rejected_survey_at_drain_time_releases_pending_assignment() {
    use macrocosmo::ai::AiBusResource;
    use macrocosmo::ai::assignments::{AssignmentKind, AssignmentTarget, PendingAssignment};
    use macrocosmo::ai::command_consumer::PendingAiShipCommand;
    use macrocosmo::ai::schema::ids::command as cmd_ids;
    use macrocosmo::ship::{CommandQueue, QueuedCommand};

    let mut app = test_app();
    // Disable AI policy emission so the test exclusively exercises the
    // drain reject path — without this the next NPC tick would
    // re-stamp the marker after we observe its removal.
    app.insert_resource(AiPlayerMode(false));
    app.insert_resource(AiBusResource::default());

    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Reject Test".into(),
            },
            Faction {
                id: "reject_test".into(),
                name: "Reject Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
        ))
        .id();

    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let frontier = spawn_test_system(
        app.world_mut(),
        "Frontier",
        [5.0, 0.0, 0.0],
        1.0,
        false,
        false,
    );
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

    // Set the ship's CommandQueue to non-empty so apply_survey_to_ship
    // takes the "queue.commands.is_empty()" reject branch on maturity.
    {
        let mut em = app.world_mut().entity_mut(scout);
        if let Some(mut queue) = em.get_mut::<CommandQueue>() {
            queue.commands.push(QueuedCommand::MoveTo { system: home });
        }
    }

    // Manually spawn the in-flight holder + stamp the marker — same
    // shape the dispatcher would produce on the survey path.
    let now = app.world().resource::<GameClock>().elapsed;
    app.world_mut().spawn(PendingAiShipCommand {
        kind: cmd_ids::survey_system(),
        target_system: frontier,
        ship: scout,
        issuer_empire: empire,
        sent_at: now,
        arrives_at: now + 1,
    });
    app.world_mut()
        .entity_mut(scout)
        .insert(PendingAssignment {
            faction: empire,
            kind: AssignmentKind::Survey,
            target: AssignmentTarget::System(frontier),
            since: now,
        });

    // Sanity: the marker is stamped before the drain runs.
    assert!(
        app.world().entity(scout).get::<PendingAssignment>().is_some(),
        "PendingAssignment must be present pre-drain (test setup invariant)",
    );

    // Walk the clock so drain_ai_ship_commands fires. arrives_at = now+1,
    // so a couple of ticks is plenty.
    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    // After rejection: the holder is despawned AND the marker is
    // removed. Pre-fix this assertion failed — the marker stayed
    // forever, locking the scout out of future surveys.
    let holder_remains = {
        let mut q = app.world_mut().query::<&PendingAiShipCommand>();
        q.iter(app.world()).any(|p| p.ship == scout)
    };
    assert!(
        !holder_remains,
        "Holder must be drained even on reject path",
    );
    assert!(
        app.world().entity(scout).get::<PendingAssignment>().is_none(),
        "PendingAssignment must be removed when survey is rejected at drain time \
         (otherwise the ship is permanently excluded from AI dispatches)",
    );
}
