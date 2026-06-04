use mlua::prelude::*;

/// Register core Rust-backed Lua modules for explicit `require(...)` use.
pub fn register_core_lua_modules(lua: &Lua) -> LuaResult<()> {
    lua.register_module("macrocosmo.condition", create_condition_module(lua)?)?;
    lua.register_module("macrocosmo.effect", create_effect_module(lua)?)?;
    Ok(())
}

pub fn create_condition_module(lua: &Lua) -> LuaResult<LuaTable> {
    let module = lua.create_table()?;

    module.set("has_tech", make_id_condition(lua, "has_tech", "id")?)?;
    module.set(
        "has_modifier",
        make_id_condition(lua, "has_modifier", "id")?,
    )?;
    module.set(
        "has_building",
        make_id_condition(lua, "has_building", "id")?,
    )?;
    module.set("has_flag", make_id_condition(lua, "has_flag", "id")?)?;
    module.set(
        "target_allows_option",
        make_id_condition(lua, "target_allows_option", "option_id")?,
    )?;
    module.set(
        "actor_has_modifier",
        make_id_condition(lua, "actor_has_modifier", "modifier_id")?,
    )?;

    module.set(
        "target_state_is",
        lua.create_function(|lua, state: String| {
            let t = lua.create_table()?;
            t.set("type", "target_state_is")?;
            t.set("state", state)?;
            Ok(t)
        })?,
    )?;

    module.set(
        "target_state_in",
        lua.create_function(|lua, args: LuaMultiValue| {
            let t = lua.create_table()?;
            t.set("type", "target_state_in")?;
            let states = lua.create_table()?;
            for (i, arg) in args.into_iter().enumerate() {
                states.set(i + 1, arg)?;
            }
            t.set("states", states)?;
            Ok(t)
        })?,
    )?;

    module.set(
        "target_standing_at_least",
        lua.create_function(|lua, threshold: f64| {
            let t = lua.create_table()?;
            t.set("type", "target_standing_at_least")?;
            t.set("threshold", threshold)?;
            Ok(t)
        })?,
    )?;

    module.set(
        "relative_power_at_least",
        lua.create_function(|lua, ratio: f64| {
            let t = lua.create_table()?;
            t.set("type", "relative_power_at_least")?;
            t.set("ratio", ratio)?;
            Ok(t)
        })?,
    )?;

    module.set(
        "actor_holds_capital_of_target",
        lua.create_function(|lua, _: ()| {
            let t = lua.create_table()?;
            t.set("type", "actor_holds_capital_of_target")?;
            Ok(t)
        })?,
    )?;

    module.set(
        "target_system_count_at_most",
        lua.create_function(|lua, count: u32| {
            let t = lua.create_table()?;
            t.set("type", "target_system_count_at_most")?;
            t.set("count", count)?;
            Ok(t)
        })?,
    )?;

    module.set(
        "target_attacked_actor_core_within",
        lua.create_function(|lua, hexadies: i64| {
            let t = lua.create_table()?;
            t.set("type", "target_attacked_actor_core_within")?;
            t.set("hexadies", hexadies)?;
            Ok(t)
        })?,
    )?;

    module.set("all", make_children_condition(lua, "all")?)?;
    module.set("any", make_children_condition(lua, "any")?)?;
    module.set("one_of", make_children_condition(lua, "one_of")?)?;
    module.set(
        "not_",
        lua.create_function(|lua, child: LuaTable| {
            let t = lua.create_table()?;
            t.set("type", "not")?;
            t.set("child", child)?;
            Ok(t)
        })?,
    )?;

    Ok(module)
}

pub fn create_effect_module(lua: &Lua) -> LuaResult<LuaTable> {
    let module = lua.create_table()?;

    module.set(
        "fire_event",
        lua.create_function(|lua, (event_id, payload): (String, Option<LuaTable>)| {
            let desc = lua.create_table()?;
            desc.set("_effect_type", "fire_event")?;
            desc.set("event_id", event_id)?;
            if let Some(p) = payload {
                desc.set("payload", p)?;
            }
            Ok(desc)
        })?,
    )?;

    module.set(
        "hide",
        lua.create_function(|lua, (label, inner): (String, LuaTable)| {
            let desc = lua.create_table()?;
            desc.set("_effect_type", "hidden")?;
            desc.set("label", label)?;
            desc.set("inner", inner)?;
            Ok(desc)
        })?,
    )?;

    Ok(module)
}

fn make_id_condition(
    lua: &Lua,
    kind: &'static str,
    id_field: &'static str,
) -> LuaResult<LuaFunction> {
    lua.create_function(move |lua, value: LuaValue| {
        let t = lua.create_table()?;
        t.set("type", kind)?;
        t.set(id_field, extract_id_from_lua_value(&value)?)?;
        Ok(t)
    })
}

fn make_children_condition(lua: &Lua, kind: &'static str) -> LuaResult<LuaFunction> {
    lua.create_function(move |lua, args: LuaMultiValue| {
        let t = lua.create_table()?;
        t.set("type", kind)?;
        let children = lua.create_table()?;
        for (i, arg) in args.into_iter().enumerate() {
            children.set(i + 1, arg)?;
        }
        t.set("children", children)?;
        Ok(t)
    })
}

fn extract_id_from_lua_value(value: &LuaValue) -> LuaResult<String> {
    match value {
        LuaValue::String(s) => Ok(s.to_string_lossy().to_string()),
        LuaValue::Table(t) => t.get("id"),
        other => Err(LuaError::runtime(format!(
            "expected string id or reference table with id, got {}",
            other.type_name()
        ))),
    }
}
