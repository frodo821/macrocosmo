//! #352 (K-3) knowledge subscription dispatcher.
//!
//! This module hosts helpers for the `on(event_id, fn)` knowledge
//! subscription surface introduced by plan-349 §3.2. It is landed in
//! Wave 1 (in parallel with K-1 #350); the actual wiring that invokes
//! `dispatch_knowledge` lives in K-2 (`gs:record_knowledge` setter) and
//! K-4 (observer drain). Until those sub-issues land, `dispatch_knowledge`
//! is exercised only from integration tests.
//!
//! Public surface:
//! - [`KnowledgeLifecycle`] — `recorded` / `observed` enum used both for
//!   routing `on()` registrations and as the dispatcher lifecycle arg.
//! - [`seal_immutable_keys`] — attaches a `__newindex` metatable to a Lua
//!   table that raises `mlua::Error::RuntimeError` on writes to any of the
//!   supplied immutable keys. Spike 10.1 from plan-349 §10.
//! - [`is_knowledge_event_id`] / [`parse_knowledge_event_id`] — event id
//!   syntax helpers used by the `on()` router (Commit 3).
//! - [`dispatch_knowledge`] — walks the
//!   [`crate::scripting::knowledge_registry::KnowledgeSubscriptionRegistry`]
//!   buckets and invokes subscribers in registration order (Commit 4).
//!
//! Invariants (plan-349 §6):
//! - subscriber error = warn log + chain continuation, **never panic**.
//! - dispatch order is deterministic: exact bucket (registration order)
//!   then wildcard bucket (registration order). Callers combine this with
//!   per-kind logic at a higher layer (#345 notification bridge) if they
//!   need a different order.
//! - callers of `dispatch_knowledge` must ensure any `&mut World` borrows
//!   have been released before calling, because subscribers may re-enter
//!   via `gs:*` setters (spike 10.4, K-2).

use bevy::prelude::warn;
use mlua::prelude::*;

use super::knowledge_registry::KnowledgeSubscriptionRegistry;

/// Lifecycle suffix for a knowledge event id (`<kind>@<lifecycle>` or
/// `*@<lifecycle>`). v1 supports only `recorded` and `observed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KnowledgeLifecycle {
    Recorded,
    Observed,
}

impl KnowledgeLifecycle {
    pub fn as_str(&self) -> &'static str {
        match self {
            KnowledgeLifecycle::Recorded => "recorded",
            KnowledgeLifecycle::Observed => "observed",
        }
    }

    /// Parse a lifecycle suffix. Returns `None` for anything other than
    /// `"recorded"` / `"observed"` so callers can surface a load-time
    /// error (plan-349 §0.5 9.2).
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "recorded" => Some(KnowledgeLifecycle::Recorded),
            "observed" => Some(KnowledgeLifecycle::Observed),
            _ => None,
        }
    }
}

/// Parsed components of a knowledge event id.
///
/// `kind` is either the literal kind id (e.g. `"vesk:famine_outbreak"`) or
/// the suffix wildcard sentinel `"*"`. `lifecycle` is the parsed
/// [`KnowledgeLifecycle`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedKnowledgeEventId {
    pub kind: String,
    pub lifecycle: KnowledgeLifecycle,
    pub is_wildcard: bool,
}

/// Quick check: does this event_id *look* like a knowledge event id? Used
/// by `on()` to decide whether to route to the knowledge subscription
/// registry vs. the legacy `_event_handlers` table.
///
/// Accepts anything ending in `@recorded` or `@observed`. Pathological
/// forms like `"@recorded"` or `"foo@"` are handled by
/// [`parse_knowledge_event_id`] with an explicit error (plan-349 §0.5 9.2).
pub fn is_knowledge_event_id(s: &str) -> bool {
    match s.rsplit_once('@') {
        Some((_, lc)) => KnowledgeLifecycle::from_str(lc).is_some(),
        None => false,
    }
}

