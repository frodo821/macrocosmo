use crate::condition::{AtomKind, Condition, ConditionAtom, ConditionScope};

/// Parse an optional `prerequisites` field from a definition table.
///
/// Accepts three shapes (all shared across `define_structure`, `define_building`,
/// `define_hull`, `define_module`):
///
/// * `nil` — no prerequisites.
/// * a condition table produced by the condition helper functions
///   (`has_tech`, `all`, `any`, `one_of`, `not_cond`, `has_flag`, ...).
/// * a function `function(ctx) return <condition table> end` — the function is
///   called with a `ConditionCtx` to allow scoped atoms like `ctx.empire:has_tech(...)`.
pub fn parse_prerequisites_field(
    table: &mlua::Table,
) -> Result<Option<Condition>, mlua::Error> {
    let prereq_value: mlua::Value = table.get("prerequisites")?;
    match prereq_value {
        mlua::Value::Table(prereq_table) => Ok(Some(parse_condition(&prereq_table)?)),
        mlua::Value::Function(func) => {
            let ctx = crate::scripting::condition_ctx::ConditionCtx;
            let result: mlua::Table = func.call(ctx)?;
            Ok(Some(parse_condition(&result)?))
        }
        mlua::Value::Nil => Ok(None),
        _ => Err(mlua::Error::RuntimeError(
            "Expected table, function, or nil for 'prerequisites' field".to_string(),
        )),
    }
}

/// Parse an optional `scope` field from a Lua table and convert to ConditionScope.
fn parse_scope(table: &mlua::Table) -> Result<ConditionScope, mlua::Error> {
    let scope_str: Option<String> = table.get("scope")?;
    match scope_str.as_deref() {
        None => Ok(ConditionScope::Any),
        Some("empire") => Ok(ConditionScope::Empire),
        Some("system") => Ok(ConditionScope::System),
        Some("planet") => Ok(ConditionScope::Planet),
        Some("ship") => Ok(ConditionScope::Ship),
        Some("any") => Ok(ConditionScope::Any),
        Some(other) => Err(mlua::Error::runtime(format!(
            "Unknown condition scope: {}",
            other
        ))),
    }
}

