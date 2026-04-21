use mlua::prelude::*;

use crate::event_system::LuaFunctionRef;

/// Extract an ID string from a Lua value that is either:
/// - A plain string -> used as-is
/// - A reference table (from `define_xxx` or `forward_ref`) -> reads the `id` field
pub fn extract_id_from_lua_value(value: &mlua::Value) -> Result<String, mlua::Error> {
    match value {
        mlua::Value::String(s) => Ok(s.to_str()?.to_string()),
        mlua::Value::Table(t) => t.get::<String>("id"),
        _ => Err(mlua::Error::RuntimeError(
            "Expected string ID or reference table".into(),
        )),
    }
}

/// Extract an ID from a Lua value, accepting both string IDs and reference tables.
/// This is the public API for use by Rust-side parsers.
pub fn extract_ref_id(value: &mlua::Value) -> Result<String, mlua::Error> {
    extract_id_from_lua_value(value)
}

/// #281: Read an optional Lua function from a field on a definition table and
/// store it as a proper `mlua::RegistryKey` wrapped inside a
/// [`LuaFunctionRef`]. Returns `None` when the field is nil/absent. Errors
/// when the field is present but is neither a function nor nil.
///
/// This mirrors the private helper in `event_api.rs::parse_lua_function_ref`
/// (introduced in #263) and is re-exposed here so multiple `define_xxx`
/// parsers (`building_api`, `structure_api`, ...) can share it without
/// duplicating the registry-key wiring.
pub fn parse_lua_function_field(
    lua: &mlua::Lua,
    table: &mlua::Table,
    field: &str,
) -> Result<Option<LuaFunctionRef>, mlua::Error> {
    let value: mlua::Value = table.get(field)?;
    match value {
        mlua::Value::Function(f) => Ok(Some(LuaFunctionRef::from_function(lua, f)?)),
        mlua::Value::Nil => Ok(None),
        _ => Err(mlua::Error::RuntimeError(format!(
            "Expected function or nil for field '{}', got {:?}",
            field,
            value.type_name()
        ))),
    }
}
