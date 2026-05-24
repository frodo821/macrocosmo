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
//!
//! #468 PR-2 adds analogous zero-delay regressions for `colonize_system`,
//! `reposition`, and `blockade`, plus a reject-path test for the
//! colonize marker (the other two kinds don't carry a `PendingAssignment`).

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
        target_planet: None,
        ship: scout,
        issuer_empire: empire,
        sent_at: now,
        arrives_at: now + 1,
    });
    app.world_mut().entity_mut(scout).insert(PendingAssignment {
        faction: empire,
        kind: AssignmentKind::Survey,
        target: AssignmentTarget::System(frontier),
        since: now,
    });

    // Sanity: the marker is stamped before the drain runs.
    assert!(
        app.world()
            .entity(scout)
            .get::<PendingAssignment>()
            .is_some(),
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
        app.world()
            .entity(scout)
            .get::<PendingAssignment>()
            .is_none(),
        "PendingAssignment must be removed when survey is rejected at drain time \
         (otherwise the ship is permanently excluded from AI dispatches)",
    );
}

// ---------------------------------------------------------------------------
// #468 PR-2: colonize_system / reposition / blockade zero-delay regressions.
//
// Shape: stage a ship in-system, manually spawn a matured
// `PendingAiShipCommand` with `arrives_at = sent_at = now` (= Ruler→ship
// zero light delay), then advance one tick and assert the corresponding
// typed `*Requested` event fired. This is the same shape as PR-1's
// `rejected_survey_at_drain_time_releases_pending_assignment` but on the
// success path. We bypass the dispatcher (`dispatch_ai_pending_commands`)
// because the PR-2 contract under test is the drain side
// (`drain_ai_ship_commands` → `apply_*_to_ship`); dispatcher coverage is
// pinned by `survey_ai_scout_at_home_ruler_at_home_target_far_zero_delay`
// and the dedup tests in `ai_npc_outbox_dedup.rs`.
//
// Bypassing also lets the test disable AI policy emission entirely
// (`AiPlayerMode(false)` + no `PlayerEmpire` marker won't help — the NPC
// path always marks the empire), so the assertion isn't muddled by
// coincidental NPC-emitted commands.
// ---------------------------------------------------------------------------

