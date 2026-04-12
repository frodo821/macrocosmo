use crate::scripting::extract_id_from_lua_value;

/// A handle representing a specific scope (empire, system, planet, ship).
/// Used by Lua code to build scoped condition tables:
/// `ctx.empire:has_tech("x")` returns `{type="has_tech", id="x", scope="empire"}`.
#[derive(Clone)]
pub struct ScopeHandle(pub String);

impl mlua::UserData for ScopeHandle {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("has_tech", |lua, this, value: mlua::Value| {
            let t = lua.create_table()?;
            t.set("type", "has_tech")?;
            t.set("id", extract_id_from_lua_value(&value)?)?;
            t.set("scope", this.0.as_str())?;
            Ok(t)
        });

        methods.add_method("has_modifier", |lua, this, value: mlua::Value| {
            let t = lua.create_table()?;
            t.set("type", "has_modifier")?;
            t.set("id", extract_id_from_lua_value(&value)?)?;
            t.set("scope", this.0.as_str())?;
            Ok(t)
        });

        methods.add_method("has_building", |lua, this, value: mlua::Value| {
            let t = lua.create_table()?;
            t.set("type", "has_building")?;
            t.set("id", extract_id_from_lua_value(&value)?)?;
            t.set("scope", this.0.as_str())?;
            Ok(t)
        });

        methods.add_method("has_flag", |lua, this, value: mlua::Value| {
            let t = lua.create_table()?;
            t.set("type", "has_flag")?;
            t.set("id", extract_id_from_lua_value(&value)?)?;
            t.set("scope", this.0.as_str())?;
            Ok(t)
        });
    }
}

/// A condition context object passed to Lua prerequisite functions.
/// Provides scoped access to condition builders:
/// ```lua
/// prerequisites = function(ctx)
///     return all(ctx.empire:has_tech("x"), ctx.system:has_building("y"))
/// end
/// ```
///
/// This object does NOT hold game state. It only builds condition tables
/// that are later parsed and evaluated on the Rust side.
pub struct ConditionCtx;

impl mlua::UserData for ConditionCtx {
    fn add_fields<F: mlua::UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("empire", |_, _| Ok(ScopeHandle("empire".into())));
        fields.add_field_method_get("system", |_, _| Ok(ScopeHandle("system".into())));
        fields.add_field_method_get("planet", |_, _| Ok(ScopeHandle("planet".into())));
        fields.add_field_method_get("ship", |_, _| Ok(ScopeHandle("ship".into())));
    }
}
