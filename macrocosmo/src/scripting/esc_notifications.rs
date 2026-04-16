//! #345 ESC-2: Lua → ESC notification bridge drain.
//!
//! The Lua-side `push_notification { ... }` API appends raw table
//! entries to the `_pending_esc_notifications` global. This module
//! hosts the Rust-side drain system that parses those entries each
//! frame and applies them to [`EscNotificationQueue`] with the
//! `NotifiedEventIds` dedup + current-tick timestamp defaulting
//! that the production push path needs.
//!
//! The drain is idempotent: a successful drain clears the Lua
//! accumulator. Malformed entries are logged via `warn!` and skipped —
//! the chain never aborts because a bridge function returned garbage
//! (plan-349 §6 subscriber-error contract, which `default_bridge.lua`
//! extends into).
//!
//! ## Field shape
//!
//! See the doc-comment on `push_notification` in [`super::globals`]
//! for the full Lua surface. The parser here accepts:
//!
//! | field         | Lua type    | Rust mapping                                   | default              |
//! |---------------|-------------|------------------------------------------------|----------------------|
//! | `title`       | string      | prepended to `message` when both present       | `""`                 |
//! | `message`     | string      | `Notification.message`                         | falls back to title  |
//! | `severity`    | string      | `"info" / "warn" / "critical"` → `Severity::*` | `Severity::Info`     |
//! | `source`      | table       | `{ kind: string, id: u64 }` → `NotificationSource` | `NotificationSource::None` |
//! | `event_id`    | string/int  | `EventId` for `NotifiedEventIds` dedup         | `None` (no dedup)    |
//! | `timestamp`   | i64         | `Notification.timestamp`                       | `GameClock.elapsed`  |
//! | `children`    | table array | recursive; each child uses the same shape      | `vec![]`             |
//!
//! `kind` / `id` pairs come from the Lua side using raw `Entity::to_bits`
//! — Lua scripts today expose entity ids via `gamestate` view fields
//! that return numbers, so this is the wire format the ScriptableKnowledge
//! epic (#349) already uses.
//!
//! ## event_id handling
//!
//! `event_id` can be either a string (Lua scripts synthesise stable
//! identifiers like `"hostile:<entity>"` from payload fields) or an
//! integer. Both are hashed into a `u64` and registered with
//! [`NotifiedEventIds`] via `try_notify` at drain time. The first
//! push for an id wins; subsequent pushes are suppressed in the same
//! frame the shared banner queue would have suppressed them. Pushes
//! without `event_id` never dedupe.
//!
//! ## Depth limit
//!
//! Nested `children` are walked up to [`CHILD_DEPTH_LIMIT`] levels deep
//! (default 4). Beyond that, further children are dropped with a
//! `warn!` and the partial subtree is retained. This matches the
//! `KNOWLEDGE_PAYLOAD_DEPTH_LIMIT = 16` style of depth bounding used
//! elsewhere in the scripting layer.

use bevy::prelude::*;
use mlua::prelude::*;

use super::ScriptEngine;
use crate::knowledge::{EventId, NotifiedEventIds};
use crate::time_system::GameClock;
use crate::ui::situation_center::{
    EscNotificationQueue, Notification, NotificationSource, PushOutcome, Severity,
};

/// Maximum `children` nesting depth accepted by the Lua bridge. Trees
/// deeper than this have their overflow children dropped with a
/// `warn!`. 4 is deep enough for realistic groupings (hostile attack →
/// per-ship loss → per-module subhit) while bounding pathological
/// input from buggy Lua bridges.
pub const CHILD_DEPTH_LIMIT: usize = 4;

/// Bevy Startup / init-side helper: ensure `_pending_esc_notifications`
/// exists on the Lua globals table. `setup_globals` already creates
/// the table; this helper is the recovery path used by the drain
/// itself (mirrors `drain_pending_notifications` in `notifications.rs`).
fn clear_pending_table(lua: &Lua) -> mlua::Result<()> {
    let new_table = lua.create_table()?;
    lua.globals().set("_pending_esc_notifications", new_table)?;
    Ok(())
}