/// Parse an event id of the form `<kind>@<lifecycle>` or
/// `*@<lifecycle>`. Returns error for:
/// - missing `@`
/// - empty kind part
/// - unknown lifecycle suffix
/// - `@` appearing in the kind part (pathological, plan-349 §0.5 9.2)
pub fn parse_knowledge_event_id(s: &str) -> mlua::Result<ParsedKnowledgeEventId> {
    let (kind, lc) = match s.rsplit_once('@') {
        Some(parts) => parts,
        None => {
            return Err(mlua::Error::RuntimeError(format!(
                "knowledge event id '{s}' missing '@<lifecycle>' suffix"
            )));
        }
    };
    let lifecycle = KnowledgeLifecycle::from_str(lc).ok_or_else(|| {
        mlua::Error::RuntimeError(format!(
            "knowledge event id '{s}' has unknown lifecycle '{lc}' (expected 'recorded' or 'observed')"
        ))
    })?;
    if kind.is_empty() {
        return Err(mlua::Error::RuntimeError(format!(
            "knowledge event id '{s}' has empty kind part before '@'"
        )));
    }
    if kind.contains('@') {
        return Err(mlua::Error::RuntimeError(format!(
            "knowledge event id '{s}' kind part may not contain '@' (namespace separator is ':')"
        )));
    }
    let is_wildcard = kind == "*";
    Ok(ParsedKnowledgeEventId {
        kind: kind.to_string(),
        lifecycle,
        is_wildcard,
    })
}

/// Match a stored subscription pattern (parsed) against an incoming event
/// `(kind_id, lifecycle)` pair. Used by `dispatch_knowledge` to gate each
/// candidate subscriber.
pub fn event_id_matches(
    pattern: &ParsedKnowledgeEventId,
    kind_id: &str,
    lifecycle: KnowledgeLifecycle,
) -> bool {
    if pattern.lifecycle != lifecycle {
        return false;
    }
    if pattern.is_wildcard {
        return true;
    }
    pattern.kind == kind_id
}

/// Attach a `__newindex` metatable to `payload` that raises
/// `mlua::Error::RuntimeError` when Lua code writes to any key in
/// `immutable_keys`. Other writes pass through normally.
///
/// plan-349 §2.6 (Option B): payload is a plain table with a metatable
/// that blocks only a fixed key set. Nested tables (e.g. `payload.payload`)
/// are unaffected — the metatable is attached only to the outermost
/// wrapper.
///
/// Implementation notes:
/// - We rely on Lua's standard rule that `__newindex` is only consulted
///   for **new** keys (keys not already present in the raw table). So we
///   do **not** pre-populate the immutable keys on the raw table; instead
///   we store them on a separate `__mc_values` sub-table accessed via
///   `__index`. Reads fall through to that sub-table, writes to sealed
///   keys land in `__newindex` and raise. Writes to unsealed keys go
///   straight into the raw payload table.
/// - This is the spike-10.1 pattern validated by
///   `tests/spike_seal_immutable_keys.rs`.
///
/// The `payload` table is expected to already contain all mutable keys
/// (e.g. `payload = { ... }`). Sealed keys are **moved** from the raw
/// table onto the internal `__mc_values` sub-table; callers should set
/// them on `payload` before calling this helper.
pub fn seal_immutable_keys(
    lua: &Lua,
    payload: &mlua::Table,
    immutable_keys: &[&str],
) -> mlua::Result<()> {
    // Move each immutable key's value off the raw payload table onto an
    // internal __mc_values table. After this, raw reads of those keys
    // return nil, which triggers __index lookup.
    let values = lua.create_table()?;
    for &key in immutable_keys {
        let v: mlua::Value = payload.get(key)?;
        values.set(key, v)?;
        payload.set(key, mlua::Value::Nil)?;
    }

    // Build the set of immutable keys as a Lua table for __newindex lookup.
    let immutable_set = lua.create_table()?;
    for &key in immutable_keys {
        immutable_set.set(key, true)?;
    }

    // __index: fall back to __mc_values for reads of sealed keys.
    let values_for_index = values.clone();
    let index_fn = lua.create_function(
        move |_, (_t, k): (mlua::Table, mlua::Value)| -> mlua::Result<mlua::Value> {
            let key_str: Option<String> = match &k {
                mlua::Value::String(s) => Some(s.to_str()?.to_string()),
                _ => None,
            };
            match key_str {
                Some(ref s) => values_for_index.get::<mlua::Value>(s.as_str()),
                None => Ok(mlua::Value::Nil),
            }
        },
    )?;

    // __newindex: block writes to sealed keys, passthrough otherwise.
    let immutable_set_for_newindex = immutable_set.clone();
    let newindex_fn = lua.create_function(
        move |_, (t, k, v): (mlua::Table, mlua::Value, mlua::Value)| -> mlua::Result<()> {
            if let mlua::Value::String(ref s) = k {
                let s_str = s.to_str()?.to_string();
                let sealed: bool = immutable_set_for_newindex
                    .get::<Option<bool>>(s_str.as_str())?
                    .unwrap_or(false);
                if sealed {
                    return Err(mlua::Error::RuntimeError(format!(
                        "immutable knowledge payload key '{s_str}'"
                    )));
                }
            }
            // Use rawset so we don't re-enter __newindex if the user
            // overwrites an existing mutable key.
            t.raw_set(k, v)?;
            Ok(())
        },
    )?;

    let mt = lua.create_table()?;
    mt.set("__index", index_fn)?;
    mt.set("__newindex", newindex_fn)?;
    // Stash the values sub-table on the metatable so it can't be casually
    // picked up from the outer payload iteration (pairs).
    mt.set("__mc_values", values)?;
    payload.set_metatable(Some(mt))?;
    Ok(())
}

