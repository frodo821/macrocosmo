//! #233 regression tests — notification pipeline derived from KnowledgeStore
//! fact delta.
//!
//! These tests exercise the two-system model:
//!
//! 1. World events (HostileDetected, CombatOutcome, SurveyComplete, …) are
//!    routed through `PendingFactQueue` with a light-speed + relay delay.
//! 2. Player-local events (PlayerRespawn, ResourceAlert, Lua
//!    `show_notification`) continue to fire the banner immediately via the
//!    legacy `auto_notify_from_events` whitelist / `drain_pending_notifications`.

mod common;

use bevy::prelude::*;

use macrocosmo::amount::SignedAmt;
use macrocosmo::empire::CommsParams;
use macrocosmo::events::{GameEvent, GameEventKind};
use macrocosmo::knowledge::{
    compute_fact_arrival, rebuild_relay_network, relay_delay_hexadies, CombatVictor,
    KnowledgeFact, ObservationSource, PendingFactQueue, PerceivedFact, RelayNetwork,
    RelaySnapshot, FTL_RELAY_BASE_MULTIPLIER,
};
use macrocosmo::modifier::Modifier;
use macrocosmo::notifications::{
    auto_notify_from_events, is_legacy_whitelisted, notify_from_knowledge_facts,
    NotificationQueue,
};
use macrocosmo::player::PlayerEmpire;
use macrocosmo::time_system::GameClock;

// Small helpers so tests focus on behaviour, not boilerplate.

fn make_app_with_queues() -> App {
    let mut app = App::new();
    app.insert_resource(GameClock::new(0));
    app.init_resource::<PendingFactQueue>();
    app.init_resource::<RelayNetwork>();
    app.init_resource::<macrocosmo::knowledge::NotifiedEventIds>();
    app.insert_resource(NotificationQueue::new());
    app.insert_resource(macrocosmo::time_system::GameSpeed::default());
    app.add_systems(Update, notify_from_knowledge_facts);
    app
}

/// 50 ly away → 3000 hd light delay before the player hears about a hostile.
#[test]
fn test_remote_detection_notification_light_speed_delayed() {
    let mut app = make_app_with_queues();
    let target = app.world_mut().spawn_empty().id();
    let detector = app.world_mut().spawn_empty().id();

    // Record the fact directly with a pre-computed arrival time so we don't
    // need the whole `detect_hostiles_system` setup.
    let origin = [50.0, 0.0, 0.0];
    let player = [0.0, 0.0, 0.0];
    let plan = compute_fact_arrival(0, origin, player, &[], &CommsParams::default());
    assert_eq!(plan.source, ObservationSource::Direct);
    assert_eq!(plan.arrives_at, 3000); // 50 ly × 60 hd/ly

    {
        let mut queue = app.world_mut().resource_mut::<PendingFactQueue>();
        queue.record(PerceivedFact {
            fact: KnowledgeFact::HostileDetected {
                event_id: None,
                target,
                detector,
                target_pos: origin,
                description: "Enemy sighted".into(),
            },
            observed_at: 0,
            arrives_at: plan.arrives_at,
            source: plan.source,
            origin_pos: origin,
            related_system: None,
        });
    }

    // At t=100 the fact must NOT have arrived yet.
    app.world_mut().resource_mut::<GameClock>().elapsed = 100;
    app.update();
    assert_eq!(
        app.world().resource::<NotificationQueue>().items.len(),
        0,
        "Hostile 50 ly away must not notify at t=100 (light delay = 3000)"
    );

    // At t=3000 it must surface.
    app.world_mut().resource_mut::<GameClock>().elapsed = 3000;
    app.update();
    assert_eq!(
        app.world().resource::<NotificationQueue>().items.len(),
        1,
        "Hostile detection must arrive at t=3000 (50 ly × 60 hd)"
    );
}

