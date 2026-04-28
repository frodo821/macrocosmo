//! #472: Regression tests for the ship-destruction observation contract.
//!
//! Codifies the dual-write contract used by #463 (`CoreConquered`) for ship
//! destruction:
//!
//! 1. `GameEvent::ShipDestroyed` is the **omniscient audit-only** record —
//!    one immediate fire at the destruction site.
//! 2. `KnowledgeFact::ShipDestroyed` carries the per-empire delayed
//!    observation; `arrives_at` is gated by light-speed (or relay-shortened)
//!    propagation from the destruction site to each empire's viewer.
//! 3. `KnowledgeFact::ShipMissing` is emitted per-empire from the perception
//!    layer when the grace window expires before destruction light arrives.
//!    No `GameEvent` counterpart — "missing" is an empire-side epistemic
//!    state with no omniscient audit moment.
//! 4. Both the `GameEvent` and the `KnowledgeFact` for a single destruction
//!    share an `EventId` so `NotifiedEventIds` dedupes the player banner.
//!
//! Spec: see `src/events.rs` module docstring (`# GameEvent semantic
//! contract`) and `src/knowledge/facts.rs::KnowledgeFact::ShipDestroyed`.

mod common;

use bevy::prelude::*;
use macrocosmo::components::Position;
use macrocosmo::events::{EventLog, GameEventKind};
use macrocosmo::galaxy::StarSystem;
use macrocosmo::knowledge::{KnowledgeFact, MISSING_GRACE_HEXADIES, PendingFactQueue};
use macrocosmo::physics::light_delay_hexadies;
use macrocosmo::player::*;
use macrocosmo::ship::*;

use common::{
    advance_time, empire_entity, set_empire_viewer_system, spawn_test_ruler, spawn_test_system,
    test_app_with_event_log,
};

/// Spawn a capital star system marked `is_capital=true` for the
/// PlayerRespawn lookup inside `resolve_combat`.
fn spawn_capital(world: &mut World, pos: [f64; 3]) -> Entity {
    let sys = world
        .spawn((
            StarSystem {
                name: "Capital".into(),
                surveyed: true,
                is_capital: true,
                star_type: "default".into(),
            },
            Position::from(pos),
            macrocosmo::galaxy::Sovereignty::default(),
            macrocosmo::technology::TechKnowledge::default(),
            macrocosmo::galaxy::SystemModifiers::default(),
        ))
        .id();
    world.spawn((
        macrocosmo::galaxy::Planet {
            name: "Capital I".into(),
            system: sys,
            planet_type: "default".into(),
        },
        macrocosmo::galaxy::SystemAttributes {
            habitability: 0.7,
            mineral_richness: 0.5,
            energy_potential: 0.5,
            research_potential: 0.5,
            max_building_slots: 4,
        },
        Position::from(pos),
    ));
    sys
}

/// Spawn a ship doomed to die on the first combat tick (hull = 0.01).
fn spawn_doomed_ship(world: &mut World, name: &str, system: Entity, pos: [f64; 3]) -> Entity {
    world
        .spawn((
            Ship {
                name: name.into(),
                design_id: "explorer_mk1".into(),
                hull_id: "corvette".into(),
                modules: Vec::new(),
                owner: Owner::Neutral,
                sublight_speed: 0.75,
                ftl_range: 10.0,
                ruler_aboard: false,
                home_port: system,
                design_revision: 0,
                fleet: None,
            },
            ShipState::InSystem { system },
            Position::from(pos),
            ShipHitpoints {
                hull: 0.01,
                hull_max: 50.0,
                armor: 0.0,
                armor_max: 0.0,
                shield: 0.0,
                shield_max: 0.0,
                shield_regen: 0.0,
            },
            ShipModifiers::default(),
            CommandQueue::default(),
            Cargo::default(),
        ))
        .id()
}

/// #472: A `KnowledgeFact::ShipDestroyed` for a remote destruction must
/// arrive at the empire's `PendingFactQueue` no earlier than the
/// destruction tick + light-speed delay from the destruction site to the
/// empire's viewer.
#[test]
fn ship_destroyed_fact_arrives_at_light_speed_for_distant_empire() {
    let mut app = test_app_with_event_log();

    // Empire viewer at origin, destruction site 5 ly away.
    let capital = spawn_capital(app.world_mut(), [0.0, 0.0, 0.0]);
    let remote_pos = [5.0, 0.0, 0.0];
    let remote = spawn_test_system(app.world_mut(), "Doom-Zone", remote_pos, 0.7, true, false);

    let empire = empire_entity(app.world_mut());
    spawn_test_ruler(app.world_mut(), empire, capital);
    set_empire_viewer_system(app.world_mut(), empire, capital);

    let _ship = spawn_doomed_ship(app.world_mut(), "Far-Away-1", remote, remote_pos);
    let _ = common::spawn_raw_hostile(
        app.world_mut(),
        remote,
        1000.0,
        1000.0,
        100.0,
        0.0,
        "space_creature",
    );

    advance_time(&mut app, 1);

    // `KnowledgeFact::ShipDestroyed` must be enqueued with an arrival time
    // gated by the 5 ly light delay (= 300 hd).
    let queue = app.world().resource::<PendingFactQueue>();
    let pending: Vec<_> = queue
        .facts
        .iter()
        .filter(|pf| matches!(pf.fact, KnowledgeFact::ShipDestroyed { .. }))
        .collect();
    assert!(
        !pending.is_empty(),
        "ShipDestroyed fact must be enqueued when a ship is destroyed"
    );
    let pf = pending[0];
    let expected_delay = light_delay_hexadies(5.0);
    assert_eq!(
        expected_delay, 300,
        "sanity: 5 ly is 300 hd of light-speed delay"
    );
    assert_eq!(
        pf.arrives_at - pf.observed_at,
        expected_delay,
        "ShipDestroyed fact must be light-delayed by exactly {} hd, got {}",
        expected_delay,
        pf.arrives_at - pf.observed_at,
    );
}

