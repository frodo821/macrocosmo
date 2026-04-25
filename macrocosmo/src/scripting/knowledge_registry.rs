//! #352 (K-3 Commit 2): [`KnowledgeSubscriptionRegistry`] resource.
//!
//! plan-349 §0.5 9.4 mandates bucketing the subscription table from v1
//! rather than full-scanning the Lua accumulator on every dispatch. This
//! module houses the Rust-side bucket store plus the `load_knowledge_subscriptions`
//! startup system that drains Lua's `_pending_knowledge_subscriptions`
//! accumulator into the bucketed registry.
//!
//! ## Invariants
//!
//! - Bucket insertion order is preserved (the `Vec<RegistryKey<Function>>` is
//!   append-only during drain). `dispatch_knowledge` relies on this for the
//!   "registration order = dispatch order" guarantee (plan-349 §6 item 5).
//! - Subscriptions are not keyed off a live Lua table after drain — the
//!   registry holds proper `mlua::RegistryKey` handles so subsequent Lua
//!   reloads (or scope teardowns) don't invalidate the functions.
//! - Unregistration is **not** supported in v1 (plan-349 §7).
//! - Subscribers whose event id fails load-time parsing are dropped with a
//!   `warn!` log and never enter the registry (deterministic rejection).
//! - `ScriptEngine` must be constructed and all script files loaded before
//!   the drain system runs so the accumulator is populated.

use std::collections::HashMap;

use bevy::prelude::*;
use mlua::prelude::*;

use super::ScriptEngine;
use super::knowledge_dispatch::{KnowledgeLifecycle, parse_knowledge_event_id};

/// Name of the Lua-side accumulator table that `on(event_id, fn)`
/// appends to for knowledge event ids. Drained by
/// [`load_knowledge_subscriptions`] at startup.
pub const PENDING_KNOWLEDGE_SUBSCRIPTIONS: &str = "_pending_knowledge_subscriptions";

/// Bucketed subscription registry.
///
/// `exact` keys are the full `"<kind>@<lifecycle>"` pattern (e.g.
/// `"vesk:famine_outbreak@recorded"`). `wildcard` keys are just the
/// lifecycle — functions registered for `"*@recorded"` / `"*@observed"`
/// live there.
///
/// Lookup is `HashMap` O(1) on both buckets; dispatch iterates the
/// per-bucket `Vec` in registration order.
#[derive(Resource, Default, Debug, Reflect)]
#[reflect(Resource)]
pub struct KnowledgeSubscriptionRegistry {
    /// Exact pattern -> subscribers in registration order.
    /// Subscriber values are `mlua::RegistryKey` handles (external,
    /// non-`Reflect`); the keys (pattern strings) remain visible.
    #[reflect(ignore)]
    pub exact: HashMap<String, Vec<mlua::RegistryKey>>,
    /// Wildcard lifecycle -> subscribers in registration order.
    /// Subscriber values are `mlua::RegistryKey` handles (opaque to
    /// reflection).
    #[reflect(ignore)]
    pub wildcard: HashMap<KnowledgeLifecycle, Vec<mlua::RegistryKey>>,
}

impl KnowledgeSubscriptionRegistry {
    /// Count of subscribers across both buckets. Used by tests only.
    #[cfg(test)]
    pub fn total_len(&self) -> usize {
        self.exact.values().map(|v| v.len()).sum::<usize>()
            + self.wildcard.values().map(|v| v.len()).sum::<usize>()
    }
}

/// Result of parsing one accumulator entry for logging / tests.
#[derive(Debug, Clone)]
pub struct DrainStats {
    pub total_seen: usize,
    pub registered: usize,
    pub skipped: usize,
}

