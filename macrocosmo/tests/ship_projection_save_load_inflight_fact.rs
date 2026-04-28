//! #483 regression: in-flight `PendingFactQueue` entries must reconcile
//! against `ShipProjection` after a save/load round-trip.
//!
//! Prior to #483, `SavedKnowledgeFact::{ShipArrived, SurveyComplete,
//! ShipDestroyed, ShipMissing}` dropped the live `ship: Entity` field on
//! serialize and rehydrated it as `Entity::PLACEHOLDER` on load. The
//! `reconcile_ship_projections` system skips PLACEHOLDER-keyed facts as
//! "no projection match", so any fact that was queued in flight at save
//! time would never reconcile against the dispatcher's projection
//! post-load — leaving the dispatched ship's `intended_*` projection layer
//! set indefinitely.
//!
//! The fix adds `ship_bits: u64` to the four ship-keyed saved variants and
//! remaps through `EntityMap` on load (SAVE_VERSION 18 → 19). This test
//! pins the contract by:
//!
//! 1. Building an empire 5 ly from a frontier system, dispatching a
//!    `Survey` command (which writes a `ShipProjection` with `intended_*`).
//! 2. Recording a `KnowledgeFact::SurveyComplete` for the surveying ship
//!    into `PendingFactQueue` with `arrives_at` BEYOND the current clock
//!    (= the fact is in flight, not yet applied).
//! 3. Saving the world while the fact is still in flight.
//! 4. Loading into a fresh `World`, advancing the clock past per-empire
//!    light arrival, and running `reconcile_ship_projections`.
//! 5. Asserting that the loaded empire's projection had its `intended_*`
//!    cleared — proving the round-tripped fact carried a non-PLACEHOLDER
//!    ship reference and matched the projection key.
//!
//! Without #483 this test fails: the loaded fact's `ship` is
//! `Entity::PLACEHOLDER` so the reconciler skips it, leaving `intended_*`
//! set.

mod common;

use bevy::prelude::*;

use macrocosmo::knowledge::{
    KnowledgeFact, KnowledgeStore, ObservationSource, PendingFactQueue, PerceivedFact,
    ShipSnapshotState, SystemVisibilityMap, SystemVisibilityTier, reconcile_ship_projections,
};
use macrocosmo::persistence::{load::load_game_from_reader, save::save_game_to_writer};
use macrocosmo::physics::light_delay_hexadies;
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::{Owner, Ship};
use macrocosmo::time_system::GameClock;

use common::{spawn_test_ruler, spawn_test_ship, spawn_test_system, test_app};

const FRONTIER_DISTANCE_LY: f64 = 5.0;

/// Build a single-empire scenario with a ship docked at home and a
/// surveyed frontier system 5 ly away. Mirrors the shape used by
/// `tests/ship_projection.rs::setup_scenario` but inlined so this file
/// has no implicit coupling to the bigger e2e suite.
struct Scenario {
    empire: Entity,
    home: Entity,
    frontier: Entity,
    ship: Entity,
    /// Light delay between home and frontier in hexadies (= 300 for 5 ly).
    light_delay: i64,
}