/// Drain the Lua-side `_pending_esc_notifications` accumulator into
/// the [`EscNotificationQueue`]. Runs in `Update` after
/// `dispatch_knowledge_observed` so subscribers that fired during
/// this tick's `@observed` dispatch land in the queue the same frame.
///
/// Field parsing is intentionally permissive: missing fields fall
/// back to sensible defaults, unknown `severity` / `source.kind`
/// values map to `Info` / `None` respectively. The only hard failure
/// is a non-table entry at the top level — those are `warn!`-logged
/// and skipped.
pub fn drain_pending_esc_notifications(world: &mut World) {
    // Fast-path: check whether anything is pending without acquiring
    // the exclusive engine scope.
    let has_pending = world
        .get_resource::<ScriptEngine>()
        .and_then(|engine| {
            let lua = engine.lua();
            let globals = lua.globals();
            let table = globals
                .get::<mlua::Table>("_pending_esc_notifications")
                .ok()?;
            let len = table.len().ok()?;
            Some(len > 0)
        })
        .unwrap_or(false);
    if !has_pending {
        return;
    }

    let now = world
        .get_resource::<GameClock>()
        .map(|c| c.elapsed)
        .unwrap_or(0);

    // Parse + clear under the engine scope, then apply pushes without
    // holding the engine borrow — so the push path can also borrow
    // `NotifiedEventIds` + `EscNotificationQueue` from the world
    // without fighting the engine resource.
    let parsed: Vec<ParsedPush> =
        world.resource_scope::<ScriptEngine, Vec<ParsedPush>>(|_world, engine| {
            let lua = engine.lua();
            parse_pending_entries(lua, now)
        });

    if parsed.is_empty() {
        return;
    }

    // Apply parsed pushes to the queue. `resource_scope` over
    // `NotifiedEventIds` lets us also hold `ResMut<EscNotificationQueue>`
    // without the borrow checker complaining about two resource borrows.
    //
    // `NotifiedEventIds::try_notify` returns `false` unless the id is
    // already `register`-ed in the `Some(false)` state. ESC-originated
    // ids never go through `FactSysParam::allocate_event_id`, so we
    // must register them here before the first push for the dedup
    // handshake to admit the push. The second push for the same id
    // then finds the entry in `Some(true)` state and is suppressed.
    world.resource_scope::<NotifiedEventIds, _>(|world, mut notified| {
        let mut queue = world.resource_mut::<EscNotificationQueue>();
        for push in parsed {
            if let Some(eid) = push.event_id {
                notified.register(eid);
            }
            match queue.push(push.notification, push.event_id, Some(&mut *notified)) {
                PushOutcome::Pushed(_) | PushOutcome::DedupedByEventId => {}
            }
        }
    });
}

/// Parsed representation of a single Lua-side `push_notification`
/// call. Kept as a private value-type between `parse_pending_entries`
/// and the queue apply loop so the apply loop never touches Lua state.
struct ParsedPush {
    notification: Notification,
    event_id: Option<EventId>,
}

/// Parse the Lua `_pending_esc_notifications` accumulator into a
/// `Vec<ParsedPush>` and clear the accumulator. Each malformed entry
/// is logged and skipped; the result is always populated as best we
/// can so partial failure does not swallow the whole batch.
fn parse_pending_entries(lua: &Lua, now: i64) -> Vec<ParsedPush> {
    let globals = lua.globals();
    let table: mlua::Table = match globals.get("_pending_esc_notifications") {
        Ok(t) => t,
        Err(e) => {
            warn!("drain_pending_esc_notifications: missing accumulator: {e}");
            return Vec::new();
        }
    };

    let len = match table.len() {
        Ok(n) => n,
        Err(e) => {
            warn!("drain_pending_esc_notifications: accumulator len error: {e}");
            if let Err(ce) = clear_pending_table(lua) {
                warn!("drain_pending_esc_notifications: recovery clear failed: {ce}");
            }
            return Vec::new();
        }
    };

    let mut out = Vec::with_capacity(len as usize);
    for i in 1..=len {
        let entry = match table.get::<mlua::Value>(i) {
            Ok(v) => v,
            Err(e) => {
                warn!("push_notification[{i}]: read error: {e}");
                continue;
            }
        };
        let entry_table = match entry {
            mlua::Value::Table(t) => t,
            other => {
                warn!(
                    "push_notification[{i}]: expected table, got {}",
                    value_type_name(&other)
                );
                continue;
            }
        };
        match parse_entry(&entry_table, now, 0) {
            Ok(push) => out.push(push),
            Err(e) => warn!("push_notification[{i}]: {e}"),
        }
    }

    // Clear the accumulator so the next frame starts fresh. We
    // replace rather than iterate-+-nil because `pairs` order on a
    // sparse Lua table is not specified.
    if let Err(e) = clear_pending_table(lua) {
        warn!("drain_pending_esc_notifications: clear failed: {e}");
    }
    out
}

