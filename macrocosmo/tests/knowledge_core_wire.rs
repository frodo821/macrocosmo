//! #354 K-5: integration tests for the `core:*` wire through the
//! scripted-knowledge pipeline.
//!
//! After K-5, Rust-origin `KnowledgeFact` variants mirror themselves as
//! `core:*` knowledge kinds — `@recorded` subscribers fire from the
//! Rust-origin dispatcher on the same tick, and `@observed` subscribers
//! (plus the notification bridge) fire when the fact's arrival time is
//! reached (plan §3.5 Commit 5).
//!
//! The matrix below exercises the observable behaviour:
//!
//! * `core:*` kinds are preloaded into `KindRegistry`.
//! * Rust fact emitter (via `FactSysParam::record`) pushes into
//!   `PendingKnowledgeRecords` → `dispatch_knowledge_recorded` fires
//!   `<core:*>@recorded` subscribers and enqueues a Scripted fact into
//!   `PendingFactQueue`.
//! * When the queue reaches `arrives_at <= clock.elapsed`,
//!   `dispatch_knowledge_observed` fires `<core:*>@observed`
//!   subscribers AND pushes a `Notification` via the bridge.
//! * `*@observed` wildcard catches core + scripted kinds.
//! * Lua `define_knowledge { id = "core:<foo>" }` fails at parse time.
//! * The notification banner output for a core variant is
//!   byte-identical to the pre-K-5 `notify_from_knowledge_facts`
//!   output (banner regression guard).

use bevy::prelude::*;
use macrocosmo::knowledge::ObservationSource;
use macrocosmo::knowledge::facts::{
    KnowledgeFact, NextEventId, NotifiedEventIds, PendingFactQueue, PerceivedFact, RelayNetwork,
};
use macrocosmo::knowledge::kind_registry::{CORE_KIND_IDS, KindRegistry};
use macrocosmo::notifications::{NotificationPriority, NotificationQueue};
use macrocosmo::scripting::ScriptEngine;
use macrocosmo::scripting::knowledge_api::parse_knowledge_definitions;
use macrocosmo::scripting::knowledge_dispatch::{
    PendingKnowledgeRecord, PendingKnowledgeRecords, dispatch_knowledge_observed,
    dispatch_knowledge_recorded,
};
use macrocosmo::scripting::knowledge_registry::{
    KnowledgeSubscriptionRegistry, drain_pending_subscriptions,
};
use macrocosmo::time_system::GameClock;

// -------------------------------------------------------------------------
// Shared harness
// -------------------------------------------------------------------------

/// Build a world with preloaded `core:*` kinds, a single `PlayerEmpire`
/// observer, and all resources K-5 dispatchers need.
fn make_world(clock_elapsed: i64) -> World {
    let mut world = World::new();
    world.insert_resource(GameClock::new(clock_elapsed));
    world.insert_resource(macrocosmo::time_system::GameSpeed::default());
    world.init_resource::<PendingFactQueue>();
    world.init_resource::<NextEventId>();
    world.init_resource::<NotifiedEventIds>();
    world.insert_resource(NotificationQueue::new());
    world.init_resource::<RelayNetwork>();
    world.init_resource::<PendingKnowledgeRecords>();

    // Preloaded core:* registry (production wiring).
    world.insert_resource(KindRegistry::preload_core());

    // Single observer empire.
    world.spawn((
        macrocosmo::player::Empire {
            name: "Test".into(),
        },
        macrocosmo::player::PlayerEmpire,
    ));

    world
}

/// Install Lua subscribers and drain into the subscription registry.
fn setup_subscribers(engine: &ScriptEngine, script: &str) -> KnowledgeSubscriptionRegistry {
    if !script.is_empty() {
        engine.lua().load(script).exec().unwrap();
    }
    let mut registry = KnowledgeSubscriptionRegistry::default();
    drain_pending_subscriptions(engine.lua(), &mut registry).unwrap();
    registry
}

