//! #305 (S-11): Parse Casus Belli definitions from Lua.
//!
//! Definitions are accumulated by `define_casus_belli { ... }` (registered in
//! `globals.rs`) into `_casus_belli_definitions`. This module drains that
//! accumulator at startup and builds the [`CasusBelliRegistry`] resource.

use std::collections::HashMap;

use crate::casus_belli::{
    AdditionalDemandGroup, CasusBelliDefinition, CasusBelliRegistry, DemandSpec, EndScenario,
};

/// Parse casus belli definitions from the Lua `_casus_belli_definitions`
/// global table. Returns a `Vec` of definitions; the caller inserts them
/// into the [`CasusBelliRegistry`].
pub fn parse_casus_belli_definitions(
    lua: &mlua::Lua,
) -> Result<Vec<CasusBelliDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_casus_belli_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table
            .get::<Option<String>>("name")?
            .unwrap_or_else(|| id.clone());
        let auto_war: bool = table.get::<Option<bool>>("auto_war")?.unwrap_or(false);

        // Parse demands array
        let demands = parse_demands_array(&table, "demands")?;

        // Parse additional_demand_groups
        let additional_demand_groups = parse_additional_demand_groups(&table)?;

        // Parse end_scenarios
        let end_scenarios = parse_end_scenarios(&table)?;

        result.push(CasusBelliDefinition {
            id,
            name,
            auto_war,
            demands,
            additional_demand_groups,
            end_scenarios,
        });
    }

    Ok(result)
}

/// Parse a `demands` array field from a Lua table.
fn parse_demands_array(table: &mlua::Table, key: &str) -> Result<Vec<DemandSpec>, mlua::Error> {
    let mut demands = Vec::new();
    let raw: mlua::Value = table.get(key)?;
    if let mlua::Value::Table(arr) = raw {
        for pair in arr.pairs::<i64, mlua::Table>() {
            let (_, demand_tbl) = pair?;
            demands.push(parse_demand(&demand_tbl)?);
        }
    }
    Ok(demands)
}

/// Parse a single demand table `{ kind = "...", ... }`.
fn parse_demand(table: &mlua::Table) -> Result<DemandSpec, mlua::Error> {
    let kind: String = table.get("kind")?;
    let mut params = HashMap::new();
    for pair in table.pairs::<String, mlua::Value>() {
        let (k, v) = pair?;
        if k == "kind" {
            continue;
        }
        if let mlua::Value::String(s) = v {
            params.insert(k, s.to_str()?.to_string());
        } else if let mlua::Value::Number(n) = v {
            params.insert(k, n.to_string());
        } else if let mlua::Value::Boolean(b) = v {
            params.insert(k, b.to_string());
        }
    }
    Ok(DemandSpec { kind, params })
}

/// Parse `additional_demand_groups` array from the CB definition table.
fn parse_additional_demand_groups(
    table: &mlua::Table,
) -> Result<Vec<AdditionalDemandGroup>, mlua::Error> {
    let mut groups = Vec::new();
    let raw: mlua::Value = table.get("additional_demand_groups")?;
    if let mlua::Value::Table(arr) = raw {
        for pair in arr.pairs::<i64, mlua::Table>() {
            let (_, group_tbl) = pair?;
            let label: String = group_tbl
                .get::<Option<String>>("label")?
                .unwrap_or_default();
            let max_picks: u32 = group_tbl.get::<Option<u32>>("max_picks")?.unwrap_or(1);
            let demands = parse_demands_array(&group_tbl, "demands")?;
            groups.push(AdditionalDemandGroup {
                label,
                max_picks,
                demands,
            });
        }
    }
    Ok(groups)
}

/// Parse `end_scenarios` array from the CB definition table.
fn parse_end_scenarios(table: &mlua::Table) -> Result<Vec<EndScenario>, mlua::Error> {
    let mut scenarios = Vec::new();
    let raw: mlua::Value = table.get("end_scenarios")?;
    if let mlua::Value::Table(arr) = raw {
        for pair in arr.pairs::<i64, mlua::Table>() {
            let (_, sc_tbl) = pair?;
            let id: String = sc_tbl.get("id")?;
            let label: String = sc_tbl
                .get::<Option<String>>("label")?
                .unwrap_or_else(|| id.clone());
            let demand_adjustments = parse_demands_array(&sc_tbl, "demand_adjustments")?;
            scenarios.push(EndScenario {
                id,
                label,
                demand_adjustments,
            });
        }
    }
    Ok(scenarios)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ScriptEngine;

    #[test]
    fn parse_basic_cb_definition() {
        let engine = ScriptEngine::new().expect("lua init");
        engine
            .lua()
            .load(
                r#"
            define_casus_belli {
                id = "core_attack",
                name = "Unprovoked Core Attack",
                auto_war = true,
                demands = {
                    { kind = "return_cores" },
                },
                end_scenarios = {
                    { id = "white_peace", label = "White Peace" },
                },
            }
        "#,
            )
            .exec()
            .expect("lua exec");

        let defs = parse_casus_belli_definitions(engine.lua()).expect("parse");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id, "core_attack");
        assert_eq!(defs[0].name, "Unprovoked Core Attack");
        assert!(defs[0].auto_war);
        assert_eq!(defs[0].demands.len(), 1);
        assert_eq!(defs[0].demands[0].kind, "return_cores");
        assert_eq!(defs[0].end_scenarios.len(), 1);
        assert_eq!(defs[0].end_scenarios[0].id, "white_peace");
    }
}
