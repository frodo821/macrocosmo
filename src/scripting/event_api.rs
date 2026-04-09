use crate::event_system::{time_to_hexadies, EventDefinition, EventTrigger, LuaFunctionRef};

/// Parse event definitions from the Lua `_event_definitions` global table.
/// Each entry should have at minimum `id`, `name`, and `description` fields.
/// The `trigger` field can be:
/// - absent or "manual" -> Manual trigger
/// - a table from mtth_trigger{} -> Mtth trigger
/// - a table from periodic_trigger{} -> Periodic trigger
/// The `on_trigger` callback is kept in the Lua table and invoked at fire time.
pub fn parse_event_definitions(lua: &mlua::Lua) -> Result<Vec<EventDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_event_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table.get("name")?;
        let description: String = table.get::<Option<String>>("description")?.unwrap_or_default();

        let trigger = parse_trigger(lua, &table, &id)?;

        result.push(EventDefinition {
            id,
            name,
            description,
            trigger,
        });
    }

    Ok(result)
}

/// Parse the `trigger` field of an event definition table.
/// Returns Manual if absent or "manual" string, Mtth or Periodic if a tagged table.
fn parse_trigger(
    lua: &mlua::Lua,
    table: &mlua::Table,
    event_id: &str,
) -> Result<EventTrigger, mlua::Error> {
    // Try to get trigger as a table first
    let trigger_value: mlua::Value = table.get("trigger")?;

    match trigger_value {
        mlua::Value::Nil => Ok(EventTrigger::Manual),
        mlua::Value::String(s) => {
            let s_str = s.to_str()?;
            if s_str == "manual" {
                Ok(EventTrigger::Manual)
            } else {
                Err(mlua::Error::RuntimeError(format!(
                    "Unknown trigger type '{}' for event '{}'",
                    s_str, event_id
                )))
            }
        }
        mlua::Value::Table(trigger_table) => {
            let type_str: Option<String> = trigger_table.get("_type")?;
            match type_str.as_deref() {
                Some("mtth") => parse_mtth_trigger(lua, &trigger_table, event_id),
                Some("periodic") => parse_periodic_trigger(lua, &trigger_table, event_id),
                Some(other) => Err(mlua::Error::RuntimeError(format!(
                    "Unknown trigger _type '{}' for event '{}'",
                    other, event_id
                ))),
                None => Err(mlua::Error::RuntimeError(format!(
                    "Trigger table missing '_type' field for event '{}'",
                    event_id
                ))),
            }
        }
        _ => Err(mlua::Error::RuntimeError(format!(
            "Invalid trigger type for event '{}': expected string, table, or nil",
            event_id
        ))),
    }
}

/// Parse an MTTH trigger table: { _type="mtth", years=N, months=N, sd=N,
///   activate_condition=fn, fire_condition=fn, max_times=N }
fn parse_mtth_trigger(
    lua: &mlua::Lua,
    table: &mlua::Table,
    _event_id: &str,
) -> Result<EventTrigger, mlua::Error> {
    let years: i64 = table.get::<Option<i64>>("years")?.unwrap_or(0);
    let months: i64 = table.get::<Option<i64>>("months")?.unwrap_or(0);
    let sd: i64 = table.get::<Option<i64>>("sd")?.unwrap_or(0);
    let mean_hexadies = time_to_hexadies(years, months, sd);

    let fire_condition = parse_lua_function_ref(lua, table, "fire_condition")?;
    let max_times: Option<u32> = table.get("max_times")?;

    Ok(EventTrigger::Mtth {
        mean_hexadies,
        fire_condition,
        max_times,
        times_triggered: 0,
    })
}

/// Parse a Periodic trigger table: { _type="periodic", years=N, months=N, sd=N,
///   fire_condition=fn, max_times=N }
fn parse_periodic_trigger(
    lua: &mlua::Lua,
    table: &mlua::Table,
    _event_id: &str,
) -> Result<EventTrigger, mlua::Error> {
    let years: i64 = table.get::<Option<i64>>("years")?.unwrap_or(0);
    let months: i64 = table.get::<Option<i64>>("months")?.unwrap_or(0);
    let sd: i64 = table.get::<Option<i64>>("sd")?.unwrap_or(0);
    let interval_hexadies = time_to_hexadies(years, months, sd);

    let fire_condition = parse_lua_function_ref(lua, table, "fire_condition")?;
    let max_times: Option<u32> = table.get("max_times")?;

    Ok(EventTrigger::Periodic {
        interval_hexadies,
        last_fired: 0,
        fire_condition,
        max_times,
        times_triggered: 0,
    })
}

