//! #345 ESC-2 integration tests.
//!
//! End-to-end verification of the ESC Notifications path:
//!
//! 1. A `KnowledgeFact` variant lands in `PendingFactQueue` with
//!    `arrives_at <= clock.elapsed`.
//! 2. `dispatch_knowledge_observed` drains the queue, builds the
//!    sealed `@observed` event table, and fires
//!    `scripts/notifications/default_bridge.lua`'s wildcard
//!    subscriber.
//! 3. The bridge calls `push_notification { ... }`, which appends to
//!    `_pending_esc_notifications`.
//! 4. `drain_pending_esc_notifications` parses the accumulator and
//!    pushes into `EscNotificationQueue`.
//! 5. Ack intents routed through `enqueue_pending_ack` +
//!    `apply_pending_acks_system` flip the entry `acked` without
//!    touching the unrelated banner `NotificationQueue`.
//!
//! These tests do **not** assert egui render output — the UI layer is
//! covered by the unit tests in
//! `ui::situation_center::notifications_tab::tests`. Here we pin the
//! wiring from `PendingFactQueue` through Lua and back to
//! `EscNotificationQueue`.
//!
//! ## Parallelism + global state
//!
//! `apply_pending_acks_system` drains a process-wide
//! `Mutex<Vec<PendingAck>>` populated from `enqueue_pending_ack`.
//! Tests in this file serialise on `ACK_SERIAL` so one test's ack
//! buffer can't bleed into another's drain. The esc bridge + the
//! banner path never touch that buffer, so tests that don't use
//! `enqueue_pending_ack` don't need the guard.

use bevy::prelude::*;

use macrocosmo::knowledge::{
    CombatVictor, EventId, KnowledgeFact, NotifiedEventIds, ObservationSource, PendingFactQueue,
    PerceivedFact, RelayNetwork,
};
use macrocosmo::notifications::NotificationQueue;
use macrocosmo::player::PlayerEmpire;
use macrocosmo::scripting::esc_notifications::drain_pending_esc_notifications;
use macrocosmo::scripting::knowledge_dispatch::dispatch_knowledge_observed;
use macrocosmo::scripting::knowledge_registry::{
    KnowledgeSubscriptionRegistry, load_knowledge_subscriptions,
};
use macrocosmo::scripting::{GameRng, ScriptEngine};
use macrocosmo::time_system::{GameClock, GameSpeed};
use macrocosmo::ui::situation_center::{
    EscNotificationQueue, PendingAck, Severity, apply_pending_acks_system,
    drain_pending_acks_for_tests, enqueue_pending_ack,
};

/// Process-wide mutex that serialises tests which push to the
/// shared ack buffer. See module-level doc.
static ACK_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn acquire_ack_serial() -> std::sync::MutexGuard<'static, ()> {
    ACK_SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