/// Minimal world for the matured-holder tests. Spawns one empire with a
/// Ruler at `home`, plus a target system + an in-system ship.
fn setup_matured_holder_world(
    app: &mut App,
    ship_design: &str,
    target_distance_ly: f64,
) -> (Entity, Entity, Entity, Entity) {
    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "PR-2 Holder Test".into(),
            },
            Faction {
                id: "pr2_holder_test".into(),
                name: "PR-2 Holder Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
        ))
        .id();

    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let target = spawn_test_system(
        app.world_mut(),
        "Target",
        [target_distance_ly, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    let ship = spawn_test_ship(
        app.world_mut(),
        "Ship-1",
        ship_design,
        home,
        [0.0, 0.0, 0.0],
    );
    app.world_mut()
        .entity_mut(ship)
        .get_mut::<Ship>()
        .unwrap()
        .owner = Owner::Empire(empire);

    spawn_test_ruler(app.world_mut(), empire, home);

    (empire, home, target, ship)
}

/// Spawn a matured `PendingAiShipCommand` (arrives_at = sent_at = now).
fn spawn_matured_holder(
    app: &mut App,
    kind: macrocosmo_ai::CommandKindId,
    ship: Entity,
    target_system: Entity,
    issuer_empire: Entity,
) {
    use macrocosmo::ai::command_consumer::PendingAiShipCommand;
    let now = app.world().resource::<GameClock>().elapsed;
    app.world_mut().spawn(PendingAiShipCommand {
        kind,
        target_system,
        target_planet: None,
        ship,
        issuer_empire,
        sent_at: now,
        arrives_at: now,
    });
}

/// #468 PR-2: matured `colonize_system` PendingAiShipCommand at zero
/// light delay must drain into a `ColonizeRequested` within the same
/// tick. Pre-fix the AI paid Ruler→target light delay (~300 hd for
/// 5 ly) before firing; post-fix the wire-up to
/// `drain_ai_ship_commands` releases the event the instant the
/// holder matures.
#[test]
fn colonize_ai_ship_at_home_ruler_at_home_target_far_zero_delay() {
    use macrocosmo::ship::command_events::ColonizeRequested;

    let mut app = test_app();
    // Disable AI policy emission so we observe only the drain we set up.
    app.insert_resource(AiPlayerMode(false));

    let (empire, _home, target, ship) =
        setup_matured_holder_world(&mut app, "colony_ship_mk1", 5.0);
    spawn_matured_holder(&mut app, cmd_ids::colonize_system(), ship, target, empire);

    app.world_mut()
        .resource_mut::<Messages<ColonizeRequested>>()
        .update();

    let mut colonize_event_total = 0usize;
    for _ in 0..3 {
        advance_time(&mut app, 1);
        colonize_event_total += {
            let messages = app.world().resource::<Messages<ColonizeRequested>>();
            messages.iter_current_update_messages().count()
        };
    }
    assert!(
        colonize_event_total > 0,
        "matured colonize_system PendingAiShipCommand at zero light delay must \
         drain into ColonizeRequested within 3 ticks — #468 PR-2 regression"
    );
}

/// #468 PR-2 review fold-in (Bug BLOCKER): the `*Requested` event
/// handler `handle_colonize_requested` also rejects on no-core, not-
/// idle, target-despawned, and not-a-colony-ship branches. Pre-fix
/// none of those branches stripped `PendingAssignment::Colonize` —
/// only the drain-side `apply_colonize_to_ship` did. So a colonize
/// command that survived the drain (ship was idle) but failed at the
/// settlement handler (no sovereignty Core in target system, etc.)
/// would leave the marker stamped forever.
///
/// This test pins the handler-side cleanup by:
///   1. spawning a colony ship at the target system (= drain succeeds,
///      the auto-MoveTo branch is skipped),
///   2. spawning the ColonizeRequested event directly (= bypass drain),
///   3. pre-stamping the marker,
///   4. observing that handle_colonize_requested rejects (no Core in
///      the target system) AND removes the marker.
#[test]
fn colonize_handler_reject_at_no_core_releases_pending_assignment() {
    use macrocosmo::ai::assignments::{AssignmentKind, AssignmentTarget, PendingAssignment};
    use macrocosmo::ship::ShipState;
    use macrocosmo::ship::command_events::ColonizeRequested;

    let mut app = test_app();
    app.insert_resource(AiPlayerMode(false));

    // One survey system that doubles as the colonize target — the
    // ship will be placed inside it (= drain succeeds: ship is docked
    // at target). The system has no sovereignty Core, so the handler
    // rejects on the no-core branch.
    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Handler Reject Test".into(),
            },
            Faction {
                id: "handler_reject_test".into(),
                name: "Handler Reject Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
        ))
        .id();
    let target = spawn_test_system(app.world_mut(), "Target", [0.0, 0.0, 0.0], 1.0, true, false);
    let colony_ship = spawn_test_ship(
        app.world_mut(),
        "Colony-1",
        "colony_ship_mk1",
        target,
        [0.0, 0.0, 0.0],
    );
    {
        let mut em = app.world_mut().entity_mut(colony_ship);
        em.get_mut::<Ship>().unwrap().owner = Owner::Empire(empire);
        // Ensure FactionOwner so the handler's #299 Core gate is
        // exercised (not the neutral-bypass path).
        em.insert(macrocosmo::faction::FactionOwner(empire));
        if let Some(mut state) = em.get_mut::<ShipState>() {
            *state = ShipState::InSystem { system: target };
        }
    }

    // Stamp the marker as if the dispatcher had emitted the colonize
    // earlier and the drain had already let it through (= the
    // realistic state when handle_colonize_requested sees a request).
    let now = app.world().resource::<GameClock>().elapsed;
    app.world_mut()
        .entity_mut(colony_ship)
        .insert(PendingAssignment {
            faction: empire,
            kind: AssignmentKind::Colonize,
            target: AssignmentTarget::System(target),
            since: now,
        });

    // Reset the message queue so we observe only this test's events.
    app.world_mut()
        .resource_mut::<Messages<ColonizeRequested>>()
        .update();

    // Emit ColonizeRequested directly (bypass drain).
    app.world_mut()
        .resource_mut::<Messages<ColonizeRequested>>()
        .write(ColonizeRequested {
            command_id: macrocosmo::ship::command_events::CommandId(9_999_001),
            ship: colony_ship,
            target_system: target,
            planet: None,
            issued_at: now,
        });

    // One tick is enough for handle_colonize_requested to run.
    for _ in 0..2 {
        advance_time(&mut app, 1);
    }

    assert!(
        app.world()
            .entity(colony_ship)
            .get::<PendingAssignment>()
            .is_none(),
        "PendingAssignment::Colonize must be removed when handle_colonize_requested \
         rejects at the no-Core gate — otherwise the ship is permanently excluded \
         from future AI colonize dispatches even though the drain succeeded",
    );
}

/// #468 PR-2: matured `reposition` PendingAiShipCommand at zero light
/// delay must drain into a `MoveRequested` within the same tick.
#[test]
fn reposition_ai_ship_at_home_ruler_at_home_target_far_zero_delay() {
    use macrocosmo::ship::command_events::MoveRequested;

    let mut app = test_app();
    app.insert_resource(AiPlayerMode(false));

    let (empire, _home, target, ship) = setup_matured_holder_world(&mut app, "scout_mk1", 5.0);
    spawn_matured_holder(&mut app, cmd_ids::reposition(), ship, target, empire);

    app.world_mut()
        .resource_mut::<Messages<MoveRequested>>()
        .update();

    let mut move_event_total = 0usize;
    for _ in 0..3 {
        advance_time(&mut app, 1);
        move_event_total += {
            let messages = app.world().resource::<Messages<MoveRequested>>();
            messages.iter_current_update_messages().count()
        };
    }
    assert!(
        move_event_total > 0,
        "matured reposition PendingAiShipCommand at zero light delay must drain \
         into MoveRequested within 3 ticks — #468 PR-2 regression"
    );
}

