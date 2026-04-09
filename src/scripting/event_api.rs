use crate::event_system::{EventDefinition, EventTrigger};

/// Parse event definitions from the Lua `_event_definitions` global table.
/// Each entry should have at minimum `id`, `name`, and `description` fields.
/// The `trigger` field defaults to `Manual` if absent.
/// The `on_trigger` callback is kept in the Lua table and invoked at fire time.
pub fn parse_event_definitions(lua: &mlua::Lua) -> Result<Vec<EventDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_event_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table.get("name")?;
        let description: String = table.get::<Option<String>>("description")?.unwrap_or_default();

        let trigger_str: Option<String> = table.get("trigger")?;
        let trigger = match trigger_str.as_deref() {
            Some("manual") | None => EventTrigger::Manual,
            Some(other) => {
                return Err(mlua::Error::RuntimeError(format!(
                    "Unknown trigger type '{}' for event '{}'",
                    other, id
                )));
            }
        };

        result.push(EventDefinition {
            id,
            name,
            description,
            trigger,
        });
    }

    Ok(result)
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
}