/// Dispatch a knowledge event to all matching subscribers.
///
/// Walks the exact bucket for `"<kind_id>@<lifecycle>"` first, then the
/// wildcard bucket for `lifecycle`, invoking each subscriber in its
/// registration order. The `payload` table is shared across subscribers:
/// they observe any mutations previous subscribers applied, as required
/// for the payload-mutation chain (plan-349 §2.4 `@recorded` flow). For
/// `@observed`, callers build a fresh per-observer copy before dispatch
/// (K-4).
///
/// Subscriber errors are logged via `warn!` and the chain continues to
/// the next subscriber (plan-349 §6 item 4). This mirrors the pattern in
/// `lifecycle::dispatch_bus_handlers`. Any error returned by *this*
/// function is a dispatcher-internal failure (e.g. registry lookup,
/// registry_value resolution), not a subscriber error.
///
/// # Reentrancy
///
/// Subscribers are permitted to call back into other `gs:*` setters or
/// even re-enter `gs:record_knowledge` itself. Callers must ensure any
/// `&mut World` borrow tied to `gs:*` has been released before invoking
/// this function — the K-2 `record_knowledge` setter releases its
/// `world_cell.try_borrow_mut()` guard before calling `dispatch_knowledge`
/// specifically to support this (spike 10.4, validated in K-2).
///
/// Until K-2 and K-4 land, this function is exercised only from tests
/// (`tests/knowledge_subscription_dispatch.rs`).
pub fn dispatch_knowledge(
    lua: &Lua,
    registry: &KnowledgeSubscriptionRegistry,
    kind_id: &str,
    lifecycle: KnowledgeLifecycle,
    payload: &mlua::Table,
) -> mlua::Result<()> {
    let exact_key = format!("{kind_id}@{}", lifecycle.as_str());

    // Exact bucket first.
    if let Some(bucket) = registry.exact.get(&exact_key) {
        for key in bucket {
            call_subscriber(lua, key, payload, &exact_key);
        }
    }

    // Wildcard bucket next.
    if let Some(bucket) = registry.wildcard.get(&lifecycle) {
        for key in bucket {
            call_subscriber(lua, key, payload, &exact_key);
        }
    }

    Ok(())
}

/// Internal helper: look up the Lua function behind a `RegistryKey` and
/// call it with the payload. Errors in either lookup or subscriber body
/// are `warn!`-logged and swallowed — the dispatch chain must always
/// continue (plan-349 §6 item 4).
fn call_subscriber(lua: &Lua, key: &mlua::RegistryKey, payload: &mlua::Table, event_id: &str) {
    match lua.registry_value::<mlua::Function>(key) {
        Ok(func) => {
            if let Err(e) = func.call::<()>(payload.clone()) {
                warn!("knowledge subscriber error for '{event_id}': {e}");
            }
        }
        Err(e) => {
            warn!("knowledge subscriber registry_value lookup failed for '{event_id}': {e}");
        }
    }
}

/// Maximum nesting depth for `deep_copy_table`. Exceeding this triggers
/// `mlua::Error::RuntimeError` (plan-349 §0.5 9.3).
pub const KNOWLEDGE_PAYLOAD_DEPTH_LIMIT: usize = 16;

