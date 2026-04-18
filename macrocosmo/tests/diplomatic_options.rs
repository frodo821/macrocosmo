//! Integration tests for #302: DiplomaticOption framework.

mod common;

use std::collections::HashMap;

use macrocosmo::faction::{DiplomaticEvent, DiplomaticInbox, PendingInboxItem};
use macrocosmo::player::{Empire, Faction};
use macrocosmo::scripting::faction_api::{DiplomaticOptionDefinition, DiplomaticOptionRegistry};
use macrocosmo::time_system::GameClock;

/// End-to-end: define option -> spawn DiplomaticEvent -> tick -> inbox delivery.
#[test]
fn test_diplomatic_option_e2e() {
    let mut app = common::test_app();

    // tick_diplomatic_events is already registered by test_app().

    // Insert a DiplomaticOptionRegistry with a test option
    let mut registry = DiplomaticOptionRegistry::default();
    registry.options.insert(
        "test_negotiate".to_string(),
        DiplomaticOptionDefinition {
            id: "test_negotiate".to_string(),
            name: "Test Negotiation".to_string(),
            description: "A test bilateral option.".to_string(),
            kind: "bilateral".to_string(),
            responses: vec![],
            payload_schema: vec![],
        },
    );
    app.insert_resource(registry);

    // Spawn sender and receiver factions
    let sender = app
        .world_mut()
        .spawn((
            Empire {
                name: "Sender".into(),
            },
            Faction::new("sender_faction", "Sender"),
        ))
        .id();

    let receiver = app
        .world_mut()
        .spawn((
            Empire {
                name: "Receiver".into(),
            },
            Faction::new("receiver_faction", "Receiver"),
            DiplomaticInbox::default(),
        ))
        .id();

    // Spawn a DiplomaticEvent that arrives at tick 5
    let mut payload = HashMap::new();
    payload.insert("terms".to_string(), "mutual_trade".to_string());

    app.world_mut().spawn(DiplomaticEvent {
        from: sender,
        to: receiver,
        option_id: "test_negotiate".to_string(),
        payload: payload.clone(),
        arrives_at: 5,
    });

    // At tick 3, event should not have arrived yet
    app.world_mut().resource_mut::<GameClock>().elapsed = 3;
    app.update();

    {
        let inbox = app.world().get::<DiplomaticInbox>(receiver).unwrap();
        assert!(
            inbox.items.is_empty(),
            "event should not arrive before arrives_at"
        );
    }

    // At tick 5, event should arrive
    app.world_mut().resource_mut::<GameClock>().elapsed = 5;
    app.update();

    {
        let inbox = app.world().get::<DiplomaticInbox>(receiver).unwrap();
        assert_eq!(inbox.items.len(), 1);
        let item = &inbox.items[0];
        assert_eq!(item.from, sender);
        assert_eq!(item.option_id, "test_negotiate");
        assert_eq!(item.payload.get("terms").unwrap(), "mutual_trade");
        assert_eq!(item.delivered_at, 5);
    }

    // The DiplomaticEvent entity should have been despawned
    let world = app.world_mut();
    let event_count = world.query::<&DiplomaticEvent>().iter(world).count();
    assert_eq!(
        event_count, 0,
        "DiplomaticEvent should be despawned after delivery"
    );
}

/// Verify that allowed_diplomatic_options is populated correctly on Faction.
#[test]
fn test_allowed_options_from_type() {
    let mut allowed = std::collections::HashSet::new();
    allowed.insert("generic_negotiation".to_string());
    allowed.insert("break_alliance".to_string());

    let faction = Faction {
        id: "test".into(),
        name: "Test".into(),
        can_diplomacy: true,
        allowed_diplomatic_options: allowed.clone(),
    };

    assert!(
        faction
            .allowed_diplomatic_options
            .contains("generic_negotiation")
    );
    assert!(
        faction
            .allowed_diplomatic_options
            .contains("break_alliance")
    );
    assert!(!faction.allowed_diplomatic_options.contains("nonexistent"));
}

