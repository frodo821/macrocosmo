use std::collections::HashMap;

use bevy::prelude::*;

use crate::amount::Amt;

/// A building definition parsed from Lua `define_building { ... }` calls.
/// Mirrors the hardcoded `BuildingType` methods but is data-driven.
#[derive(Clone, Debug)]
pub struct BuildingDefinition {
    pub id: String,
    pub name: String,
    pub minerals_cost: Amt,
    pub energy_cost: Amt,
    pub build_time: i64,
    pub maintenance: Amt,
    pub production_bonus_minerals: Amt,
    pub production_bonus_energy: Amt,
    pub production_bonus_research: Amt,
    pub production_bonus_food: Amt,
}

/// Registry of all building definitions loaded from Lua scripts.
/// Parallel data source to the hardcoded `BuildingType` enum.
/// Future work will migrate BuildingType consumers to read from this registry.
#[derive(Resource, Default)]
pub struct BuildingRegistry {
    pub buildings: HashMap<String, BuildingDefinition>,
}

impl BuildingRegistry {
    /// Look up a building definition by its id.
    pub fn get(&self, id: &str) -> Option<&BuildingDefinition> {
        self.buildings.get(id)
    }

    /// Insert a building definition, replacing any existing definition with the same id.
    pub fn insert(&mut self, def: BuildingDefinition) {
        self.buildings.insert(def.id.clone(), def);
    }
}

/// Parse building definitions from the Lua `_building_definitions` global table.
/// Each entry should have at minimum `id` and `name` fields.
pub fn parse_building_definitions(lua: &mlua::Lua) -> Result<Vec<BuildingDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_building_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table.get("name")?;

        // Parse cost table (optional)
        let (minerals_cost, energy_cost) = parse_cost_table(&table)?;

        let build_time: i64 = table.get::<Option<i64>>("build_time")?.unwrap_or(10);
        let maintenance_f64: f64 = table.get::<Option<f64>>("maintenance")?.unwrap_or(0.0);
        let maintenance = Amt::from_f64(maintenance_f64);

        // Parse production_bonus table (optional)
        let (pb_minerals, pb_energy, pb_research, pb_food) =
            parse_production_bonus_table(&table)?;

        result.push(BuildingDefinition {
            id,
            name,
            minerals_cost,
            energy_cost,
            build_time,
            maintenance,
            production_bonus_minerals: pb_minerals,
            production_bonus_energy: pb_energy,
            production_bonus_research: pb_research,
            production_bonus_food: pb_food,
        });
    }

    Ok(result)
}

/// Parse the `cost = { minerals = N, energy = N }` sub-table.
fn parse_cost_table(table: &mlua::Table) -> Result<(Amt, Amt), mlua::Error> {
    let cost_value: mlua::Value = table.get("cost")?;
    match cost_value {
        mlua::Value::Table(cost_table) => {
            let minerals: f64 = cost_table
                .get::<Option<f64>>("minerals")?
                .unwrap_or(0.0);
            let energy: f64 = cost_table
                .get::<Option<f64>>("energy")?
                .unwrap_or(0.0);
            Ok((Amt::from_f64(minerals), Amt::from_f64(energy)))
        }
        mlua::Value::Nil => Ok((Amt::ZERO, Amt::ZERO)),
        _ => Err(mlua::Error::RuntimeError(
            "Expected table or nil for 'cost' field".to_string(),
        )),
    }
}

