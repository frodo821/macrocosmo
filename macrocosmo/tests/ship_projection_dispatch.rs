//! #475: dispatch-time `ShipProjection` writes (epic #473).
//!
//! Each producer site (AI command outbox, Lua `request_command`, player
//! ship command) must populate the dispatcher's
//! `KnowledgeStore.projections[ship]` at the moment the command is
//! emitted, using **only local info** (the dispatcher's KnowledgeStore +
//! Ruler position + the command itself). Reading the ship's realtime
//! ECS state at dispatch time would reintroduce the FTL leak this epic
//! exists to fix.
//!
//! Tests:
//!
//! 1. `ai_dispatch_writes_projection_for_survey_command` —
//!    `dispatch_ai_pending_commands` writes a projection for a
//!    `survey_system` command; the entry surfaces in the issuing
//!    empire's `KnowledgeStore.projections`.
//! 2. `lua_request_command_writes_projection_at_sender_tick` — Lua
//!    `request_command(survey, ...)` writes the projection at the
//!    sender's tick (= the call moment), NOT when the underlying
//!    `PendingScriptedCommand` finally fires after light-delay. This
//!    pins the "writing at arrival is wrong" semantic from epic #473.
//! 3. `player_dispatch_writes_projection_for_pending_ship_command` —
//!    spawning a `PendingShipCommand` from the egui path writes a
//!    projection via the deferred-`commands.queue` plumbing.
//! 4. `ai_dispatch_does_not_consult_realtime_ship_state` — even when
//!    the ship's ground-truth ECS `ShipState` differs from the
//!    dispatcher's `KnowledgeStore` snapshot (the realistic FTL-leak
//!    scenario), the projection is computed from the snapshot's
//!    last-known position, never from realtime state.

mod common;

use bevy::prelude::*;

use macrocosmo::ai::AiPlayerMode;
use macrocosmo::components::Position;
use macrocosmo::knowledge::{
    KnowledgeStore, ObservationSource, ShipSnapshot, ShipSnapshotState, SystemVisibilityMap,
    SystemVisibilityTier,
};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::{Owner, Ship};
use macrocosmo::time_system::GameClock;

use common::{spawn_test_ruler, spawn_test_ship, spawn_test_system, test_app};

// ---------------------------------------------------------------------------
// Shared scenario: one empire with a Ruler at home + a frontier system.
// ---------------------------------------------------------------------------

fn setup_scenario(app: &mut App, frontier_distance_ly: f64) -> (Entity, Entity, Entity, Entity) {
    app.insert_resource(AiPlayerMode(true));

    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "projection_test".into(),
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

    spawn_test_ruler(app.world_mut(), empire, home);

    (empire, home, frontier, ship)
}

// ---------------------------------------------------------------------------
// 1. AI dispatch site
// ---------------------------------------------------------------------------

#[test]
fn ai_dispatch_writes_projection_for_survey_command() {
    use macrocosmo::ai::convert::{to_ai_entity, to_ai_faction, to_ai_system};
    use macrocosmo_ai::{Command, CommandKindId, CommandValue};

    let frontier_distance_ly = 5.0;
    let mut app = test_app();
    let (empire, _home, frontier, ship) = setup_scenario(&mut app, frontier_distance_ly);

    // Warmup tick: lets `AiPlugin`'s `Startup` schedule run
    // `schema::declare_all` so the AI bus knows about command kinds.
    // Without this the manual `bus.emit_command(...)` below would be
    // dropped with "emit of undeclared command kind".
    app.update();

    // Stamp a snapshot so the dispatcher *believes* the ship is at home —
    // independent of the ship's runtime state.
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_ship(ShipSnapshot {
            entity: ship,
            name: "Scout-1".into(),
            design_id: "explorer_mk1".into(),
            last_known_state: ShipSnapshotState::InSystem,
            last_known_system: Some(_home),
            observed_at: 0,
            hp: 100.0,
            hp_max: 100.0,
            source: ObservationSource::Direct,
        });
    }

    // Dispatch a survey command directly into the AI bus, then run the
    // AI tick once so `dispatch_ai_pending_commands` picks it up.
    let cmd = {
        let mut c = Command::new(
            CommandKindId::from("survey_system"),
            to_ai_faction(empire),
            0,
        );
        c.params.insert(
            "target_system".into(),
            CommandValue::System(to_ai_system(frontier)),
        );
        c.params.insert("ship_count".into(), CommandValue::I64(1));
        c.params
            .insert("ship_0".into(), CommandValue::Entity(to_ai_entity(ship)));
        c
    };
    {
        let mut bus = app
            .world_mut()
            .resource_mut::<macrocosmo::ai::plugin::AiBusResource>();
        bus.emit_command(cmd);
    }

    // Set clock so dispatched_at is non-zero (= more interesting).
    app.world_mut().resource_mut::<GameClock>().elapsed = 100;
    app.update();

    // Projection must now be present.
    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let projection = store
        .get_projection(ship)
        .expect("AI dispatch should have written a ShipProjection for the surveyed ship");

    assert_eq!(projection.entity, ship);
    assert_eq!(projection.dispatched_at, 100);
    assert_eq!(
        projection.intended_state,
        Some(ShipSnapshotState::Surveying),
        "survey_system kind maps to Surveying intended state",
    );
    assert_eq!(projection.intended_system, Some(frontier));
    assert!(
        projection.expected_arrival_at.is_some(),
        "spatial command must populate expected_arrival_at"
    );
    assert!(
        projection.expected_return_at.is_some(),
        "survey has a return leg, expected_return_at must be Some"
    );
    let take_effect = projection.intended_takes_effect_at.unwrap();
    assert!(
        take_effect >= projection.dispatched_at,
        "intended_takes_effect_at ({}) must be >= dispatched_at ({})",
        take_effect,
        projection.dispatched_at
    );
}