/// Deep-copy a Lua table, recursing into nested tables up to `depth_limit`.
///
/// Returns `mlua::Error::RuntimeError` if:
/// - nesting exceeds `depth_limit`
/// - a `Function` or `UserData` value is encountered (schema violation,
///   spike 10.3)
///
/// Metatables are **not** copied — the result is a plain table. Callers
/// that need sealed metadata should call `seal_immutable_keys` on the
/// copy separately.
pub fn deep_copy_table(
    lua: &Lua,
    src: &mlua::Table,
    depth_limit: usize,
) -> mlua::Result<mlua::Table> {
    if depth_limit == 0 {
        return Err(mlua::Error::RuntimeError(
            "deep_copy_table: depth limit exceeded".into(),
        ));
    }
    let dst = lua.create_table()?;
    for pair in src.pairs::<mlua::Value, mlua::Value>() {
        let (k, v) = pair?;
        let copied_v = match v {
            mlua::Value::Table(ref t) => {
                mlua::Value::Table(deep_copy_table(lua, t, depth_limit - 1)?)
            }
            mlua::Value::Function(_) => {
                return Err(mlua::Error::RuntimeError(
                    "deep_copy_table: Function values are not allowed in knowledge payloads".into(),
                ));
            }
            mlua::Value::UserData(_) => {
                return Err(mlua::Error::RuntimeError(
                    "deep_copy_table: UserData values are not allowed in knowledge payloads".into(),
                ));
            }
            other => other,
        };
        dst.set(k, copied_v)?;
    }
    Ok(dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_exact_knowledge_event_id() {
        let p = parse_knowledge_event_id("vesk:famine_outbreak@recorded").unwrap();
        assert_eq!(p.kind, "vesk:famine_outbreak");
        assert_eq!(p.lifecycle, KnowledgeLifecycle::Recorded);
        assert!(!p.is_wildcard);
    }

    #[test]
    fn parse_wildcard_observed() {
        let p = parse_knowledge_event_id("*@observed").unwrap();
        assert_eq!(p.kind, "*");
        assert_eq!(p.lifecycle, KnowledgeLifecycle::Observed);
        assert!(p.is_wildcard);
    }

    #[test]
    fn parse_missing_at_errors() {
        let e = parse_knowledge_event_id("no_suffix").unwrap_err();
        let msg = format!("{e}");
        assert!(msg.contains("missing '@"), "got: {msg}");
    }

    #[test]
    fn parse_unknown_lifecycle_errors() {
        let e = parse_knowledge_event_id("foo@expired").unwrap_err();
        let msg = format!("{e}");
        assert!(msg.contains("unknown lifecycle"), "got: {msg}");
    }

    #[test]
    fn parse_empty_kind_errors() {
        let e = parse_knowledge_event_id("@recorded").unwrap_err();
        let msg = format!("{e}");
        assert!(msg.contains("empty kind"), "got: {msg}");
    }

    #[test]
    fn parse_double_at_errors() {
        let e = parse_knowledge_event_id("foo@bar@recorded").unwrap_err();
        let msg = format!("{e}");
        assert!(msg.contains("may not contain '@'"), "got: {msg}");
    }

    #[test]
    fn is_knowledge_event_id_classifies_correctly() {
        assert!(is_knowledge_event_id("foo@recorded"));
        assert!(is_knowledge_event_id("*@observed"));
        assert!(!is_knowledge_event_id("harvest_ended"));
        assert!(!is_knowledge_event_id("foo@expired")); // unknown lifecycle
        assert!(!is_knowledge_event_id("plain_string"));
    }

    #[test]
    fn event_id_matches_exact() {
        let pat = parse_knowledge_event_id("foo:bar@recorded").unwrap();
        assert!(event_id_matches(
            &pat,
            "foo:bar",
            KnowledgeLifecycle::Recorded
        ));
        assert!(!event_id_matches(
            &pat,
            "foo:bar",
            KnowledgeLifecycle::Observed
        ));
        assert!(!event_id_matches(
            &pat,
            "other",
            KnowledgeLifecycle::Recorded
        ));
    }

    #[test]
    fn event_id_matches_wildcard() {
        let pat = parse_knowledge_event_id("*@observed").unwrap();
        assert!(event_id_matches(
            &pat,
            "anything",
            KnowledgeLifecycle::Observed
        ));
        assert!(event_id_matches(
            &pat,
            "foo:bar",
            KnowledgeLifecycle::Observed
        ));
        assert!(!event_id_matches(
            &pat,
            "foo:bar",
            KnowledgeLifecycle::Recorded
        ));
    }

    // --- deep_copy_table ---

    #[test]
    fn deep_copy_flat_table() {
        let lua = Lua::new();
        let src = lua.create_table().unwrap();
        src.set("a", 1).unwrap();
        src.set("b", "hello").unwrap();
        let dst = deep_copy_table(&lua, &src, KNOWLEDGE_PAYLOAD_DEPTH_LIMIT).unwrap();
        assert_eq!(dst.get::<i64>("a").unwrap(), 1);
        assert_eq!(dst.get::<String>("b").unwrap(), "hello");
        // Mutation isolation: mutating dst should not affect src.
        dst.set("a", 99).unwrap();
        assert_eq!(src.get::<i64>("a").unwrap(), 1);
    }

    #[test]
    fn deep_copy_nested_table() {
        let lua = Lua::new();
        let inner = lua.create_table().unwrap();
        inner.set("x", 42).unwrap();
        let src = lua.create_table().unwrap();
        src.set("inner", inner).unwrap();
        let dst = deep_copy_table(&lua, &src, KNOWLEDGE_PAYLOAD_DEPTH_LIMIT).unwrap();
        let dst_inner: mlua::Table = dst.get("inner").unwrap();
        dst_inner.set("x", 99).unwrap();
        // Original should be untouched.
        let src_inner: mlua::Table = src.get("inner").unwrap();
        assert_eq!(src_inner.get::<i64>("x").unwrap(), 42);
    }

    // Spike 10.3: Function value in table triggers error.
    #[test]
    fn spike_deep_copy_rejects_function() {
        let lua = Lua::new();
        let src = lua.create_table().unwrap();
        let f: mlua::Function = lua.load("function() end").eval().unwrap();
        src.set("callback", f).unwrap();
        let err = deep_copy_table(&lua, &src, KNOWLEDGE_PAYLOAD_DEPTH_LIMIT).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("Function"), "got: {msg}");
    }

    #[test]
    fn deep_copy_depth_limit_exceeded() {
        let lua = Lua::new();
        // Build a chain 3 levels deep, then copy with limit=2 -> should error.
        let t1 = lua.create_table().unwrap();
        let t2 = lua.create_table().unwrap();
        let t3 = lua.create_table().unwrap();
        t3.set("leaf", true).unwrap();
        t2.set("child", t3).unwrap();
        t1.set("child", t2).unwrap();
        let err = deep_copy_table(&lua, &t1, 2).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("depth limit"), "got: {msg}");
    }

    // Spike 10.4: borrow_mut release -> dispatch -> re-borrow is safe.
    // This tests the pattern used by gs:record_knowledge (K-2 Commit 3).
    #[test]
    fn spike_reentrancy_release_before_dispatch() {
        use std::cell::RefCell;

        let lua = Lua::new();
        let counter = RefCell::new(0i32);

        // Simulate: borrow_mut -> release -> dispatch (lua call) -> re-borrow.
        {
            let mut borrow = counter.borrow_mut();
            *borrow += 1;
            // release borrow
        }

        // Now call a Lua function that in turn would "re-borrow" (simulated).
        let f: mlua::Function = lua.load("function() end").eval().unwrap();
        f.call::<()>(()).unwrap();

        // re-borrow succeeds
        {
            let mut borrow = counter.borrow_mut();
            *borrow += 1;
        }
        assert_eq!(*counter.borrow(), 2);
    }

    // Spike 10.4: verify that a scope closure can release its world
    // borrow, call dispatch_knowledge (which invokes subscribers), and
    // re-borrow without conflict.
    #[test]
    fn spike_scope_closure_borrow_release_dispatch_reborrow() {
        use super::*;
        use crate::scripting::knowledge_registry::{
            KnowledgeSubscriptionRegistry, drain_pending_subscriptions,
        };
        use std::cell::RefCell;

        let lua = Lua::new();
        // Set up the on() global + accumulator.
        let engine = crate::scripting::ScriptEngine::new().unwrap();
        engine
            .lua()
            .load(
                r#"
            _side_effect = 0
            on("test:kind@recorded", function(e)
                _side_effect = _side_effect + 1
            end)
        "#,
            )
            .exec()
            .unwrap();
        let mut registry = KnowledgeSubscriptionRegistry::default();
        drain_pending_subscriptions(engine.lua(), &mut registry).unwrap();

        // Simulate the RefCell<&mut World> pattern.
        let mut world_data: i32 = 0;
        let world_cell = RefCell::new(&mut world_data);

        // Step 1: borrow, do work, release.
        {
            let mut borrow = world_cell.try_borrow_mut().unwrap();
            **borrow = 42;
        }
        // Step 2: dispatch (Lua executes, could re-enter world via gs:*).
        let payload = engine.lua().create_table().unwrap();
        dispatch_knowledge(
            engine.lua(),
            &registry,
            "test:kind",
            KnowledgeLifecycle::Recorded,
            &payload,
        )
        .unwrap();
        // Step 3: re-borrow succeeds.
        {
            let borrow = world_cell.try_borrow_mut().unwrap();
            assert_eq!(**borrow, 42);
        }
        let se: i64 = engine.lua().globals().get("_side_effect").unwrap();
        assert_eq!(se, 1);
    }
}
