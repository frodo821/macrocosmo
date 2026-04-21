//! #350: Lua `define_knowledge` parser for the ScriptableKnowledge epic
//! (#349) K-1 slice.
//!
//! Responsibilities:
//!
//! 1. Walk the `_knowledge_kind_definitions` accumulator (populated by the
//!    `define_knowledge { id, payload_schema }` global added in
//!    `scripting/globals.rs`).
//! 2. Parse each entry into a [`KnowledgeKindDef`] via the
//!    [`kind_registry`](crate::knowledge::kind_registry) types.
//! 3. Enforce the K-1 invariants (plan-349 §0.5 9.2 / 9.6, §2.3, §3.1):
//!    * `id` is required and non-empty.
//!    * `id` must not contain `@` (lifecycle separator is reserved).
//!    * Lua-origin kinds cannot use the `core:` namespace.
//!    * Namespace-less ids are accepted with a `warn!`.
//!    * `payload_schema`, if present, must be a flat table mapping field
//!      names to recognised type tags (`"number" | "string" | "boolean" |
//!      "table" | "entity"`). Function / userdata / nested table values
//!      are rejected (plan §2.5 deep-copy invariant + §3.1 payload_schema
//!      validation). Duplicate field names within a single schema are
//!      rejected.
//!    * Duplicate kind ids across the whole accumulator are rejected.
//!
//! The parser is total (returns `Result<Vec<KnowledgeKindDef>, mlua::Error>`);
//! the K-1 commit 4 startup system drains the vec into `KindRegistry`.
//!
//! Intentionally **not** implemented in this slice:
//! * Actual subscription registration (`_knowledge_subscribers` wiring) —
//!   K-1 commit 3 adds a placeholder registry hook, dispatch lives in K-3.
//! * Payload-value validation at `record_knowledge` time — K-2 consumes
//!   [`KindRegistry::get`] + the schema to validate Lua-side payloads.

use mlua::prelude::*;

use crate::knowledge::kind_registry::{
    KindOrigin, KnowledgeKindDef, KnowledgeKindId, PayloadFieldType, PayloadSchema,
    parse_id_with_warn,
};

/// Name of the Lua global that accumulates `define_knowledge { ... }` tables.
/// Matches the `register_define_fn(lua, "knowledge", "_knowledge_kind_definitions")`
/// registration in `globals.rs`.
pub const KNOWLEDGE_DEF_ACCUMULATOR: &str = "_knowledge_kind_definitions";

/// Parse all `define_knowledge` entries from the Lua state into
/// [`KnowledgeKindDef`]s. Returns an error on the **first** invalid entry
/// (duplicate id, schema type mismatch, etc.) — mirrors the other `parse_*`
/// surfaces in `scripting/`.
///
/// The returned vec preserves Lua iteration order (which is 1-indexed array
/// traversal of the accumulator).
pub fn parse_knowledge_definitions(lua: &Lua) -> Result<Vec<KnowledgeKindDef>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get(KNOWLEDGE_DEF_ACCUMULATOR)?;

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;
        let def = parse_one(&table)?;
        if !seen.insert(def.id.as_str().to_string()) {
            return Err(mlua::Error::RuntimeError(format!(
                "define_knowledge: duplicate kind id '{}'",
                def.id
            )));
        }
        result.push(def);
    }

    Ok(result)
}

/// Parse a single Lua definition table. Exposed for unit-test convenience.
pub fn parse_one(table: &mlua::Table) -> Result<KnowledgeKindDef, mlua::Error> {
    let raw_id: String = table.get::<Option<String>>("id")?.ok_or_else(|| {
        mlua::Error::RuntimeError("define_knowledge: 'id' field is required".to_string())
    })?;

    // id parsing: empty / `@`-containing ids surface as RuntimeError.
    let id = parse_id_with_warn(&raw_id)
        .map_err(|e| mlua::Error::RuntimeError(format!("define_knowledge: {e}")))?;

    // Core-namespace Lua redefinitions are rejected at parse time so the
    // error surfaces close to the offending script. The registry insert
    // would also catch this, but catching early is friendlier.
    if id.is_core() {
        return Err(mlua::Error::RuntimeError(format!(
            "define_knowledge: 'core:' namespace is reserved for Rust-side built-in kinds (got '{raw_id}')"
        )));
    }

    let payload_schema = parse_payload_schema(table, id.as_str())?;

    Ok(KnowledgeKindDef {
        id,
        payload_schema,
        origin: KindOrigin::Lua,
    })
}