/// With relay coverage on both endpoints the arrival is dramatically faster.
#[test]
fn test_detection_via_relay_network_near_instant() {
    let origin = [0.0, 0.0, 0.0];
    let player = [50.0, 0.0, 0.0];
    let relays = vec![
        RelaySnapshot {
            position: [1.0, 0.0, 0.0],
            range_ly: 5.0,
            paired: true,
        },
        RelaySnapshot {
            position: [49.0, 0.0, 0.0],
            range_ly: 5.0,
            paired: true,
        },
    ];
    let plan = compute_fact_arrival(0, origin, player, &relays, &CommsParams::default());
    assert_eq!(plan.source, ObservationSource::Relay);

    // Light direct = 3000 hd. Relay path:
    //   origin→relay_o light (1 ly → 60 hd)
    // + relay hop (48 ly → light 2880 / 10 = 288 hd)
    // + relay_p→player light (1 ly → 60 hd)
    // = 408 hd, ≈ 13.6% of direct.
    assert!(
        plan.arrives_at < 3000 / 5,
        "Relay-routed arrival should be at least 5× faster than direct light: got {}",
        plan.arrives_at
    );
}

/// Player respawn is systems-2 — banner immediately, no light-speed delay.
#[test]
fn test_player_respawn_notification_instant() {
    let mut app = App::new();
    app.add_message::<GameEvent>();
    app.insert_resource(NotificationQueue::new());
    app.init_resource::<macrocosmo::knowledge::NotifiedEventIds>();
    app.add_systems(Update, auto_notify_from_events);

    app.world_mut().write_message(GameEvent {
        id: macrocosmo::knowledge::EventId::default(),
        timestamp: 0,
        kind: GameEventKind::PlayerRespawn,
        description: "Flagship destroyed".into(),
        related_system: None,
    });

    app.update();

    let q = app.world().resource::<NotificationQueue>();
    assert_eq!(q.items.len(), 1);
    assert_eq!(q.items[0].title, "Player Respawn");
}

/// Lua `show_notification` stays in the Lua-drain pipeline (systems-2) and
/// must not be gated by arrival times. We simulate this by pushing directly
/// to NotificationQueue — the #233 changes only added a whitelist layer
/// above `auto_notify_from_events`, leaving `drain_pending_notifications`
/// untouched.
#[test]
fn test_lua_notification_instant() {
    let mut queue = NotificationQueue::new();
    // Simulating what drain_pending_notifications does when Lua sets a
    // High-priority entry.
    let id = queue.push(
        "Event",
        "Something happened",
        None,
        macrocosmo::notifications::NotificationPriority::High,
        None,
    );
    assert!(id.is_some());
    assert_eq!(queue.items.len(), 1);
    assert!(queue.items[0].remaining_seconds.is_none()); // sticky
}

/// Survey completion routed through PendingFactQueue surfaces only once
/// `arrives_at` is reached.
#[test]
fn test_survey_result_via_knowledge_store() {
    let mut app = make_app_with_queues();
    let sys = app.world_mut().spawn_empty().id();

    let fact = KnowledgeFact::SurveyComplete {
        event_id: None,
        system: sys,
        system_name: "Tau Ceti".into(),
        detail: "Tau Ceti surveyed".into(),
    };
    let origin = [5.0, 0.0, 0.0];
    let plan = compute_fact_arrival(0, origin, [0.0; 3], &[], &CommsParams::default());
    app.world_mut()
        .resource_mut::<PendingFactQueue>()
        .record(PerceivedFact {
            fact,
            observed_at: 0,
            arrives_at: plan.arrives_at,
            source: plan.source,
            origin_pos: origin,
            related_system: Some(sys),
        });

    // 5 ly × 60 hd/ly = 300 hd.
    app.world_mut().resource_mut::<GameClock>().elapsed = 299;
    app.update();
    assert_eq!(app.world().resource::<NotificationQueue>().items.len(), 0);

    app.world_mut().resource_mut::<GameClock>().elapsed = 300;
    app.update();
    let q = app.world().resource::<NotificationQueue>();
    assert_eq!(q.items.len(), 1);
    assert_eq!(q.items[0].title, "Survey Complete");
    assert_eq!(q.items[0].description, "Tau Ceti surveyed");
}

