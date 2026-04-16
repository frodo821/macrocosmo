//! #351 (K-2): `PayloadSnapshot` — serde-compatible representation of a
//! Lua-origin knowledge payload.
//!
//! When a Lua script calls `gs:record_knowledge { kind, payload = { ... } }`,
//! the subscriber chain mutates the payload in-place (Lua tables). After the
//! chain completes we need to capture the final payload as a Rust struct that
//! can survive in `PendingFactQueue` without holding Lua references.
//!
//! `PayloadSnapshot` is that struct — a recursive `HashMap<String, PayloadValue>`
//! tree. K-4 later reconstructs a Lua table from it for `@observed` dispatch
//! via `snapshot_to_lua`.
//!
//! # Invariants
//!
//! * Function / UserData values are rejected (`snapshot_from_lua` returns error).
//! * Depth is bounded by `KNOWLEDGE_PAYLOAD_DEPTH_LIMIT` (plan §0.5 9.3).

use bevy::prelude::*;
use mlua::prelude::*;
use std::collections::HashMap;

use super::kind_registry::{KnowledgeKindId, PayloadFieldType, PayloadSchema};
use crate::scripting::knowledge_dispatch::KNOWLEDGE_PAYLOAD_DEPTH_LIMIT;

/// Serde-compatible snapshot of a Lua payload table. Survives without
/// holding any Lua references.
#[derive(Clone, Debug, Default)]
pub struct PayloadSnapshot {
    pub fields: HashMap<String, PayloadValue>,
}

/// Individual payload field value. Mirrors Lua's type system minus
/// Function / UserData (which are schema violations).
#[derive(Clone, Debug)]
pub enum PayloadValue {
    Number(f64),
    Int(i64),
    String(String),
    Boolean(bool),
    Table(PayloadSnapshot),
    /// Entity encoded as `u64` bits (from `Entity::to_bits()`).
    Entity(u64),
    Nil,
}

/// Capture a Lua table as a `PayloadSnapshot`, recursing into nested
/// tables up to `depth_limit`. Returns error for Function / UserData
/// values or depth limit exceeded.
pub fn snapshot_from_lua(
    lua: &Lua,
    table: &mlua::Table,
    depth_limit: usize,
) -> LuaResult<PayloadSnapshot> {
    if depth_limit == 0 {
        return Err(LuaError::RuntimeError(
            "snapshot_from_lua: depth limit exceeded".into(),
        ));
    }
    let mut fields = HashMap::new();
    for pair in table.pairs::<mlua::Value, mlua::Value>() {
        let (k, v) = pair?;
        let key = match k {
            mlua::Value::String(ref s) => s.to_str()?.to_string(),
            mlua::Value::Integer(i) => i.to_string(),
            mlua::Value::Number(n) => n.to_string(),
            _ => continue, // skip non-string keys
        };
        let value = lua_value_to_payload(lua, v, depth_limit - 1)?;
        fields.insert(key, value);
    }
    Ok(PayloadSnapshot { fields })
}

fn lua_value_to_payload(lua: &Lua, v: mlua::Value, depth: usize) -> LuaResult<PayloadValue> {
    match v {
        mlua::Value::Nil => Ok(PayloadValue::Nil),
        mlua::Value::Boolean(b) => Ok(PayloadValue::Boolean(b)),
        mlua::Value::Integer(i) => Ok(PayloadValue::Int(i)),
        mlua::Value::Number(n) => Ok(PayloadValue::Number(n)),
        mlua::Value::String(s) => Ok(PayloadValue::String(s.to_str()?.to_string())),
        mlua::Value::Table(ref t) => {
            let snap = snapshot_from_lua(lua, t, depth)?;
            Ok(PayloadValue::Table(snap))
        }
        mlua::Value::Function(_) => Err(LuaError::RuntimeError(
            "snapshot_from_lua: Function values not allowed in knowledge payloads".into(),
        )),
        mlua::Value::UserData(_) => Err(LuaError::RuntimeError(
            "snapshot_from_lua: UserData values not allowed in knowledge payloads".into(),
        )),
        _ => Ok(PayloadValue::Nil), // LightUserData, Thread, Error -> treat as nil
    }
}

/// Reconstruct a Lua table from a `PayloadSnapshot`. Used by K-4 for
/// `@observed` dispatch (per-observer copy).
pub fn snapshot_to_lua(lua: &Lua, snapshot: &PayloadSnapshot) -> LuaResult<mlua::Table> {
    let t = lua.create_table()?;
    for (k, v) in &snapshot.fields {
        t.set(k.as_str(), payload_value_to_lua(lua, v)?)?;
    }
    Ok(t)
}

fn payload_value_to_lua(lua: &Lua, v: &PayloadValue) -> LuaResult<mlua::Value> {
    match v {
        PayloadValue::Nil => Ok(mlua::Value::Nil),
        PayloadValue::Boolean(b) => Ok(mlua::Value::Boolean(*b)),
        PayloadValue::Int(i) => Ok(mlua::Value::Integer(*i)),
        PayloadValue::Number(n) => Ok(mlua::Value::Number(*n)),
        PayloadValue::String(s) => Ok(mlua::Value::String(lua.create_string(s)?)),
        PayloadValue::Table(snap) => {
            let t = snapshot_to_lua(lua, snap)?;
            Ok(mlua::Value::Table(t))
        }
        PayloadValue::Entity(bits) => Ok(mlua::Value::Integer(*bits as i64)),
    }
}