/// Parse a single `push_notification` table into a [`ParsedPush`].
/// Recursion is depth-bounded by [`CHILD_DEPTH_LIMIT`]; children
/// beyond the limit are dropped with a `warn!`.
fn parse_entry(table: &mlua::Table, now: i64, depth: usize) -> mlua::Result<ParsedPush> {
    let severity = parse_severity(table.get::<Option<String>>("severity").ok().flatten());

    let title: Option<String> = table.get("title").ok();
    let message_field: Option<String> = table.get("message").ok();
    // Prefer `message` for the body; fall back to `title` when
    // `message` is absent so simple `push_notification { title = ... }`
    // calls still render something meaningful.
    let message = match (title.as_deref(), message_field.as_deref()) {
        (Some(t), Some(m)) if !t.is_empty() && !m.is_empty() => format!("{t}: {m}"),
        (_, Some(m)) if !m.is_empty() => m.to_string(),
        (Some(t), _) => t.to_string(),
        _ => String::new(),
    };

    let timestamp: i64 = table.get::<i64>("timestamp").unwrap_or(now);

    let source = match table.get::<mlua::Value>("source") {
        Ok(mlua::Value::Table(st)) => parse_source(&st),
        Ok(_) | Err(_) => NotificationSource::None,
    };

    let event_id = parse_event_id(table);

    let children: Vec<Notification> = if depth + 1 >= CHILD_DEPTH_LIMIT {
        // Silently drop the children sub-list at the depth limit.
        let _ = table.get::<mlua::Value>("children");
        Vec::new()
    } else {
        parse_children(table, now, depth + 1)?
    };

    let notification = Notification {
        id: 0, // Overwritten by `EscNotificationQueue::push`.
        source,
        timestamp,
        severity,
        message,
        acked: false,
        children,
    };

    Ok(ParsedPush {
        notification,
        event_id,
    })
}

fn parse_children(table: &mlua::Table, now: i64, depth: usize) -> mlua::Result<Vec<Notification>> {
    let children_val: mlua::Value = match table.get("children") {
        Ok(v) => v,
        Err(_) => return Ok(Vec::new()),
    };
    let children_table = match children_val {
        mlua::Value::Table(t) => t,
        mlua::Value::Nil => return Ok(Vec::new()),
        other => {
            warn!(
                "push_notification.children: expected table, got {}",
                value_type_name(&other)
            );
            return Ok(Vec::new());
        }
    };
    let len = children_table.len().unwrap_or(0);
    let mut out = Vec::with_capacity(len.max(0) as usize);
    for i in 1..=len {
        let child_val = match children_table.get::<mlua::Value>(i) {
            Ok(v) => v,
            Err(e) => {
                warn!("push_notification.children[{i}]: read error: {e}");
                continue;
            }
        };
        let child_table = match child_val {
            mlua::Value::Table(t) => t,
            other => {
                warn!(
                    "push_notification.children[{i}]: expected table, got {}",
                    value_type_name(&other)
                );
                continue;
            }
        };
        match parse_entry(&child_table, now, depth) {
            Ok(parsed) => out.push(parsed.notification),
            Err(e) => warn!("push_notification.children[{i}]: {e}"),
        }
    }
    Ok(out)
}

fn parse_severity(raw: Option<String>) -> Severity {
    match raw.as_deref() {
        Some("warn") | Some("warning") => Severity::Warn,
        Some("critical") | Some("crit") | Some("error") => Severity::Critical,
        // "info" / None / anything unknown → Info.
        _ => Severity::Info,
    }
}