/// #472: An on-site destruction (origin == empire viewer position) takes the
/// local-immediate path — `record_fact_or_local` short-circuits to the
/// notification queue, so no fact lands in `PendingFactQueue` (or, if it
/// does for some downstream consumer, `arrives_at == observed_at`).
#[test]
fn ship_destroyed_fact_local_immediate() {
    let mut app = test_app_with_event_log();

    let capital = spawn_capital(app.world_mut(), [0.0, 0.0, 0.0]);
    let empire = empire_entity(app.world_mut());
    spawn_test_ruler(app.world_mut(), empire, capital);
    set_empire_viewer_system(app.world_mut(), empire, capital);

    let _ship = spawn_doomed_ship(app.world_mut(), "Home-Guard", capital, [0.0, 0.0, 0.0]);
    let _ = common::spawn_raw_hostile(
        app.world_mut(),
        capital,
        1000.0,
        1000.0,
        100.0,
        0.0,
        "space_creature",
    );

    advance_time(&mut app, 1);

    // Local short-circuit: any ShipDestroyed fact in the queue must have
    // zero light-speed delay. The `GameEvent` audit fire still produces an
    // EventLog entry on the same tick.
    let queue = app.world().resource::<PendingFactQueue>();
    for pf in &queue.facts {
        if matches!(pf.fact, KnowledgeFact::ShipDestroyed { .. }) {
            assert_eq!(
                pf.arrives_at,
                pf.observed_at,
                "On-site ShipDestroyed fact must be local-immediate, got delay {}",
                pf.arrives_at - pf.observed_at
            );
        }
    }
    let log = app.world().resource::<EventLog>();
    assert!(
        log.entries
            .iter()
            .any(|e| e.kind == GameEventKind::ShipDestroyed),
        "GameEvent::ShipDestroyed must fire immediately at the destruction site"
    );
}

/// #472: For a far-away destruction (light delay ≫ MISSING_GRACE_HEXADIES),
/// a remote empire receives `ShipMissing` first (at grace expiry) then
/// `ShipDestroyed` (at light arrival).
#[test]
fn ship_missing_precedes_destroyed_for_remote_empire() {
    let mut app = test_app_with_event_log();

    let capital = spawn_capital(app.world_mut(), [0.0, 0.0, 0.0]);
    // 10 ly is well beyond MISSING_GRACE_HEXADIES (5 hd by default).
    let remote_pos = [10.0, 0.0, 0.0];
    let remote = spawn_test_system(app.world_mut(), "Doom-Zone", remote_pos, 0.7, true, false);

    let empire = empire_entity(app.world_mut());
    spawn_test_ruler(app.world_mut(), empire, capital);
    set_empire_viewer_system(app.world_mut(), empire, capital);

    let _ship = spawn_doomed_ship(app.world_mut(), "Lost-Ship", remote, remote_pos);
    let _ = common::spawn_raw_hostile(
        app.world_mut(),
        remote,
        1000.0,
        1000.0,
        100.0,
        0.0,
        "space_creature",
    );

    advance_time(&mut app, 1);

    // Sanity: light delay is much larger than the grace window.
    let light_delay = light_delay_hexadies(10.0);
    assert!(
        light_delay > MISSING_GRACE_HEXADIES,
        "light delay ({}) must exceed grace window ({}) for this test",
        light_delay,
        MISSING_GRACE_HEXADIES
    );

    // Tick past the grace window but before the light arrives.
    advance_time(&mut app, MISSING_GRACE_HEXADIES + 1);

    let queue = app.world().resource::<PendingFactQueue>();
    let missing_count = queue
        .facts
        .iter()
        .filter(|pf| matches!(pf.fact, KnowledgeFact::ShipMissing { .. }))
        .count();
    assert!(
        missing_count >= 1,
        "ShipMissing fact must be emitted once the grace window expires"
    );

    // Tick to (well past) light arrival; the destroyed fact must be present.
    advance_time(&mut app, light_delay);

    let queue = app.world().resource::<PendingFactQueue>();
    let destroyed_count = queue
        .facts
        .iter()
        .filter(|pf| {
            matches!(pf.fact, KnowledgeFact::ShipDestroyed { .. })
                && pf.arrives_at
                    <= app
                        .world()
                        .resource::<macrocosmo::time_system::GameClock>()
                        .elapsed
        })
        .count();
    // The fact lands in the queue at observed_at and `arrives_at` is
    // observed_at + light_delay — once the clock reaches that value the
    // dispatch system would normally drain it. We just check it exists at
    // or before "now".
    assert!(
        destroyed_count >= 1,
        "ShipDestroyed fact must have a settled arrival by now"
    );
}