/// Build a minimal headless app wired with the ESC + knowledge +
/// scripting resources needed for the pipeline. Loads
/// `macrocosmo/scripts/init.lua` (which now requires the default
/// bridge) so the `*@observed` subscriber is live.
fn make_integration_app() -> App {
    let mut app = App::new();

    // Knowledge + time + banner resources.
    app.init_resource::<PendingFactQueue>()
        .init_resource::<NotifiedEventIds>()
        .init_resource::<RelayNetwork>()
        .insert_resource(NotificationQueue::new())
        .insert_resource(GameClock::new(0))
        .insert_resource(GameSpeed::default());

    // ESC resources.
    app.init_resource::<EscNotificationQueue>()
        .add_systems(Update, apply_pending_acks_system);

    // Scripting — boot the engine with the crate-local scripts/ dir
    // so `default_bridge.lua` is picked up by `require()`. We don't
    // wire the full `ScriptingPlugin` because the integration test
    // only needs the `init.lua` + knowledge subscription registry +
    // the two drain systems (dispatch + esc drain). That keeps the
    // app boot time tight and avoids pulling in Startup hooks that
    // would try to allocate galaxy state we don't spawn here.
    let rng = GameRng::default().handle();
    let scripts_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts");
    let engine = ScriptEngine::new_with_rng_and_dir(rng, scripts_dir.clone()).expect("engine");
    // Mirror ScriptingPlugin's `init_scripting` globals setup path —
    // the engine constructor runs `setup_globals` already via
    // `new_with_rng_and_dir`, so we can proceed directly to
    // `load_all_scripts`.
    engine
        .lua()
        .load(r#"require("init")"#)
        .exec()
        .expect("init.lua");
    app.insert_resource(engine);

    // Drain `_pending_knowledge_subscriptions` (populated by `on(...)`
    // calls inside default_bridge.lua during the `require("init")`
    // pass above) into the bucketed registry so the dispatcher can
    // walk them.
    app.init_resource::<KnowledgeSubscriptionRegistry>();
    let mut sys = bevy::ecs::system::IntoSystem::into_system(load_knowledge_subscriptions);
    sys.initialize(app.world_mut());
    sys.run((), app.world_mut());

    // Hook the two drain systems we need. `dispatch_knowledge_observed`
    // fires the wildcard subscriber, which calls `push_notification`;
    // `drain_pending_esc_notifications` consumes the Lua accumulator.
    app.add_systems(
        Update,
        (dispatch_knowledge_observed, drain_pending_esc_notifications).chain(),
    );

    // The dispatcher iterates `PlayerEmpire`-tagged entities as
    // observer empires — spawn one so the observed path actually
    // runs (otherwise the subscriber never fires and the test gives
    // a false pass).
    app.world_mut().spawn(PlayerEmpire);
    app
}

fn enqueue_hostile_fact(app: &mut App, target_bits: u64, description: &str) {
    let target = Entity::from_bits(target_bits);
    let detector = app.world_mut().spawn_empty().id();
    let mut queue = app.world_mut().resource_mut::<PendingFactQueue>();
    queue.record(PerceivedFact {
        fact: KnowledgeFact::HostileDetected {
            event_id: None,
            target,
            detector,
            target_pos: [0.0, 0.0, 0.0],
            description: description.into(),
        },
        observed_at: 0,
        arrives_at: 0,
        source: ObservationSource::Direct,
        origin_pos: [0.0, 0.0, 0.0],
        related_system: None,
    });
}

#[test]
fn hostile_detected_flows_through_bridge_to_esc_queue() {
    let mut app = make_integration_app();
    enqueue_hostile_fact(&mut app, 42, "Warship sighted");

    app.update();

    let q = app.world().resource::<EscNotificationQueue>();
    assert_eq!(q.items.len(), 1, "bridge should land one notification");
    let n = &q.items[0];
    assert_eq!(n.severity, Severity::Warn);
    assert!(
        n.message.contains("Warship sighted"),
        "message should include payload description, got: {:?}",
        n.message,
    );
}

#[test]
fn repeated_hostile_for_same_target_dedupes_via_event_id() {
    let mut app = make_integration_app();
    // Two independent facts for the same target entity. The bridge
    // synthesises `event_id = "core:hostile:<target>"` so the second
    // push is suppressed.
    enqueue_hostile_fact(&mut app, 99, "alpha");
    enqueue_hostile_fact(&mut app, 99, "beta");

    app.update();

    let q = app.world().resource::<EscNotificationQueue>();
    assert_eq!(q.items.len(), 1, "event_id dedup should collapse to one");
    // First-push-wins ordering: "alpha" lands, "beta" is suppressed.
    assert!(
        q.items[0].message.contains("alpha"),
        "first push wins: {:?}",
        q.items[0].message
    );
}

#[test]
fn distinct_targets_do_not_dedupe() {
    let mut app = make_integration_app();
    enqueue_hostile_fact(&mut app, 11, "a");
    enqueue_hostile_fact(&mut app, 22, "b");
    enqueue_hostile_fact(&mut app, 33, "c");

    app.update();

    let q = app.world().resource::<EscNotificationQueue>();
    assert_eq!(q.items.len(), 3);
}

#[test]
fn ack_affects_esc_queue_only_not_banner() {
    let _guard = acquire_ack_serial();
    let _ = drain_pending_acks_for_tests(); // clean leftover buffer

    let mut app = make_integration_app();

    // Enqueue a fact that produces BOTH an ESC notification (via
    // the Lua bridge) AND a banner (via the K-5 inline core:* bridge
    // in `dispatch_knowledge_observed`). We verify the two queues
    // are independent: acking one does not touch the other.
    let target = app.world_mut().spawn_empty().id();
    let detector = app.world_mut().spawn_empty().id();
    let mut queue = app.world_mut().resource_mut::<PendingFactQueue>();
    queue.record(PerceivedFact {
        fact: KnowledgeFact::HostileDetected {
            event_id: Some(EventId(777)),
            target,
            detector,
            target_pos: [0.0, 0.0, 0.0],
            description: "Combined push".into(),
        },
        observed_at: 0,
        arrives_at: 0,
        source: ObservationSource::Direct,
        origin_pos: [0.0, 0.0, 0.0],
        related_system: None,
    });
    // Register the banner-side event id so the K-5 bridge's
    // `try_notify` admits the banner push (mirrors `FactSysParam::
    // allocate_event_id` + auto-pause contract).
    app.world_mut()
        .resource_mut::<NotifiedEventIds>()
        .register(EventId(777));

    app.update();

    // Both queues populated.
    let esc_id = {
        let esc = app.world().resource::<EscNotificationQueue>();
        assert_eq!(esc.items.len(), 1, "ESC got the Lua-bridge push");
        esc.items[0].id
    };
    let banner_len_before = app.world().resource::<NotificationQueue>().items.len();
    assert!(banner_len_before >= 1, "banner path also fired");

    // Ack the ESC entry through the render-path thread-local.
    enqueue_pending_ack(PendingAck::Single(esc_id));
    app.update();

    let esc = app.world().resource::<EscNotificationQueue>();
    assert_eq!(esc.total_unacked(), 0, "ESC ack propagated");
    assert!(
        esc.items.iter().find(|n| n.id == esc_id).unwrap().acked,
        "matching ESC entry marked acked"
    );
    let banner_len_after = app.world().resource::<NotificationQueue>().items.len();
    assert_eq!(
        banner_len_before, banner_len_after,
        "banner queue unaffected by ESC ack",
    );
}

#[test]
fn cascade_ack_propagates_to_children() {
    let _guard = acquire_ack_serial();
    let _ = drain_pending_acks_for_tests();

    let mut app = make_integration_app();

    // Manually push a parent-with-children notification into the
    // queue so we can exercise cascade ack end-to-end. The Lua
    // bridge emits flat notifications today, but the queue / ack
    // contract supports tree structures for future bridges.
    let mut esc = app.world_mut().resource_mut::<EscNotificationQueue>();
    use macrocosmo::ui::situation_center::{Notification, NotificationSource};
    let parent = Notification {
        id: 0,
        source: NotificationSource::None,
        timestamp: 0,
        severity: Severity::Warn,
        message: "parent".into(),
        acked: false,
        children: vec![
            Notification {
                id: 0,
                source: NotificationSource::None,
                timestamp: 0,
                severity: Severity::Info,
                message: "c1".into(),
                acked: false,
                children: Vec::new(),
            },
            Notification {
                id: 0,
                source: NotificationSource::None,
                timestamp: 0,
                severity: Severity::Critical,
                message: "c2".into(),
                acked: false,
                children: Vec::new(),
            },
        ],
    };
    match esc.push(parent, None, None) {
        macrocosmo::ui::situation_center::PushOutcome::Pushed(id) => {
            drop(esc);
            enqueue_pending_ack(PendingAck::Single(id));
        }
        other => panic!("unexpected outcome: {other:?}"),
    }

    app.update();

    let esc = app.world().resource::<EscNotificationQueue>();
    assert_eq!(
        esc.total_unacked(),
        0,
        "cascade ack should flatten the subtree"
    );
    assert!(esc.items[0].acked);
    assert!(esc.items[0].children.iter().all(|c| c.acked));
}

#[test]
fn bridge_does_not_touch_banner_queue_directly() {
    // The ESC bridge goes Lua → `_pending_esc_notifications` →
    // `EscNotificationQueue`. It must NEVER push into the banner
    // queue itself — banners are driven by the separate inline
    // K-5 bridge inside `dispatch_knowledge_observed`. This test
    // pushes a `push_notification` directly without any fact, which
    // should populate ONLY the ESC queue.
    let mut app = make_integration_app();
    {
        let engine = app.world().resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"
            push_notification {
                message = "direct lua push",
                severity = "info"
            }
        "#,
            )
            .exec()
            .expect("lua exec");
    }
    app.update();

    let esc = app.world().resource::<EscNotificationQueue>();
    assert_eq!(esc.items.len(), 1);
    let banner = app.world().resource::<NotificationQueue>();
    assert_eq!(
        banner.items.len(),
        0,
        "direct push_notification must not leak into banner queue",
    );
}

