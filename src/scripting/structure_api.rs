use std::collections::HashMap;

use crate::amount::Amt;
use crate::deep_space::{CapabilityParams, ResourceCost, StructureDefinition};
use crate::scripting::condition_parser::parse_condition;

/// Parse structure definitions from the Lua `_structure_definitions` global table.
/// Each entry should have at minimum `id` and `name` fields.
pub fn parse_structure_definitions(lua: &mlua::Lua) -> Result<Vec<StructureDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_structure_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table.get("name")?;
        let description: String = table.get::<Option<String>>("description")?.unwrap_or_default();
        let max_hp: f64 = table.get::<Option<f64>>("max_hp")?.unwrap_or(100.0);

        // Parse cost as a sub-table: { minerals = N, energy = N }
        let cost = parse_cost_table(&table)?;

        let build_time: i64 = table.get::<Option<i64>>("build_time")?.unwrap_or(10);

        let energy_drain_raw: f64 = table.get::<Option<f64>>("energy_drain")?.unwrap_or(0.0);
        // energy_drain is specified in millis in Lua (e.g. 100 = 0.1 units)
        let energy_drain = Amt::milli(energy_drain_raw as u64);

        // Parse prerequisites as an optional Condition table
        let prerequisites = parse_prerequisites(&table)?;

        // Parse capabilities as a table of tables: { cap_name = { range = N }, ... }
        let capabilities = parse_capabilities_map(&table)?;

        result.push(StructureDefinition {
            id,
            name,
            description,
            max_hp,
            cost,
            build_time,
            energy_drain,
            prerequisites,
            capabilities,
        });
    }

    Ok(result)
}

/// Parse the `cost = { minerals = N, energy = N }` sub-table.
fn parse_cost_table(table: &mlua::Table) -> Result<ResourceCost, mlua::Error> {
    let cost_value: mlua::Value = table.get("cost")?;
    match cost_value {
        mlua::Value::Table(cost_table) => {
            let minerals_raw: f64 = cost_table.get::<Option<f64>>("minerals")?.unwrap_or(0.0);
            let energy_raw: f64 = cost_table.get::<Option<f64>>("energy")?.unwrap_or(0.0);
            Ok(ResourceCost {
                minerals: Amt::from_f64(minerals_raw),
                energy: Amt::from_f64(energy_raw),
            })
        }
        mlua::Value::Nil => Ok(ResourceCost::default()),
        _ => Err(mlua::Error::RuntimeError(
            "Expected table or nil for 'cost' field".to_string(),
        )),
    }
}

/// Parse optional `prerequisites` field as a Condition tree.
/// Accepts either a condition table (from has_tech/all/any/etc.) or a function
/// that receives a ConditionCtx and returns a condition table.
fn parse_prerequisites(table: &mlua::Table) -> Result<Option<crate::condition::Condition>, mlua::Error> {
    let prereq_value: mlua::Value = table.get("prerequisites")?;
    match prereq_value {
        mlua::Value::Table(prereq_table) => {
            let cond = parse_condition(&prereq_table)?;
            Ok(Some(cond))
        }
        mlua::Value::Function(func) => {
            let ctx = crate::scripting::condition_ctx::ConditionCtx;
            let result: mlua::Table = func.call(ctx)?;
            let cond = parse_condition(&result)?;
            Ok(Some(cond))
        }
        mlua::Value::Nil => Ok(None),
        _ => Err(mlua::Error::RuntimeError(
            "Expected table, function, or nil for 'prerequisites' field".to_string(),
        )),
    }
}