/// #472: `GameEvent::ShipDestroyed` fires exactly once at the destruction
/// site (not once per empire, not at light-arrival). Pre-#472 the event
/// was deferred to per-empire light-arrival timing; pre-#435 it fired
/// immediately at every empire (audit-equivalent today).
#[test]
fn game_event_ship_destroyed_fires_once_at_destruction_site() {
    let mut app = test_app_with_event_log();

    let capital = spawn_capital(app.world_mut(), [0.0, 0.0, 0.0]);
    let remote_pos = [3.0, 0.0, 0.0];
    let remote = spawn_test_system(app.world_mut(), "Doom-Zone", remote_pos, 0.7, true, false);

    let empire = empire_entity(app.world_mut());
    spawn_test_ruler(app.world_mut(), empire, capital);
    set_empire_viewer_system(app.world_mut(), empire, capital);

    let _ship = spawn_doomed_ship(app.world_mut(), "Audit-Subject", remote, remote_pos);
    let _ = common::spawn_raw_hostile(
        app.world_mut(),
        remote,
        1000.0,
        1000.0,
        100.0,
        0.0,
        "space_creature",
    );

    advance_time(&mut app, 1);

    let log = app.world().resource::<EventLog>();
    let count = log
        .entries
        .iter()
        .filter(|e| e.kind == GameEventKind::ShipDestroyed)
        .count();
    assert_eq!(
        count,
        1,
        "GameEvent::ShipDestroyed must fire exactly once at the destruction \
         tick (omniscient audit). EventLog: {:?}",
        log.entries
            .iter()
            .map(|e| (&e.kind, &e.description))
            .collect::<Vec<_>>()
    );

    // Tick past light arrival; no extra ShipDestroyed event must appear.
    advance_time(&mut app, light_delay_hexadies(3.0) + 5);

    let log = app.world().resource::<EventLog>();
    let count_after = log
        .entries
        .iter()
        .filter(|e| e.kind == GameEventKind::ShipDestroyed)
        .count();
    assert_eq!(
        count_after, 1,
        "GameEvent::ShipDestroyed must NOT re-fire when light arrives at the \
         viewer (the per-empire observation flows through KnowledgeFact)."
    );
}

/// #472: The `EventId` allocated for the audit `GameEvent::ShipDestroyed`
/// must be the same id carried by the per-faction
/// `KnowledgeFact::ShipDestroyed`, so `NotifiedEventIds` dedupes the
/// banner on the local-immediate path.
#[test]
fn ship_destroyed_event_id_matches_fact() {
    let mut app = test_app_with_event_log();

    let capital = spawn_capital(app.world_mut(), [0.0, 0.0, 0.0]);
    let remote_pos = [4.0, 0.0, 0.0];
    let remote = spawn_test_system(app.world_mut(), "Doom-Zone", remote_pos, 0.7, true, false);

    let empire = empire_entity(app.world_mut());
    spawn_test_ruler(app.world_mut(), empire, capital);
    set_empire_viewer_system(app.world_mut(), empire, capital);

    let _ship = spawn_doomed_ship(app.world_mut(), "ID-Pair", remote, remote_pos);
    let _ = common::spawn_raw_hostile(
        app.world_mut(),
        remote,
        1000.0,
        1000.0,
        100.0,
        0.0,
        "space_creature",
    );

    advance_time(&mut app, 1);

    let log = app.world().resource::<EventLog>();
    let event = log
        .entries
        .iter()
        .find(|e| e.kind == GameEventKind::ShipDestroyed)
        .expect("GameEvent::ShipDestroyed must be present in the log");
    let event_id = event.id;

    let queue = app.world().resource::<PendingFactQueue>();
    let fact_with_id = queue
        .facts
        .iter()
        .find_map(|pf| match &pf.fact {
            KnowledgeFact::ShipDestroyed {
                event_id: Some(id), ..
            } => Some(*id),
            _ => None,
        })
        .expect("KnowledgeFact::ShipDestroyed must carry an EventId");
    assert_eq!(
        event_id, fact_with_id,
        "GameEvent and KnowledgeFact must share the same EventId for \
         banner dedup (#249 / #472)"
    );
}