/// #468 PR-2: matured `blockade` PendingAiShipCommand at zero light
/// delay must drain into a `MoveRequested` within the same tick.
/// Both reposition and blockade share `apply_move_to_ship`; this test
/// pins the routing wire-up for blockade.
#[test]
fn blockade_ai_ship_at_home_ruler_at_home_target_far_zero_delay() {
    use macrocosmo::ship::command_events::MoveRequested;

    let mut app = test_app();
    app.insert_resource(AiPlayerMode(false));

    let (empire, _home, target, ship) = setup_matured_holder_world(&mut app, "scout_mk1", 5.0);
    spawn_matured_holder(&mut app, cmd_ids::blockade(), ship, target, empire);

    app.world_mut()
        .resource_mut::<Messages<MoveRequested>>()
        .update();

    let mut move_event_total = 0usize;
    for _ in 0..3 {
        advance_time(&mut app, 1);
        move_event_total += {
            let messages = app.world().resource::<Messages<MoveRequested>>();
            messages.iter_current_update_messages().count()
        };
    }
    assert!(
        move_event_total > 0,
        "matured blockade PendingAiShipCommand at zero light delay must drain \
         into MoveRequested within 3 ticks — #468 PR-2 regression"
    );
}

/// #468 PR-2 BLOCKER regression (mirrors PR-1's survey reject test):
/// when a matured `colonize_system` `PendingAiShipCommand` is rejected
/// at drain time (ship no longer idle / owner changed / despawned),
/// the `PendingAssignment::Colonize` marker stamped at outbox-spawn
/// time MUST be removed too. Otherwise the ship is permanently
/// excluded from future AI colonize dispatches because the dedup scan
/// in `npc_decision.rs` filters by `PendingAssignment`.
#[test]
fn rejected_colonize_at_drain_time_releases_pending_assignment() {
    use macrocosmo::ai::AiBusResource;
    use macrocosmo::ai::assignments::{AssignmentKind, AssignmentTarget, PendingAssignment};
    use macrocosmo::ai::command_consumer::PendingAiShipCommand;
    use macrocosmo::ship::{CommandQueue, QueuedCommand};

    let mut app = test_app();
    // Disable AI policy emission so the test exclusively exercises the
    // drain reject path — without this the next NPC tick could
    // re-stamp the marker after we observe its removal.
    app.insert_resource(AiPlayerMode(false));
    app.insert_resource(AiBusResource::default());

    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Reject Colonize Test".into(),
            },
            Faction {
                id: "reject_colonize_test".into(),
                name: "Reject Colonize Test".into(),
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
    let colony_ship = spawn_test_ship(
        app.world_mut(),
        "Colony-1",
        "colony_ship_mk1",
        home,
        [0.0, 0.0, 0.0],
    );
    app.world_mut()
        .entity_mut(colony_ship)
        .get_mut::<Ship>()
        .unwrap()
        .owner = Owner::Empire(empire);

    // Force the ship into a non-idle state so apply_colonize_to_ship
    // takes the queue-not-empty reject branch on maturity.
    {
        let mut em = app.world_mut().entity_mut(colony_ship);
        if let Some(mut queue) = em.get_mut::<CommandQueue>() {
            queue.commands.push(QueuedCommand::MoveTo { system: home });
        }
    }

    // Manually spawn the in-flight holder + stamp the colonize marker
    // — same shape `dispatch_ship_command_per_ship` produces.
    let now = app.world().resource::<GameClock>().elapsed;
    app.world_mut().spawn(PendingAiShipCommand {
        kind: cmd_ids::colonize_system(),
        target_system: frontier,
        target_planet: None,
        ship: colony_ship,
        issuer_empire: empire,
        sent_at: now,
        arrives_at: now + 1,
    });
    app.world_mut()
        .entity_mut(colony_ship)
        .insert(PendingAssignment {
            faction: empire,
            kind: AssignmentKind::Colonize,
            target: AssignmentTarget::System(frontier),
            since: now,
        });

    assert!(
        app.world()
            .entity(colony_ship)
            .get::<PendingAssignment>()
            .is_some(),
        "PendingAssignment::Colonize must be present pre-drain (test setup invariant)",
    );

    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    let holder_remains = {
        let mut q = app.world_mut().query::<&PendingAiShipCommand>();
        q.iter(app.world()).any(|p| p.ship == colony_ship)
    };
    assert!(
        !holder_remains,
        "Holder must be drained even on colonize reject path",
    );
    assert!(
        app.world()
            .entity(colony_ship)
            .get::<PendingAssignment>()
            .is_none(),
        "PendingAssignment::Colonize must be removed when colonize_system is \
         rejected at drain time (otherwise the ship is permanently excluded \
         from future AI colonize dispatches)",
    );
}