/// Drain the Lua-side `_pending_knowledge_subscriptions` accumulator into
/// the Rust [`KnowledgeSubscriptionRegistry`]. Safe to call multiple times;
/// each invocation moves the current pending entries and replaces the
/// accumulator with an empty table.
///
/// Returns [`DrainStats`] so callers / tests can confirm load behaviour.
pub fn drain_pending_subscriptions(
    lua: &Lua,
    registry: &mut KnowledgeSubscriptionRegistry,
) -> mlua::Result<DrainStats> {
    let globals = lua.globals();
    let pending: mlua::Table = match globals.get(PENDING_KNOWLEDGE_SUBSCRIPTIONS) {
        Ok(t) => t,
        Err(_) => {
            // No accumulator — nothing to drain. Not an error because
            // tests may instantiate a bare `Lua` without setup_globals.
            return Ok(DrainStats {
                total_seen: 0,
                registered: 0,
                skipped: 0,
            });
        }
    };
    let len = pending.len().unwrap_or(0);
    let mut stats = DrainStats {
        total_seen: len as usize,
        registered: 0,
        skipped: 0,
    };
    for i in 1..=len {
        let entry: mlua::Table = match pending.get(i) {
            Ok(t) => t,
            Err(e) => {
                warn!("knowledge subscription entry {i} not a table: {e}");
                stats.skipped += 1;
                continue;
            }
        };
        let event_id: String = match entry.get("event_id") {
            Ok(s) => s,
            Err(e) => {
                warn!("knowledge subscription entry {i} missing event_id: {e}");
                stats.skipped += 1;
                continue;
            }
        };
        let func: mlua::Function = match entry.get("func") {
            Ok(f) => f,
            Err(e) => {
                warn!("knowledge subscription entry {i} ('{event_id}') missing func: {e}");
                stats.skipped += 1;
                continue;
            }
        };
        let parsed = match parse_knowledge_event_id(&event_id) {
            Ok(p) => p,
            Err(e) => {
                warn!("knowledge subscription entry {i} ('{event_id}') parse error: {e}");
                stats.skipped += 1;
                continue;
            }
        };
        let key = match lua.create_registry_value(func) {
            Ok(k) => k,
            Err(e) => {
                warn!("knowledge subscription entry {i} ('{event_id}') registry_value error: {e}");
                stats.skipped += 1;
                continue;
            }
        };
        if parsed.is_wildcard {
            registry
                .wildcard
                .entry(parsed.lifecycle)
                .or_default()
                .push(key);
        } else {
            registry
                .exact
                .entry(event_id.clone())
                .or_default()
                .push(key);
        }
        stats.registered += 1;
    }

    // Replace the accumulator with a fresh empty table so subsequent
    // script loads (e.g. hot-reload) start clean.
    globals.set(PENDING_KNOWLEDGE_SUBSCRIPTIONS, lua.create_table()?)?;
    Ok(stats)
}

