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

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal harness: fresh Lua with the `_knowledge_kind_definitions`
    /// accumulator wired up. We don't bring in the full `setup_globals`
    /// machinery because parse_* only needs the accumulator table to exist.
    fn setup_lua() -> Lua {
        let lua = Lua::new();
        let globals = lua.globals();
        let acc = lua.create_table().unwrap();
        globals.set(KNOWLEDGE_DEF_ACCUMULATOR, acc).unwrap();
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
}
