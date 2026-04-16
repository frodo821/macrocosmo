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

use bevy::prelude::*;
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

// ======================================================================
// #351 K-2 Commit 4: Rust-origin knowledge record path
//
// Rust systems that produce knowledge facts cannot call Lua directly
// (feedback_rust_no_lua_callback). Instead they push records into
// PendingKnowledgeRecords, and a separate system
// (dispatch_knowledge_recorded) drains the queue with ScriptEngine
// exclusive access and fires @recorded subscribers.
//
// This is the system skeleton — K-5 will wire existing Rust fact
// emitters to push into this queue.
// ======================================================================

/// A pending knowledge record request from Rust-origin code.
#[derive(Debug, Clone)]
pub struct PendingKnowledgeRecord {
    pub kind_id: String,
    pub origin_system: Option<Entity>,
    pub payload_snapshot: crate::knowledge::payload::PayloadSnapshot,
    pub recorded_at: i64,
}

/// Resource queue for Rust-origin knowledge records awaiting @recorded
/// dispatch. Drained by [`dispatch_knowledge_recorded`] each tick.
#[derive(Resource, Default, Debug)]
pub struct PendingKnowledgeRecords {
    pub records: Vec<PendingKnowledgeRecord>,
}

impl PendingKnowledgeRecords {
    pub fn push(&mut self, record: PendingKnowledgeRecord) {
        self.records.push(record);
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// System that drains [`PendingKnowledgeRecords`], dispatches `@recorded`
/// subscribers for each record, and enqueues the (possibly mutated)
/// results into `PendingFactQueue`.
///
/// Scheduled `.after(knowledge emitters)` in the Update schedule so
/// records pushed by Rust systems are dispatched within the same tick
/// (plan-349 §0.5 9.1).
///
/// This system takes exclusive `&mut World` access to use
/// `resource_scope` for `ScriptEngine`. The `@recorded` subscriber
/// chain runs inside the scope with the subscription registry
/// temporarily removed from the world (same pattern as
/// `gs:record_knowledge` in gamestate_scope.rs).
pub fn dispatch_knowledge_recorded(world: &mut World) {
    // Fast-path: nothing to drain.
    let has_records = world
        .get_resource::<PendingKnowledgeRecords>()
        .map(|r| !r.is_empty())
        .unwrap_or(false);
    if !has_records {
        return;
    }

    // Take the pending records.
    let records = {
        let mut res = world.resource_mut::<PendingKnowledgeRecords>();
        std::mem::take(&mut res.records)
    };

    // Take the subscription registry out of the world so we don't need
    // to hold a world borrow during Lua dispatch.
    let registry_opt = world.remove_resource::<KnowledgeSubscriptionRegistry>();

    // Use resource_scope for ScriptEngine to get Lua access.
    world.resource_scope::<super::ScriptEngine, _>(|world, engine| {
        let lua = engine.lua();
        for record in &records {
            // Build the event table for @recorded dispatch.
            let event = match build_recorded_event_table(lua, record) {
                Ok(t) => t,
                Err(e) => {
                    warn!(
                        "dispatch_knowledge_recorded: failed to build event table for '{}': {e}",
                        record.kind_id
                    );
                    continue;
                }
            };

            // Seal immutable keys.
            if let Err(e) =
                seal_immutable_keys(lua, &event, &["kind", "origin_system", "recorded_at"])
            {
                warn!(
                    "dispatch_knowledge_recorded: seal error for '{}': {e}",
                    record.kind_id
                );
                continue;
            }

            // Dispatch @recorded subscribers.
            if let Some(ref registry) = registry_opt {
                if let Err(e) = dispatch_knowledge(
                    lua,
                    registry,
                    &record.kind_id,
                    KnowledgeLifecycle::Recorded,
                    &event,
                ) {
                    warn!(
                        "dispatch_knowledge_recorded: dispatch error for '{}': {e}",
                        record.kind_id
                    );
                }
            }

            // Snapshot the final payload after subscriber mutations.
            let final_payload: mlua::Table = match event.get("payload") {
                Ok(t) => t,
                Err(e) => {
                    warn!(
                        "dispatch_knowledge_recorded: payload read error for '{}': {e}",
                        record.kind_id
                    );
                    continue;
                }
            };
            let snapshot = match crate::knowledge::payload::snapshot_from_lua(
                lua,
                &final_payload,
                KNOWLEDGE_PAYLOAD_DEPTH_LIMIT,
            ) {
                Ok(s) => s,
                Err(e) => {
                    warn!(
                        "dispatch_knowledge_recorded: snapshot error for '{}': {e}",
                        record.kind_id
                    );
                    continue;
                }
            };

            // Enqueue via the same Lua-free apply helper.
            use crate::scripting::gamestate_scope::apply::{
                ParsedKnowledgeRecord, enqueue_scripted_fact,
            };
            if let Err(e) = enqueue_scripted_fact(
                world,
                ParsedKnowledgeRecord {
                    kind_id: record.kind_id.clone(),
                    origin_system: record.origin_system,
                    payload_snapshot: snapshot,
                    recorded_at: record.recorded_at,
                },
            ) {
                warn!(
                    "dispatch_knowledge_recorded: enqueue error for '{}': {e}",
                    record.kind_id
                );
            }
        }
    });

    // Put the subscription registry back.
    if let Some(registry) = registry_opt {
        world.insert_resource(registry);
    }
}

/// Build a Lua event table for @recorded dispatch from a Rust-origin
/// pending record.
fn build_recorded_event_table(
    lua: &Lua,
    record: &PendingKnowledgeRecord,
) -> mlua::Result<mlua::Table> {
    let event = lua.create_table()?;
    event.set("kind", record.kind_id.as_str())?;
    if let Some(origin) = record.origin_system {
        event.set("origin_system", origin.to_bits())?;
    }
    event.set("recorded_at", record.recorded_at)?;
    let payload = crate::knowledge::payload::snapshot_to_lua(lua, &record.payload_snapshot)?;
    event.set("payload", payload)?;
    Ok(event)
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

    // #353 K-4 Commit 1: per-observer isolation. Each call to
    // `snapshot_to_lua` must produce an independent Lua table so mutations
    // made by one observer's subscriber chain do not leak to the next
    // observer's copy (plan-349 §2.5, §3.4 test matrix).
    #[test]
    fn snapshot_to_lua_produces_independent_tables() {
        use crate::knowledge::payload::{PayloadSnapshot, PayloadValue, snapshot_to_lua};

        let lua = Lua::new();
        let mut fields = std::collections::HashMap::new();
        fields.insert("severity".to_string(), PayloadValue::Number(0.7));
        let snapshot = PayloadSnapshot { fields };

        // Reconstruct the Lua table twice — one per simulated observer.
        let observer_a = snapshot_to_lua(&lua, &snapshot).unwrap();
        let observer_b = snapshot_to_lua(&lua, &snapshot).unwrap();

        // Mutate observer A's copy; observer B must remain unchanged.
        observer_a.set("severity", 1.0_f64).unwrap();

        let a_val: f64 = observer_a.get("severity").unwrap();
        let b_val: f64 = observer_b.get("severity").unwrap();
        assert!((a_val - 1.0).abs() < f64::EPSILON);
        assert!(
            (b_val - 0.7).abs() < f64::EPSILON,
            "observer B payload must not be affected by observer A mutation, got {b_val}"
        );
    }

    // Per-observer isolation also holds for nested tables.
    #[test]
    fn snapshot_to_lua_nested_tables_are_independent() {
        use crate::knowledge::payload::{PayloadSnapshot, PayloadValue, snapshot_to_lua};

        let mut inner_fields = std::collections::HashMap::new();
        inner_fields.insert("count".to_string(), PayloadValue::Int(5));
        let mut outer_fields = std::collections::HashMap::new();
        outer_fields.insert(
            "stats".to_string(),
            PayloadValue::Table(PayloadSnapshot {
                fields: inner_fields,
            }),
        );
        let snapshot = PayloadSnapshot {
            fields: outer_fields,
        };

        let lua = Lua::new();
        let a = snapshot_to_lua(&lua, &snapshot).unwrap();
        let b = snapshot_to_lua(&lua, &snapshot).unwrap();

        let a_stats: mlua::Table = a.get("stats").unwrap();
        a_stats.set("count", 99).unwrap();

        let b_stats: mlua::Table = b.get("stats").unwrap();
        assert_eq!(
            b_stats.get::<i64>("count").unwrap(),
            5,
            "observer B nested table must be independent of observer A"
        );
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