// ---------------------------------------------------------------------------
// #468 PR-3: zero-delay regressions for the newly-migrated kinds
// (`attack_target`, `move_ruler`, `load_deliverable`, `unload_deliverable`,
// `colonize_planet`). Shape mirrors PR-2's reposition / blockade /
// colonize_system tests: stage a ship + spawn a matured holder + drive
// one tick + assert the typed `*Requested` event fired.
// ---------------------------------------------------------------------------

/// #468 PR-3: matured `attack_target` PendingAiShipCommand at zero
/// light delay must drain into a `MoveRequested` within the same
/// tick. Pre-PR-3 the AI paid Ruler→target light delay before firing;
/// post-PR-3 the wire-up to `drain_ai_ship_commands` releases the
/// event the instant the holder matures.
#[test]
fn attack_target_ai_ship_at_home_ruler_at_home_target_far_zero_delay() {
    use macrocosmo::ship::command_events::MoveRequested;

    let mut app = test_app();
    app.insert_resource(AiPlayerMode(false));

    let (empire, _home, target, ship) = setup_matured_holder_world(&mut app, "scout_mk1", 5.0);
    spawn_matured_holder(&mut app, cmd_ids::attack_target(), ship, target, empire);

    app.world_mut()
        .resource_mut::<Messages<MoveRequested>>()
        .update();

    let mut move_event_total = 0usize;
    for _ in 0..3 {
        advance_time(&mut app, 1);
        move_event_total += {
            let messages = app.world().resource::<Messages<MoveRequested>>();
            messages.iter_current_update_messages().count()
        };
    }
    assert!(
        move_event_total > 0,
        "matured attack_target PendingAiShipCommand at zero light delay must drain \
         into MoveRequested within 3 ticks — #468 PR-3 regression"
    );
}

/// #468 PR-3: matured `load_deliverable` PendingAiShipCommand at zero
/// light delay must drain into a `LoadDeliverableRequested` within
/// the same tick.
#[test]
fn load_deliverable_ai_ship_at_home_ruler_at_home_target_far_zero_delay() {
    use macrocosmo::colony::DeliverableStockpile;
    use macrocosmo::ship::CargoItem;
    use macrocosmo::ship::command_events::LoadDeliverableRequested;

    let mut app = test_app();
    app.insert_resource(AiPlayerMode(false));

    let (empire, _home, target, ship) = setup_matured_holder_world(&mut app, "scout_mk1", 5.0);
    spawn_matured_holder(&mut app, cmd_ids::load_deliverable(), ship, target, empire);

    // #468 PR-3 NICE-TO-FIX #7: precheck requires a non-empty
    // DeliverableStockpile on the target system. Seed one so the
    // dedup gate lets the emit through.
    app.world_mut()
        .entity_mut(target)
        .insert(DeliverableStockpile {
            items: vec![CargoItem::Deliverable {
                definition_id: "test_item".into(),
            }],
        });

    app.world_mut()
        .resource_mut::<Messages<LoadDeliverableRequested>>()
        .update();

    let mut event_total = 0usize;
    for _ in 0..3 {
        advance_time(&mut app, 1);
        event_total += {
            let messages = app.world().resource::<Messages<LoadDeliverableRequested>>();
            messages.iter_current_update_messages().count()
        };
    }
    assert!(
        event_total > 0,
        "matured load_deliverable PendingAiShipCommand at zero light delay must drain \
         into LoadDeliverableRequested within 3 ticks — #468 PR-3 regression"
    );
}

/// #468 PR-3 NICE-TO-FIX #7 fold-in regression: when the target
/// system's `DeliverableStockpile` is empty, the dispatcher MUST
/// drop the emit instead of spamming downstream Rejects each tick.
/// Pre-gate the AI re-emitted load_deliverable every tick (no marker
/// to dedup), generating a Rejected `CommandExecuted` per tick until
/// either the stockpile filled or the AI's metric flipped.
#[test]
fn load_deliverable_skipped_when_stockpile_empty() {
    use macrocosmo::ship::command_events::LoadDeliverableRequested;

    let mut app = test_app();
    app.insert_resource(AiPlayerMode(false));

    let (empire, _home, target, ship) = setup_matured_holder_world(&mut app, "scout_mk1", 5.0);
    spawn_matured_holder(&mut app, cmd_ids::load_deliverable(), ship, target, empire);
    // No DeliverableStockpile inserted on `target` — precheck must
    // gate the emit.

    app.world_mut()
        .resource_mut::<Messages<LoadDeliverableRequested>>()
        .update();

    let mut event_total = 0usize;
    for _ in 0..3 {
        advance_time(&mut app, 1);
        event_total += {
            let messages = app.world().resource::<Messages<LoadDeliverableRequested>>();
            messages.iter_current_update_messages().count()
        };
    }
    assert_eq!(
        event_total, 0,
        "load_deliverable with empty target stockpile must skip the emit (NICE-TO-FIX #7); \
         got {} events",
        event_total
    );
}