fn parse_source(table: &mlua::Table) -> NotificationSource {
    let kind: String = table
        .get::<String>("kind")
        .unwrap_or_else(|_| "none".to_string());
    let id_bits: Option<u64> = table.get::<u64>("id").ok();
    match (kind.as_str(), id_bits) {
        ("empire", Some(bits)) => NotificationSource::Empire(Entity::from_bits(bits)),
        ("system", Some(bits)) => NotificationSource::System(Entity::from_bits(bits)),
        ("colony", Some(bits)) => NotificationSource::Colony(Entity::from_bits(bits)),
        ("ship", Some(bits)) => NotificationSource::Ship(Entity::from_bits(bits)),
        ("fleet", Some(bits)) => NotificationSource::Fleet(Entity::from_bits(bits)),
        ("faction", Some(bits)) => NotificationSource::Faction(Entity::from_bits(bits)),
        ("build_order", Some(bits)) => NotificationSource::BuildOrder(bits),
        // Missing id or unknown kind falls back to None.
        _ => NotificationSource::None,
    }
}

/// Parse `event_id` — accept either a string or an integer.
/// String ids are hashed to `u64` via `DefaultHasher` so the
/// `NotifiedEventIds` tri-state map (keyed by `EventId(u64)`) can
/// dedupe them without carrying a separate string-id registry.
fn parse_event_id(table: &mlua::Table) -> Option<EventId> {
    let v: mlua::Value = table.get("event_id").ok()?;
    match v {
        mlua::Value::Nil => None,
        mlua::Value::String(s) => {
            let raw = s.to_str().ok()?.to_string();
            if raw.is_empty() {
                return None;
            }
            Some(EventId(hash_string_to_u64(&raw)))
        }
        mlua::Value::Integer(i) => Some(EventId(i as u64)),
        mlua::Value::Number(n) => {
            // Lua numbers without an integer cast — best-effort convert.
            if n.is_finite() && n >= 0.0 {
                Some(EventId(n as u64))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn hash_string_to_u64(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    // Namespace the hash so string ids never collide with raw integer
    // ids — e.g. the integer `42` and the string `"42"` produce
    // different `EventId` values.
    "esc:push_notification:".hash(&mut hasher);
    s.hash(&mut hasher);
    hasher.finish()
}

fn value_type_name(v: &mlua::Value) -> &'static str {
    match v {
        mlua::Value::Nil => "nil",
        mlua::Value::Boolean(_) => "boolean",
        mlua::Value::Integer(_) => "integer",
        mlua::Value::Number(_) => "number",
        mlua::Value::String(_) => "string",
        mlua::Value::Table(_) => "table",
        mlua::Value::Function(_) => "function",
        mlua::Value::UserData(_) => "userdata",
        mlua::Value::Thread(_) => "thread",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::situation_center::Notification;

    fn new_engine() -> ScriptEngine {
        // Reuse the same test-engine bootstrap pattern as the other
        // scripting tests — instantiate with a deterministic RNG and
        // the crate-relative scripts dir (we don't load scripts here,
        // only set up globals).
        let rng = crate::scripting::GameRng::default().handle();
        ScriptEngine::new_with_rng_and_dir(rng, scripts_dir_for_tests()).expect("engine boot")
    }

    fn scripts_dir_for_tests() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts")
    }

    /// Helper — build a world with the minimal resources the drain
    /// needs. Inserts a fresh `EscNotificationQueue` + `GameClock` +
    /// `NotifiedEventIds` + a real `ScriptEngine`. Returns the world
    /// plus a reference to the engine lua handle for pushing test
    /// entries directly.
    fn make_test_world() -> World {
        let mut world = World::new();
        world.insert_resource(EscNotificationQueue::default());
        world.insert_resource(NotifiedEventIds::default());
        world.insert_resource(GameClock::new(0));
        let engine = new_engine();
        // Mirror the globals setup normally done by `init_scripting`
        // so `_pending_esc_notifications` exists on the Lua side.
        let lua = engine.lua();
        super::super::globals::setup_globals(lua, &scripts_dir_for_tests()).expect("setup_globals");
        world.insert_resource(engine);
        world
    }

    fn run_drain(world: &mut World) {
        let mut sys = bevy::ecs::system::IntoSystem::into_system(drain_pending_esc_notifications);
        sys.initialize(world);
        sys.run((), world);
    }

    fn call_push_notification(world: &World, body: &str) {
        let engine = world.resource::<ScriptEngine>();
        let lua = engine.lua();
        lua.load(body).exec().expect("lua exec");
    }

    #[test]
    fn push_notification_drains_to_queue_with_defaults() {
        let mut world = make_test_world();
        call_push_notification(
            &world,
            r#"push_notification { message = "hello world", severity = "warn" }"#,
        );
        run_drain(&mut world);

        let q = world.resource::<EscNotificationQueue>();
        assert_eq!(q.items.len(), 1);
        assert_eq!(q.items[0].message, "hello world");
        assert_eq!(q.items[0].severity, Severity::Warn);
        assert!(matches!(q.items[0].source, NotificationSource::None));
    }

    #[test]
    fn push_notification_dedupes_by_event_id_string() {
        let mut world = make_test_world();
        call_push_notification(
            &world,
            r#"
            push_notification {
                event_id = "hostile:123",
                severity = "critical",
                message = "first"
            }
            push_notification {
                event_id = "hostile:123",
                severity = "critical",
                message = "second"
            }
        "#,
        );
        run_drain(&mut world);

        let q = world.resource::<EscNotificationQueue>();
        assert_eq!(q.items.len(), 1, "dedup by event_id");
        assert_eq!(q.items[0].message, "first");
    }

    #[test]
    fn push_notification_dedupes_by_event_id_integer() {
        let mut world = make_test_world();
        call_push_notification(
            &world,
            r#"
            push_notification { event_id = 42, message = "a" }
            push_notification { event_id = 42, message = "b" }
        "#,
        );
        run_drain(&mut world);
        let q = world.resource::<EscNotificationQueue>();
        assert_eq!(q.items.len(), 1);
    }

    #[test]
    fn push_notification_no_event_id_never_dedupes() {
        let mut world = make_test_world();
        call_push_notification(
            &world,
            r#"
            push_notification { message = "x" }
            push_notification { message = "x" }
            push_notification { message = "x" }
        "#,
        );
        run_drain(&mut world);
        let q = world.resource::<EscNotificationQueue>();
        assert_eq!(q.items.len(), 3);
    }

    #[test]
    fn push_notification_accepts_source_table() {
        let mut world = make_test_world();
        let entity = world.spawn_empty().id();
        let bits = entity.to_bits();
        call_push_notification(
            &world,
            &format!(
                r#"push_notification {{
                    message = "system-scoped",
                    severity = "info",
                    source = {{ kind = "system", id = {bits} }}
                }}"#
            ),
        );
        run_drain(&mut world);

        let q = world.resource::<EscNotificationQueue>();
        assert_eq!(q.items.len(), 1);
        match q.items[0].source {
            NotificationSource::System(e) => assert_eq!(e.to_bits(), bits),
            other => panic!("expected System source, got {other:?}"),
        }
    }

    #[test]
    fn push_notification_unknown_severity_defaults_to_info() {
        let mut world = make_test_world();
        call_push_notification(
            &world,
            r#"push_notification { message = "m", severity = "garbage" }"#,
        );
        run_drain(&mut world);
        let q = world.resource::<EscNotificationQueue>();
        assert_eq!(q.items.len(), 1);
        assert_eq!(q.items[0].severity, Severity::Info);
    }

    #[test]
    fn push_notification_timestamp_defaults_to_game_clock() {
        let mut world = make_test_world();
        world.resource_mut::<GameClock>().elapsed = 4321;
        call_push_notification(&world, r#"push_notification { message = "m" }"#);
        run_drain(&mut world);
        let q = world.resource::<EscNotificationQueue>();
        assert_eq!(q.items[0].timestamp, 4321);
    }

    #[test]
    fn push_notification_explicit_timestamp_overrides_clock() {
        let mut world = make_test_world();
        world.resource_mut::<GameClock>().elapsed = 1;
        call_push_notification(
            &world,
            r#"push_notification { message = "m", timestamp = 9999 }"#,
        );
        run_drain(&mut world);
        let q = world.resource::<EscNotificationQueue>();
        assert_eq!(q.items[0].timestamp, 9999);
    }

    #[test]
    fn push_notification_parses_children() {
        let mut world = make_test_world();
        call_push_notification(
            &world,
            r#"push_notification {
                message = "parent",
                severity = "warn",
                children = {
                    { message = "child1", severity = "info" },
                    { message = "child2", severity = "critical" },
                }
            }"#,
        );
        run_drain(&mut world);

        let q = world.resource::<EscNotificationQueue>();
        assert_eq!(q.items.len(), 1);
        assert_eq!(q.items[0].message, "parent");
        assert_eq!(q.items[0].children.len(), 2);
        assert_eq!(q.items[0].children[0].message, "child1");
        assert_eq!(q.items[0].children[1].severity, Severity::Critical);
    }

    #[test]
    fn push_notification_drain_clears_accumulator() {
        let mut world = make_test_world();
        call_push_notification(&world, r#"push_notification { message = "a" }"#);
        run_drain(&mut world);
        // Second drain with no new pushes must be a no-op.
        run_drain(&mut world);
        let q = world.resource::<EscNotificationQueue>();
        assert_eq!(q.items.len(), 1, "no double-push after drain");
    }

    #[test]
    fn push_notification_string_id_and_int_id_dont_collide() {
        let mut world = make_test_world();
        call_push_notification(
            &world,
            r#"
            push_notification { event_id = 42, message = "int" }
            push_notification { event_id = "42", message = "str" }
        "#,
        );
        run_drain(&mut world);
        let q = world.resource::<EscNotificationQueue>();
        // The hash namespacing prevents the integer `42` and string
        // `"42"` from colliding — both pushes should land.
        assert_eq!(q.items.len(), 2);
    }

    #[test]
    fn push_notification_malformed_entry_is_skipped() {
        let mut world = make_test_world();
        // Push a garbage non-table directly to the accumulator and a
        // valid entry after it; the valid entry must still land.
        {
            let engine = world.resource::<ScriptEngine>();
            let lua = engine.lua();
            lua.load(
                r#"
                _pending_esc_notifications[#_pending_esc_notifications + 1] = 123
                push_notification { message = "valid" }
                "#,
            )
            .exec()
            .expect("lua exec");
        }
        run_drain(&mut world);
        let q = world.resource::<EscNotificationQueue>();
        assert_eq!(q.items.len(), 1);
        assert_eq!(q.items[0].message, "valid");
    }

    #[test]
    fn push_notification_title_and_message_combine() {
        let mut world = make_test_world();
        call_push_notification(
            &world,
            r#"push_notification { title = "Hostile", message = "detected" }"#,
        );
        run_drain(&mut world);
        let q = world.resource::<EscNotificationQueue>();
        assert_eq!(q.items[0].message, "Hostile: detected");
    }

    #[test]
    fn push_notification_depth_limit_drops_overflow() {
        let mut world = make_test_world();
        // Build a depth-6 chain; the limit is 4, so depths 0..=3 survive
        // and anything beyond is dropped.
        call_push_notification(
            &world,
            r#"push_notification {
                message = "d0",
                children = {{
                    message = "d1",
                    children = {{
                        message = "d2",
                        children = {{
                            message = "d3",
                            children = {{
                                message = "d4-dropped",
                                children = {{ message = "d5-dropped" }}
                            }}
                        }}
                    }}
                }}
            }"#,
        );
        run_drain(&mut world);
        let q = world.resource::<EscNotificationQueue>();
        assert_eq!(q.items.len(), 1);
        let mut depth = 0;
        let mut node: &Notification = &q.items[0];
        while let Some(first) = node.children.first() {
            depth += 1;
            node = first;
        }
        // Depths 0,1,2,3 reachable → `depth` traversed = 3 edges.
        assert_eq!(
            depth, 3,
            "depth limit caps the tree at {CHILD_DEPTH_LIMIT} levels"
        );
    }
}