/// Parse the `production_bonus = { minerals = N, energy = N, research = N, food = N }` sub-table.
fn parse_production_bonus_table(
    table: &mlua::Table,
) -> Result<(Amt, Amt, Amt, Amt), mlua::Error> {
    let pb_value: mlua::Value = table.get("production_bonus")?;
    match pb_value {
        mlua::Value::Table(pb_table) => {
            let minerals: f64 = pb_table
                .get::<Option<f64>>("minerals")?
                .unwrap_or(0.0);
            let energy: f64 = pb_table
                .get::<Option<f64>>("energy")?
                .unwrap_or(0.0);
            let research: f64 = pb_table
                .get::<Option<f64>>("research")?
                .unwrap_or(0.0);
            let food: f64 = pb_table
                .get::<Option<f64>>("food")?
                .unwrap_or(0.0);
            Ok((
                Amt::from_f64(minerals),
                Amt::from_f64(energy),
                Amt::from_f64(research),
                Amt::from_f64(food),
            ))
        }
        mlua::Value::Nil => Ok((Amt::ZERO, Amt::ZERO, Amt::ZERO, Amt::ZERO)),
        _ => Err(mlua::Error::RuntimeError(
            "Expected table or nil for 'production_bonus' field".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ScriptEngine;

    #[test]
    fn test_parse_building_definitions() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_building {
                id = "mine",
                name = "Mine",
                cost = { minerals = 150, energy = 50 },
                build_time = 10,
                maintenance = 0.2,
                production_bonus = { minerals = 3.0 },
            }
            define_building {
                id = "farm",
                name = "Farm",
                cost = { minerals = 100, energy = 50 },
                build_time = 20,
                maintenance = 0.3,
                production_bonus = { food = 5.0 },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_building_definitions(lua).unwrap();
        assert_eq!(defs.len(), 2);

        // Mine
        assert_eq!(defs[0].id, "mine");
        assert_eq!(defs[0].name, "Mine");
        assert_eq!(defs[0].minerals_cost, Amt::units(150));
        assert_eq!(defs[0].energy_cost, Amt::units(50));
        assert_eq!(defs[0].build_time, 10);
        assert_eq!(defs[0].maintenance, Amt::new(0, 200));
        assert_eq!(defs[0].production_bonus_minerals, Amt::units(3));
        assert_eq!(defs[0].production_bonus_energy, Amt::ZERO);
        assert_eq!(defs[0].production_bonus_research, Amt::ZERO);
        assert_eq!(defs[0].production_bonus_food, Amt::ZERO);

        // Farm
        assert_eq!(defs[1].id, "farm");
        assert_eq!(defs[1].name, "Farm");
        assert_eq!(defs[1].minerals_cost, Amt::units(100));
        assert_eq!(defs[1].energy_cost, Amt::units(50));
        assert_eq!(defs[1].build_time, 20);
        assert_eq!(defs[1].maintenance, Amt::new(0, 300));
        assert_eq!(defs[1].production_bonus_minerals, Amt::ZERO);
        assert_eq!(defs[1].production_bonus_energy, Amt::ZERO);
        assert_eq!(defs[1].production_bonus_research, Amt::ZERO);
        assert_eq!(defs[1].production_bonus_food, Amt::units(5));
    }

    #[test]
    fn test_parse_building_minimal() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_building {
                id = "basic",
                name = "Basic Building",
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_building_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id, "basic");
        assert_eq!(defs[0].name, "Basic Building");
        assert_eq!(defs[0].minerals_cost, Amt::ZERO);
        assert_eq!(defs[0].energy_cost, Amt::ZERO);
        assert_eq!(defs[0].build_time, 10); // default
        assert_eq!(defs[0].maintenance, Amt::ZERO);
        assert_eq!(defs[0].production_bonus_minerals, Amt::ZERO);
        assert_eq!(defs[0].production_bonus_energy, Amt::ZERO);
        assert_eq!(defs[0].production_bonus_research, Amt::ZERO);
        assert_eq!(defs[0].production_bonus_food, Amt::ZERO);
    }

    #[test]
    fn test_building_registry_lookup() {
        let mut registry = BuildingRegistry::default();
        assert!(registry.get("mine").is_none());

        registry.insert(BuildingDefinition {
            id: "mine".to_string(),
            name: "Mine".to_string(),
            minerals_cost: Amt::units(150),
            energy_cost: Amt::units(50),
            build_time: 10,
            maintenance: Amt::new(0, 200),
            production_bonus_minerals: Amt::units(3),
            production_bonus_energy: Amt::ZERO,
            production_bonus_research: Amt::ZERO,
            production_bonus_food: Amt::ZERO,
        });

        let mine = registry.get("mine").unwrap();
        assert_eq!(mine.name, "Mine");
        assert_eq!(mine.minerals_cost, Amt::units(150));
        assert_eq!(mine.production_bonus_minerals, Amt::units(3));

        // Insert another
        registry.insert(BuildingDefinition {
            id: "farm".to_string(),
            name: "Farm".to_string(),
            minerals_cost: Amt::units(100),
            energy_cost: Amt::units(50),
            build_time: 20,
            maintenance: Amt::new(0, 300),
            production_bonus_minerals: Amt::ZERO,
            production_bonus_energy: Amt::ZERO,
            production_bonus_research: Amt::ZERO,
            production_bonus_food: Amt::units(5),
        });

        assert_eq!(registry.buildings.len(), 2);
        assert!(registry.get("farm").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    /// MAJOR #9: Verify BuildingRegistry loaded from scripts/buildings/basic.lua.
    #[test]
    fn test_building_registry_loaded_from_lua() {
        let engine = ScriptEngine::new().unwrap();

        // Load the actual building definitions file
        let building_script = std::path::Path::new("scripts/buildings/basic.lua");
        if !building_script.exists() {
            // Try from the workspace root (worktree directory)
            let alt_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/buildings/basic.lua");
            if alt_path.exists() {
                engine.load_file(&alt_path).unwrap();
            } else {
                panic!("scripts/buildings/basic.lua not found at {:?} or {:?}", building_script, alt_path);
            }
        } else {
            engine.load_file(building_script).unwrap();
        }

        let defs = parse_building_definitions(engine.lua()).unwrap();

        // basic.lua defines 6 buildings: mine, power_plant, research_lab, shipyard, port, farm
        assert_eq!(defs.len(), 6, "Expected 6 building definitions from basic.lua");

        // Build a registry from the parsed definitions
        let mut registry = BuildingRegistry::default();
        for def in &defs {
            registry.insert(def.clone());
        }

        // Verify Mine has minerals production bonus = 3.0
        let mine = registry.get("mine").expect("Mine should be in registry");
        assert_eq!(mine.name, "Mine");
        assert_eq!(mine.production_bonus_minerals, Amt::units(3));
        assert_eq!(mine.minerals_cost, Amt::units(150));
        assert_eq!(mine.energy_cost, Amt::units(50));
        assert_eq!(mine.build_time, 10);
        assert_eq!(mine.maintenance, Amt::new(0, 200));

        // Verify Farm has food production bonus = 5.0
        let farm = registry.get("farm").expect("Farm should be in registry");
        assert_eq!(farm.name, "Farm");
        assert_eq!(farm.production_bonus_food, Amt::units(5));

        // Verify Shipyard has no production bonus
        let shipyard = registry.get("shipyard").expect("Shipyard should be in registry");
        assert_eq!(shipyard.name, "Shipyard");
        assert_eq!(shipyard.production_bonus_minerals, Amt::ZERO);
        assert_eq!(shipyard.production_bonus_energy, Amt::ZERO);
        assert_eq!(shipyard.production_bonus_research, Amt::ZERO);
        assert_eq!(shipyard.production_bonus_food, Amt::ZERO);
        assert_eq!(shipyard.maintenance, Amt::units(1));
    }

    #[test]
    fn test_building_registry_replace() {
        let mut registry = BuildingRegistry::default();

        registry.insert(BuildingDefinition {
            id: "mine".to_string(),
            name: "Mine".to_string(),
            minerals_cost: Amt::units(150),
            energy_cost: Amt::units(50),
            build_time: 10,
            maintenance: Amt::new(0, 200),
            production_bonus_minerals: Amt::units(3),
            production_bonus_energy: Amt::ZERO,
            production_bonus_research: Amt::ZERO,
            production_bonus_food: Amt::ZERO,
        });

        // Replace with updated values
        registry.insert(BuildingDefinition {
            id: "mine".to_string(),
            name: "Advanced Mine".to_string(),
            minerals_cost: Amt::units(200),
            energy_cost: Amt::units(75),
            build_time: 15,
            maintenance: Amt::new(0, 300),
            production_bonus_minerals: Amt::units(5),
            production_bonus_energy: Amt::ZERO,
            production_bonus_research: Amt::ZERO,
            production_bonus_food: Amt::ZERO,
        });

        assert_eq!(registry.buildings.len(), 1);
        let mine = registry.get("mine").unwrap();
        assert_eq!(mine.name, "Advanced Mine");
        assert_eq!(mine.production_bonus_minerals, Amt::units(5));
    }
}