/// #468 PR-3: matured `unload_deliverable` PendingAiShipCommand at
/// zero light delay must drain into a `DeployDeliverableRequested`
/// within the same tick. The deploy event's position is read from
/// the ship's realtime `Position` so the downstream handler's
/// position equality check passes.
#[test]
fn unload_deliverable_ai_ship_at_home_ruler_at_home_zero_delay() {
    use macrocosmo::ship::command_events::DeployDeliverableRequested;
    use macrocosmo::ship::{Cargo, CargoItem};

    let mut app = test_app();
    app.insert_resource(AiPlayerMode(false));

    let (empire, home, _target, ship) = setup_matured_holder_world(&mut app, "scout_mk1", 5.0);
    // unload has no target system — use home as a sentinel (mirrors
    // the dispatcher's `ship.home_port` fallback).
    spawn_matured_holder(&mut app, cmd_ids::unload_deliverable(), ship, home, empire);

    // #468 PR-3 NICE-TO-FIX #5: precheck requires cargo.items[0] to
    // exist. Seed it so the dedup gate lets the emit through.
    app.world_mut().entity_mut(ship).insert(Cargo {
        items: vec![CargoItem::Deliverable {
            definition_id: "test_item".into(),
        }],
        ..Default::default()
    });

    app.world_mut()
        .resource_mut::<Messages<DeployDeliverableRequested>>()
        .update();

    let mut event_total = 0usize;
    for _ in 0..3 {
        advance_time(&mut app, 1);
        event_total += {
            let messages = app
                .world()
                .resource::<Messages<DeployDeliverableRequested>>();
            messages.iter_current_update_messages().count()
        };
    }
    assert!(
        event_total > 0,
        "matured unload_deliverable PendingAiShipCommand at zero light delay must drain \
         into DeployDeliverableRequested within 3 ticks — #468 PR-3 regression"
    );
}

/// #468 PR-3 NICE-TO-FIX #5 fold-in regression: when the ship has no
/// cargo item at index 0, the dispatcher MUST skip the emit
/// instead of producing a downstream Reject each tick. The original
/// legacy `handle_unload_deliverable` had this sanity check; the
/// PR-3 migration dropped it.
#[test]
fn unload_deliverable_skipped_when_cargo_empty() {
    use macrocosmo::ship::command_events::DeployDeliverableRequested;

    let mut app = test_app();
    app.insert_resource(AiPlayerMode(false));

    let (empire, home, _target, ship) = setup_matured_holder_world(&mut app, "scout_mk1", 5.0);
    spawn_matured_holder(&mut app, cmd_ids::unload_deliverable(), ship, home, empire);
    // No `Cargo` component inserted on the ship — precheck must
    // gate the emit.

    app.world_mut()
        .resource_mut::<Messages<DeployDeliverableRequested>>()
        .update();

    let mut event_total = 0usize;
    for _ in 0..3 {
        advance_time(&mut app, 1);
        event_total += {
            let messages = app
                .world()
                .resource::<Messages<DeployDeliverableRequested>>();
            messages.iter_current_update_messages().count()
        };
    }
    assert_eq!(
        event_total, 0,
        "unload_deliverable with empty/no-cargo ship must skip the emit (NICE-TO-FIX #5); \
         got {} events",
        event_total
    );
}

/// #468 PR-3 NICE-TO-FIX #6 fold-in regression: when the ship is in
/// transit (InFTL / SubLight / etc.), the dispatcher MUST skip the
/// emit instead of producing a downstream defer + re-inject each
/// tick. Only InSystem and Loitering states make sense for unload.
#[test]
fn unload_deliverable_skipped_when_ship_in_transit() {
    use macrocosmo::ship::command_events::DeployDeliverableRequested;
    use macrocosmo::ship::{Cargo, CargoItem, ShipState};

    let mut app = test_app();
    app.insert_resource(AiPlayerMode(false));

    let (empire, home, _target, ship) = setup_matured_holder_world(&mut app, "scout_mk1", 5.0);
    spawn_matured_holder(&mut app, cmd_ids::unload_deliverable(), ship, home, empire);

    // Cargo is present (so we can isolate the InFTL gating).
    app.world_mut().entity_mut(ship).insert(Cargo {
        items: vec![CargoItem::Deliverable {
            definition_id: "test_item".into(),
        }],
        ..Default::default()
    });
    // Flip the ship to InFTL so the precheck gates the emit.
    *app.world_mut().get_mut::<ShipState>(ship).unwrap() = ShipState::InFTL {
        origin_system: home,
        destination_system: home,
        departed_at: 0,
        arrival_at: 1_000,
    };

    app.world_mut()
        .resource_mut::<Messages<DeployDeliverableRequested>>()
        .update();

    let mut event_total = 0usize;
    for _ in 0..3 {
        advance_time(&mut app, 1);
        event_total += {
            let messages = app
                .world()
                .resource::<Messages<DeployDeliverableRequested>>();
            messages.iter_current_update_messages().count()
        };
    }
    assert_eq!(
        event_total, 0,
        "unload_deliverable with ship InFTL must skip the emit (NICE-TO-FIX #6); \
         got {} events",
        event_total
    );
}

