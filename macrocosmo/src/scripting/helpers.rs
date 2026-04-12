use mlua::prelude::*;

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
