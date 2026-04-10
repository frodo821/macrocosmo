use std::collections::HashMap;

use crate::amount::Amt;
use crate::modifier::ModifiedValue;
use crate::species::{JobDefinition, SpeciesDefinition};

/// Parse species definitions from the Lua `_species_definitions` global table.
pub fn parse_species_definitions(lua: &mlua::Lua) -> Result<Vec<SpeciesDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_species_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let name: String = table.get("name")?;
        let growth_rate: f64 = table.get::<Option<f64>>("growth_rate")?.unwrap_or(0.01);

        // Parse job_bonuses table (optional): { miner = 0.1, researcher = 0.2, ... }
        let mut job_bonuses = HashMap::new();
        let bonuses_value: mlua::Value = table.get("job_bonuses")?;
        if let mlua::Value::Table(bonuses_table) = bonuses_value {
            for pair in bonuses_table.pairs::<String, f64>() {
                let (job_id, bonus) = pair?;
                // Create a ModifiedValue with base = 1.0 + bonus
                // The bonus is stored as a fraction, e.g. 0.1 = +10%
                let base_value = Amt::from_f64(1.0 + bonus);
                job_bonuses.insert(job_id, ModifiedValue::new(base_value));
            }
        }

        result.push(SpeciesDefinition {
            id,
            name,
            base_growth_rate: growth_rate,
            job_bonuses,
        });
    }

    Ok(result)
}

/// Parse job definitions from the Lua `_job_definitions` global table.
pub fn parse_job_definitions(lua: &mlua::Lua) -> Result<Vec<JobDefinition>, mlua::Error> {
    let defs: mlua::Table = lua.globals().get("_job_definitions")?;
    let mut result = Vec::new();

    for pair in defs.pairs::<i64, mlua::Table>() {
        let (_, table) = pair?;

        let id: String = table.get("id")?;
        let label: String = table.get("label")?;

        // Parse base_output table (optional): { minerals = 0.6, energy = 0.5, ... }
        let mut base_output = HashMap::new();
        let output_value: mlua::Value = table.get("base_output")?;
        if let mlua::Value::Table(output_table) = output_value {
            for pair in output_table.pairs::<String, f64>() {
                let (resource, amount) = pair?;
                base_output.insert(resource, Amt::from_f64(amount));
            }
        }

        result.push(JobDefinition {
            id,
            label,
            base_output,
        });
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ScriptEngine;

    #[test]
    fn test_parse_species_definitions() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_species {
                id = "human",
                name = "Human",
                growth_rate = 0.01,
                job_bonuses = {
                    miner = 0.0,
                    researcher = 0.1,
                },
            }
            define_species {
                id = "alien",
                name = "Alien",
                growth_rate = 0.02,
                job_bonuses = {
                    miner = 0.2,
                },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_species_definitions(lua).unwrap();
        assert_eq!(defs.len(), 2);

        // Human
        assert_eq!(defs[0].id, "human");
        assert_eq!(defs[0].name, "Human");
        assert!((defs[0].base_growth_rate - 0.01).abs() < 1e-10);
        assert_eq!(defs[0].job_bonuses.len(), 2);
        // miner bonus = 0.0 -> base = 1.0
        let miner_bonus = defs[0].job_bonuses.get("miner").unwrap();
        assert_eq!(miner_bonus.base(), Amt::units(1));
        // researcher bonus = 0.1 -> base = 1.1
        let researcher_bonus = defs[0].job_bonuses.get("researcher").unwrap();
        assert_eq!(researcher_bonus.base(), Amt::new(1, 100));

        // Alien
        assert_eq!(defs[1].id, "alien");
        assert_eq!(defs[1].name, "Alien");
        assert!((defs[1].base_growth_rate - 0.02).abs() < 1e-10);
        let miner_bonus = defs[1].job_bonuses.get("miner").unwrap();
        assert_eq!(miner_bonus.base(), Amt::new(1, 200));
    }

    #[test]
    fn test_parse_species_minimal() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_species {
                id = "basic",
                name = "Basic Species",
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_species_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id, "basic");
        assert_eq!(defs[0].name, "Basic Species");
        assert!((defs[0].base_growth_rate - 0.01).abs() < 1e-10); // default
        assert!(defs[0].job_bonuses.is_empty());
    }

    #[test]
    fn test_parse_job_definitions() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_job {
                id = "miner",
                label = "Miner",
                base_output = { minerals = 0.6 },
            }
            define_job {
                id = "farmer",
                label = "Farmer",
                base_output = { food = 0.6 },
            }
            define_job {
                id = "researcher",
                label = "Researcher",
                base_output = { research = 0.5 },
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_job_definitions(lua).unwrap();
        assert_eq!(defs.len(), 3);

        // Miner
        assert_eq!(defs[0].id, "miner");
        assert_eq!(defs[0].label, "Miner");
        assert_eq!(
            defs[0].base_output.get("minerals"),
            Some(&Amt::new(0, 600))
        );

        // Farmer
        assert_eq!(defs[1].id, "farmer");
        assert_eq!(defs[1].label, "Farmer");
        assert_eq!(defs[1].base_output.get("food"), Some(&Amt::new(0, 600)));

        // Researcher
        assert_eq!(defs[2].id, "researcher");
        assert_eq!(defs[2].label, "Researcher");
        assert_eq!(
            defs[2].base_output.get("research"),
            Some(&Amt::new(0, 500))
        );
    }

    #[test]
    fn test_parse_job_minimal() {
        let engine = ScriptEngine::new().unwrap();
        let lua = engine.lua();

        lua.load(
            r#"
            define_job {
                id = "idle",
                label = "Idle",
            }
            "#,
        )
        .exec()
        .unwrap();

        let defs = parse_job_definitions(lua).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id, "idle");
        assert_eq!(defs[0].label, "Idle");
        assert!(defs[0].base_output.is_empty());
    }

    /// Test loading species definitions from the actual scripts/species/human.lua file.
    #[test]
    fn test_species_registry_loaded_from_lua() {
        let engine = ScriptEngine::new().unwrap();

        let species_script =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/species/human.lua");
        if !species_script.exists() {
            panic!(
                "scripts/species/human.lua not found at {:?}",
                species_script
            );
        }
        engine.load_file(&species_script).unwrap();

        let defs = parse_species_definitions(engine.lua()).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].id, "human");
        assert_eq!(defs[0].name, "Human");
        assert!((defs[0].base_growth_rate - 0.01).abs() < 1e-10);
        assert!(defs[0].job_bonuses.contains_key("miner"));
        assert!(defs[0].job_bonuses.contains_key("researcher"));
        assert!(defs[0].job_bonuses.contains_key("farmer"));
    }

    /// Test loading job definitions from the actual scripts/jobs/basic.lua file.
    #[test]
    fn test_job_registry_loaded_from_lua() {
        let engine = ScriptEngine::new().unwrap();

        let jobs_script =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/jobs/basic.lua");
        if !jobs_script.exists() {
            panic!("scripts/jobs/basic.lua not found at {:?}", jobs_script);
        }
        engine.load_file(&jobs_script).unwrap();

        let defs = parse_job_definitions(engine.lua()).unwrap();
        assert!(defs.len() >= 4, "Expected at least 4 job definitions");

        let ids: Vec<&str> = defs.iter().map(|d| d.id.as_str()).collect();
        assert!(ids.contains(&"miner"));
        assert!(ids.contains(&"farmer"));
        assert!(ids.contains(&"researcher"));
        assert!(ids.contains(&"power_worker"));
    }
}
