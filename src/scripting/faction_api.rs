use std::collections::HashMap;

use bevy::prelude::*;

/// A faction definition loaded from Lua scripts.
#[derive(Debug, Clone)]
pub struct FactionDefinition {
    pub id: String,
    pub name: String,
    /// Whether this faction defines an `on_game_start` callback.
    /// The actual function is looked up from `_faction_definitions` at call time.
    pub has_on_game_start: bool,
}

/// Registry of all faction definitions loaded from Lua.
#[derive(Resource, Default, Debug)]
pub struct FactionRegistry {
    pub factions: HashMap<String, FactionDefinition>,
}

/// Parse faction definitions from the Lua `_faction_definitions` global table.
pub fn parse_faction_definitions(lua: &mlua::Lua) -> Result<Vec<FactionDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_faction_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table.get("name")?;
        let has_on_game_start = matches!(
            table.get::<mlua::Value>("on_game_start").unwrap_or(mlua::Value::Nil),
            mlua::Value::Function(_)
        );

        result.push(FactionDefinition {
            id,
            name,
            has_on_game_start,
        });
    }

    Ok(result)
}

/// Look up the `on_game_start` Lua function for the given faction id, if any.
/// Returns Ok(None) if the faction is not defined or has no callback.
pub fn lookup_on_game_start(
    lua: &mlua::Lua,
    faction_id: &str,
) -> Result<Option<mlua::Function>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_faction_definitions")?;
    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;
        let id: String = table.get("id")?;
        if id == faction_id {
            let value: mlua::Value = table.get("on_game_start")?;
            if let mlua::Value::Function(f) = value {
                return Ok(Some(f));
            }
            return Ok(None);
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ScriptEngine;

    #[test]
    fn test_parse_faction_definitions() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_faction {
                id = "humanity_empire",
                name = "Terran Federation",
            }
            define_faction {
                id = "alien_hive",
                name = "Zyx Collective",
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_faction_definitions(lua).unwrap();
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].id, "humanity_empire");
        assert_eq!(defs[0].name, "Terran Federation");
        assert!(!defs[0].has_on_game_start);
        assert_eq!(defs[1].id, "alien_hive");
        assert_eq!(defs[1].name, "Zyx Collective");
        assert!(!defs[1].has_on_game_start);
    }

    #[test]
    fn test_parse_faction_with_on_game_start() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_faction {
                id = "humanity_empire",
                name = "Terran Federation",
                on_game_start = function(ctx) end,
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_faction_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert!(defs[0].has_on_game_start);
    }

    #[test]
    fn test_lookup_on_game_start_returns_function() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_faction {
                id = "humanity_empire",
                name = "Terran Federation",
                on_game_start = function(ctx) return 42 end,
            }
            "#,
        )
        .exec()
        .unwrap();

        let func = lookup_on_game_start(lua, "humanity_empire").unwrap();
        assert!(func.is_some());
        let result: i64 = func.unwrap().call(()).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn test_lookup_on_game_start_missing() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"define_faction { id = "humanity_empire", name = "Terran Federation" }"#,
        )
        .exec()
        .unwrap();

        let func = lookup_on_game_start(lua, "humanity_empire").unwrap();
        assert!(func.is_none());

        let func2 = lookup_on_game_start(lua, "nonexistent").unwrap();
        assert!(func2.is_none());
    }

    #[test]
    fn test_define_faction_returns_reference() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let result: mlua::Table = lua
            .load(r#"return define_faction { id = "test_faction", name = "Test" }"#)
            .eval()
            .unwrap();

        let def_type: String = result.get("_def_type").unwrap();
        assert_eq!(def_type, "faction");
        let id: String = result.get("id").unwrap();
        assert_eq!(id, "test_faction");
    }

    #[test]
    fn test_parse_faction_empty() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let defs = parse_faction_definitions(lua).unwrap();
        assert_eq!(defs.len(), 0);
    }
}
