//! #352 (K-3 Commit 4): Integration tests for `dispatch_knowledge` +
//! end-to-end routing through `on()` → accumulator → drain → dispatcher.
//!
//! These tests exercise the full K-3 surface without touching K-2 /
//! K-4 call sites (which land later). The pattern is:
//!
//! 1. Spin up a `ScriptEngine` (real `setup_globals`).
//! 2. Register subscribers via Lua `on(...)` calls.
//! 3. Drain the accumulator into a `KnowledgeSubscriptionRegistry`.
//! 4. Build a payload table and call `dispatch_knowledge(...)`.
//! 5. Assert side effects written to Lua globals by the subscribers.
//!
//! This mirrors how K-2 (`gs:record_knowledge`) and K-4 (observer drain)
//! will call the dispatcher at runtime, minus the world-borrow
//! bookkeeping (which is K-2/K-4 concern, not K-3).

use macrocosmo::scripting::ScriptEngine;
use macrocosmo::scripting::knowledge_dispatch::{KnowledgeLifecycle, dispatch_knowledge};
use macrocosmo::scripting::knowledge_registry::{
    KnowledgeSubscriptionRegistry, drain_pending_subscriptions,
};

fn build(script: &str) -> (ScriptEngine, KnowledgeSubscriptionRegistry) {
    let engine = ScriptEngine::new().unwrap();
    engine.lua().load(script).exec().unwrap();
    let mut registry = KnowledgeSubscriptionRegistry::default();
    drain_pending_subscriptions(engine.lua(), &mut registry).unwrap();
    (engine, registry)
}

#[test]
fn dispatch_per_kind_subscriber_fires() {
    let (engine, registry) = build(
        r#"
        _fired = {}
        on("vesk:famine_outbreak@recorded", function(e)
            table.insert(_fired, "exact")
        end)
        "#,
    );

    let payload = engine.lua().create_table().unwrap();
    dispatch_knowledge(
        engine.lua(),
        &registry,
        "vesk:famine_outbreak",
        KnowledgeLifecycle::Recorded,
        &payload,
    )
    .unwrap();

    let fired: mlua::Table = engine.lua().globals().get("_fired").unwrap();
    assert_eq!(fired.len().unwrap(), 1);
    let s: String = fired.get(1).unwrap();
    assert_eq!(s, "exact");
}

#[test]
fn dispatch_only_matches_same_lifecycle() {
    let (engine, registry) = build(
        r#"
        _fired = {}
        on("vesk:famine@recorded", function(e) table.insert(_fired, "rec") end)
        on("vesk:famine@observed", function(e) table.insert(_fired, "obs") end)
        "#,
    );

    // Record-time dispatch should fire only the recorded subscriber.
    let payload = engine.lua().create_table().unwrap();
    dispatch_knowledge(
        engine.lua(),
        &registry,
        "vesk:famine",
        KnowledgeLifecycle::Recorded,
        &payload,
    )
    .unwrap();

    let fired: mlua::Table = engine.lua().globals().get("_fired").unwrap();
    assert_eq!(fired.len().unwrap(), 1);
    let s: String = fired.get(1).unwrap();
    assert_eq!(s, "rec");
}

#[test]
fn dispatch_wildcard_observed_matches_any_kind() {
    let (engine, registry) = build(
        r#"
        _fired = {}
        on("*@observed", function(e) table.insert(_fired, e.kind or "?") end)
        "#,
    );

    // Payload passes through; subscriber reads `e.kind` — we supply it
    // on the payload table directly (K-2 would seal this; K-3 test just
    // wants to verify the function sees the payload reference).
    let payload = engine.lua().create_table().unwrap();
    payload.set("kind", "mod:alien_ruins").unwrap();
    dispatch_knowledge(
        engine.lua(),
        &registry,
        "mod:alien_ruins",
        KnowledgeLifecycle::Observed,
        &payload,
    )
    .unwrap();

    // Second kind — same wildcard subscriber fires again.
    payload.set("kind", "mod:trade_offer").unwrap();
    dispatch_knowledge(
        engine.lua(),
        &registry,
        "mod:trade_offer",
        KnowledgeLifecycle::Observed,
        &payload,
    )
    .unwrap();

    let fired: mlua::Table = engine.lua().globals().get("_fired").unwrap();
    assert_eq!(fired.len().unwrap(), 2);
    let first: String = fired.get(1).unwrap();
    let second: String = fired.get(2).unwrap();
    assert_eq!(first, "mod:alien_ruins");
    assert_eq!(second, "mod:trade_offer");
}

