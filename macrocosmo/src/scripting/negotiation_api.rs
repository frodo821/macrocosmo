//! #321: Parse negotiation item kind definitions from Lua.
//!
//! Definitions are accumulated by `define_negotiation_item_kind { ... }`
//! (registered in `globals.rs`) into `_negotiation_item_kind_definitions`.
//! This module drains that accumulator at startup and builds the
//! [`NegotiationItemKindRegistry`] resource.

use crate::negotiation::{MergeStrategy, NegotiationItemKindDefinition};

/// The Lua global accumulator name for negotiation item kind definitions.
pub const ACCUMULATOR: &str = "_negotiation_item_kind_definitions";

/// Parse negotiation item kind definitions from the Lua accumulator.
/// Returns a `Vec` of definitions; the caller inserts them into the
/// [`NegotiationItemKindRegistry`].
pub fn parse_negotiation_item_kind_definitions(
    lua: &mlua::Lua,
) -> Result<Vec<NegotiationItemKindDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get(ACCUMULATOR)?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table
            .get::<Option<String>>("name")?
            .unwrap_or_else(|| id.clone());

        // Parse merge strategy (default: "list")
        let merge_str: String = table
            .get::<Option<String>>("merge")?
            .unwrap_or_else(|| "list".to_string());
        let merge_strategy = MergeStrategy::from_str(&merge_str).ok_or_else(|| {
            mlua::Error::RuntimeError(format!(
                "negotiation item kind '{id}': unknown merge strategy '{merge_str}' \
                 (expected 'list', 'sum', or 'replace')"
            ))
        })?;

        // Check for validate / apply function presence
        let has_validate = matches!(
            table.get::<mlua::Value>("validate")?,
            mlua::Value::Function(_)
        );
        let has_apply = matches!(table.get::<mlua::Value>("apply")?, mlua::Value::Function(_));

        result.push(NegotiationItemKindDefinition {
            id,
            name,
            merge_strategy,
            has_validate,
            has_apply,
        });
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ScriptEngine;

    #[test]
    fn parse_basic_negotiation_item_kind() {
        let engine = ScriptEngine::new().expect("lua init");
        engine
            .lua()
            .load(
                r#"
            define_negotiation_item_kind {
                id = "territory",
                name = "Territory Cession",
                merge = "list",
                validate = function(ctx) return true end,
                apply = function(ctx) end,
            }
        "#,
            )
            .exec()
            .expect("lua exec");

        let defs = parse_negotiation_item_kind_definitions(engine.lua()).expect("parse");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id, "territory");
        assert_eq!(defs[0].name, "Territory Cession");
        assert_eq!(defs[0].merge_strategy, MergeStrategy::List);
        assert!(defs[0].has_validate);
        assert!(defs[0].has_apply);
    }

    #[test]
    fn parse_merge_strategies() {
        let engine = ScriptEngine::new().expect("lua init");
        engine
            .lua()
            .load(
                r#"
            define_negotiation_item_kind { id = "a", merge = "list" }
            define_negotiation_item_kind { id = "b", merge = "sum" }
            define_negotiation_item_kind { id = "c", merge = "replace" }
        "#,
            )
            .exec()
            .expect("lua exec");

        let defs = parse_negotiation_item_kind_definitions(engine.lua()).expect("parse");
        assert_eq!(defs.len(), 3);
        assert_eq!(defs[0].merge_strategy, MergeStrategy::List);
        assert_eq!(defs[1].merge_strategy, MergeStrategy::Sum);
        assert_eq!(defs[2].merge_strategy, MergeStrategy::Replace);
    }

    #[test]
    fn parse_defaults_name_and_merge() {
        let engine = ScriptEngine::new().expect("lua init");
        engine
            .lua()
            .load(
                r#"
            define_negotiation_item_kind { id = "resources" }
        "#,
            )
            .exec()
            .expect("lua exec");

        let defs = parse_negotiation_item_kind_definitions(engine.lua()).expect("parse");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "resources"); // defaults to id
        assert_eq!(defs[0].merge_strategy, MergeStrategy::List); // defaults to list
        assert!(!defs[0].has_validate);
        assert!(!defs[0].has_apply);
    }

    #[test]
    fn parse_invalid_merge_strategy_errors() {
        let engine = ScriptEngine::new().expect("lua init");
        engine
            .lua()
            .load(
                r#"
            define_negotiation_item_kind { id = "bad", merge = "invalid" }
        "#,
            )
            .exec()
            .expect("lua exec");

        let result = parse_negotiation_item_kind_definitions(engine.lua());
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("unknown merge strategy"),
            "Expected error about merge strategy, got: {err_msg}"
        );
    }

    #[test]
    fn all_builtin_kinds_loaded() {
        let engine = ScriptEngine::new().expect("lua init");
        engine
            .lua()
            .load(
                r#"
            define_negotiation_item_kind {
                id = "resources",
                name = "Resource Transfer",
                merge = "sum",
            }
            define_negotiation_item_kind {
                id = "technology",
                name = "Technology Access",
                merge = "list",
            }
            define_negotiation_item_kind {
                id = "territory",
                name = "Territory Cession",
                merge = "list",
            }
            define_negotiation_item_kind {
                id = "peace",
                name = "Peace Treaty",
                merge = "replace",
            }
            define_negotiation_item_kind {
                id = "alliance",
                name = "Alliance Pact",
                merge = "replace",
            }
            define_negotiation_item_kind {
                id = "standing_modifier",
                name = "Standing Modifier",
                merge = "sum",
            }
            define_negotiation_item_kind {
                id = "return_cores",
                name = "Return Conquered Cores",
                merge = "list",
            }
            define_negotiation_item_kind {
                id = "trade_agreement",
                name = "Trade Agreement",
                merge = "replace",
            }
        "#,
            )
            .exec()
            .expect("lua exec");

        let defs = parse_negotiation_item_kind_definitions(engine.lua()).expect("parse");
        assert_eq!(defs.len(), 8);

        let ids: Vec<&str> = defs.iter().map(|d| d.id.as_str()).collect();
        assert!(ids.contains(&"resources"));
        assert!(ids.contains(&"technology"));
        assert!(ids.contains(&"territory"));
        assert!(ids.contains(&"peace"));
        assert!(ids.contains(&"alliance"));
        assert!(ids.contains(&"standing_modifier"));
        assert!(ids.contains(&"return_cores"));
        assert!(ids.contains(&"trade_agreement"));
    }
}
