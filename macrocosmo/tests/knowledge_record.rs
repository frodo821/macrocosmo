//! #351 K-2: Integration tests for `gs:record_knowledge` + `@recorded`
//! sync dispatch + PendingFactQueue integration.
//!
//! Tests exercise:
//! - Lua-origin record -> @recorded subscriber chain -> PendingFactQueue
//! - Subscriber payload mutation (chain sequential)
//! - Sealed metadata write error
//! - Unknown kind error
//! - Schema violation error
//! - Wildcard subscriber
//! - Rust-origin record path (PendingKnowledgeRecords -> system -> queue)

mod common;

use bevy::prelude::*;
use macrocosmo::knowledge::facts::{KnowledgeFact, PendingFactQueue};
use macrocosmo::knowledge::kind_registry::{
    KindOrigin, KindRegistry, KnowledgeKindDef, KnowledgeKindId, PayloadFieldType, PayloadSchema,
};
use macrocosmo::knowledge::payload::PayloadValue;
use macrocosmo::scripting::ScriptEngine;
use macrocosmo::scripting::gamestate_scope::{GamestateMode, dispatch_with_gamestate};
use macrocosmo::scripting::knowledge_dispatch::PendingKnowledgeRecords;
use macrocosmo::scripting::knowledge_registry::{
    KnowledgeSubscriptionRegistry, drain_pending_subscriptions,
};
use macrocosmo::time_system::GameClock;

/// Build a minimal world with resources required by record_knowledge.
fn make_world_with_kind(kind_id: &str, schema_fields: Vec<(&str, PayloadFieldType)>) -> World {
    let mut world = World::new();
    world.insert_resource(GameClock::new(100));
    world.init_resource::<PendingFactQueue>();
    world.init_resource::<macrocosmo::knowledge::facts::NextEventId>();
    world.init_resource::<macrocosmo::knowledge::facts::NotifiedEventIds>();
    world.insert_resource(macrocosmo::notifications::NotificationQueue::new());
    world.init_resource::<macrocosmo::knowledge::facts::RelayNetwork>();
    world.init_resource::<PendingKnowledgeRecords>();

    let mut registry = KindRegistry::default();
    let schema = PayloadSchema {
        fields: schema_fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    };
    registry
        .insert(KnowledgeKindDef {
            id: KnowledgeKindId::parse(kind_id).unwrap(),
            payload_schema: schema,
            origin: KindOrigin::Lua,
        })
        .unwrap();
    world.insert_resource(registry);

    // Player empire for vantage computation.
    world.spawn((
        macrocosmo::player::Empire {
            name: "Test".into(),
        },
        macrocosmo::player::PlayerEmpire,
        macrocosmo::components::Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        macrocosmo::technology::GameFlags::default(),
        macrocosmo::condition::ScopedFlags::default(),
        macrocosmo::technology::EmpireModifiers::default(),
    ));

    world
}

/// Register Lua on() subscribers and drain into registry.
fn setup_subscribers(engine: &ScriptEngine, script: &str) -> KnowledgeSubscriptionRegistry {
    if !script.is_empty() {
        engine.lua().load(script).exec().unwrap();
    }
    let mut registry = KnowledgeSubscriptionRegistry::default();
    drain_pending_subscriptions(engine.lua(), &mut registry).unwrap();
    registry
}

/// Run a Lua script inside a gamestate scope with ReadWrite mode.
fn run_in_scope(world: &mut World, engine: &ScriptEngine, lua_code: &str) -> mlua::Result<()> {
    let lua = engine.lua();
    let payload = lua.create_table()?;
    dispatch_with_gamestate(lua, world, &payload, GamestateMode::ReadWrite, |lua, p| {
        let gs: mlua::Table = p.get("gamestate")?;
        lua.globals().set("_gs", gs)?;
        lua.load(lua_code).exec()?;
        lua.globals().set("_gs", mlua::Value::Nil)?;
        Ok(())
    })
}

#[test]
fn record_knowledge_no_subscribers_enqueues_fact() {
    let engine = ScriptEngine::new().unwrap();
    let mut world = make_world_with_kind("test:kind", vec![]);
    let registry = setup_subscribers(&engine, "");
    world.insert_resource(registry);
    world.insert_resource(engine);

    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        run_in_scope(
            world,
            &engine,
            r#"
            _gs:record_knowledge({
                kind = "test:kind",
                payload = { value = 42 },
            })
            "#,
        )
        .unwrap();
    });

    let queue = world.resource::<PendingFactQueue>();
    assert_eq!(queue.facts.len(), 1);
    match &queue.facts[0].fact {
        KnowledgeFact::Scripted {
            kind_id,
            payload_snapshot,
            recorded_at,
            ..
        } => {
            assert_eq!(kind_id, "test:kind");
            assert_eq!(*recorded_at, 100);
            assert!(payload_snapshot.fields.contains_key("value"));
        }
        other => panic!("Expected Scripted, got {:?}", other),
    }
}