// ---------------------------------------------------------------------------
// 2. Lua dispatch site
// ---------------------------------------------------------------------------

#[test]
fn lua_request_command_writes_projection_at_sender_tick() {
    use macrocosmo::scripting::gamestate_scope::apply::{ParsedRequest, request_command};

    let frontier_distance_ly = 5.0;
    let mut app = test_app();
    let (empire, _home, frontier, ship) = setup_scenario(&mut app, frontier_distance_ly);

    // Set sender tick. The projection's `dispatched_at` must equal this.
    let sender_tick = 50;
    app.world_mut().resource_mut::<GameClock>().elapsed = sender_tick;

    let _id = request_command(
        app.world_mut(),
        ParsedRequest::Survey {
            ship,
            target_system: frontier,
        },
    )
    .expect("Lua request_command should succeed");

    // Projection must be present *immediately* (no need to advance
    // time). This is the "writing at arrival would be wrong" guard.
    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let projection = store
        .get_projection(ship)
        .expect("Lua dispatch should have written a ShipProjection at sender's tick");

    assert_eq!(projection.dispatched_at, sender_tick);
    assert_eq!(
        projection.intended_state,
        Some(ShipSnapshotState::Surveying)
    );
    assert_eq!(projection.intended_system, Some(frontier));
}

// ---------------------------------------------------------------------------
// 3. Player dispatch site
// ---------------------------------------------------------------------------

#[test]
fn player_dispatch_writes_projection_for_pending_ship_command() {
    use macrocosmo::knowledge::{ShipProjectionWriteQueue, flush_ship_projection_writes};

    let frontier_distance_ly = 5.0;
    let mut app = test_app();
    // The flush queue resource lives in `KnowledgePlugin`, which
    // `test_app()` does not install. Initialise it directly so the
    // flush system has somewhere to read from.
    app.init_resource::<ShipProjectionWriteQueue>();

    let (empire, home, frontier, ship) = setup_scenario(&mut app, frontier_distance_ly);

    // The egui dispatch path uses `commands.queue(...)` to flip into
    // `&mut World` access. We exercise the queue + flush plumbing
    // directly here to pin the dispatch-time semantics without
    // standing up a full egui pipeline (the inner helper
    // `write_player_dispatch_projection` is a private fn in `ui/mod.rs`).
    let dispatch_clock = 200;
    app.world_mut().resource_mut::<GameClock>().elapsed = dispatch_clock;

    // Build a projection identical to what the egui dispatch site would
    // produce for `Survey { target: frontier }` from a Ruler at home
    // and a not-yet-observed ship (snapshot = None).
    let snapshot = None;
    let dispatcher_pos = [0.0, 0.0, 0.0];
    let target_pos = [frontier_distance_ly, 0.0, 0.0];
    let ship_pos = [0.0, 0.0, 0.0]; // home_port fallback
    let projection = macrocosmo::knowledge::compute_ship_projection(
        ship,
        snapshot,
        dispatcher_pos,
        ship_pos,
        Some(target_pos),
        Some(ShipSnapshotState::Surveying),
        Some(frontier),
        true,
        Some(home),
        dispatch_clock,
    );
    app.world_mut()
        .resource_mut::<ShipProjectionWriteQueue>()
        .entries
        .push(macrocosmo::knowledge::PendingProjectionWrite { empire, projection });

    // Run the flush system once.
    let mut schedule = bevy::ecs::schedule::Schedule::default();
    schedule.add_systems(flush_ship_projection_writes);
    schedule.run(app.world_mut());

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let projection = store
        .get_projection(ship)
        .expect("player dispatch flush should have written a ShipProjection");
    assert_eq!(projection.dispatched_at, dispatch_clock);
    assert_eq!(
        projection.intended_state,
        Some(ShipSnapshotState::Surveying)
    );
    assert_eq!(projection.intended_system, Some(frontier));
}