/// Parse `capabilities = { cap_name = { range = N }, ... }` as a HashMap.
fn parse_capabilities_map(table: &mlua::Table) -> Result<HashMap<String, CapabilityParams>, mlua::Error> {
    let caps_value: mlua::Value = table.get("capabilities")?;
    match caps_value {
        mlua::Value::Table(caps_table) => {
            let mut caps = HashMap::new();
            for pair in caps_table.pairs::<String, mlua::Table>() {
                let (key, params_table) = pair?;
                let range: f64 = params_table.get::<Option<f64>>("range")?.unwrap_or(0.0);
                caps.insert(key, CapabilityParams { range });
            }
            Ok(caps)
        }
        mlua::Value::Nil => Ok(HashMap::new()),
        _ => Err(mlua::Error::RuntimeError(
            "Expected table or nil for 'capabilities' field".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::condition::{AtomKind, Condition, ConditionAtom, ConditionScope};
    use crate::scripting::ScriptEngine;

    #[test]
    fn test_parse_structure_definitions() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_structure {
                id = "sensor_buoy",
                name = "Sensor Buoy",
                description = "Detects sublight vessel movements.",
                max_hp = 20,
                cost = { minerals = 50, energy = 30 },
                build_time = 15,
                capabilities = {
                    detect_sublight = { range = 3.0 },
                },
                energy_drain = 100,
            }
            define_structure {
                id = "interdictor",
                name = "Interdictor",
                description = "Disrupts FTL travel.",
                max_hp = 80,
                cost = { minerals = 300, energy = 200 },
                build_time = 45,
                capabilities = {
                    ftl_interdiction = { range = 5.0 },
                },
                energy_drain = 1000,
                prerequisites = has_tech("ftl_interdiction_tech"),
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_structure_definitions(lua).unwrap();
        assert_eq!(defs.len(), 2);

        // Sensor Buoy
        assert_eq!(defs[0].id, "sensor_buoy");
        assert_eq!(defs[0].name, "Sensor Buoy");
        assert_eq!(defs[0].description, "Detects sublight vessel movements.");
        assert_eq!(defs[0].max_hp, 20.0);
        assert_eq!(defs[0].cost.minerals, Amt::units(50));
        assert_eq!(defs[0].cost.energy, Amt::units(30));
        assert_eq!(defs[0].build_time, 15);
        assert!(defs[0].capabilities.contains_key("detect_sublight"));
        assert_eq!(defs[0].capabilities["detect_sublight"].range, 3.0);
        assert_eq!(defs[0].energy_drain, Amt::milli(100));
        assert!(defs[0].prerequisites.is_none());

        // Interdictor
        assert_eq!(defs[1].id, "interdictor");
        assert_eq!(defs[1].name, "Interdictor");
        assert_eq!(defs[1].max_hp, 80.0);
        assert!(defs[1].capabilities.contains_key("ftl_interdiction"));
        assert_eq!(defs[1].capabilities["ftl_interdiction"].range, 5.0);
        assert_eq!(defs[1].energy_drain, Amt::units(1));
        assert_eq!(
            defs[1].prerequisites,
            Some(Condition::Atom(ConditionAtom::has_tech(
                "ftl_interdiction_tech"
            )))
        );
    }

    #[test]
    fn test_parse_structure_minimal() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_structure {
                id = "basic",
                name = "Basic Structure",
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_structure_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id, "basic");
        assert_eq!(defs[0].name, "Basic Structure");
        assert_eq!(defs[0].description, "");
        assert_eq!(defs[0].max_hp, 100.0); // default
        assert_eq!(defs[0].cost.minerals, Amt::ZERO);
        assert_eq!(defs[0].cost.energy, Amt::ZERO);
        assert_eq!(defs[0].build_time, 10); // default
        assert!(defs[0].capabilities.is_empty());
        assert_eq!(defs[0].energy_drain, Amt::ZERO);
        assert!(defs[0].prerequisites.is_none());
    }

    #[test]
    fn test_parse_structure_with_complex_prerequisites() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_structure {
                id = "advanced",
                name = "Advanced Structure",
                prerequisites = all(
                    has_tech("tech_a"),
                    any(has_modifier("mod_b"), has_building("bldg_c"))
                ),
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_structure_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(
            defs[0].prerequisites,
            Some(Condition::All(vec![
                Condition::Atom(ConditionAtom::has_tech("tech_a")),
                Condition::Any(vec![
                    Condition::Atom(ConditionAtom::has_modifier("mod_b")),
                    Condition::Atom(ConditionAtom::has_building("bldg_c")),
                ]),
            ]))
        );
    }

    #[test]
    fn test_parse_structure_with_function_prerequisites() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_structure {
                id = "scoped_station",
                name = "Scoped Station",
                prerequisites = function(ctx)
                    return all(
                        ctx.empire:has_tech("advanced_sensors"),
                        ctx.system:has_building("shipyard")
                    )
                end,
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_structure_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(
            defs[0].prerequisites,
            Some(Condition::All(vec![
                Condition::Atom(ConditionAtom::scoped(
                    AtomKind::HasTech("advanced_sensors".into()),
                    ConditionScope::Empire,
                )),
                Condition::Atom(ConditionAtom::scoped(
                    AtomKind::HasBuilding("shipyard".into()),
                    ConditionScope::System,
                )),
            ]))
        );
    }

    #[test]
    fn test_parse_structure_from_lua_file() {
        let engine = ScriptEngine::new().unwrap();

        let structure_script =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/structures/definitions.lua");
        if !structure_script.exists() {
            // Skip if file doesn't exist yet (it may not be created yet during development)
            return;
        }

        engine.load_file(&structure_script).unwrap();
        let defs = parse_structure_definitions(engine.lua()).unwrap();

        assert!(
            defs.len() >= 3,
            "Expected at least 3 structure definitions from definitions.lua, got {}",
            defs.len()
        );

        // Build a quick lookup
        let map: std::collections::HashMap<String, _> =
            defs.into_iter().map(|d| (d.id.clone(), d)).collect();

        let buoy = map.get("sensor_buoy").expect("sensor_buoy should exist");
        assert_eq!(buoy.name, "Sensor Buoy");
        assert!(buoy.capabilities.contains_key("detect_sublight"));

        let relay = map.get("ftl_comm_relay").expect("ftl_comm_relay should exist");
        assert!(relay.capabilities.contains_key("ftl_comm_relay"));

        let interdictor = map.get("interdictor").expect("interdictor should exist");
        assert!(interdictor.capabilities.contains_key("ftl_interdiction"));
    }
}