/// Parse the `payload_schema` sub-table. Missing / nil / empty table yields
/// `PayloadSchema::default()` (v1 explicitly accepts schema-less kinds —
/// plan §3.1 payload_schema validation: "v1 緩い validation").
pub fn parse_payload_schema(
    table: &mlua::Table,
    kind_id: &str,
) -> Result<PayloadSchema, mlua::Error> {
    let schema_value: mlua::Value = table.get("payload_schema")?;
    let schema_table = match schema_value {
        mlua::Value::Nil => return Ok(PayloadSchema::default()),
        mlua::Value::Table(t) => t,
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "define_knowledge: kind '{kind_id}': payload_schema must be a table, got {}",
                lua_type_name(&other)
            )));
        }
    };

    let mut schema = PayloadSchema::default();

    for pair in schema_table.pairs::<mlua::Value, mlua::Value>() {
        let (key, value) = pair?;

        // Only string keys are valid field names. Numeric keys (array style)
        // are rejected — schema is a name → type map.
        let field_name = match key {
            mlua::Value::String(s) => s.to_str()?.to_string(),
            other => {
                return Err(mlua::Error::RuntimeError(format!(
                    "define_knowledge: kind '{kind_id}': payload_schema keys must be strings, got {}",
                    lua_type_name(&other)
                )));
            }
        };

        // Lua tables do not preserve duplicate keys on insertion, but defensive
        // check here in case a buggy `pairs()` implementation returns the same
        // key twice — also guards against future shape changes.
        if schema.fields.contains_key(&field_name) {
            return Err(mlua::Error::RuntimeError(format!(
                "define_knowledge: kind '{kind_id}': duplicate field '{field_name}' in payload_schema"
            )));
        }

        let field_type = parse_field_type(&field_name, &value, kind_id)?;
        schema.fields.insert(field_name, field_type);
    }

    Ok(schema)
}

fn parse_field_type(
    field_name: &str,
    value: &mlua::Value,
    kind_id: &str,
) -> Result<PayloadFieldType, mlua::Error> {
    match value {
        mlua::Value::String(s) => {
            let tag = s.to_str()?;
            PayloadFieldType::parse(&tag).ok_or_else(|| {
                mlua::Error::RuntimeError(format!(
                    "define_knowledge: kind '{kind_id}': unknown payload type '{tag}' for field '{field_name}' (expected one of: number, string, boolean, table, entity)"
                ))
            })
        }
        mlua::Value::Table(_) => Err(mlua::Error::RuntimeError(format!(
            "define_knowledge: kind '{kind_id}': nested schemas are not supported in v1 (field '{field_name}')"
        ))),
        mlua::Value::Function(_) | mlua::Value::UserData(_) => {
            Err(mlua::Error::RuntimeError(format!(
                "define_knowledge: kind '{kind_id}': payload_schema field '{field_name}' must be a type name string (got {})",
                lua_type_name(value)
            )))
        }
        other => Err(mlua::Error::RuntimeError(format!(
            "define_knowledge: kind '{kind_id}': payload_schema field '{field_name}' must be a string type tag (got {})",
            lua_type_name(other)
        ))),
    }
}