/// Validate a Lua payload table against a `PayloadSchema`. Schema violations
/// are returned as `RuntimeError`. Unknown fields produce a warning but are
/// allowed (v1 loose validation, plan §3.1).
pub fn validate_payload_schema(
    kind_id: &KnowledgeKindId,
    schema: &PayloadSchema,
    table: &mlua::Table,
) -> LuaResult<()> {
    if schema.is_empty() {
        return Ok(());
    }
    for pair in table.pairs::<mlua::Value, mlua::Value>() {
        let (k, v) = pair?;
        let key = match k {
            mlua::Value::String(ref s) => s.to_str()?.to_string(),
            _ => continue,
        };
        if let Some(expected_type) = schema.fields.get(&key) {
            let actual_ok = match (expected_type, &v) {
                (PayloadFieldType::Number, mlua::Value::Number(_))
                | (PayloadFieldType::Number, mlua::Value::Integer(_)) => true,
                (PayloadFieldType::String, mlua::Value::String(_)) => true,
                (PayloadFieldType::Boolean, mlua::Value::Boolean(_)) => true,
                (PayloadFieldType::Table, mlua::Value::Table(_)) => true,
                (PayloadFieldType::Entity, mlua::Value::Integer(_))
                | (PayloadFieldType::Entity, mlua::Value::Number(_)) => true,
                _ => false,
            };
            if !actual_ok {
                return Err(LuaError::RuntimeError(format!(
                    "record_knowledge('{}'): payload field '{}' expected type '{}', got '{}'",
                    kind_id,
                    key,
                    expected_type.as_str(),
                    v.type_name(),
                )));
            }
        }
        // Unknown fields: allowed in v1 (warn only is done at a higher level if desired).
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_round_trip_flat() {
        let lua = Lua::new();
        let src = lua.create_table().unwrap();
        src.set("name", "test").unwrap();
        src.set("value", 42.5).unwrap();
        src.set("active", true).unwrap();

        let snap = snapshot_from_lua(&lua, &src, KNOWLEDGE_PAYLOAD_DEPTH_LIMIT).unwrap();
        assert_eq!(snap.fields.len(), 3);

        let rebuilt = snapshot_to_lua(&lua, &snap).unwrap();
        assert_eq!(rebuilt.get::<String>("name").unwrap(), "test");
        let v: f64 = rebuilt.get("value").unwrap();
        assert!((v - 42.5).abs() < f64::EPSILON);
        assert!(rebuilt.get::<bool>("active").unwrap());
    }

    #[test]
    fn snapshot_round_trip_nested() {
        let lua = Lua::new();
        let inner = lua.create_table().unwrap();
        inner.set("x", 1).unwrap();
        let src = lua.create_table().unwrap();
        src.set("inner", inner).unwrap();

        let snap = snapshot_from_lua(&lua, &src, KNOWLEDGE_PAYLOAD_DEPTH_LIMIT).unwrap();
        let rebuilt = snapshot_to_lua(&lua, &snap).unwrap();
        let ri: mlua::Table = rebuilt.get("inner").unwrap();
        assert_eq!(ri.get::<i64>("x").unwrap(), 1);
    }

    #[test]
    fn snapshot_rejects_function() {
        let lua = Lua::new();
        let src = lua.create_table().unwrap();
        let f: mlua::Function = lua.load("function() end").eval().unwrap();
        src.set("cb", f).unwrap();
        assert!(snapshot_from_lua(&lua, &src, KNOWLEDGE_PAYLOAD_DEPTH_LIMIT).is_err());
    }

    #[test]
    fn snapshot_depth_limit() {
        let lua = Lua::new();
        let t1 = lua.create_table().unwrap();
        let t2 = lua.create_table().unwrap();
        let t3 = lua.create_table().unwrap();
        t3.set("leaf", true).unwrap();
        t2.set("c", t3).unwrap();
        t1.set("c", t2).unwrap();
        assert!(snapshot_from_lua(&lua, &t1, 2).is_err());
        assert!(snapshot_from_lua(&lua, &t1, 3).is_ok());
    }

    #[test]
    fn validate_schema_type_mismatch() {
        let lua = Lua::new();
        let schema = PayloadSchema {
            fields: [("severity".to_string(), PayloadFieldType::Number)]
                .into_iter()
                .collect(),
        };
        let t = lua.create_table().unwrap();
        t.set("severity", "high").unwrap(); // string instead of number
        let id = KnowledgeKindId::parse("test:kind").unwrap();
        let err = validate_payload_schema(&id, &schema, &t);
        assert!(err.is_err());
    }

    #[test]
    fn validate_schema_allows_unknown_fields() {
        let lua = Lua::new();
        let schema = PayloadSchema {
            fields: [("severity".to_string(), PayloadFieldType::Number)]
                .into_iter()
                .collect(),
        };
        let t = lua.create_table().unwrap();
        t.set("severity", 0.7).unwrap();
        t.set("unknown_field", "allowed").unwrap();
        let id = KnowledgeKindId::parse("test:kind").unwrap();
        assert!(validate_payload_schema(&id, &schema, &t).is_ok());
    }
}