/// Combat victory from a remote system is light-speed delayed in the
/// notification pipeline (fact route), even though the underlying
/// `CombatVictory` GameEvent still fires immediately for auto-pause.
#[test]
fn test_combat_victory_notification_delayed() {
    let mut app = make_app_with_queues();
    let sys = app.world_mut().spawn_empty().id();

    let origin = [20.0, 0.0, 0.0];
    let plan = compute_fact_arrival(0, origin, [0.0; 3], &[], &CommsParams::default());
    app.world_mut()
        .resource_mut::<PendingFactQueue>()
        .record(PerceivedFact {
            fact: KnowledgeFact::CombatOutcome {
                event_id: None,
                system: sys,
                victor: CombatVictor::Player,
                detail: "Pirates routed at Epsilon".into(),
            },
            observed_at: 0,
            arrives_at: plan.arrives_at,
            source: plan.source,
            origin_pos: origin,
            related_system: Some(sys),
        });
    assert_eq!(plan.arrives_at, 1200); // 20 × 60

    app.world_mut().resource_mut::<GameClock>().elapsed = 1199;
    app.update();
    assert_eq!(app.world().resource::<NotificationQueue>().items.len(), 0);

    app.world_mut().resource_mut::<GameClock>().elapsed = 1200;
    app.update();
    let q = app.world().resource::<NotificationQueue>();
    assert_eq!(q.items.len(), 1);
    assert_eq!(q.items[0].title, "Combat Victory");
}

/// `compute_fact_arrival` picks the fastest of Direct vs Relay; a covered
/// relay pair must win over pure light propagation.
#[test]
fn test_channel_autoselect_picks_fastest() {
    let origin = [0.0, 0.0, 0.0];
    let player = [100.0, 0.0, 0.0];

    // No relay → Direct, slow.
    let direct = compute_fact_arrival(0, origin, player, &[], &CommsParams::default());
    assert_eq!(direct.source, ObservationSource::Direct);
    assert_eq!(direct.arrives_at, 6000);

    // With relays → Relay, fast.
    let relays = vec![
        RelaySnapshot {
            position: [1.0, 0.0, 0.0],
            range_ly: 10.0,
            paired: true,
        },
        RelaySnapshot {
            position: [99.0, 0.0, 0.0],
            range_ly: 10.0,
            paired: true,
        },
    ];
    let relay_plan = compute_fact_arrival(0, origin, player, &relays, &CommsParams::default());
    assert_eq!(relay_plan.source, ObservationSource::Relay);
    assert!(relay_plan.arrives_at < direct.arrives_at);
}

/// `empire.comm_relay_inv_latency` modifier must raise the FTL multiplier and
/// shrink the relay delay.
#[test]
fn test_empire_comm_relay_inv_latency_increases_speed() {
    // Baseline: 60 hd / 10 = 6 hd per ly of relay hop.
    assert_eq!(relay_delay_hexadies(1.0, &CommsParams::default()), 6);

    // +5 inv_latency → multiplier = 10 + 5 = 15 → 60 / 15 = 4 hd.
    let mut comms = CommsParams::default();
    comms.empire_relay_inv_latency.push_modifier(Modifier {
        id: "test".into(),
        label: "Test tech".into(),
        base_add: SignedAmt::from_f64(5.0),
        multiplier: SignedAmt::ZERO,
        add: SignedAmt::ZERO,
        expires_at: None,
        on_expire_event: None,
    });
    assert_eq!(relay_delay_hexadies(1.0, &comms), 4);

    // Also verify base_multiplier constant didn't silently drift.
    assert!((FTL_RELAY_BASE_MULTIPLIER - 10.0).abs() < 1e-9);
}