#[test]
fn dispatch_wildcard_and_exact_both_fire_in_registration_order() {
    let (engine, registry) = build(
        r#"
        _order = {}
        on("kind_a@recorded", function(e) table.insert(_order, "exact_1") end)
        on("*@recorded", function(e) table.insert(_order, "wild_1") end)
        on("kind_a@recorded", function(e) table.insert(_order, "exact_2") end)
        on("*@recorded", function(e) table.insert(_order, "wild_2") end)
        "#,
    );

    let payload = engine.lua().create_table().unwrap();
    dispatch_knowledge(
        engine.lua(),
        &registry,
        "kind_a",
        KnowledgeLifecycle::Recorded,
        &payload,
    )
    .unwrap();

    // Dispatcher spec: exact bucket first (in registration order), then
    // wildcard bucket (in registration order). So expected order is
    // exact_1, exact_2, wild_1, wild_2.
    let order: mlua::Table = engine.lua().globals().get("_order").unwrap();
    let seen: Vec<String> = (1..=order.len().unwrap())
        .map(|i| order.get::<String>(i).unwrap())
        .collect();
    assert_eq!(
        seen,
        vec![
            "exact_1".to_string(),
            "exact_2".to_string(),
            "wild_1".to_string(),
            "wild_2".to_string(),
        ]
    );
}

#[test]
fn dispatch_subscriber_error_continues_chain() {
    // First subscriber raises; subsequent subscribers must still fire.
    let (engine, registry) = build(
        r#"
        _fired = {}
        on("kind_x@recorded", function(e)
            table.insert(_fired, "before_error")
            error("intentional test error")
        end)
        on("kind_x@recorded", function(e)
            table.insert(_fired, "after_error")
        end)
        on("*@recorded", function(e)
            table.insert(_fired, "wildcard")
        end)
        "#,
    );

    let payload = engine.lua().create_table().unwrap();
    let res = dispatch_knowledge(
        engine.lua(),
        &registry,
        "kind_x",
        KnowledgeLifecycle::Recorded,
        &payload,
    );
    // Dispatcher itself must return Ok — subscriber errors are swallowed
    // and warn-logged (plan-349 §6 item 4).
    assert!(
        res.is_ok(),
        "dispatcher must not surface subscriber error: {res:?}"
    );

    let fired: mlua::Table = engine.lua().globals().get("_fired").unwrap();
    let seen: Vec<String> = (1..=fired.len().unwrap())
        .map(|i| fired.get::<String>(i).unwrap())
        .collect();
    assert_eq!(
        seen,
        vec![
            "before_error".to_string(),
            "after_error".to_string(),
            "wildcard".to_string(),
        ]
    );
}

#[test]
fn dispatch_empty_registry_is_noop() {
    let engine = ScriptEngine::new().unwrap();
    let registry = KnowledgeSubscriptionRegistry::default();

    let payload = engine.lua().create_table().unwrap();
    // No subscribers, no error.
    dispatch_knowledge(
        engine.lua(),
        &registry,
        "nothing:registered",
        KnowledgeLifecycle::Recorded,
        &payload,
    )
    .unwrap();
}

#[test]
fn dispatch_payload_is_shared_across_chain() {
    // plan-349 §2.4: subscribers see each other's payload mutations
    // during a single dispatch (chain-of-responsibility). K-3 builds this
    // baseline; K-2 adds sealed metadata on top.
    let (engine, registry) = build(
        r#"
        on("enrich@recorded", function(e)
            e.enriched_by_first = true
        end)
        on("enrich@recorded", function(e)
            if e.enriched_by_first then
                e.second_saw_first = true
            end
        end)
        "#,
    );

    let payload = engine.lua().create_table().unwrap();
    dispatch_knowledge(
        engine.lua(),
        &registry,
        "enrich",
        KnowledgeLifecycle::Recorded,
        &payload,
    )
    .unwrap();

    let first: bool = payload.get("enriched_by_first").unwrap();
    let second: bool = payload.get("second_saw_first").unwrap();
    assert!(first, "first subscriber mutation must land on payload");
    assert!(
        second,
        "second subscriber must see first subscriber's mutation (chain-of-responsibility)"
    );
}

#[test]
fn dispatch_respects_drain_then_fresh_register() {
    // Register one subscriber, drain, register another, drain again.
    // Both must fire (the registry accumulates across drains).
    let engine = ScriptEngine::new().unwrap();
    let mut registry = KnowledgeSubscriptionRegistry::default();

    engine
        .lua()
        .load(
            r#"
            _fired = {}
            on("incr@recorded", function(e) table.insert(_fired, "a") end)
            "#,
        )
        .exec()
        .unwrap();
    drain_pending_subscriptions(engine.lua(), &mut registry).unwrap();

    // Add a second registration after drain.
    engine
        .lua()
        .load(
            r#"
            on("incr@recorded", function(e) table.insert(_fired, "b") end)
            "#,
        )
        .exec()
        .unwrap();
    drain_pending_subscriptions(engine.lua(), &mut registry).unwrap();

    let payload = engine.lua().create_table().unwrap();
    dispatch_knowledge(
        engine.lua(),
        &registry,
        "incr",
        KnowledgeLifecycle::Recorded,
        &payload,
    )
    .unwrap();

    let fired: mlua::Table = engine.lua().globals().get("_fired").unwrap();
    let seen: Vec<String> = (1..=fired.len().unwrap())
        .map(|i| fired.get::<String>(i).unwrap())
        .collect();
    assert_eq!(seen, vec!["a".to_string(), "b".to_string()]);
}