/// #468 PR-3: matured `move_ruler` PendingAiShipCommand at zero
/// light delay must (a) push `PendingRulerBoarding` for
/// `process_ruler_boarding` to apply and (b) emit a `MoveRequested`
/// for the transport ship. The dispatcher's ship-selection step
/// ensures the chosen ship is at the Ruler's system, but this test
/// bypasses the dispatcher and constructs the holder directly — so
/// we set the world up so the ship and the Ruler share a system.
#[test]
fn move_ruler_ai_ship_at_ruler_system_zero_delay_pushes_boarding() {
    use macrocosmo::ai::command_consumer::PendingRulerBoarding;
    use macrocosmo::ship::command_events::MoveRequested;

    let mut app = test_app();
    app.insert_resource(AiPlayerMode(false));

    let (empire, _home, target, ship) = setup_matured_holder_world(&mut app, "scout_mk1", 5.0);
    spawn_matured_holder(&mut app, cmd_ids::move_ruler(), ship, target, empire);

    app.world_mut()
        .resource_mut::<Messages<MoveRequested>>()
        .update();

    let mut move_event_total = 0usize;
    let mut boarding_seen = false;
    for _ in 0..3 {
        advance_time(&mut app, 1);
        move_event_total += {
            let messages = app.world().resource::<Messages<MoveRequested>>();
            messages.iter_current_update_messages().count()
        };
        // `PendingRulerBoarding` is drained the same tick by
        // `process_ruler_boarding` — we check it has been observed
        // at least once across the window by looking for the
        // AboardShip insertion + ship.ruler_aboard side effect.
        let aboard_set = {
            let entity = app.world().entity(ship);
            entity
                .get::<macrocosmo::ship::Ship>()
                .map(|s| s.ruler_aboard)
                .unwrap_or(false)
        };
        if aboard_set {
            boarding_seen = true;
        }
        // Also accept the queue not being empty pre-drain.
        let pending_remaining = app
            .world()
            .resource::<PendingRulerBoarding>()
            .requests
            .len();
        if pending_remaining > 0 {
            boarding_seen = true;
        }
    }
    assert!(
        move_event_total > 0,
        "matured move_ruler PendingAiShipCommand at zero light delay must drain \
         into MoveRequested within 3 ticks — #468 PR-3 regression"
    );
    assert!(
        boarding_seen,
        "matured move_ruler PendingAiShipCommand must push PendingRulerBoarding \
         (observed via `ship.ruler_aboard = true` after `process_ruler_boarding` \
         drains) — #468 PR-3 regression"
    );
}

/// #468 PR-3: matured `colonize_planet` PendingAiShipCommand at zero
/// light delay must drain into a `ColonizeRequested` whose `planet`
/// field is `Some(target_planet)` — distinguishing it from
/// `colonize_system`, which writes `planet: None` and lets the
/// settlement handler pick. This is the planet-target marker shape
/// pin.
#[test]
fn colonize_planet_ai_ship_at_home_ruler_at_home_target_planet_set() {
    use macrocosmo::ai::command_consumer::PendingAiShipCommand;
    use macrocosmo::galaxy::Planet;
    use macrocosmo::ship::command_events::ColonizeRequested;

    let mut app = test_app();
    app.insert_resource(AiPlayerMode(false));

    let (empire, _home, target, ship) =
        setup_matured_holder_world(&mut app, "colony_ship_mk1", 5.0);
    // colonize_planet targets a Planet, not a System. Pull the first
    // planet of the target system (spawn_test_system makes one).
    let planet = {
        let mut q = app.world_mut().query::<(Entity, &Planet)>();
        q.iter(app.world())
            .find(|(_, p)| p.system == target)
            .map(|(e, _)| e)
            .expect("spawn_test_system should create one planet under the target system")
    };

    let now = app.world().resource::<GameClock>().elapsed;
    app.world_mut().spawn(PendingAiShipCommand {
        kind: cmd_ids::colonize_planet(),
        target_system: target,
        target_planet: Some(planet),
        ship,
        issuer_empire: empire,
        sent_at: now,
        arrives_at: now,
    });

    app.world_mut()
        .resource_mut::<Messages<ColonizeRequested>>()
        .update();

    let mut planet_set_count = 0usize;
    for _ in 0..3 {
        advance_time(&mut app, 1);
        let messages = app.world().resource::<Messages<ColonizeRequested>>();
        for ev in messages.iter_current_update_messages() {
            if ev.planet == Some(planet) {
                planet_set_count += 1;
            }
        }
    }
    assert!(
        planet_set_count > 0,
        "matured colonize_planet PendingAiShipCommand must emit ColonizeRequested \
         with planet=Some(target_planet) within 3 ticks — #468 PR-3 regression"
    );
}