/// Try to read a Lua function from a table field and store it as a registry key.
fn parse_lua_function_ref(
    lua: &mlua::Lua,
    table: &mlua::Table,
    field: &str,
) -> Result<Option<LuaFunctionRef>, mlua::Error> {
    let value: mlua::Value = table.get(field)?;
    match value {
        mlua::Value::Function(f) => {
            let key = lua.create_registry_value(f)?;
            // Registry keys in mlua are RegistryKey, not i64.
            // For now, store a hash/placeholder since LuaFunctionRef uses i64.
            // We use the debug representation to get a unique identifier.
            // TODO: Consider changing LuaFunctionRef to hold RegistryKey directly.
            let key_id = format!("{:?}", key);
            let hash = key_id.len() as i64; // simple placeholder
            // Keep the registry value alive by NOT dropping `key` — leak it into the registry
            std::mem::forget(key);
            Ok(Some(LuaFunctionRef(hash)))
        }
        mlua::Value::Nil => Ok(None),
        _ => Err(mlua::Error::RuntimeError(format!(
            "Expected function or nil for field '{}', got {:?}",
            field,
            value.type_name()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ScriptEngine;

    #[test]
    fn test_parse_event_definitions() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_event {
                id = "harvest_ended",
                name = "End of Harvest",
                description = "The bountiful harvest season has ended.",
            }
            define_event {
                id = "plague",
                name = "Plague",
                description = "A terrible plague strikes the colony.",
                trigger = "manual",
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_event_definitions(lua).unwrap();
        assert_eq!(defs.len(), 2);

        assert_eq!(defs[0].id, "harvest_ended");
        assert_eq!(defs[0].name, "End of Harvest");
        assert_eq!(
            defs[0].description,
            "The bountiful harvest season has ended."
        );
        assert!(matches!(defs[0].trigger, EventTrigger::Manual));

        assert_eq!(defs[1].id, "plague");
        assert_eq!(defs[1].name, "Plague");
        assert!(matches!(defs[1].trigger, EventTrigger::Manual));
    }

    #[test]
    fn test_parse_event_no_description() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_event {
                id = "minimal",
                name = "Minimal Event",
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_event_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].description, "");
    }

    #[test]
    fn test_parse_event_unknown_trigger() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_event {
                id = "bad",
                name = "Bad Event",
                trigger = "unknown_type",
            }
            "#,
        )
        .exec()
        .unwrap();

        let result = parse_event_definitions(lua);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_event_with_on_trigger_callback() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_event {
                id = "with_callback",
                name = "Callback Event",
                description = "Has a callback.",
                on_trigger = function(event)
                    -- callback logic here
                end,
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_event_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id, "with_callback");
        // on_trigger is stored in the Lua table, not in EventDefinition
    }

    #[test]
    fn test_parse_mtth_trigger() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_event {
                id = "pirate_raid",
                name = "Pirate Raid",
                description = "Pirates attack!",
                trigger = mtth_trigger {
                    years = 6,
                },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_event_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id, "pirate_raid");
        match &defs[0].trigger {
            EventTrigger::Mtth {
                mean_hexadies,
                max_times,
                times_triggered,
                ..
            } => {
                assert_eq!(*mean_hexadies, 360); // 6 years * 60
                assert_eq!(*max_times, None);
                assert_eq!(*times_triggered, 0);
            }
            other => panic!("Expected Mtth trigger, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_mtth_trigger_with_months_and_max() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_event {
                id = "rare_event",
                name = "Rare Event",
                trigger = mtth_trigger {
                    years = 1,
                    months = 2,
                    max_times = 3,
                },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_event_definitions(lua).unwrap();
        match &defs[0].trigger {
            EventTrigger::Mtth {
                mean_hexadies,
                max_times,
                ..
            } => {
                assert_eq!(*mean_hexadies, 70); // 60 + 10
                assert_eq!(*max_times, Some(3));
            }
            other => panic!("Expected Mtth trigger, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_periodic_trigger() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_event {
                id = "annual_census",
                name = "Annual Census",
                description = "Yearly report.",
                trigger = periodic_trigger {
                    years = 1,
                },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_event_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id, "annual_census");
        match &defs[0].trigger {
            EventTrigger::Periodic {
                interval_hexadies,
                last_fired,
                max_times,
                times_triggered,
                ..
            } => {
                assert_eq!(*interval_hexadies, 60); // 1 year
                assert_eq!(*last_fired, 0);
                assert_eq!(*max_times, None);
                assert_eq!(*times_triggered, 0);
            }
            other => panic!("Expected Periodic trigger, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_periodic_trigger_months() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_event {
                id = "monthly_report",
                name = "Monthly Report",
                trigger = periodic_trigger {
                    months = 1,
                },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_event_definitions(lua).unwrap();
        match &defs[0].trigger {
            EventTrigger::Periodic {
                interval_hexadies, ..
            } => {
                assert_eq!(*interval_hexadies, 5); // 1 month = 5 hexadies
            }
            other => panic!("Expected Periodic trigger, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_mtth_with_conditions() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_event {
                id = "conditional_event",
                name = "Conditional",
                trigger = mtth_trigger {
                    years = 5,
                    fire_condition = function() return true end,
                },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_event_definitions(lua).unwrap();
        match &defs[0].trigger {
            EventTrigger::Mtth {
                fire_condition,
                ..
            } => {
                assert!(fire_condition.is_some());
            }
            other => panic!("Expected Mtth trigger, got {:?}", other),
        }
    }
}