/// Push a core `KnowledgeFact::HostileDetected` fact straight into the
/// `PendingFactQueue` with a pre-computed arrival time. Mirrors what
/// `FactSysParam::record` would do on the remote-arrival path, but
/// without the light-speed maths.
fn push_core_hostile_at(
    world: &mut World,
    arrives_at: i64,
    target_bits: u64,
    detector_bits: u64,
    description: &str,
) {
    let fact = KnowledgeFact::HostileDetected {
        event_id: None,
        target: Entity::from_bits(target_bits),
        detector: Entity::from_bits(detector_bits),
        target_pos: [10.0, 20.0, 30.0],
        description: description.to_string(),
    };
    let mut queue = world.resource_mut::<PendingFactQueue>();
    queue.record(PerceivedFact {
        fact,
        observed_at: 0,
        arrives_at,
        source: ObservationSource::Direct,
        origin_pos: [10.0, 20.0, 30.0],
        related_system: None,
    });
}

/// Inject a Scripted fact alongside core facts so wildcard coverage
/// tests can assert both paths fire.
fn push_scripted_fact_at(world: &mut World, kind_id: &str, arrives_at: i64) {
    let fact = KnowledgeFact::Scripted {
        event_id: None,
        kind_id: kind_id.to_string(),
        origin_system: None,
        payload_snapshot: macrocosmo::knowledge::payload::PayloadSnapshot::default(),
        recorded_at: 0,
    };
    let mut queue = world.resource_mut::<PendingFactQueue>();
    queue.record(PerceivedFact {
        fact,
        observed_at: 0,
        arrives_at,
        source: ObservationSource::Direct,
        origin_pos: [0.0; 3],
        related_system: None,
    });
}

// -------------------------------------------------------------------------
// preload + namespace protection
// -------------------------------------------------------------------------

#[test]
fn core_namespace_kinds_are_preloaded() {
    let world = make_world(100);
    let registry = world.resource::<KindRegistry>();
    for id in CORE_KIND_IDS {
        assert!(
            registry.contains(id),
            "core:* kind '{id}' must be preloaded into KindRegistry"
        );
    }
}

