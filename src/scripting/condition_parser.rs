use crate::condition::{Condition, ConditionAtom};

/// Parse a Condition tree from a Lua table produced by the condition helper functions
/// (`has_tech`, `has_modifier`, `has_building`, `all`, `any`, `one_of`, `not_cond`).
pub fn parse_condition(table: &mlua::Table) -> Result<Condition, mlua::Error> {
    let cond_type: String = table.get("type")?;
    match cond_type.as_str() {
        "has_tech" => {
            let id: String = table.get("id")?;
            Ok(Condition::Atom(ConditionAtom::HasTech(id)))
        }
        "has_modifier" => {
            let id: String = table.get("id")?;
            Ok(Condition::Atom(ConditionAtom::HasModifier(id)))
        }
        "has_building" => {
            let id: String = table.get("id")?;
            Ok(Condition::Atom(ConditionAtom::HasBuilding(id)))
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
            Condition::Atom(ConditionAtom::HasTech("laser_weapons".into()))
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
            Condition::Atom(ConditionAtom::HasModifier("war_economy".into()))
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
            Condition::Atom(ConditionAtom::HasBuilding("shipyard".into()))
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
                Condition::Atom(ConditionAtom::HasTech("a".into())),
                Condition::Atom(ConditionAtom::HasTech("b".into())),
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
                Condition::Atom(ConditionAtom::HasTech("a".into())),
                Condition::Atom(ConditionAtom::HasModifier("b".into())),
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
                Condition::Atom(ConditionAtom::HasTech("a".into())),
                Condition::Atom(ConditionAtom::HasTech("b".into())),
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
            Condition::Not(Box::new(Condition::Atom(ConditionAtom::HasTech(
                "forbidden".into()
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
                Condition::Atom(ConditionAtom::HasTech("a".into())),
                Condition::Any(vec![
                    Condition::Atom(ConditionAtom::HasModifier("m".into())),
                    Condition::Not(Box::new(Condition::Atom(ConditionAtom::HasBuilding(
                        "b".into()
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
}