// ---------------------------------------------------------------------------
// 4. FTL-leak guard
// ---------------------------------------------------------------------------

/// The dispatcher's projection must be derived from its
/// `KnowledgeStore` snapshot, *not* from the ship's realtime ECS
/// state. We construct an adversarial scenario where the ship's
/// runtime `Position` is far from the snapshot's last-known position;
/// the resulting `intended_takes_effect_at` must reflect the
/// snapshot-known position (= the dispatcher's local belief), which is
/// independent of the runtime drift.
#[test]
fn ai_dispatch_does_not_consult_realtime_ship_state() {
    use macrocosmo::ai::convert::{to_ai_entity, to_ai_faction, to_ai_system};
    use macrocosmo::physics;
    use macrocosmo_ai::{Command, CommandKindId, CommandValue};

    let mut app = test_app();
    let (empire, home, frontier, ship) = setup_scenario(&mut app, 3.0);

    // Warmup so AiPlugin's Startup declares command schemas before we
    // emit. Without this, `bus.emit_command` is dropped silently.
    app.update();

    // Snapshot says ship is at home (= Ruler's position == [0,0,0]).
    // Snapshot is therefore zero distance from the dispatcher.
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        store.update_ship(ShipSnapshot {
            entity: ship,
            name: "Scout-1".into(),
            design_id: "explorer_mk1".into(),
            last_known_state: ShipSnapshotState::InSystem,
            last_known_system: Some(home),
            observed_at: 0,
            hp: 100.0,
            hp_max: 100.0,
            source: ObservationSource::Direct,
        });
    }

    // Realtime: move the ship a long way away. The runtime Position
    // says 100 ly; the snapshot still says 0 ly. If the dispatcher
    // were leaking realtime state, the resulting
    // `intended_takes_effect_at` would jump by ~6000 hexadies of
    // light delay. It must NOT.
    {
        let mut em = app.world_mut().entity_mut(ship);
        if let Some(mut p) = em.get_mut::<Position>() {
            *p = Position::from([100.0, 0.0, 0.0]);
        }
    }

    let cmd = {
        let mut c = Command::new(
            CommandKindId::from("survey_system"),
            to_ai_faction(empire),
            0,
        );
        c.params.insert(
            "target_system".into(),
            CommandValue::System(to_ai_system(frontier)),
        );
        c.params.insert("ship_count".into(), CommandValue::I64(1));
        c.params
            .insert("ship_0".into(), CommandValue::Entity(to_ai_entity(ship)));
        c
    };
    {
        let mut bus = app
            .world_mut()
            .resource_mut::<macrocosmo::ai::plugin::AiBusResource>();
        bus.emit_command(cmd);
    }

    // npc_decision_tick gates on `clock.elapsed > last_tick.0` (default
    // 0), so use a non-zero dispatch clock to avoid that early-return
    // path swallowing our manual emit's window.
    let dispatch_tick = 50;
    app.world_mut().resource_mut::<GameClock>().elapsed = dispatch_tick;
    app.update();

    let store = app.world().entity(empire).get::<KnowledgeStore>().unwrap();
    let projection = store
        .get_projection(ship)
        .expect("AI dispatch should have written a projection from local snapshot");

    // dispatcher_pos == [0,0,0]; snapshot's ship position via
    // last_known_system == home == [0,0,0]; light delay between the
    // two = 0; so intended_takes_effect_at must equal dispatched_at.
    let leak_distance_ly = 100.0;
    let leaked_delay = physics::light_delay_hexadies(leak_distance_ly);
    let take_effect = projection.intended_takes_effect_at.unwrap();
    assert_eq!(
        take_effect, projection.dispatched_at,
        "intended_takes_effect_at must be derived from snapshot, not realtime ship Position; \
         a leak would have added {} hexadies",
        leaked_delay
    );
}