#[test]
fn sample_kinds_are_not_bridged_by_default() {
    // The default bridge only maps `core:*`. A scripted fact with a
    // `sample:*` kind should pass through `dispatch_knowledge_observed`
    // without creating an ESC notification (the bridge's policy
    // table returns `nil` → skip).
    let mut app = make_integration_app();

    use macrocosmo::knowledge::payload::PayloadSnapshot;
    let snapshot = PayloadSnapshot {
        fields: std::collections::HashMap::new(),
    };
    let mut queue = app.world_mut().resource_mut::<PendingFactQueue>();
    queue.record(PerceivedFact {
        fact: KnowledgeFact::Scripted {
            event_id: None,
            kind_id: "sample:combat_report".into(),
            origin_system: None,
            payload_snapshot: snapshot,
            recorded_at: 0,
        },
        observed_at: 0,
        arrives_at: 0,
        source: ObservationSource::Direct,
        origin_pos: [0.0, 0.0, 0.0],
        related_system: None,
    });

    app.update();

    let esc = app.world().resource::<EscNotificationQueue>();
    assert_eq!(
        esc.items.len(),
        0,
        "sample:* kinds intentionally not mapped by the default bridge"
    );
}

#[test]
fn combat_defeat_marked_critical_by_policy() {
    let mut app = make_integration_app();
    let system = app.world_mut().spawn_empty().id();
    let mut queue = app.world_mut().resource_mut::<PendingFactQueue>();
    queue.record(PerceivedFact {
        fact: KnowledgeFact::CombatOutcome {
            event_id: None,
            system,
            victor: CombatVictor::Hostile,
            detail: "Fleet lost".into(),
        },
        observed_at: 0,
        arrives_at: 0,
        source: ObservationSource::Direct,
        origin_pos: [0.0, 0.0, 0.0],
        related_system: Some(system),
    });
    app.update();

    let esc = app.world().resource::<EscNotificationQueue>();
    assert_eq!(esc.items.len(), 1);
    assert_eq!(
        esc.items[0].severity,
        Severity::Critical,
        "defeat policy elevates to critical",
    );
    assert!(esc.items[0].message.contains("defeat"));
}