#[test]
fn record_knowledge_subscriber_mutates_payload() {
    let engine = ScriptEngine::new().unwrap();
    let mut world = make_world_with_kind("test:kind", vec![("severity", PayloadFieldType::Number)]);
    let registry = setup_subscribers(
        &engine,
        r#"
        on("test:kind@recorded", function(e)
            e.payload.severity = 0.5
        end)
        "#,
    );
    world.insert_resource(registry);
    world.insert_resource(engine);

    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        run_in_scope(
            world,
            &engine,
            r#"
            _gs:record_knowledge({
                kind = "test:kind",
                payload = { severity = 0.9 },
            })
            "#,
        )
        .unwrap();
    });

    let queue = world.resource::<PendingFactQueue>();
    assert_eq!(queue.facts.len(), 1);
    if let KnowledgeFact::Scripted {
        payload_snapshot, ..
    } = &queue.facts[0].fact
    {
        match payload_snapshot.fields.get("severity") {
            Some(PayloadValue::Number(n)) => {
                assert!(
                    (*n - 0.5).abs() < f64::EPSILON,
                    "severity should be 0.5 (mutated by subscriber), got {n}"
                );
            }
            other => panic!("Expected Number(0.5), got {:?}", other),
        }
    } else {
        panic!("Expected Scripted fact");
    }
}

#[test]
fn record_knowledge_subscriber_chain_sequential() {
    let engine = ScriptEngine::new().unwrap();
    let mut world = make_world_with_kind("test:kind", vec![]);
    let registry = setup_subscribers(
        &engine,
        r#"
        on("test:kind@recorded", function(e) e.payload.step = "first" end)
        on("test:kind@recorded", function(e) e.payload.step = "second" end)
        "#,
    );
    world.insert_resource(registry);
    world.insert_resource(engine);

    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        run_in_scope(
            world,
            &engine,
            r#"
            _gs:record_knowledge({
                kind = "test:kind",
                payload = {},
            })
            "#,
        )
        .unwrap();
    });

    let queue = world.resource::<PendingFactQueue>();
    if let KnowledgeFact::Scripted {
        payload_snapshot, ..
    } = &queue.facts[0].fact
    {
        match payload_snapshot.fields.get("step") {
            Some(PayloadValue::String(s)) => {
                assert_eq!(s, "second", "Last subscriber should win");
            }
            other => panic!("Expected String(\"second\"), got {:?}", other),
        }
    }
}

#[test]
fn record_knowledge_sealed_metadata_write_errors() {
    let engine = ScriptEngine::new().unwrap();
    let mut world = make_world_with_kind("test:kind", vec![]);
    let registry = setup_subscribers(
        &engine,
        r#"
        _seal_error = nil
        on("test:kind@recorded", function(e)
            local ok, err = pcall(function() e.kind = "other" end)
            _seal_error = tostring(err)
        end)
        "#,
    );
    world.insert_resource(registry);
    world.insert_resource(engine);

    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        run_in_scope(
            world,
            &engine,
            r#"
            _gs:record_knowledge({
                kind = "test:kind",
                payload = {},
            })
            "#,
        )
        .unwrap();

        let err: String = engine.lua().globals().get("_seal_error").unwrap();
        assert!(
            err.contains("immutable"),
            "Expected immutable key error, got: {err}"
        );
    });
}

#[test]
fn record_knowledge_unknown_kind_errors() {
    let engine = ScriptEngine::new().unwrap();
    let mut world = make_world_with_kind("test:kind", vec![]);
    let registry = setup_subscribers(&engine, "");
    world.insert_resource(registry);
    world.insert_resource(engine);

    let mut errored = false;
    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        let result = run_in_scope(
            world,
            &engine,
            r#"
            _gs:record_knowledge({
                kind = "nonexistent:kind",
                payload = {},
            })
            "#,
        );
        errored = result.is_err();
        if let Err(e) = result {
            let msg = format!("{e}");
            assert!(
                msg.contains("unknown kind"),
                "Expected 'unknown kind' error, got: {msg}"
            );
        }
    });
    assert!(errored, "Should error on unknown kind");
}