fn setup_scenario(app: &mut App) -> Scenario {
    let empire = app
        .world_mut()
        .spawn((
            Empire {
                name: "Test".into(),
            },
            PlayerEmpire,
            Faction {
                id: "save_load_inflight_fact".into(),
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
        [FRONTIER_DISTANCE_LY, 0.0, 0.0],
        1.0,
        true,
        false,
    );
    {
        let mut em = app.world_mut().entity_mut(empire);
        let mut vis = em.get_mut::<SystemVisibilityMap>().unwrap();
        vis.set(home, SystemVisibilityTier::Local);
        vis.set(frontier, SystemVisibilityTier::Surveyed);
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

    let light_delay = light_delay_hexadies(FRONTIER_DISTANCE_LY);
    assert_eq!(
        light_delay, 300,
        "sanity: 5 ly is 300 hexadies of light-speed delay"
    );

    Scenario {
        empire,
        home,
        frontier,
        ship,
        light_delay,
    }
}

/// Issue a `Survey` command via the Lua dispatch site so the
/// `ShipProjection` is populated with `intended_*` exactly as the live
/// dispatcher would. Returns the dispatch tick.
fn dispatch_survey_command(app: &mut App, ship: Entity, frontier: Entity) -> i64 {
    use macrocosmo::scripting::gamestate_scope::apply::{ParsedRequest, request_command};

    if app
        .world()
        .get_resource::<macrocosmo::ship::command_events::NextCommandId>()
        .is_none()
    {
        app.world_mut()
            .insert_resource(macrocosmo::ship::command_events::NextCommandId::default());
    }

    let dispatch_tick = app.world().resource::<GameClock>().elapsed;
    request_command(
        app.world_mut(),
        ParsedRequest::Survey {
            ship,
            target_system: frontier,
        },
    )
    .expect("Lua request_command should succeed");
    dispatch_tick
}

#[test]
fn inflight_survey_complete_fact_reconciles_after_save_load() {
    let mut src = test_app();
    let s = setup_scenario(&mut src);

    // Dispatch the survey. After this, the empire's projection for `ship`
    // has `intended_state = Some(Surveying)`, `intended_system = Some(frontier)`,
    // and `intended_takes_effect_at = Some(T0 + light_delay)`.
    let _t0 = dispatch_survey_command(&mut src, s.ship, s.frontier);

    // Sanity-check the source projection has the intended_* layer set.
    {
        let p = src
            .world()
            .entity(s.empire)
            .get::<KnowledgeStore>()
            .expect("empire has KnowledgeStore")
            .get_projection(s.ship)
            .expect("dispatch must populate a projection")
            .clone();
        assert_eq!(p.intended_state, Some(ShipSnapshotState::Surveying));
        assert_eq!(p.intended_system, Some(s.frontier));
        assert!(
            p.intended_takes_effect_at.is_some(),
            "dispatch must populate intended_takes_effect_at"
        );
    }

    // Push a `SurveyComplete` fact into PendingFactQueue. The fact is
    // observed at the frontier at tick `observed_at`; per-empire arrival
    // at the home empire is `observed_at + light_delay`. We deliberately
    // keep the clock BELOW that arrival so the reconciler does NOT
    // consume the fact pre-save — it is in flight at save time, which is
    // the exact scenario the issue describes.
    let observed_at: i64 = 50;
    let pf = PerceivedFact {
        fact: KnowledgeFact::SurveyComplete {
            event_id: None,
            system: s.frontier,
            system_name: "Frontier".into(),
            detail: "Surveyed".into(),
            ship: s.ship,
        },
        observed_at,
        // `arrives_at` is recomputed per-empire by the reconciler from
        // origin_pos vs the empire's vantage; the stored value is just
        // the first vantage that recorded it. We set it to the home
        // empire's expected arrival for shape-conformance.
        arrives_at: observed_at + s.light_delay,
        source: ObservationSource::Direct,
        origin_pos: [FRONTIER_DISTANCE_LY, 0.0, 0.0],
        related_system: Some(s.frontier),
    };
    src.world_mut()
        .resource_mut::<PendingFactQueue>()
        .record(pf);

    // Pre-save sanity: clock is well before per-empire arrival, so the
    // fact is genuinely "in flight" and the reconciler must NOT have
    // applied it yet.
    src.world_mut().resource_mut::<GameClock>().elapsed = observed_at;
    {
        let mut schedule = bevy::ecs::schedule::Schedule::default();
        schedule.add_systems(reconcile_ship_projections);
        schedule.run(src.world_mut());
        let p = src
            .world()
            .entity(s.empire)
            .get::<KnowledgeStore>()
            .unwrap()
            .get_projection(s.ship)
            .unwrap()
            .clone();
        assert_eq!(
            p.intended_state,
            Some(ShipSnapshotState::Surveying),
            "fact must NOT have reconciled yet — clock is below per-empire \
             arrival, so the fact is in flight at save time"
        );
    }

    // Save the world while the fact is still in flight.
    let mut bytes: Vec<u8> = Vec::new();
    save_game_to_writer(src.world_mut(), &mut bytes).expect("save");
    assert!(!bytes.is_empty(), "postcard produced an empty blob");

    // Load into a fresh world.
    let mut dst = World::new();
    load_game_from_reader(&mut dst, &bytes[..]).expect("load");

    // Re-resolve the empire entity in the loaded world.
    let dst_empire = dst
        .query_filtered::<Entity, With<PlayerEmpire>>()
        .iter(&dst)
        .next()
        .expect("PlayerEmpire must round-trip");

    // Re-resolve the ship and frontier system entities by name (the
    // loaded entity ids are remapped through EntityMap).
    let dst_ship = {
        let mut q = dst.query::<(Entity, &Ship)>();
        q.iter(&dst)
            .find(|(_, sh)| sh.name == "Scout-1")
            .map(|(e, _)| e)
            .expect("Scout-1 must round-trip")
    };
    let dst_frontier = {
        let mut q = dst.query::<(Entity, &macrocosmo::galaxy::StarSystem)>();
        q.iter(&dst)
            .find(|(_, sys)| sys.name == "Frontier")
            .map(|(e, _)| e)
            .expect("Frontier system must round-trip")
    };

    // Pre-condition: the projection survived the round-trip and still
    // carries the intended_* layer (= the fact has not yet reconciled).
    {
        let p = dst
            .entity(dst_empire)
            .get::<KnowledgeStore>()
            .expect("loaded empire has KnowledgeStore")
            .get_projection(dst_ship)
            .expect("ShipProjection must round-trip and remap to dst_ship")
            .clone();
        assert_eq!(
            p.intended_state,
            Some(ShipSnapshotState::Surveying),
            "round-tripped projection must still carry intended_*"
        );
        assert_eq!(p.intended_system, Some(dst_frontier));
    }

    // Pre-condition: the queued fact survived the round-trip and the
    // remapped fact's `ship` field is the loaded ship entity (NOT
    // `Entity::PLACEHOLDER`). This is the actual #483 contract — the
    // assertion that fails on the unfixed code.
    {
        let queue = dst
            .get_resource::<PendingFactQueue>()
            .expect("PendingFactQueue must round-trip");
        let pf = queue
            .facts
            .iter()
            .find(|pf| matches!(pf.fact, KnowledgeFact::SurveyComplete { .. }))
            .expect("SurveyComplete fact must round-trip in PendingFactQueue");
        let KnowledgeFact::SurveyComplete { ship, .. } = pf.fact else {
            unreachable!()
        };
        assert_eq!(
            ship, dst_ship,
            "round-tripped SurveyComplete must carry the remapped ship \
             entity (not Entity::PLACEHOLDER) — this is the core #483 fix"
        );
    }

    // Make sure RelayNetwork exists in the loaded world (the load
    // pipeline may not always seed it for hand-built test worlds).
    if dst
        .get_resource::<macrocosmo::knowledge::RelayNetwork>()
        .is_none()
    {
        dst.insert_resource(macrocosmo::knowledge::RelayNetwork::default());
    }

    // Advance the loaded clock past per-empire light arrival.
    let arrival_tick = observed_at + s.light_delay + 1;
    if dst.get_resource::<GameClock>().is_none() {
        dst.insert_resource(GameClock::new(arrival_tick));
    } else {
        dst.resource_mut::<GameClock>().elapsed = arrival_tick;
    }

    // Run the reconciler in isolation (matches the pattern in the
    // existing ship_projection.rs end-to-end suite).
    let mut schedule = bevy::ecs::schedule::Schedule::default();
    schedule.add_systems(reconcile_ship_projections);
    schedule.run(&mut dst);

    // Acceptance: the round-tripped in-flight fact has reconciled.
    let p_after = dst
        .entity(dst_empire)
        .get::<KnowledgeStore>()
        .expect("loaded empire has KnowledgeStore")
        .get_projection(dst_ship)
        .expect("projection retained after reconcile")
        .clone();
    assert_eq!(
        p_after.projected_state,
        ShipSnapshotState::InSystem,
        "loaded-world reconcile of an in-flight SurveyComplete must \
         advance projected_state to InSystem"
    );
    assert_eq!(
        p_after.projected_system,
        Some(dst_frontier),
        "loaded-world reconcile must advance projected_system to the \
         surveyed frontier"
    );
    assert!(
        p_after.intended_state.is_none() && p_after.intended_system.is_none(),
        "loaded-world reconcile of an in-flight fact must clear intended_* \
         — without #483 this would still be Some(Surveying)"
    );
    assert!(
        p_after.intended_takes_effect_at.is_none(),
        "loaded-world reconcile must clear intended_takes_effect_at"
    );
    let _ = s.home; // suppress unused-field on the Scenario builder
}