fn lua_type_name(value: &mlua::Value) -> &'static str {
    match value {
        mlua::Value::Nil => "nil",
        mlua::Value::Boolean(_) => "boolean",
        mlua::Value::Integer(_) => "integer",
        mlua::Value::Number(_) => "number",
        mlua::Value::String(_) => "string",
        mlua::Value::Table(_) => "table",
        mlua::Value::Function(_) => "function",
        mlua::Value::UserData(_) => "userdata",
        mlua::Value::Thread(_) => "thread",
        mlua::Value::LightUserData(_) => "lightuserdata",
        mlua::Value::Error(_) => "error",
        _ => "other",
    }
}

/// Expose a noop convenience around [`KnowledgeKindId::parse`] so callers
/// (tests, future K-2 consumers) can build ids without depending on the
/// registry module directly.
pub fn parse_kind_id(raw: &str) -> Result<KnowledgeKindId, mlua::Error> {
    KnowledgeKindId::parse(raw).map_err(|e| mlua::Error::RuntimeError(format!("{e}")))
}

/// Name of the Lua table that records every `<id>@recorded` /
/// `<id>@observed` event id auto-reserved by `define_knowledge`. The
/// entry value is `true` — the table is used as a set.
///
/// K-3 (#352) will consume this lookup in its `on(...)` router to validate
/// subscribers and in the dispatcher to avoid walking unknown kind ids.
/// K-1 only **populates** the table as a side-effect of kind registration;
/// actual handler entries go into `_knowledge_subscribers`.
pub const KNOWLEDGE_RESERVED_EVENTS_TABLE: &str = "_knowledge_reserved_events";

/// Name of the Lua table that holds subscription entries (populated by
/// K-3's extended `on(...)`). K-1 reserves the table so downstream code
/// sees a stable shape.
pub const KNOWLEDGE_SUBSCRIBERS_TABLE: &str = "_knowledge_subscribers";

/// Walk `defs` and reserve `<id>@recorded` / `<id>@observed` event ids for
/// every kind by writing `true` entries into the
/// `_knowledge_reserved_events` Lua table (plan-349 §3.1 commit 3, §2.2.1).
///
/// K-1 responsibility is **reservation only** — the dispatch side (walking
/// `_knowledge_subscribers` and firing handlers) lives in K-3 (#352). K-3
/// reads `_knowledge_reserved_events` when `on("foo@recorded", fn)` is
/// registered to confirm the kind exists, and the dispatch code uses it to
/// know which ids are knowledge-lifecycle vs plain event ids.
///
/// This function is idempotent per (id, lifecycle) — re-reserving is a
/// no-op, so callers can drain the accumulator multiple times safely
/// during tests / hot reload.
///
/// Errors: propagates `mlua::Error` from table operations. Does **not**
/// error on duplicate reservations (the registry's duplicate-id check
/// catches those at `insert` time).
pub fn register_auto_lifecycle_events(
    lua: &Lua,
    defs: &[KnowledgeKindDef],
) -> Result<(), mlua::Error> {
    let reserved: mlua::Table = lua.globals().get(KNOWLEDGE_RESERVED_EVENTS_TABLE)?;
    for def in defs {
        reserved.set(def.id.recorded_event_id(), true)?;
        reserved.set(def.id.observed_event_id(), true)?;
    }
    Ok(())
}