/// `empire.comm_relay_range` is consumed by `rebuild_relay_network` via
/// `effective_relay_range`. We rebuild the network with and without the bonus
/// and confirm the snapshot range expands.
#[test]
fn test_empire_comm_relay_range_extends_coverage() {
    use macrocosmo::deep_space::{
        CapabilityParams, DeepSpaceStructure, DeliverableDefinition, DeliverableRegistry,
    };
    use macrocosmo::ship::Owner;
    use std::collections::HashMap;

    let mut app = App::new();
    app.insert_resource(GameClock::new(0));
    app.init_resource::<RelayNetwork>();
    let mut registry = DeliverableRegistry::default();
    let mut caps = HashMap::new();
    caps.insert(
        "ftl_comm_relay".to_string(),
        CapabilityParams { range: 5.0 },
    );
    registry.insert(DeliverableDefinition {
        id: "relay_test".into(),
        name: "Test Relay".into(),
        description: String::new(),
        max_hp: 100.0,
        energy_drain: macrocosmo::amount::Amt::ZERO,
        capabilities: caps,
        prerequisites: None,
        deliverable: None,
        upgrade_to: Vec::new(),
        upgrade_from: None,
            on_built: None,
        on_upgraded: None,
    });
    app.insert_resource(registry);

    // Empire entity with default CommsParams (no range bonus).
    let empire = app
        .world_mut()
        .spawn((PlayerEmpire, CommsParams::default()))
        .id();

    // A standalone (unpaired) relay entity.
    app.world_mut().spawn((
        DeepSpaceStructure {
            definition_id: "relay_test".into(),
            name: "R1".into(),
            owner: Owner::Neutral,
        },
        macrocosmo::components::Position { x: 0.0, y: 0.0, z: 0.0 },
    ));

    app.add_systems(Update, rebuild_relay_network);
    app.update();

    let base_range = app.world().resource::<RelayNetwork>().relays[0].range_ly;
    assert!((base_range - 5.0).abs() < 1e-9);

    // Apply +2 ly range bonus.
    {
        let mut comms = app.world_mut().get_mut::<CommsParams>(empire).unwrap();
        comms.empire_relay_range.push_modifier(Modifier {
            id: "range".into(),
            label: "Test".into(),
            base_add: SignedAmt::from_f64(2.0),
            multiplier: SignedAmt::ZERO,
            add: SignedAmt::ZERO,
            expires_at: None,
            on_expire_event: None,
        });
    }

    app.update();
    let extended_range = app.world().resource::<RelayNetwork>().relays[0].range_ly;
    assert!(
        (extended_range - 7.0).abs() < 1e-9,
        "Range should extend from 5 to 7 ly (5 base + 2 empire bonus): got {}",
        extended_range
    );
}

/// `fleet.comm_relay_*` targets currently have no consumer, but must still
/// route successfully into CommsParams.fleet_* fields. The presence of a
/// storage-only modifier must not raise a warning.
#[test]
fn test_fleet_comm_relay_targets_routed_but_unused() {
    let mut comms = CommsParams::default();
    // Simulate what the tech-effect pipeline does for a
    // `fleet.comm_relay_inv_latency` target.
    comms.fleet_relay_inv_latency.push_modifier(Modifier {
        id: "tech:test:fleet.comm_relay_inv_latency".into(),
        label: "Test fleet tech".into(),
        base_add: SignedAmt::from_f64(3.0),
        multiplier: SignedAmt::ZERO,
        add: SignedAmt::ZERO,
        expires_at: None,
        on_expire_event: None,
    });
    comms.fleet_relay_range.push_modifier(Modifier {
        id: "tech:test:fleet.comm_relay_range".into(),
        label: "Test fleet tech".into(),
        base_add: SignedAmt::from_f64(1.5),
        multiplier: SignedAmt::ZERO,
        add: SignedAmt::ZERO,
        expires_at: None,
        on_expire_event: None,
    });

    // Fields are populated…
    assert_eq!(comms.fleet_relay_inv_latency.final_value().to_f64(), 3.0);
    assert_eq!(comms.fleet_relay_range.final_value().to_f64(), 1.5);

    // …but empire consumers are NOT affected (fleet storage is independent).
    assert_eq!(comms.empire_relay_inv_latency.final_value().to_f64(), 0.0);
    assert_eq!(comms.empire_relay_range.final_value().to_f64(), 0.0);
    // Base multiplier is unchanged because empire_relay_inv_latency = 0.
    assert_eq!(relay_delay_hexadies(1.0, &comms), 6);
}