#[test]
fn record_knowledge_schema_violation_errors() {
    let engine = ScriptEngine::new().unwrap();
    let mut world = make_world_with_kind("test:kind", vec![("severity", PayloadFieldType::Number)]);
    let registry = setup_subscribers(&engine, "");
    world.insert_resource(registry);
    world.insert_resource(engine);

    let mut errored = false;
    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        let result = run_in_scope(
            world,
            &engine,
            r#"
            _gs:record_knowledge({
                kind = "test:kind",
                payload = { severity = "high" },
            })
            "#,
        );
        errored = result.is_err();
    });
    assert!(errored, "Schema violation should error");
}

#[test]
fn record_knowledge_wildcard_subscriber_fires() {
    let engine = ScriptEngine::new().unwrap();
    let mut world = make_world_with_kind("test:kind", vec![]);
    let registry = setup_subscribers(
        &engine,
        r#"
        _wildcard_fired = false
        on("*@recorded", function(e)
            _wildcard_fired = true
        end)
        "#,
    );
    world.insert_resource(registry);
    world.insert_resource(engine);

    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        run_in_scope(
            world,
            &engine,
            r#"
            _gs:record_knowledge({
                kind = "test:kind",
                payload = {},
            })
            "#,
        )
        .unwrap();

        let fired: bool = engine.lua().globals().get("_wildcard_fired").unwrap();
        assert!(fired, "*@recorded wildcard subscriber should fire");
    });
}

#[test]
fn record_knowledge_subscriber_error_continues_chain() {
    let engine = ScriptEngine::new().unwrap();
    let mut world = make_world_with_kind("test:kind", vec![]);
    let registry = setup_subscribers(
        &engine,
        r#"
        _chain_reached = false
        on("test:kind@recorded", function(e)
            error("intentional error")
        end)
        on("test:kind@recorded", function(e)
            _chain_reached = true
        end)
        "#,
    );
    world.insert_resource(registry);
    world.insert_resource(engine);

    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        // Should not propagate the subscriber error.
        run_in_scope(
            world,
            &engine,
            r#"
            _gs:record_knowledge({
                kind = "test:kind",
                payload = {},
            })
            "#,
        )
        .unwrap();

        let reached: bool = engine.lua().globals().get("_chain_reached").unwrap();
        assert!(
            reached,
            "Second subscriber should fire despite first erroring"
        );
    });

    // Fact should still be enqueued.
    let queue = world.resource::<PendingFactQueue>();
    assert_eq!(queue.facts.len(), 1);
}

#[test]
fn rust_origin_dispatch_knowledge_recorded() {
    use macrocosmo::knowledge::payload::PayloadSnapshot;
    use macrocosmo::scripting::knowledge_dispatch::{
        PendingKnowledgeRecord, dispatch_knowledge_recorded,
    };

    let engine = ScriptEngine::new().unwrap();
    let mut world = make_world_with_kind("test:kind", vec![]);
    let registry = setup_subscribers(
        &engine,
        r#"
        _rust_origin_fired = false
        on("test:kind@recorded", function(e)
            _rust_origin_fired = true
            e.payload.enriched = true
        end)
        "#,
    );
    world.insert_resource(registry);
    world.insert_resource(engine);

    // Push a Rust-origin record.
    {
        let mut pending = world.resource_mut::<PendingKnowledgeRecords>();
        pending.push(PendingKnowledgeRecord {
            kind_id: "test:kind".to_string(),
            origin_system: None,
            payload_snapshot: PayloadSnapshot {
                fields: [("initial".to_string(), PayloadValue::Boolean(true))]
                    .into_iter()
                    .collect(),
            },
            recorded_at: 100,
        });
    }

    // Run the dispatch system.
    dispatch_knowledge_recorded(&mut world);

    // Verify subscriber fired.
    world.resource_scope::<ScriptEngine, _>(|_world, engine| {
        let fired: bool = engine.lua().globals().get("_rust_origin_fired").unwrap();
        assert!(fired, "Rust-origin @recorded subscriber should fire");
    });

    // Verify fact was enqueued with enriched payload.
    let queue = world.resource::<PendingFactQueue>();
    assert_eq!(queue.facts.len(), 1);
    if let KnowledgeFact::Scripted {
        payload_snapshot, ..
    } = &queue.facts[0].fact
    {
        assert!(
            payload_snapshot.fields.contains_key("enriched"),
            "Subscriber should have enriched the payload"
        );
        assert!(
            payload_snapshot.fields.contains_key("initial"),
            "Original payload should be preserved"
        );
    } else {
        panic!("Expected Scripted fact");
    }
}