/// #354 (plan §2.3 / §3.5 Test plan): Lua cannot shadow a core:* id.
/// The parse-time guard in `knowledge_api` fires even before the
/// registry insert guard.
#[test]
fn define_knowledge_core_namespace_is_rejected_at_parse() {
    let engine = ScriptEngine::new().unwrap();
    // Populate the accumulator with a malicious redefinition.
    engine
        .lua()
        .load(r#"define_knowledge { id = "core:hostile_detected" }"#)
        .exec()
        .unwrap();
    let err = parse_knowledge_definitions(engine.lua()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("core:"),
        "expected core:* namespace message: {msg}"
    );
    assert!(
        msg.contains("reserved"),
        "expected reserved-namespace message: {msg}"
    );
}

// -------------------------------------------------------------------------
// @recorded: Rust-origin fact mirrors through PendingKnowledgeRecords and
// fires core:*@recorded subscribers in the same tick.
// -------------------------------------------------------------------------

/// Simulates `FactSysParam::record` by pushing the core variant's
/// `(core_kind_id, payload_snapshot)` directly into
/// `PendingKnowledgeRecords`, then running `dispatch_knowledge_recorded`.
fn push_core_pending_record(world: &mut World, fact: &KnowledgeFact, recorded_at: i64) {
    let kind_id = fact.core_kind_id().expect("core variant has kind id");
    let snapshot = fact
        .to_core_payload_snapshot()
        .expect("core variant produces snapshot");
    let origin_system = fact.core_origin_system();
    let mut pending = world.resource_mut::<PendingKnowledgeRecords>();
    pending.push(PendingKnowledgeRecord {
        kind_id: kind_id.to_string(),
        origin_system,
        payload_snapshot: snapshot,
        recorded_at,
    });
}

#[test]
fn rust_origin_fires_core_hostile_detected_recorded() {
    let engine = ScriptEngine::new().unwrap();
    let mut world = make_world(100);
    let registry = setup_subscribers(
        &engine,
        r#"
        _recorded_fired = 0
        _recorded_target = nil
        _recorded_description = nil
        on("core:hostile_detected@recorded", function(e)
            _recorded_fired = _recorded_fired + 1
            _recorded_target = e.payload.target
            _recorded_description = e.payload.description
        end)
        "#,
    );
    world.insert_resource(registry);
    world.insert_resource(engine);

    // Simulate Rust-side fact emission for a remote-detected hostile.
    let fact = KnowledgeFact::HostileDetected {
        event_id: None,
        target: Entity::from_bits(111),
        detector: Entity::from_bits(222),
        target_pos: [1.0, 2.0, 3.0],
        description: "pirate patrol".into(),
    };
    push_core_pending_record(&mut world, &fact, 100);

    dispatch_knowledge_recorded(&mut world);

    world.resource_scope::<ScriptEngine, _>(|_world, engine| {
        let fired: i64 = engine.lua().globals().get("_recorded_fired").unwrap();
        assert_eq!(
            fired, 1,
            "core:hostile_detected@recorded fired exactly once"
        );
        let target: i64 = engine.lua().globals().get("_recorded_target").unwrap();
        assert_eq!(target, 111, "payload.target = Entity(111) bits");
        let description: String = engine.lua().globals().get("_recorded_description").unwrap();
        assert_eq!(description, "pirate patrol");
    });

    // After dispatch, the resulting Scripted fact must sit in
    // PendingFactQueue so `@observed` can drain it later.
    let queue = world.resource::<PendingFactQueue>();
    assert_eq!(queue.facts.len(), 1);
    match &queue.facts[0].fact {
        KnowledgeFact::Scripted { kind_id, .. } => {
            assert_eq!(kind_id, "core:hostile_detected");
        }
        other => panic!("expected Scripted, got {other:?}"),
    }
}

// -------------------------------------------------------------------------
// @observed: arrival-time fact drains via dispatch_knowledge_observed and
// fires core:*@observed subscribers per observer with lag metadata.
// -------------------------------------------------------------------------

#[test]
fn core_hostile_detected_fires_observed_on_arrival() {
    let engine = ScriptEngine::new().unwrap();
    let mut world = make_world(500);
    let registry = setup_subscribers(
        &engine,
        r#"
        _observed_fires = 0
        _last_lag = nil
        _last_description = nil
        _last_target = nil
        on("core:hostile_detected@observed", function(e)
            _observed_fires = _observed_fires + 1
            _last_lag = e.lag_hexadies
            _last_description = e.payload.description
            _last_target = e.payload.target
        end)
        "#,
    );
    world.insert_resource(registry);
    world.insert_resource(engine);

    // Fact observed at t=100 at origin, arriving at t=500. Player is
    // 400 hexadies away in light-delay terms.
    {
        let mut queue = world.resource_mut::<PendingFactQueue>();
        queue.record(PerceivedFact {
            fact: KnowledgeFact::HostileDetected {
                event_id: None,
                target: Entity::from_bits(777),
                detector: Entity::from_bits(888),
                target_pos: [0.0, 0.0, 0.0],
                description: "incoming fleet".into(),
            },
            observed_at: 100,
            arrives_at: 500,
            source: ObservationSource::Direct,
            origin_pos: [0.0; 3],
            related_system: None,
        });
    }

    dispatch_knowledge_observed(&mut world);

    world.resource_scope::<ScriptEngine, _>(|_world, engine| {
        let fires: i64 = engine.lua().globals().get("_observed_fires").unwrap();
        assert_eq!(fires, 1, "core:hostile_detected@observed fires once");
        // lag_hexadies = arrives_at - observed_at = 500 - 100 = 400.
        // `observed_at` in the event table is the PerceivedFact.observed_at.
        let lag: i64 = engine.lua().globals().get("_last_lag").unwrap();
        assert_eq!(lag, 400, "lag_hexadies should reflect 500 - 100");
        let description: String = engine.lua().globals().get("_last_description").unwrap();
        assert_eq!(description, "incoming fleet");
        let target: i64 = engine.lua().globals().get("_last_target").unwrap();
        assert_eq!(target, 777);
    });

    // Queue fully drained.
    let queue = world.resource::<PendingFactQueue>();
    assert_eq!(queue.pending_len(), 0);
}

// -------------------------------------------------------------------------
// Per-observer isolation for core variants (plan §3.5 Test plan).
// -------------------------------------------------------------------------

#[test]
fn core_observed_per_observer_payload_isolation() {
    let engine = ScriptEngine::new().unwrap();
    let mut world = make_world(100);
    // Add a second observer empire so per-observer dispatch fires twice.
    world.spawn((
        macrocosmo::player::Empire {
            name: "Observer2".into(),
        },
        macrocosmo::player::PlayerEmpire,
    ));

    let registry = setup_subscribers(
        &engine,
        r#"
        _seen = {}
        on("core:hostile_detected@observed", function(e)
            table.insert(_seen, { observer = e.observer_empire, description = e.payload.description })
            -- Mutate only this observer's payload; the next one must not see it.
            e.payload.description = "MUTATED"
        end)
        "#,
    );
    world.insert_resource(registry);
    world.insert_resource(engine);

    push_core_hostile_at(&mut world, 100, 1, 2, "original");

    dispatch_knowledge_observed(&mut world);

    world.resource_scope::<ScriptEngine, _>(|_world, engine| {
        let seen: mlua::Table = engine.lua().globals().get("_seen").unwrap();
        assert_eq!(seen.len().unwrap(), 2, "each observer fires once");
        let a: mlua::Table = seen.get(1).unwrap();
        let b: mlua::Table = seen.get(2).unwrap();
        let ad: String = a.get("description").unwrap();
        let bd: String = b.get("description").unwrap();
        assert_eq!(ad, "original", "observer A sees original");
        assert_eq!(
            bd, "original",
            "observer B must see the ORIGINAL (per-observer isolation), not A's mutation"
        );

        let a_bits: i64 = a.get("observer").unwrap();
        let b_bits: i64 = b.get("observer").unwrap();
        assert_ne!(a_bits, b_bits, "observers carry distinct entity bits");
    });
}

// -------------------------------------------------------------------------
// Wildcard `*@observed` catches both core + scripted in registration order.
// -------------------------------------------------------------------------

#[test]
fn wildcard_observed_catches_core_and_scripted() {
    let engine = ScriptEngine::new().unwrap();
    let mut world = make_world(100);
    // Register a non-core kind so the scripted leg has something to
    // dispatch against.
    world
        .resource_mut::<KindRegistry>()
        .insert(macrocosmo::knowledge::kind_registry::KnowledgeKindDef {
            id: macrocosmo::knowledge::kind_registry::KnowledgeKindId::parse("mod:alarm").unwrap(),
            payload_schema: Default::default(),
            origin: macrocosmo::knowledge::kind_registry::KindOrigin::Lua,
        })
        .unwrap();

    let registry = setup_subscribers(
        &engine,
        r#"
        _wildcard_kinds = {}
        on("*@observed", function(e)
            table.insert(_wildcard_kinds, e.kind)
        end)
        "#,
    );
    world.insert_resource(registry);
    world.insert_resource(engine);

    push_core_hostile_at(&mut world, 100, 1, 2, "core");
    push_scripted_fact_at(&mut world, "mod:alarm", 100);

    dispatch_knowledge_observed(&mut world);

    world.resource_scope::<ScriptEngine, _>(|_world, engine| {
        let kinds: mlua::Table = engine.lua().globals().get("_wildcard_kinds").unwrap();
        assert_eq!(
            kinds.len().unwrap(),
            2,
            "wildcard must fire for both core + scripted"
        );
        let k1: String = kinds.get(1).unwrap();
        let k2: String = kinds.get(2).unwrap();
        // Insertion order was core first, scripted second.
        assert_eq!(k1, "core:hostile_detected");
        assert_eq!(k2, "mod:alarm");
    });
}

// -------------------------------------------------------------------------
// Notification bridge regression: the banner a core fact produces on the
// K-5 bridge must match the pre-K-5 `notify_from_knowledge_facts` output.
// -------------------------------------------------------------------------

#[test]
fn core_notification_bridge_matches_pre_k5_banner() {
    // Reproduce the exact wire the old drainer used to emit for a
    // SurveyComplete fact (high-frequency event that existed well before
    // K-5). Banner title/description/priority must stay byte-identical
    // so `notification_knowledge_pipeline.rs`'s Spike 10.5 matrix does
    // not regress under the new drain.
    let engine = ScriptEngine::new().unwrap();
    let mut world = make_world(100);
    // No Lua subscribers — we only verify the Rust-side bridge output.
    let registry = setup_subscribers(&engine, "");
    world.insert_resource(registry);
    world.insert_resource(engine);

    let sys = world.spawn_empty().id();
    {
        let mut queue = world.resource_mut::<PendingFactQueue>();
        queue.record(PerceivedFact {
            fact: KnowledgeFact::SurveyComplete {
                event_id: None,
                system: sys,
                system_name: "Tau Ceti".into(),
                detail: "Tau Ceti surveyed".into(),
                ship: bevy::prelude::Entity::PLACEHOLDER,
            },
            observed_at: 0,
            arrives_at: 100,
            source: ObservationSource::Direct,
            origin_pos: [0.0; 3],
            related_system: Some(sys),
        });
    }

    dispatch_knowledge_observed(&mut world);

    let q = world.resource::<NotificationQueue>();
    assert_eq!(q.items.len(), 1, "core variant must produce a banner");
    assert_eq!(
        q.items[0].title, "Survey Complete",
        "banner title must match the pre-K-5 output from KnowledgeFact::title()"
    );
    assert_eq!(
        q.items[0].description, "Tau Ceti surveyed",
        "banner description must match the pre-K-5 output from KnowledgeFact::description()"
    );
    assert_eq!(
        q.items[0].priority,
        NotificationPriority::Medium,
        "SurveyComplete is Medium priority"
    );
}

/// Low-priority core variants (ShipArrived, StructureBuilt) must NOT
/// produce a banner — the same pre-K-5 contract.
#[test]
fn core_low_priority_facts_stay_silent_through_bridge() {
    let engine = ScriptEngine::new().unwrap();
    let mut world = make_world(100);
    let registry = setup_subscribers(&engine, "");
    world.insert_resource(registry);
    world.insert_resource(engine);

    {
        let mut queue = world.resource_mut::<PendingFactQueue>();
        queue.record(PerceivedFact {
            fact: KnowledgeFact::ShipArrived {
                event_id: None,
                system: None,
                name: "Corvette".into(),
                detail: "Arrived".into(),
                ship: bevy::prelude::Entity::PLACEHOLDER,
            },
            observed_at: 0,
            arrives_at: 100,
            source: ObservationSource::Direct,
            origin_pos: [0.0; 3],
            related_system: None,
        });
    }

    dispatch_knowledge_observed(&mut world);

    let q = world.resource::<NotificationQueue>();
    assert_eq!(q.items.len(), 0, "Low-priority ShipArrived must not banner");
}

/// High-priority core variants auto-pause `GameSpeed` (matches the
/// pre-K-5 `notify_from_knowledge_facts.speed.pause()` call).
#[test]
fn core_high_priority_fact_auto_pauses_game_speed() {
    let engine = ScriptEngine::new().unwrap();
    let mut world = make_world(100);
    // `GameSpeed::default()` starts paused (hexadies_per_second = 0);
    // unpause it here so the bridge's `speed.pause()` call has an
    // observable transition to assert against.
    world
        .resource_mut::<macrocosmo::time_system::GameSpeed>()
        .unpause();
    assert!(
        !world
            .resource::<macrocosmo::time_system::GameSpeed>()
            .is_paused(),
        "GameSpeed should be running after explicit unpause"
    );

    let registry = setup_subscribers(&engine, "");
    world.insert_resource(registry);
    world.insert_resource(engine);

    // HostileDetected is High priority per `KnowledgeFact::priority`.
    push_core_hostile_at(&mut world, 100, 1, 2, "hostile");

    dispatch_knowledge_observed(&mut world);

    let speed = world.resource::<macrocosmo::time_system::GameSpeed>();
    assert!(
        speed.is_paused(),
        "K-5 bridge must auto-pause on High-priority banner (hostile detected)"
    );
    let notifs = world.resource::<NotificationQueue>();
    assert_eq!(notifs.items.len(), 1);
    assert_eq!(notifs.items[0].priority, NotificationPriority::High);
}