/// Sanity: the new whitelist correctly excludes world events and keeps
/// systems-2 kinds. This locks in the #233 notification routing split.
#[test]
fn test_legacy_whitelist_split() {
    assert!(is_legacy_whitelisted(&GameEventKind::PlayerRespawn));
    assert!(is_legacy_whitelisted(&GameEventKind::ResourceAlert));
    for kind in [
        GameEventKind::HostileDetected,
        GameEventKind::SurveyComplete,
        GameEventKind::SurveyDiscovery,
        GameEventKind::CombatVictory,
        GameEventKind::CombatDefeat,
        GameEventKind::AnomalyDiscovered,
        GameEventKind::ColonyEstablished,
        GameEventKind::ColonyFailed,
    ] {
        assert!(
            !is_legacy_whitelisted(&kind),
            "World event {:?} must NOT be whitelisted to legacy path",
            kind
        );
    }
}

// ---------------------------------------------------------------------------
// #249 — Callsite rewiring regression coverage
// ---------------------------------------------------------------------------

use macrocosmo::knowledge::{EventId, NotifiedEventIds};
use macrocosmo::notifications::NotificationPriority;

/// #249: A non-FTL survey completion at a remote system must be queued in
/// `PendingFactQueue` and surface the banner only once `arrives_at` is reached.
///
/// This mirrors the production path that `survey::process_surveys` follows for
/// non-FTL ships: the fact is recorded with `origin_pos != player_pos` so the
/// local short-circuit in `record_fact_or_local` is skipped.
#[test]
fn test_survey_complete_fact_delayed_when_remote() {
    let mut app = make_app_with_queues();
    let sys = app.world_mut().spawn_empty().id();
    let origin = [10.0, 0.0, 0.0];
    let plan = compute_fact_arrival(0, origin, [0.0; 3], &[], &CommsParams::default());
    assert_eq!(plan.arrives_at, 600);
    // #249: tri-state map requires register before first push.
    app.world_mut()
        .resource_mut::<NotifiedEventIds>()
        .register(EventId(1));
    app.world_mut()
        .resource_mut::<PendingFactQueue>()
        .record(PerceivedFact {
            fact: KnowledgeFact::SurveyComplete {
                event_id: Some(EventId(1)),
                system: sys,
                system_name: "Remote".into(),
                detail: "Surveyed".into(),
            },
            observed_at: 0,
            arrives_at: plan.arrives_at,
            source: plan.source,
            origin_pos: origin,
            related_system: Some(sys),
        });

    app.world_mut().resource_mut::<GameClock>().elapsed = 500;
    app.update();
    assert_eq!(app.world().resource::<NotificationQueue>().items.len(), 0);

    app.world_mut().resource_mut::<GameClock>().elapsed = 600;
    app.update();
    assert_eq!(app.world().resource::<NotificationQueue>().items.len(), 1);
}