/// Startup system that drains Lua-side knowledge subscriptions into the
/// [`KnowledgeSubscriptionRegistry`] resource. Scheduled to run
/// `.after(load_all_scripts)` so the accumulator is populated.
pub fn load_knowledge_subscriptions(mut commands: Commands, engine: Res<ScriptEngine>) {
    let mut registry = KnowledgeSubscriptionRegistry::default();
    match drain_pending_subscriptions(engine.lua(), &mut registry) {
        Ok(stats) => {
            info!(
                "Loaded {} knowledge subscription(s) ({} exact + {} wildcard, {} skipped of {} seen)",
                stats.registered,
                registry.exact.values().map(|v| v.len()).sum::<usize>(),
                registry.wildcard.values().map(|v| v.len()).sum::<usize>(),
                stats.skipped,
                stats.total_seen,
            );
        }
        Err(e) => {
            warn!("drain_pending_subscriptions failed: {e}");
        }
    }
    commands.insert_resource(registry);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `Lua` state with just the accumulator primed. We avoid
    /// bootstrapping the full `ScriptEngine` here because the router
    /// extension (Commit 3) is what normally writes entries — for drain
    /// unit tests we construct entries by hand.
    fn lua_with_accumulator() -> Lua {
        let lua = Lua::new();
        lua.globals()
            .set(PENDING_KNOWLEDGE_SUBSCRIPTIONS, lua.create_table().unwrap())
            .unwrap();
        lua
    }

    fn push_entry(lua: &Lua, event_id: &str, func_body: &str) {
        let entry = lua.create_table().unwrap();
        entry.set("event_id", event_id).unwrap();
        let f: mlua::Function = lua.load(func_body).eval().unwrap();
        entry.set("func", f).unwrap();
        let pending: mlua::Table = lua.globals().get(PENDING_KNOWLEDGE_SUBSCRIPTIONS).unwrap();
        let len = pending.len().unwrap();
        pending.set(len + 1, entry).unwrap();
    }

    #[test]
    fn drain_routes_exact_and_wildcard() {
        let lua = lua_with_accumulator();
        push_entry(&lua, "foo:bar@recorded", "function() end");
        push_entry(&lua, "*@observed", "function() end");
        push_entry(&lua, "foo:bar@recorded", "function() end");

        let mut registry = KnowledgeSubscriptionRegistry::default();
        let stats = drain_pending_subscriptions(&lua, &mut registry).unwrap();
        assert_eq!(stats.total_seen, 3);
        assert_eq!(stats.registered, 3);
        assert_eq!(stats.skipped, 0);
        assert_eq!(registry.exact.get("foo:bar@recorded").unwrap().len(), 2);
        assert_eq!(
            registry
                .wildcard
                .get(&KnowledgeLifecycle::Observed)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(registry.total_len(), 3);

        // Drain clears the accumulator.
        let pending: mlua::Table = lua.globals().get(PENDING_KNOWLEDGE_SUBSCRIPTIONS).unwrap();
        assert_eq!(pending.len().unwrap(), 0);
    }

    #[test]
    fn drain_skips_malformed_entries() {
        let lua = lua_with_accumulator();
        push_entry(&lua, "malformed_no_at", "function() end");
        push_entry(&lua, "foo@expired", "function() end");
        push_entry(&lua, "legit:kind@recorded", "function() end");

        let mut registry = KnowledgeSubscriptionRegistry::default();
        let stats = drain_pending_subscriptions(&lua, &mut registry).unwrap();
        assert_eq!(stats.total_seen, 3);
        assert_eq!(stats.registered, 1);
        assert_eq!(stats.skipped, 2);
        assert_eq!(registry.exact.get("legit:kind@recorded").unwrap().len(), 1);
    }

    #[test]
    fn drain_missing_accumulator_is_noop() {
        let lua = Lua::new();
        let mut registry = KnowledgeSubscriptionRegistry::default();
        let stats = drain_pending_subscriptions(&lua, &mut registry).unwrap();
        assert_eq!(stats.total_seen, 0);
        assert_eq!(stats.registered, 0);
        assert_eq!(stats.skipped, 0);
    }

    #[test]
    fn drain_preserves_registration_order_for_exact_bucket() {
        let lua = lua_with_accumulator();
        // Three subscribers for the same event id, different bodies so
        // we can distinguish them via invocation side effect.
        push_entry(
            &lua,
            "same:kind@recorded",
            "function() _order = (_order or '') .. 'a' end",
        );
        push_entry(
            &lua,
            "same:kind@recorded",
            "function() _order = (_order or '') .. 'b' end",
        );
        push_entry(
            &lua,
            "same:kind@recorded",
            "function() _order = (_order or '') .. 'c' end",
        );

        let mut registry = KnowledgeSubscriptionRegistry::default();
        drain_pending_subscriptions(&lua, &mut registry).unwrap();
        let bucket = registry.exact.get("same:kind@recorded").unwrap();
        assert_eq!(bucket.len(), 3);

        // Call them in the stored order; the '_order' global should be "abc".
        for key in bucket {
            let f: mlua::Function = lua.registry_value(key).unwrap();
            f.call::<()>(()).unwrap();
        }
        let order: String = lua.globals().get("_order").unwrap();
        assert_eq!(order, "abc");
    }
}