/// Parse a Condition tree from a Lua table produced by the condition helper functions
/// (`has_tech`, `has_modifier`, `has_building`, `has_flag`, `all`, `any`, `one_of`, `not_cond`).
pub fn parse_condition(table: &mlua::Table) -> Result<Condition, mlua::Error> {
    let cond_type: String = table.get("type")?;
    match cond_type.as_str() {
        "has_tech" => {
            let id: String = table.get("id")?;
            let scope = parse_scope(table)?;
            Ok(Condition::Atom(ConditionAtom::scoped(
                AtomKind::HasTech(id),
                scope,
            )))
        }
        "has_modifier" => {
            let id: String = table.get("id")?;
            let scope = parse_scope(table)?;
            Ok(Condition::Atom(ConditionAtom::scoped(
                AtomKind::HasModifier(id),
                scope,
            )))
        }
        "has_building" => {
            let id: String = table.get("id")?;
            let scope = parse_scope(table)?;
            Ok(Condition::Atom(ConditionAtom::scoped(
                AtomKind::HasBuilding(id),
                scope,
            )))
        }
        "has_flag" => {
            let id: String = table.get("id")?;
            let scope = parse_scope(table)?;
            Ok(Condition::Atom(ConditionAtom::scoped(
                AtomKind::HasFlag(id),
                scope,
            )))
        }
        "all" => {
            let children: mlua::Table = table.get("children")?;
            let conds = children
                .sequence_values::<mlua::Table>()
                .map(|t| parse_condition(&t?))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Condition::All(conds))
        }
        "any" => {
            let children: mlua::Table = table.get("children")?;
            let conds = children
                .sequence_values::<mlua::Table>()
                .map(|t| parse_condition(&t?))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Condition::Any(conds))
        }
        "one_of" => {
            let children: mlua::Table = table.get("children")?;
            let conds = children
                .sequence_values::<mlua::Table>()
                .map(|t| parse_condition(&t?))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Condition::OneOf(conds))
        }
        "not" => {
            let child: mlua::Table = table.get("child")?;
            let cond = parse_condition(&child)?;
            Ok(Condition::Not(Box::new(cond)))
        }
        other => Err(mlua::Error::runtime(format!(
            "Unknown condition type: {}",
            other
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ScriptEngine;

    #[test]
    fn test_parse_has_tech() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let table: mlua::Table = lua
            .load(r#"return has_tech("laser_weapons")"#)
            .eval()
            .unwrap();
        let cond = parse_condition(&table).unwrap();
        assert_eq!(
            cond,
            Condition::Atom(ConditionAtom::has_tech("laser_weapons"))
        );
    }

    #[test]
    fn test_parse_has_modifier() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let table: mlua::Table = lua
            .load(r#"return has_modifier("war_economy")"#)
            .eval()
            .unwrap();
        let cond = parse_condition(&table).unwrap();
        assert_eq!(
            cond,
            Condition::Atom(ConditionAtom::has_modifier("war_economy"))
        );
    }

    #[test]
    fn test_parse_has_building() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let table: mlua::Table = lua
            .load(r#"return has_building("shipyard")"#)
            .eval()
            .unwrap();
        let cond = parse_condition(&table).unwrap();
        assert_eq!(
            cond,
            Condition::Atom(ConditionAtom::has_building("shipyard"))
        );
    }

    #[test]
    fn test_parse_has_flag() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let table: mlua::Table = lua
            .load(r#"return has_flag("my_flag")"#)
            .eval()
            .unwrap();
        let cond = parse_condition(&table).unwrap();
        assert_eq!(
            cond,
            Condition::Atom(ConditionAtom::has_flag("my_flag"))
        );
    }

    #[test]
    fn test_parse_all() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let table: mlua::Table = lua
            .load(r#"return all(has_tech("a"), has_tech("b"))"#)
            .eval()
            .unwrap();
        let cond = parse_condition(&table).unwrap();
        assert_eq!(
            cond,
            Condition::All(vec![
                Condition::Atom(ConditionAtom::has_tech("a")),
                Condition::Atom(ConditionAtom::has_tech("b")),
            ])
        );
    }

    #[test]
    fn test_parse_any() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let table: mlua::Table = lua
            .load(r#"return any(has_tech("a"), has_modifier("b"))"#)
            .eval()
            .unwrap();
        let cond = parse_condition(&table).unwrap();
        assert_eq!(
            cond,
            Condition::Any(vec![
                Condition::Atom(ConditionAtom::has_tech("a")),
                Condition::Atom(ConditionAtom::has_modifier("b")),
            ])
        );
    }

    #[test]
    fn test_parse_one_of() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let table: mlua::Table = lua
            .load(r#"return one_of(has_tech("a"), has_tech("b"))"#)
            .eval()
            .unwrap();
        let cond = parse_condition(&table).unwrap();
        assert_eq!(
            cond,
            Condition::OneOf(vec![
                Condition::Atom(ConditionAtom::has_tech("a")),
                Condition::Atom(ConditionAtom::has_tech("b")),
            ])
        );
    }

    #[test]
    fn test_parse_not() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let table: mlua::Table = lua
            .load(r#"return not_cond(has_tech("forbidden"))"#)
            .eval()
            .unwrap();
        let cond = parse_condition(&table).unwrap();
        assert_eq!(
            cond,
            Condition::Not(Box::new(Condition::Atom(ConditionAtom::has_tech(
                "forbidden"
            ))))
        );
    }

    #[test]
    fn test_parse_nested() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let table: mlua::Table = lua
            .load(r#"return all(has_tech("a"), any(has_modifier("m"), not_cond(has_building("b"))))"#)
            .eval()
            .unwrap();
        let cond = parse_condition(&table).unwrap();
        assert_eq!(
            cond,
            Condition::All(vec![
                Condition::Atom(ConditionAtom::has_tech("a")),
                Condition::Any(vec![
                    Condition::Atom(ConditionAtom::has_modifier("m")),
                    Condition::Not(Box::new(Condition::Atom(ConditionAtom::has_building(
                        "b"
                    )))),
                ]),
            ])
        );
    }

    #[test]
    fn test_parse_unknown_type_errors() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let table: mlua::Table = lua
            .load(r#"return { type = "bogus" }"#)
            .eval()
            .unwrap();
        assert!(parse_condition(&table).is_err());
    }

    #[test]
    fn test_parse_has_flag_lua_helper_table_shape() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        let table: mlua::Table = lua
            .load(r#"return has_flag("test_flag")"#)
            .eval()
            .unwrap();
        let typ: String = table.get("type").unwrap();
        assert_eq!(typ, "has_flag");
        let id: String = table.get("id").unwrap();
        assert_eq!(id, "test_flag");
    }

    #[test]
    fn test_condition_ctx_scoped_has_tech() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        // Register ConditionCtx as a global for testing
        lua.globals()
            .set("ctx", crate::scripting::condition_ctx::ConditionCtx)
            .unwrap();

        let table: mlua::Table = lua
            .load(r#"return ctx.empire:has_tech("advanced_sensors")"#)
            .eval()
            .unwrap();

        let typ: String = table.get("type").unwrap();
        assert_eq!(typ, "has_tech");
        let id: String = table.get("id").unwrap();
        assert_eq!(id, "advanced_sensors");
        let scope: String = table.get("scope").unwrap();
        assert_eq!(scope, "empire");
    }

    #[test]
    fn test_condition_ctx_scoped_has_flag() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.globals()
            .set("ctx", crate::scripting::condition_ctx::ConditionCtx)
            .unwrap();

        let table: mlua::Table = lua
            .load(r#"return ctx.system:has_flag("fortified")"#)
            .eval()
            .unwrap();

        let typ: String = table.get("type").unwrap();
        assert_eq!(typ, "has_flag");
        let id: String = table.get("id").unwrap();
        assert_eq!(id, "fortified");
        let scope: String = table.get("scope").unwrap();
        assert_eq!(scope, "system");
    }
}