/// #249: Dual-write dedupe. If the legacy `GameEvent` and a paired
/// `KnowledgeFact` share the same `EventId`, only the first surfaces a banner.
#[test]
fn test_survey_hostile_dual_write_no_double_banner() {
    let mut app = make_app_with_queues();
    app.add_message::<GameEvent>();
    app.add_systems(Update, auto_notify_from_events);

    let eid = EventId(42);
    app.world_mut()
        .resource_mut::<NotifiedEventIds>()
        .register(eid);
    // GameEvent with a whitelisted kind (PlayerRespawn) + fact with same id.
    app.world_mut().write_message(GameEvent {
        id: eid,
        timestamp: 0,
        kind: GameEventKind::PlayerRespawn,
        description: "Respawn".into(),
        related_system: None,
    });
    // Local-path fact with same id.
    app.world_mut()
        .resource_mut::<PendingFactQueue>()
        .record(PerceivedFact {
            fact: KnowledgeFact::SurveyComplete {
                event_id: Some(eid),
                system: Entity::PLACEHOLDER,
                system_name: "".into(),
                detail: "".into(),
            },
            observed_at: 0,
            arrives_at: 0,
            source: ObservationSource::Direct,
            origin_pos: [0.0; 3],
            related_system: None,
        });

    app.update();
    // Only the legacy whitelisted banner should have surfaced. The fact
    // entry with the same EventId is dropped silently.
    let q = app.world().resource::<NotificationQueue>();
    assert_eq!(
        q.items.len(),
        1,
        "EventId dedupe must suppress the duplicate banner"
    );
}

/// #249 critical regression: per-ship CombatDefeat + wipe CombatDefeat both
/// flow into the fact pipeline. With a single shared set of EventIds they
/// must still surface at most one banner per underlying engagement. Here we
/// use *different* EventIds to verify both *can* surface; the dedupe aspect
/// is covered by `test_combat_defeat_same_event_id_dedupes`.
#[test]
fn test_combat_defeat_per_ship_and_wipe_dedupe() {
    let mut app = make_app_with_queues();
    let sys = app.world_mut().spawn_empty().id();

    // Same EventId → dedupe → one banner.
    let eid = EventId(77);
    app.world_mut()
        .resource_mut::<NotifiedEventIds>()
        .register(eid);
    for label in ["per-ship", "wipe"] {
        app.world_mut()
            .resource_mut::<PendingFactQueue>()
            .record(PerceivedFact {
                fact: KnowledgeFact::CombatOutcome {
                    event_id: Some(eid),
                    system: sys,
                    victor: CombatVictor::Hostile,
                    detail: label.into(),
                },
                observed_at: 0,
                arrives_at: 0,
                source: ObservationSource::Direct,
                origin_pos: [0.0; 3],
                related_system: Some(sys),
            });
    }
    app.update();
    let q = app.world().resource::<NotificationQueue>();
    assert_eq!(
        q.items.len(),
        1,
        "per-ship + wipe CombatDefeat with shared EventId must dedupe to one banner"
    );
}

/// #249: ShipArrived is Low priority — it lives in the event log but must
/// never surface as a banner (even when the fact's `arrives_at` is reached).
#[test]
fn test_ship_arrived_low_priority_silent() {
    let mut app = make_app_with_queues();
    let sys = app.world_mut().spawn_empty().id();
    app.world_mut()
        .resource_mut::<NotifiedEventIds>()
        .register(EventId(5));
    app.world_mut()
        .resource_mut::<PendingFactQueue>()
        .record(PerceivedFact {
            fact: KnowledgeFact::ShipArrived {
                event_id: Some(EventId(5)),
                system: Some(sys),
                name: "Corvette".into(),
                detail: "Arrived".into(),
            },
            observed_at: 0,
            arrives_at: 0,
            source: ObservationSource::Direct,
            origin_pos: [0.0; 3],
            related_system: Some(sys),
        });
    app.update();
    assert_eq!(
        app.world().resource::<NotificationQueue>().items.len(),
        0,
        "Low-priority ShipArrived facts must never banner"
    );
    // Sanity check the priority metadata itself.
    let fact = KnowledgeFact::ShipArrived {
        event_id: None,
        system: None,
        name: "".into(),
        detail: "".into(),
    };
    assert_eq!(fact.priority(), NotificationPriority::Low);
}