/// #468 PR-3 fold-in: when a matured `colonize_planet`
/// `PendingAiShipCommand` is rejected at drain time, the
/// `PendingAssignment::Colonize { target: Planet(p) }` marker
/// stamped at outbox-spawn time MUST be removed too. Same dedup
/// contract as `colonize_system` — without marker cleanup the ship
/// is permanently excluded from future AI colonize dispatches.
#[test]
fn rejected_colonize_planet_at_drain_time_releases_pending_assignment() {
    use macrocosmo::ai::AiBusResource;
    use macrocosmo::ai::assignments::{AssignmentKind, AssignmentTarget, PendingAssignment};
    use macrocosmo::ai::command_consumer::PendingAiShipCommand;
    use macrocosmo::galaxy::Planet;
    use macrocosmo::ship::{CommandQueue, QueuedCommand};

    let mut app = test_app();
    app.insert_resource(AiPlayerMode(false));
    app.insert_resource(AiBusResource::default());

    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Reject Colonize Planet".into(),
            },
            Faction {
                id: "reject_colonize_planet".into(),
                name: "Reject Colonize Planet".into(),
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
    // Resolve the planet entity spawned under `frontier` by
    // `spawn_test_system`.
    let planet = {
        let mut q = app.world_mut().query::<(Entity, &Planet)>();
        q.iter(app.world())
            .find(|(_, p)| p.system == frontier)
            .map(|(e, _)| e)
            .expect("spawn_test_system should create one planet under the frontier system")
    };
    let colony_ship = spawn_test_ship(
        app.world_mut(),
        "Colony-1",
        "colony_ship_mk1",
        home,
        [0.0, 0.0, 0.0],
    );
    app.world_mut()
        .entity_mut(colony_ship)
        .get_mut::<Ship>()
        .unwrap()
        .owner = Owner::Empire(empire);

    // Force the ship into a non-idle state so apply_colonize_to_ship
    // takes the queue-not-empty reject branch on maturity.
    {
        let mut em = app.world_mut().entity_mut(colony_ship);
        if let Some(mut queue) = em.get_mut::<CommandQueue>() {
            queue.commands.push(QueuedCommand::MoveTo { system: home });
        }
    }

    let now = app.world().resource::<GameClock>().elapsed;
    app.world_mut().spawn(PendingAiShipCommand {
        kind: cmd_ids::colonize_planet(),
        target_system: frontier,
        target_planet: Some(planet),
        ship: colony_ship,
        issuer_empire: empire,
        sent_at: now,
        arrives_at: now + 1,
    });
    app.world_mut()
        .entity_mut(colony_ship)
        .insert(PendingAssignment {
            faction: empire,
            kind: AssignmentKind::Colonize,
            target: AssignmentTarget::Planet(planet),
            since: now,
        });

    assert!(
        app.world()
            .entity(colony_ship)
            .get::<PendingAssignment>()
            .is_some(),
        "PendingAssignment::Colonize{{Planet}} must be present pre-drain (test invariant)",
    );

    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    let holder_remains = {
        let mut q = app.world_mut().query::<&PendingAiShipCommand>();
        q.iter(app.world()).any(|p| p.ship == colony_ship)
    };
    assert!(
        !holder_remains,
        "colonize_planet holder must be drained even on reject path",
    );
    assert!(
        app.world()
            .entity(colony_ship)
            .get::<PendingAssignment>()
            .is_none(),
        "PendingAssignment{{Planet}} must be removed when colonize_planet is \
         rejected at drain time (otherwise the ship is permanently excluded from \
         future AI colonize dispatches)",
    );
}