/// True if `event_id` is one of the auto-registered `<id>@<lifecycle>`
/// reservations. K-3 uses this to route `on(...)` registrations between
/// `_knowledge_subscribers` and `_event_handlers` (plan §2.9). Includes
/// the `*@recorded` / `*@observed` wildcards — those are valid
/// knowledge-lifecycle subscriptions even though no kind id "`*`" exists
/// in the registry.
pub fn is_reserved_knowledge_event(lua: &Lua, event_id: &str) -> Result<bool, mlua::Error> {
    // Wildcard always counts as knowledge-lifecycle.
    if matches!(event_id, "*@recorded" | "*@observed") {
        return Ok(true);
    }
    let reserved: mlua::Table = lua.globals().get(KNOWLEDGE_RESERVED_EVENTS_TABLE)?;
    let present: Option<bool> = reserved.get(event_id)?;
    Ok(present.unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal harness: fresh Lua with the `_knowledge_kind_definitions`
    /// accumulator wired up. We don't bring in the full `setup_globals`
    /// machinery because parse_* only needs the accumulator table to exist.
    fn setup_lua() -> Lua {
        let lua = Lua::new();
        let globals = lua.globals();
        globals
            .set(KNOWLEDGE_DEF_ACCUMULATOR, lua.create_table().unwrap())
            .unwrap();
        // Reserve the K-3 placeholders so `register_auto_lifecycle_events`
        // can write into them. Production uses `setup_globals`, but parser
        // tests don't need the rest of that plumbing.
        globals
            .set(KNOWLEDGE_SUBSCRIBERS_TABLE, lua.create_table().unwrap())
            .unwrap();
        globals
            .set(KNOWLEDGE_RESERVED_EVENTS_TABLE, lua.create_table().unwrap())
            .unwrap();
        // Mimic the `define_knowledge` surface just enough for the parser tests.
        lua.load(
            r#"
            function define_knowledge(t)
                local defs = _knowledge_kind_definitions
                t._def_type = "knowledge"
                defs[#defs + 1] = t
                return t
            end
            "#,
        )
        .exec()
        .unwrap();
        lua
    }

    #[test]
    fn parse_minimum_id_only() {
        let lua = setup_lua();
        lua.load(r#"define_knowledge { id = "vesk:famine_outbreak" }"#)
            .exec()
            .unwrap();

        let defs = parse_knowledge_definitions(&lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id.as_str(), "vesk:famine_outbreak");
        assert!(defs[0].payload_schema.is_empty());
        assert_eq!(defs[0].origin, KindOrigin::Lua);
    }

    #[test]
    fn parse_with_payload_schema_all_types() {
        let lua = setup_lua();
        lua.load(
            r#"
            define_knowledge {
                id = "vesk:famine_outbreak",
                payload_schema = {
                    severity = "number",
                    label = "string",
                    active = "boolean",
                    extras = "table",
                    colony = "entity",
                },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_knowledge_definitions(&lua).unwrap();
        assert_eq!(defs.len(), 1);
        let schema = &defs[0].payload_schema;
        assert_eq!(schema.fields.len(), 5);
        assert_eq!(
            schema.fields.get("severity"),
            Some(&PayloadFieldType::Number)
        );
        assert_eq!(schema.fields.get("label"), Some(&PayloadFieldType::String));
        assert_eq!(
            schema.fields.get("active"),
            Some(&PayloadFieldType::Boolean)
        );
        assert_eq!(schema.fields.get("extras"), Some(&PayloadFieldType::Table));
        assert_eq!(schema.fields.get("colony"), Some(&PayloadFieldType::Entity));
    }

    #[test]
    fn parse_bool_alias_accepted() {
        let lua = setup_lua();
        lua.load(
            r#"
            define_knowledge {
                id = "mod:bool_alias",
                payload_schema = { flag = "bool" },
            }
            "#,
        )
        .exec()
        .unwrap();
        let defs = parse_knowledge_definitions(&lua).unwrap();
        assert_eq!(
            defs[0].payload_schema.fields.get("flag"),
            Some(&PayloadFieldType::Boolean)
        );
    }

    #[test]
    fn parse_missing_id_errors() {
        let lua = setup_lua();
        lua.load(r#"define_knowledge { payload_schema = { x = "number" } }"#)
            .exec()
            .unwrap();
        let err = parse_knowledge_definitions(&lua).unwrap_err();
        let s = err.to_string();
        assert!(s.contains("'id' field is required"), "unexpected: {s}");
    }

    #[test]
    fn parse_empty_id_errors() {
        let lua = setup_lua();
        lua.load(r#"define_knowledge { id = "" }"#).exec().unwrap();
        let err = parse_knowledge_definitions(&lua).unwrap_err();
        let s = err.to_string();
        assert!(s.contains("non-empty"), "unexpected: {s}");
    }

    #[test]
    fn parse_rejects_id_with_at_symbol() {
        let lua = setup_lua();
        lua.load(r#"define_knowledge { id = "vesk:bad@recorded" }"#)
            .exec()
            .unwrap();
        let err = parse_knowledge_definitions(&lua).unwrap_err();
        let s = err.to_string();
        assert!(
            s.contains("reserved for lifecycle events"),
            "unexpected: {s}"
        );
    }

    #[test]
    fn parse_rejects_core_namespace() {
        let lua = setup_lua();
        lua.load(r#"define_knowledge { id = "core:hostile_detected" }"#)
            .exec()
            .unwrap();
        let err = parse_knowledge_definitions(&lua).unwrap_err();
        let s = err.to_string();
        assert!(s.contains("core:"), "unexpected: {s}");
        assert!(s.contains("reserved"), "unexpected: {s}");
    }

    #[test]
    fn parse_rejects_duplicate_kind_ids() {
        let lua = setup_lua();
        lua.load(
            r#"
            define_knowledge { id = "vesk:famine" }
            define_knowledge { id = "vesk:famine" }
            "#,
        )
        .exec()
        .unwrap();
        let err = parse_knowledge_definitions(&lua).unwrap_err();
        let s = err.to_string();
        assert!(
            s.contains("duplicate kind id 'vesk:famine'"),
            "unexpected: {s}"
        );
    }

    #[test]
    fn parse_rejects_unknown_type_tag() {
        let lua = setup_lua();
        lua.load(
            r#"
            define_knowledge {
                id = "vesk:wrong",
                payload_schema = { severity = "cucumber" },
            }
            "#,
        )
        .exec()
        .unwrap();
        let err = parse_knowledge_definitions(&lua).unwrap_err();
        let s = err.to_string();
        assert!(
            s.contains("unknown payload type 'cucumber'"),
            "unexpected: {s}"
        );
    }

    #[test]
    fn parse_rejects_nested_schema() {
        let lua = setup_lua();
        lua.load(
            r#"
            define_knowledge {
                id = "vesk:nested",
                payload_schema = {
                    geo = { lat = "number", lon = "number" },
                },
            }
            "#,
        )
        .exec()
        .unwrap();
        let err = parse_knowledge_definitions(&lua).unwrap_err();
        let s = err.to_string();
        assert!(
            s.contains("nested schemas are not supported"),
            "unexpected: {s}"
        );
    }

    #[test]
    fn parse_rejects_function_in_schema() {
        let lua = setup_lua();
        lua.load(
            r#"
            define_knowledge {
                id = "vesk:func",
                payload_schema = { severity = function(x) return x end },
            }
            "#,
        )
        .exec()
        .unwrap();
        let err = parse_knowledge_definitions(&lua).unwrap_err();
        let s = err.to_string();
        assert!(
            s.contains("must be a type name string") || s.contains("must be a string type tag"),
            "unexpected: {s}"
        );
    }

    #[test]
    fn parse_rejects_non_table_schema() {
        let lua = setup_lua();
        lua.load(r#"define_knowledge { id = "vesk:bad_schema", payload_schema = 42 }"#)
            .exec()
            .unwrap();
        let err = parse_knowledge_definitions(&lua).unwrap_err();
        let s = err.to_string();
        assert!(
            s.contains("payload_schema must be a table"),
            "unexpected: {s}"
        );
    }

    #[test]
    fn parse_rejects_numeric_schema_keys() {
        let lua = setup_lua();
        lua.load(
            r#"
            -- array-style schema: key 1 is the integer 1, not a field name
            define_knowledge {
                id = "vesk:array_schema",
                payload_schema = { "number", "string" },
            }
            "#,
        )
        .exec()
        .unwrap();
        let err = parse_knowledge_definitions(&lua).unwrap_err();
        let s = err.to_string();
        assert!(s.contains("keys must be strings"), "unexpected: {s}");
    }

    #[test]
    fn parse_accepts_multiple_kinds_preserving_order() {
        let lua = setup_lua();
        lua.load(
            r#"
            define_knowledge { id = "mod:first" }
            define_knowledge { id = "mod:second", payload_schema = { x = "number" } }
            define_knowledge { id = "mod:third" }
            "#,
        )
        .exec()
        .unwrap();
        let defs = parse_knowledge_definitions(&lua).unwrap();
        assert_eq!(defs.len(), 3);
        assert_eq!(defs[0].id.as_str(), "mod:first");
        assert_eq!(defs[1].id.as_str(), "mod:second");
        assert_eq!(defs[2].id.as_str(), "mod:third");
        assert_eq!(defs[1].payload_schema.fields.len(), 1);
    }

    #[test]
    fn parse_namespaceless_id_is_accepted_with_warn() {
        // The parser itself does not fail (plan §0.5 9.6 warn only); the
        // warn log goes through `bevy::log::warn!` which is a no-op in this
        // test harness. We only assert the parse succeeds and round-trips.
        let lua = setup_lua();
        lua.load(r#"define_knowledge { id = "no_namespace" }"#)
            .exec()
            .unwrap();
        let defs = parse_knowledge_definitions(&lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id.as_str(), "no_namespace");
        assert_eq!(defs[0].id.namespace(), None);
    }

    // --- register_auto_lifecycle_events + is_reserved_knowledge_event ---

    #[test]
    fn register_auto_events_populates_reserved_table() {
        let lua = setup_lua();
        lua.load(
            r#"
            define_knowledge { id = "vesk:famine_outbreak" }
            define_knowledge { id = "mod:combat_report" }
            "#,
        )
        .exec()
        .unwrap();
        let defs = parse_knowledge_definitions(&lua).unwrap();
        register_auto_lifecycle_events(&lua, &defs).unwrap();

        // Both lifecycle events for each kind must be reserved.
        for id in [
            "vesk:famine_outbreak@recorded",
            "vesk:famine_outbreak@observed",
            "mod:combat_report@recorded",
            "mod:combat_report@observed",
        ] {
            assert!(
                is_reserved_knowledge_event(&lua, id).unwrap(),
                "expected {id} reserved"
            );
        }
    }

    #[test]
    fn unregistered_event_ids_are_not_reserved() {
        let lua = setup_lua();
        lua.load(r#"define_knowledge { id = "vesk:famine_outbreak" }"#)
            .exec()
            .unwrap();
        let defs = parse_knowledge_definitions(&lua).unwrap();
        register_auto_lifecycle_events(&lua, &defs).unwrap();

        // Unknown kind id — not reserved.
        assert!(!is_reserved_knowledge_event(&lua, "mod:unknown@recorded").unwrap());
        // Plain non-lifecycle event id — not reserved.
        assert!(!is_reserved_knowledge_event(&lua, "harvest_ended").unwrap());
        // Missing lifecycle suffix for a known kind — not reserved.
        assert!(!is_reserved_knowledge_event(&lua, "vesk:famine_outbreak").unwrap());
    }

    #[test]
    fn wildcard_lifecycle_ids_are_always_reserved() {
        // `*@recorded` / `*@observed` must count as knowledge-lifecycle
        // regardless of whether any kind is registered — plan §2.9.
        let lua = setup_lua();
        assert!(is_reserved_knowledge_event(&lua, "*@recorded").unwrap());
        assert!(is_reserved_knowledge_event(&lua, "*@observed").unwrap());
        // Wildcard with unknown lifecycle is NOT reserved.
        assert!(!is_reserved_knowledge_event(&lua, "*@expired").unwrap());
    }

    #[test]
    fn register_auto_events_is_idempotent() {
        let lua = setup_lua();
        lua.load(r#"define_knowledge { id = "mod:twice" }"#)
            .exec()
            .unwrap();
        let defs = parse_knowledge_definitions(&lua).unwrap();
        register_auto_lifecycle_events(&lua, &defs).unwrap();
        // Call again — must not error.
        register_auto_lifecycle_events(&lua, &defs).unwrap();
        assert!(is_reserved_knowledge_event(&lua, "mod:twice@recorded").unwrap());
    }
}