/// DiplomaticEvent without a receiver DiplomaticInbox is gracefully dropped.
#[test]
fn test_diplomatic_event_no_inbox_drops_gracefully() {
    let mut app = common::test_app();

    // tick_diplomatic_events is already registered by test_app().

    let sender = app.world_mut().spawn(Faction::new("sender", "Sender")).id();

    // Receiver has no DiplomaticInbox component
    let receiver = app
        .world_mut()
        .spawn(Faction::new("receiver", "Receiver"))
        .id();

    app.world_mut().spawn(DiplomaticEvent {
        from: sender,
        to: receiver,
        option_id: "test".to_string(),
        payload: HashMap::new(),
        arrives_at: 0,
    });

    app.world_mut().resource_mut::<GameClock>().elapsed = 1;
    app.update();

    // Event should be despawned (no crash)
    let world = app.world_mut();
    let event_count = world.query::<&DiplomaticEvent>().iter(world).count();
    assert_eq!(event_count, 0);
}

/// Round-trip save/load test for DiplomaticEvent and DiplomaticInbox.
#[test]
fn test_diplomatic_event_save_load() {
    use macrocosmo::persistence::savebag::{
        SavedDiplomaticEvent, SavedDiplomaticInbox, SavedFaction,
    };

    // Use spawned entities for valid Entity values
    let mut app = common::test_app();
    let from = app.world_mut().spawn_empty().id();
    let to = app.world_mut().spawn_empty().id();

    let live_event = DiplomaticEvent {
        from,
        to,
        option_id: "negotiate".to_string(),
        payload: {
            let mut m = HashMap::new();
            m.insert("key".to_string(), "val".to_string());
            m
        },
        arrives_at: 42,
    };

    let saved = SavedDiplomaticEvent::from_live(&live_event);
    assert_eq!(saved.option_id, "negotiate");
    assert_eq!(saved.arrives_at, 42);
    assert_eq!(saved.payload.get("key").unwrap(), "val");

    // Test SavedDiplomaticInbox round-trip
    let live_inbox = DiplomaticInbox {
        items: vec![PendingInboxItem {
            from,
            option_id: "negotiate".to_string(),
            payload: HashMap::new(),
            delivered_at: 10,
        }],
    };

    let saved_inbox = SavedDiplomaticInbox::from_live(&live_inbox);
    assert_eq!(saved_inbox.items.len(), 1);
    assert_eq!(saved_inbox.items[0].option_id, "negotiate");
    assert_eq!(saved_inbox.items[0].delivered_at, 10);

    // Test SavedFaction with allowed_diplomatic_options
    let faction = Faction {
        id: "test".into(),
        name: "Test".into(),
        can_diplomacy: false,
        allowed_diplomatic_options: ["opt_a".to_string()].into_iter().collect(),
    };
    let saved_faction = SavedFaction::from_live(&faction);
    assert_eq!(saved_faction.allowed_diplomatic_options.len(), 1);
    assert!(
        saved_faction
            .allowed_diplomatic_options
            .contains(&"opt_a".to_string())
    );

    let restored = saved_faction.into_live();
    assert!(restored.allowed_diplomatic_options.contains("opt_a"));
}

/// send_diplomatic_event helper spawns a DiplomaticEvent with correct fields.
#[test]
fn test_send_diplomatic_event_helper() {
    let mut app = common::test_app();

    let sender = app.world_mut().spawn(Faction::new("s", "S")).id();
    let receiver = app.world_mut().spawn(Faction::new("r", "R")).id();

    let mut payload = HashMap::new();
    payload.insert("k".to_string(), "v".to_string());

    // Use a local GameClock snapshot to avoid borrow conflicts with
    // app.world_mut().commands().
    let clock = GameClock::new(0);
    macrocosmo::faction::send_diplomatic_event(
        &mut app.world_mut().commands(),
        &clock,
        sender,
        receiver,
        "test_option",
        payload,
        10,
    );
    // Flush commands
    app.world_mut().flush();

    let world = app.world_mut();
    let events: Vec<_> = world.query::<&DiplomaticEvent>().iter(world).collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].option_id, "test_option");
    assert_eq!(events[0].from, sender);
    assert_eq!(events[0].to, receiver);
    assert_eq!(events[0].arrives_at, 10); // clock.elapsed=0 + delay=10
    assert_eq!(events[0].payload.get("k").unwrap(), "v");
}