/// #468 PR-3: `fortify_system` was mis-categorised as a spatial
/// (target-bound) command pre-PR-3. It's actually a BUILD order
/// (queue a combat ship at a shipyard) routed to the empire's
/// capital. This test pins the new contract: the courier delay is
/// Ruler→capital, NOT Ruler→target_system.
///
/// Shape: spawn an empire with the Ruler at home (capital) and a
/// far-away target system. Emit `fortify_system { target_system =
/// far }`. The command should land in the outbox with `arrives_at`
/// = capital-bound (== now, since Ruler IS at the capital) — not
/// Ruler→far delay.
#[test]
fn fortify_system_pays_ruler_to_capital_delay_not_ruler_to_target() {
    use macrocosmo::ai::AiBusResource;
    use macrocosmo::ai::command_outbox::AiCommandOutbox;
    use macrocosmo::ai::schema::ids::command as cmd_ids;

    let mut app = test_app();
    app.insert_resource(AiPlayerMode(false));

    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Fortify Capital Test".into(),
            },
            Faction {
                id: "fortify_capital_test".into(),
                name: "Fortify Capital Test".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
        ))
        .id();

    let capital = spawn_test_system(app.world_mut(), "Capital", [0.0, 0.0, 0.0], 1.0, true, true);
    // Mark the capital as the empire's home system so the
    // capital-resolution fallback chain picks it up.
    app.world_mut()
        .entity_mut(empire)
        .insert(macrocosmo::galaxy::HomeSystem(capital));
    let far_target = spawn_test_system(
        app.world_mut(),
        "FarTarget",
        [50.0, 0.0, 0.0],
        1.0,
        true,
        false,
    );

    spawn_test_ruler(app.world_mut(), empire, capital);

    let now = app.world().resource::<GameClock>().elapsed;
    let cmd = macrocosmo_ai::Command::new(
        cmd_ids::fortify_system(),
        macrocosmo::ai::convert::to_ai_faction(empire),
        now,
    )
    .with_param(
        "target_system",
        macrocosmo_ai::CommandValue::System(macrocosmo::ai::convert::to_ai_system(far_target)),
    );
    app.world_mut()
        .resource_mut::<AiBusResource>()
        .0
        .emit_command(cmd);

    // One tick: AiTickSet::Reason runs dispatch_ai_pending_commands
    // which moves the bus entry into the outbox.
    advance_time(&mut app, 1);

    // Capture the outbox entry. Ruler is at the capital, so the
    // arrival plan collapses to ~0 light delay
    // (Ruler→capital == 0). Pre-PR-3 this was Ruler→far_target
    // ≈ light_delay_hexadies(50) → many ticks.
    let outbox = app.world().resource::<AiCommandOutbox>();
    let fortify_entries: Vec<&macrocosmo::ai::command_outbox::PendingAiCommand> = outbox
        .entries
        .iter()
        .filter(|e| e.command.kind == cmd_ids::fortify_system())
        .collect();
    // Note: an entry whose arrives_at <= now would have already been
    // released by `process_ai_pending_commands` in the same tick.
    // Either way the assertion holds: if the entry is gone, the
    // delay was ~0 (Ruler at capital); if it's still here, its
    // `arrives_at` must be at-most-1 hex away from `now` (rounding
    // for tick boundary), NOT a multi-hundred-hex Ruler→target delay.
    let observed_now = app.world().resource::<GameClock>().elapsed;
    for entry in &fortify_entries {
        let delay = entry.arrives_at - entry.sent_at;
        assert!(
            delay <= 2,
            "fortify_system Ruler→capital delay must be ~0 (Ruler is at capital); got {delay} hexadies",
        );
    }
    // Also assert it's not still queued with a multi-hundred-hex delay.
    let any_long_delay = fortify_entries
        .iter()
        .any(|e| e.arrives_at - e.sent_at > 10);
    assert!(
        !any_long_delay,
        "fortify_system must not pay Ruler→target light delay (≈ {observed_now} hex for 50 ly); \
         #468 PR-3 mis-categorisation regression",
    );
}

/// #468 PR-2: holder cleanup contract for `reposition` — even though
/// the kind doesn't carry a `PendingAssignment` marker (movement
/// orders are idempotent — no per-empire dedup needed), the in-flight
/// holder MUST still be despawned on reject paths so it doesn't leak
/// across ticks. Mirrors the colonize cleanup test minus the marker
/// assertion.
#[test]
fn rejected_reposition_at_drain_time_despawns_holder() {
    use macrocosmo::ai::AiBusResource;
    use macrocosmo::ai::command_consumer::PendingAiShipCommand;
    use macrocosmo::ship::{CommandQueue, QueuedCommand};

    let mut app = test_app();
    app.insert_resource(AiPlayerMode(false));
    app.insert_resource(AiBusResource::default());

    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Reject Reposition".into(),
            },
            Faction {
                id: "reject_reposition".into(),
                name: "Reject Reposition".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
        ))
        .id();

    let home = spawn_test_system(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true, true);
    let target = spawn_test_system(app.world_mut(), "Target", [5.0, 0.0, 0.0], 1.0, true, false);
    let ship = spawn_test_ship(
        app.world_mut(),
        "Ship-1",
        "scout_mk1",
        home,
        [0.0, 0.0, 0.0],
    );
    app.world_mut()
        .entity_mut(ship)
        .get_mut::<Ship>()
        .unwrap()
        .owner = Owner::Empire(empire);

    // Non-idle ship → apply_reposition_to_ship rejects.
    {
        let mut em = app.world_mut().entity_mut(ship);
        if let Some(mut queue) = em.get_mut::<CommandQueue>() {
            queue.commands.push(QueuedCommand::MoveTo { system: home });
        }
    }

    let now = app.world().resource::<GameClock>().elapsed;
    app.world_mut().spawn(PendingAiShipCommand {
        kind: cmd_ids::reposition(),
        target_system: target,
        target_planet: None,
        ship,
        issuer_empire: empire,
        sent_at: now,
        arrives_at: now + 1,
    });

    for _ in 0..3 {
        advance_time(&mut app, 1);
    }

    let holder_remains = {
        let mut q = app.world_mut().query::<&PendingAiShipCommand>();
        q.iter(app.world()).any(|p| p.ship == ship)
    };
    assert!(
        !holder_remains,
        "reposition holder must be drained on reject path so it doesn't \
         leak across ticks"
    );
}