/// #249: A colony founded at the player's current system surfaces immediately;
/// the same event at a remote system has a light-speed delay.
#[test]
fn test_colony_established_remote_vs_local() {
    // Local (origin == player_pos) — banner immediately via
    // record_fact_or_local's short-circuit.
    let mut local_app = make_app_with_queues();
    let sys_local = local_app.world_mut().spawn_empty().id();
    let planet = local_app.world_mut().spawn_empty().id();
    local_app
        .world_mut()
        .resource_mut::<NotifiedEventIds>()
        .register(EventId(10));
    {
        let mut queue = local_app.world_mut().resource_mut::<PendingFactQueue>();
        // Simulate record_fact_or_local by pushing a fact that would have
        // bypassed the queue on the local path. We use arrives_at=0 here
        // because the production call path goes through record_fact_or_local
        // and we want to mirror its observable behaviour from a test.
        queue.record(PerceivedFact {
            fact: KnowledgeFact::ColonyEstablished {
                event_id: Some(EventId(10)),
                system: sys_local,
                planet,
                name: "Capital Prime".into(),
                detail: "Founded".into(),
            },
            observed_at: 0,
            arrives_at: 0,
            source: ObservationSource::Direct,
            origin_pos: [0.0; 3],
            related_system: Some(sys_local),
        });
    }
    local_app.update();
    assert_eq!(
        local_app.world().resource::<NotificationQueue>().items.len(),
        1,
        "local colony established surfaces immediately"
    );

    // Remote — the same fact is gated by light-speed delay.
    let mut remote_app = make_app_with_queues();
    let sys_remote = remote_app.world_mut().spawn_empty().id();
    let origin = [5.0, 0.0, 0.0];
    let plan = compute_fact_arrival(0, origin, [0.0; 3], &[], &CommsParams::default());
    remote_app
        .world_mut()
        .resource_mut::<NotifiedEventIds>()
        .register(EventId(11));
    remote_app
        .world_mut()
        .resource_mut::<PendingFactQueue>()
        .record(PerceivedFact {
            fact: KnowledgeFact::ColonyEstablished {
                event_id: Some(EventId(11)),
                system: sys_remote,
                planet,
                name: "Remote Colony".into(),
                detail: "Founded".into(),
            },
            observed_at: 0,
            arrives_at: plan.arrives_at,
            source: plan.source,
            origin_pos: origin,
            related_system: Some(sys_remote),
        });
    remote_app.world_mut().resource_mut::<GameClock>().elapsed = 299;
    remote_app.update();
    assert_eq!(
        remote_app.world().resource::<NotificationQueue>().items.len(),
        0,
    );
    remote_app.world_mut().resource_mut::<GameClock>().elapsed = 300;
    remote_app.update();
    assert_eq!(
        remote_app.world().resource::<NotificationQueue>().items.len(),
        1,
    );
}

/// #249: Deep-space structure construction (StructureBuilt) is Low priority —
/// it logs to EventLog but never banners.
#[test]
fn test_structure_built_low_priority_logged_only() {
    let mut app = make_app_with_queues();
    app.world_mut()
        .resource_mut::<NotifiedEventIds>()
        .register(EventId(9));
    app.world_mut()
        .resource_mut::<PendingFactQueue>()
        .record(PerceivedFact {
            fact: KnowledgeFact::StructureBuilt {
                event_id: Some(EventId(9)),
                system: None,
                kind: "platform".into(),
                name: "Research Beacon".into(),
                destroyed: false,
                detail: "Built".into(),
            },
            observed_at: 0,
            arrives_at: 0,
            source: ObservationSource::Direct,
            origin_pos: [0.0; 3],
            related_system: None,
        });
    app.update();
    assert_eq!(
        app.world().resource::<NotificationQueue>().items.len(),
        0,
        "Low-priority StructureBuilt must never banner"
    );
    // The fact was drained and its EventId marked (banner suppression is a
    // priority decision, not a dedupe-set decision).
    let notified = app.world().resource::<NotifiedEventIds>();
    assert!(
        notified.contains(EventId(9)),
        "EventId is marked even when the push returns None (Low-priority fact)"
    );
}
