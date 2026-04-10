use crate::amount::Amt;
use crate::deep_space::StructureDefinition;

/// Parse structure definitions from the Lua `_structure_definitions` global table.
/// Each entry should have at minimum `id` and `name` fields.
pub fn parse_structure_definitions(lua: &mlua::Lua) -> Result<Vec<StructureDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_structure_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table.get("name")?;
        let max_hp: f64 = table.get::<Option<f64>>("max_hp")?.unwrap_or(100.0);

        let build_cost_minerals_raw: f64 = table.get::<Option<f64>>("build_cost_minerals")?.unwrap_or(0.0);
        let build_cost_energy_raw: f64 = table.get::<Option<f64>>("build_cost_energy")?.unwrap_or(0.0);
        let build_cost_minerals = Amt::from_f64(build_cost_minerals_raw);
        let build_cost_energy = Amt::from_f64(build_cost_energy_raw);

        let build_time: i64 = table.get::<Option<i64>>("build_time")?.unwrap_or(10);

        // Parse capabilities array
        let capabilities = parse_capabilities(&table)?;

        let detection_range: f64 = table.get::<Option<f64>>("detection_range")?.unwrap_or(0.0);
        let interdiction_range: f64 = table.get::<Option<f64>>("interdiction_range")?.unwrap_or(0.0);

        let energy_drain_raw: f64 = table.get::<Option<f64>>("energy_drain")?.unwrap_or(0.0);
        // energy_drain is specified in millis in Lua (e.g. 100 = 0.1 units)
        let energy_drain = Amt::milli(energy_drain_raw as u64);

        let prerequisite_tech: Option<String> = table.get("prerequisite_tech")?;

        result.push(StructureDefinition {
            id,
            name,
            max_hp,
            build_cost_minerals,
            build_cost_energy,
            build_time,
            capabilities,
            detection_range,
            interdiction_range,
            energy_drain,
            prerequisite_tech,
        });
    }

    Ok(result)
}

/// Parse the `capabilities = { "detect_sublight", "ftl_comm" }` array from a Lua table.
fn parse_capabilities(table: &mlua::Table) -> Result<Vec<String>, mlua::Error> {
    let caps_value: mlua::Value = table.get("capabilities")?;
    match caps_value {
        mlua::Value::Table(caps_table) => {
            let mut caps = Vec::new();
            for pair in caps_table.pairs::<i64, String>() {
                let (_, cap) = pair?;
                caps.push(cap);
            }
            Ok(caps)
        }
        mlua::Value::Nil => Ok(Vec::new()),
        _ => Err(mlua::Error::RuntimeError(
            "Expected table or nil for 'capabilities' field".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
                max_hp = 20,
                build_cost_minerals = 50,
                build_cost_energy = 30,
                build_time = 15,
                capabilities = { "detect_sublight" },
                detection_range = 3.0,
                energy_drain = 100,
            }
            define_structure {
                id = "interdictor",
                name = "Interdictor",
                max_hp = 80,
                build_cost_minerals = 300,
                build_cost_energy = 200,
                build_time = 45,
                capabilities = { "ftl_interdiction" },
                interdiction_range = 5.0,
                energy_drain = 1000,
                prerequisite_tech = "ftl_interdiction_tech",
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
        assert_eq!(defs[0].max_hp, 20.0);
        assert_eq!(defs[0].build_cost_minerals, Amt::units(50));
        assert_eq!(defs[0].build_cost_energy, Amt::units(30));
        assert_eq!(defs[0].build_time, 15);
        assert_eq!(defs[0].capabilities, vec!["detect_sublight"]);
        assert_eq!(defs[0].detection_range, 3.0);
        assert_eq!(defs[0].interdiction_range, 0.0);
        assert_eq!(defs[0].energy_drain, Amt::milli(100));
        assert!(defs[0].prerequisite_tech.is_none());

        // Interdictor
        assert_eq!(defs[1].id, "interdictor");
        assert_eq!(defs[1].name, "Interdictor");
        assert_eq!(defs[1].max_hp, 80.0);
        assert_eq!(defs[1].capabilities, vec!["ftl_interdiction"]);
        assert_eq!(defs[1].interdiction_range, 5.0);
        assert_eq!(defs[1].energy_drain, Amt::units(1));
        assert_eq!(defs[1].prerequisite_tech, Some("ftl_interdiction_tech".to_string()));
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
        assert_eq!(defs[0].max_hp, 100.0); // default
        assert_eq!(defs[0].build_cost_minerals, Amt::ZERO);
        assert_eq!(defs[0].build_cost_energy, Amt::ZERO);
        assert_eq!(defs[0].build_time, 10); // default
        assert!(defs[0].capabilities.is_empty());
        assert_eq!(defs[0].detection_range, 0.0);
        assert_eq!(defs[0].interdiction_range, 0.0);
        assert_eq!(defs[0].energy_drain, Amt::ZERO);
        assert!(defs[0].prerequisite_tech.is_none());
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
        assert!(buoy.capabilities.contains(&"detect_sublight".to_string()));

        let relay = map.get("ftl_comm_relay").expect("ftl_comm_relay should exist");
        assert!(relay.capabilities.contains(&"ftl_comm".to_string()));

        let interdictor = map.get("interdictor").expect("interdictor should exist");
        assert!(interdictor.capabilities.contains(&"ftl_interdiction".to_string()));
    }
}
